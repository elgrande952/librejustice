use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use lj_dtos::{
    ActivitySource, EntitySearchReport, ReadKind, ReadReport, SearchContext, SearchEngine,
};
use opentelemetry::KeyValue;

use crate::auth::OptionalUser;
use crate::state::AppState;
use crate::usage::Caller;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Page,
    App,
    Api,
    Mcp,
}

impl Surface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::App => "app",
            Self::Api => "api",
            Self::Mcp => "mcp",
        }
    }
}

fn count(kind: ReadKind, surface: Surface, provenance: &'static str) {
    lj_telemetry::meter()
        .u64_counter("librejustice.reads")
        .with_description("Lectures de décisions et de textes, par forme, surface et provenance.")
        .build()
        .add(
            1,
            &[
                KeyValue::new("kind", kind.as_str()),
                KeyValue::new("surface", surface.as_str()),
                KeyValue::new("provenance", provenance),
            ],
        );
}

pub fn served(kind: ReadKind, surface: Surface, caller: &Caller, reference: &str) {
    if !caller.first_party {
        count(kind, surface, "direct");
    }
    caller.record_read(kind, reference);
}

fn browser(state: &AppState, user: Option<&str>, headers: &HeaderMap) -> Caller {
    let identity = lj_identity::Identity::resolve(user, headers);
    Caller::http(
        ActivitySource::Web,
        user.is_some(),
        SearchContext::User,
        headers,
        &state.settings,
    )
    .pseudonymise(&identity, state.pseudonymiser.as_ref(), headers)
}

async fn within_budget(state: &AppState, headers: &HeaderMap) -> bool {
    let budget_key = crate::ratelimit::caller_key(None, headers);
    state
        .budgets
        .take(crate::ratelimit::Cost::ReadReport, budget_key.as_ref())
        .await
        != crate::ratelimit::Verdict::Exhausted
}

pub async fn report(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    headers: HeaderMap,
    Json(report): Json<ReadReport>,
) -> StatusCode {
    if !within_budget(&state, &headers).await {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    let caller = browser(&state, user.as_deref(), &headers);
    caller.record_read(report.kind, &report.reference);
    if caller.first_party {
        return StatusCode::NO_CONTENT;
    }
    match report.landing {
        true => {
            let own = own_host(&state.settings.web_base_url);
            let from = provenance(
                report.referrer.as_deref(),
                report.utm_source.as_deref(),
                &own,
            );
            count(report.kind, Surface::Page, from);
        }
        false => count(report.kind, Surface::App, "internal"),
    }
    StatusCode::NO_CONTENT
}

pub async fn report_entity_search(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    headers: HeaderMap,
    Json(report): Json<EntitySearchReport>,
) -> StatusCode {
    if !within_budget(&state, &headers).await {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    browser(&state, user.as_deref(), &headers).record_search(
        SearchEngine::Entites,
        report.query.trim(),
        crate::usage::Shape {
            page: 1,
            filters: "",
        },
        None,
    );
    StatusCode::NO_CONTENT
}

fn own_host(web_base_url: &str) -> String {
    url::Url::parse(web_base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

fn provenance(referrer: Option<&str>, utm_source: Option<&str>, own_host: &str) -> &'static str {
    if let Some(utm) = utm_source.map(str::trim).filter(|u| !u.is_empty()) {
        return origin_of(&utm.to_ascii_lowercase()).unwrap_or("other");
    }
    let Some(host) = referrer
        .and_then(|r| url::Url::parse(r).ok())
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
    else {
        return "direct";
    };
    if host == own_host || host.ends_with(&format!(".{own_host}")) {
        return "internal";
    }
    origin_of(&host).unwrap_or("other")
}

const ASSISTANTS: [(&str, &str); 7] = [
    ("chatgpt.com", "chatgpt"),
    ("chat.openai.com", "chatgpt"),
    ("claude.ai", "claude"),
    ("perplexity.ai", "perplexity"),
    ("gemini.google.com", "gemini"),
    ("copilot.microsoft.com", "copilot"),
    ("chat.mistral.ai", "mistral"),
];

const ENGINES: [&str; 8] = [
    "google",
    "bing.com",
    "duckduckgo.com",
    "qwant.com",
    "ecosia.org",
    "search.yahoo.com",
    "search.brave.com",
    "yandex",
];

fn origin_of(name: &str) -> Option<&'static str> {
    let name = name.strip_prefix("www.").unwrap_or(name);
    let under = |domain: &str| name == domain || name.ends_with(&format!(".{domain}"));
    if let Some((_, label)) = ASSISTANTS
        .iter()
        .find(|(domain, label)| under(domain) || name == *label)
    {
        return Some(label);
    }
    let labels: Vec<&str> = name.split('.').collect();
    ENGINES
        .iter()
        .any(|engine| match engine.contains('.') {
            true => under(engine),
            false => labels.contains(engine),
        })
        .then_some("search_engine")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn de(referrer: &str) -> &'static str {
        provenance(Some(referrer), None, "librejustice.fr")
    }

    #[test]
    fn un_assistant_se_reconnait_a_son_domaine() {
        assert_eq!(de("https://chatgpt.com/"), "chatgpt");
        assert_eq!(de("https://chat.openai.com/c/abc"), "chatgpt");
        assert_eq!(de("https://claude.ai/chat/123"), "claude");
        assert_eq!(de("https://www.perplexity.ai/search?q=x"), "perplexity");
        assert_eq!(de("https://chat.mistral.ai/"), "mistral");
    }

    #[test]
    fn gemini_n_est_pas_le_moteur_de_google() {
        assert_eq!(de("https://gemini.google.com/app"), "gemini");
        assert_eq!(de("https://www.google.fr/"), "search_engine");
        assert_eq!(
            de("android-app://com.google.android.googlequicksearchbox/"),
            "search_engine"
        );
    }

    #[test]
    fn un_moteur_se_reconnait_a_son_domaine() {
        assert_eq!(de("https://www.bing.com/"), "search_engine");
        assert_eq!(de("https://duckduckgo.com/"), "search_engine");
        assert_eq!(de("https://www.qwant.com/?q=x"), "search_engine");
    }

    #[test]
    fn un_domaine_qui_contient_un_nom_ne_le_prend_pas() {
        assert_eq!(de("https://notclaude.ai/"), "other");
        assert_eq!(de("https://bing.com.example.org/"), "other");
    }

    #[test]
    fn le_site_lui_meme_est_interne() {
        assert_eq!(
            de("https://librejustice.fr/decision-search?q=bail"),
            "internal"
        );
        assert_eq!(de("https://beta.librejustice.fr/"), "internal");
    }

    #[test]
    fn sans_referer_l_arrivee_est_directe() {
        assert_eq!(provenance(None, None, "librejustice.fr"), "direct");
        assert_eq!(
            provenance(Some("n'importe quoi"), None, "librejustice.fr"),
            "direct"
        );
    }

    #[test]
    fn l_utm_prime_sur_le_referer() {
        let de_utm = |utm| {
            provenance(
                Some("https://www.google.com/"),
                Some(utm),
                "librejustice.fr",
            )
        };
        assert_eq!(de_utm("chatgpt.com"), "chatgpt");
        assert_eq!(de_utm("perplexity"), "perplexity");
        assert_eq!(de_utm("newsletter"), "other");
        assert_eq!(de_utm("  "), "search_engine");
    }
}
