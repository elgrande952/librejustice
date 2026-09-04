//! Telemetrie partagee : subscriber tracing + export DIRECT vers la gateway
//! OTLP Grafana Cloud (traces + logs + metriques), HTTP/protobuf + Basic Auth.
//!
//! Trois roles :
//!
//! - export OTLP/HTTP des 3 signaux vers un endpoint unique, en Basic Auth ;
//! - scrub PII des spans avant export ([`ScrubbingSpanProcessor`]) : IP et
//!   `db.statement` retires, query-strings tronquees ;
//!   `librejustice.search.query` est conserve ;
//! - echantillonnage par tete ([`RootSampler`]), qui abandonne la trace d'un
//!   crawl non surveille des son span racine.
//!
//! Sans credentials Grafana, on reste sur un simple subscriber fmt. En
//! court-vivant, [`init`] rend un [`TelemetryGuard`] dont le `Drop` flush.

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

/// Nom de scope OTel commun (tracer + meter). Les instruments (ex. scraper
/// Postgres cote `lj-api`) passent par [`meter`].
const SCOPE: &str = "librejustice";

/// Credentials de la gateway OTLP Grafana Cloud. Presents = export actif.
pub struct OtlpCreds {
    /// Base URL de la gateway (ex. `https://otlp-gateway-...grafana.net/otlp`).
    /// On y ajoute soi-même `/v1/{traces,logs,metrics}` par signal (cf.
    /// [`OtlpExport::signal_endpoint`]) : `with_endpoint` sur un builder par-signal
    /// prend l'URL telle quelle.
    pub endpoint: String,
    /// Utilisateur OTLP (numero d'instance Grafana Cloud).
    pub user: String,
    /// Cle d'API Grafana Cloud.
    pub api_key: String,
}

/// Parametres d'initialisation de la telemetrie.
pub struct InitOpts {
    /// Filtre de niveaux du subscriber (chaque binaire le construit a sa facon :
    /// `RUST_LOG` cote api, `LIBREJUSTICE_LOG_LEVEL` cote ingest).
    pub filter: EnvFilter,
    /// Format du layer fmt stdout : `true` = JSON, `false` = texte.
    pub json: bool,
    /// `service.name` du `Resource` (ex. `librejustice-api`, `librejustice-ingest`).
    pub service_name: String,
    /// `deployment.environment` du `Resource` (ex. `prod`), si pose.
    pub deployment_environment: Option<String>,
    /// `host.name` du `Resource` : la **machine**. A defaut, le hostname du
    /// noyau, qui vaut l'identifiant du conteneur des qu'on tourne dedans — d'ou
    /// l'override, pose par le compose.
    pub host_name: Option<String>,
    /// `container.name` du `Resource`, pose par le compose et donc **absent**
    /// d'un lancement sur l'hote : c'est ce qui distingue la chaine du cron d'un
    /// tir a la main.
    pub container_name: Option<String>,
    /// Plancher du bridge logs -> LogRecords OTLP (Loki), par cible. L'API reste
    /// sur `WARN` par defaut et n'ouvre `INFO` que sur sa cible d'usage ; l'ingest
    /// passe tout a `INFO` pour exporter les breadcrumbs de cycle de vie
    /// (debut/fin de phase).
    pub otlp_log_filter: Targets,
    /// Credentials OTLP. `None` = subscriber fmt seul (pas d'export).
    pub otlp: Option<OtlpCreds>,
}

/// Garde de vie de la telemetrie : flush des batch processors a la
/// destruction. A lier dans `main`
/// (`let _guard = lj_telemetry::init(...)?;`).
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

/// Config OTLP resolue (endpoint normalise + en-tete d'auth + resource).
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

    /// URL complète d'un signal : `<endpoint>/v1/<signal>` (ex.
    /// `.../otlp/v1/traces`). `with_endpoint` par-signal est utilisé verbatim, le
    /// suffixe doit donc être ajouté ici.
    fn signal_endpoint(&self, signal: &str) -> String {
        format!("{}/v1/{signal}", self.endpoint)
    }
}

/// Installe le subscriber tracing global et, si [`InitOpts::otlp`] est fourni,
/// branche l'export OTLP/HTTP des traces + logs + metriques. Renvoie un guard de
/// flush a garder vivant jusqu'a la fin du process.
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
    // Bridge logs : ce que `otlp_log_filter` laisse passer part en LogRecords
    // OTLP.
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
        "opentelemetry : traces + logs + metriques -> Grafana Cloud OTLP"
    );

    Ok(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider: Some(meter_provider),
        logger_provider: Some(logger_provider),
    })
}

/// `Meter` du scope `librejustice` : point d'acces unique pour creer des
/// instruments (ex. observables du scraper Postgres). No-op si l'export OTel
/// n'est pas actif (provider global par defaut).
pub fn meter() -> opentelemetry::metrics::Meter {
    global::meter(SCOPE)
}

/// Attribut qui marque le span racine d'une requete servie a un crawl non
/// surveille. Ecrit par [`unattended_request_span`], lu par
/// [`RootSampler`] — les deux doivent nommer la meme cle.
const UNATTENDED_KEY: &str = "librejustice.unattended";

/// Span racine d'une requete servie a un crawl non surveille.
///
/// C'est ici, et une seule fois par requete, que se prend la decision de ne pas
/// tracer : [`RootSampler`] y lit [`UNATTENDED_KEY`] et repond `Drop`.
/// Le `SpanContext` non echantillonne qui en resulte porte la decision a toute
/// la descendance via `ParentBased` — les `db.*` du rendu SSR compris.
///
/// Ce que l'appelant doit garantir : que les spans a couper naissent bien
/// **sous** ce span. Le lien parent-enfant passe par le contexte de tache de
/// `tracing`, donc chaque frontiere de tache doit le reporter explicitement
/// (`Instrument` sur le handler, sur le corps de reponse et sur les taches de
/// l'executeur Leptos, cote `lj-server`).
pub fn unattended_request_span() -> tracing::Span {
    tracing::info_span!("http.request", librejustice.unattended = true)
}

/// Prefixe des spans d'acces base. Un tel span qui se retrouve RACINE n'a aucun
/// contexte : ni requete, ni etape de chaine. Voir [`RootSampler`].
const DB_SPAN_PREFIX: &str = "db.";

/// Delegue racine du `ParentBased`. Deux regles, toutes deux sur le span racine :
/// abandon si le span porte [`UNATTENDED_KEY`], abandon si son nom commence par
/// [`DB_SPAN_PREFIX`]. Tout le reste est echantillonne.
///
/// La seconde vise l'ingest, qui appelle le depot ligne par ligne sans envelopper
/// ses commandes dans un span : chaque acces y forme une trace d'un seul span,
/// que rien ne rattache a une execution, et 500/s saturent l'export. Cote serveur
/// ces memes spans ont toujours le span de requete pour parent, donc ils ne
/// passent pas ici — mesure : 16 traces racines `db.*` par heure contre 30 979
/// pour l'ingest.
///
/// Un span qui a deja un parent ne passe pas ici — `ParentBased` suit alors le
/// bit `sampled` du parent.
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

/// Sampler du provider de traces.
fn sampler() -> Sampler {
    Sampler::ParentBased(Box::new(RootSampler))
}

/// Construit le `TracerProvider` OTLP/HTTP et le pose en global. Le
/// [`ScrubbingSpanProcessor`] nettoie la PII avant de deleguer au
/// `BatchSpanProcessor` (serialisation + transmission sur thread dedie).
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

/// Construit le `MeterProvider` OTLP/HTTP (PeriodicReader, export ~60s) et le
/// pose en global.
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

/// Construit le `LoggerProvider` OTLP/HTTP (BatchLogProcessor) consomme par le
/// bridge `tracing`.
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

/// Cles d'attributs supprimees avant export (PII directe).
const PII_DROP_KEYS: &[&str] = &[
    "client.address",
    "client.socket.address",
    "net.peer.ip",
    "net.sock.peer.addr",
    "http.client_ip",
    "db.statement",
];

/// Cles d'URL dont on tronque la query-string (`?...` retire).
const URL_TRUNCATE_KEYS: &[&str] = &["url.full", "http.url", "http.target"];

/// `SpanProcessor` qui scrub la PII puis delegue au `BatchSpanProcessor`.
///
/// C'est le seul endroit ou la PII est retiree avant qu'elle ne quitte le
/// process. `librejustice.search.query` n'est pas dans les listes : il part
/// intact.
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

/// Nom d'hote, lu du noyau : un shell non interactif et une unite systemd
/// n'exportent pas `HOSTNAME`, si bien que tout ce qui tourne hors conteneur
/// s'etiquetait « unknown » — 6 488 lignes sur 24 h, mesure du 12/08/2026.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|nom| nom.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// `SpanProcessor` qui note le nom de chaque span parvenu jusqu'a l'export.
    /// Un span abandonne par le sampler n'atteint jamais `on_end`.
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

    /// Joue `f` sous un subscriber branche sur [`sampler`] et rend les noms des
    /// spans effectivement exportes, dans l'ordre de fermeture.
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
