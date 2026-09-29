use anyhow::Result;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use opentelemetry::trace::{Link, SpanKind, TraceId, TraceState, TracerProvider as _};
use opentelemetry::{global, Context, KeyValue, Value};
use opentelemetry_otlp::{
    LogExporter, MetricExporter, SpanExporter, WithExportConfig, WithHttpConfig,
};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{
    BatchSpanProcessor, Sampler, SamplingDecision, SamplingResult, SdkTracerProvider, ShouldSample,
    Span, SpanData, SpanProcessor,
};
use opentelemetry_sdk::Resource;
use std::collections::HashMap;
use std::time::Duration;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const SCOPE: &str = "librejustice";

pub struct OtlpCreds {
    pub endpoint: String,
    pub user: String,
    pub api_key: String,
}

pub struct InitOpts {
    pub filter: EnvFilter,
    pub json: bool,
    pub service_name: String,
    pub deployment_environment: Option<String>,
    pub host_name: Option<String>,
    pub container_name: Option<String>,
    pub otlp_log_filter: Targets,
    pub otlp: Option<OtlpCreds>,
}

#[must_use = "garder le guard vivant jusqu'a la fin du process, sinon les signaux bufferises ne partent pas"]
#[derive(Default)]
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.tracer_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = &self.meter_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = &self.logger_provider {
            let _ = p.shutdown();
        }
    }
}

struct OtlpExport {
    endpoint: String,
    auth: String,
    resource: Resource,
}

impl OtlpExport {
    fn new(creds: OtlpCreds, opts: &InitOpts) -> Self {
        let token = STANDARD.encode(format!("{}:{}", creds.user, creds.api_key));
        let host = opts.host_name.clone().unwrap_or_else(hostname);
        let mut attrs = vec![KeyValue::new("host.name", host)];
        if let Some(env) = &opts.deployment_environment {
            attrs.push(KeyValue::new("deployment.environment", env.clone()));
        }
        if let Some(nom) = &opts.container_name {
            attrs.push(KeyValue::new("container.name", nom.clone()));
        }
        let resource = Resource::builder()
            .with_service_name(opts.service_name.clone())
            .with_attributes(attrs)
            .build();
        Self {
            endpoint: creds.endpoint.trim_end_matches('/').to_string(),
            auth: format!("Basic {token}"),
            resource,
        }
    }

    fn headers(&self) -> HashMap<String, String> {
        HashMap::from([("Authorization".to_string(), self.auth.clone())])
    }

    fn signal_endpoint(&self, signal: &str) -> String {
        format!("{}/v1/{signal}", self.endpoint)
    }
}

pub fn init(mut opts: InitOpts) -> Result<TelemetryGuard> {
    let fmt_layer = if opts.json {
        tracing_subscriber::fmt::layer()
            .json()
            .with_target(true)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer().with_target(true).boxed()
    };

    let Some(creds) = opts.otlp.take() else {
        tracing_subscriber::registry()
            .with(opts.filter)
            .with(fmt_layer)
            .init();
        return Ok(TelemetryGuard::default());
    };

    let export = OtlpExport::new(creds, &opts);

    let tracer_provider = build_tracer_provider(&export)?;
    let meter_provider = build_meter_provider(&export)?;
    let logger_provider = build_logger_provider(&export)?;

    let traces_layer = tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer(SCOPE));
    let logs_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider)
            .with_filter(opts.otlp_log_filter);

    tracing_subscriber::registry()
        .with(opts.filter)
        .with(fmt_layer)
        .with(traces_layer)
        .with(logs_layer)
        .init();

    tracing::info!(
        endpoint = %export.endpoint,
        service = %opts.service_name,
        "opentelemetry: traces + logs + metrics -> Grafana Cloud OTLP"
    );

    Ok(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider: Some(meter_provider),
        logger_provider: Some(logger_provider),
    })
}

pub fn meter() -> opentelemetry::metrics::Meter {
    global::meter(SCOPE)
}

const UNATTENDED_KEY: &str = "librejustice.unattended";

pub fn unattended_request_span() -> tracing::Span {
    tracing::info_span!("http.request", librejustice.unattended = true)
}

const DB_SPAN_PREFIX: &str = "db.";

#[derive(Debug, Clone)]
struct RootSampler;

impl ShouldSample for RootSampler {
    fn should_sample(
        &self,
        _parent_context: Option<&Context>,
        _trace_id: TraceId,
        name: &str,
        _span_kind: &SpanKind,
        attributes: &[KeyValue],
        _links: &[Link],
    ) -> SamplingResult {
        let unattended = attributes
            .iter()
            .any(|kv| kv.key.as_str() == UNATTENDED_KEY && kv.value == Value::Bool(true));
        SamplingResult {
            decision: if unattended || name.starts_with(DB_SPAN_PREFIX) {
                SamplingDecision::Drop
            } else {
                SamplingDecision::RecordAndSample
            },
            attributes: Vec::new(),
            trace_state: TraceState::default(),
        }
    }
}

fn sampler() -> Sampler {
    Sampler::ParentBased(Box::new(RootSampler))
}

fn build_tracer_provider(export: &OtlpExport) -> Result<SdkTracerProvider> {
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(export.signal_endpoint("traces"))
        .with_headers(export.headers())
        .build()?;

    let processor = ScrubbingSpanProcessor {
        inner: BatchSpanProcessor::builder(exporter).build(),
    };

    let provider = SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_sampler(sampler())
        .with_resource(export.resource.clone())
        .build();

    global::set_tracer_provider(provider.clone());
    Ok(provider)
}

fn build_meter_provider(export: &OtlpExport) -> Result<SdkMeterProvider> {
    let exporter = MetricExporter::builder()
        .with_http()
        .with_endpoint(export.signal_endpoint("metrics"))
        .with_headers(export.headers())
        .build()?;

    let reader = PeriodicReader::builder(exporter).build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(export.resource.clone())
        .build();

    global::set_meter_provider(provider.clone());
    Ok(provider)
}

fn build_logger_provider(export: &OtlpExport) -> Result<SdkLoggerProvider> {
    let exporter = LogExporter::builder()
        .with_http()
        .with_endpoint(export.signal_endpoint("logs"))
        .with_headers(export.headers())
        .build()?;

    let provider = SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(export.resource.clone())
        .build();
    Ok(provider)
}

const PII_DROP_KEYS: &[&str] = &[
    "client.address",
    "client.socket.address",
    "net.peer.ip",
    "net.sock.peer.addr",
    "http.client_ip",
    "db.statement",
];

const URL_TRUNCATE_KEYS: &[&str] = &["url.full", "http.url", "http.target"];

#[derive(Debug)]
struct ScrubbingSpanProcessor {
    inner: BatchSpanProcessor,
}

impl SpanProcessor for ScrubbingSpanProcessor {
    fn on_start(&self, span: &mut Span, cx: &opentelemetry::Context) {
        self.inner.on_start(span, cx);
    }

    fn on_end(&self, mut span: SpanData) {
        span.attributes
            .retain(|kv| !PII_DROP_KEYS.contains(&kv.key.as_str()));
        for kv in &mut span.attributes {
            if URL_TRUNCATE_KEYS.contains(&kv.key.as_str()) {
                if let Value::String(url) = &kv.value {
                    if let Some(q) = url.as_str().find('?') {
                        let truncated = url.as_str()[..q].to_string();
                        kv.value = Value::String(truncated.into());
                    }
                }
            }
        }
        self.inner.on_end(span);
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|nom| nom.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<String>>>);

    impl SpanProcessor for Recorder {
        fn on_start(&self, _span: &mut Span, _cx: &Context) {}

        fn on_end(&self, span: SpanData) {
            self.0.lock().unwrap().push(span.name.into_owned());
        }

        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }

        fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
            Ok(())
        }

        fn set_resource(&mut self, _resource: &Resource) {}
    }

    fn exported_spans(f: impl FnOnce()) -> Vec<String> {
        let recorder = Recorder::default();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(recorder.clone())
            .with_sampler(sampler())
            .build();
        let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer(SCOPE));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), f);
        let names = recorder.0.lock().unwrap().clone();
        names
    }

    #[test]
    fn un_crawl_non_surveille_n_exporte_ni_sa_racine_ni_ses_enfants() {
        let spans = exported_spans(|| {
            unattended_request_span().in_scope(|| {
                tracing::info_span!("db.exemple").in_scope(|| {});
            });
        });
        assert!(spans.is_empty(), "spans exportes : {spans:?}");
    }

    #[test]
    fn une_requete_ordinaire_exporte_sa_racine_et_ses_enfants() {
        let spans = exported_spans(|| {
            tracing::info_span!("http.request").in_scope(|| {
                tracing::info_span!("db.exemple").in_scope(|| {});
            });
        });
        assert_eq!(spans, ["db.exemple", "http.request"]);
    }
}
