//! The embedder is shared for the whole process life, never re-instantiated in
//! response to a request.

use crate::config::Settings;
use lj_llm::backend::{AnyEmbedder, DummyEmbedder, LocalHost};
use lj_llm::cloudflare::CloudflareWorkersAIEmbedder;
use lj_llm::openai_http::OpenAIHttpEmbedder;

/// Construit l'embedder de requête selon `settings.embed_backend`
/// (`dummy` / `openai-http` / `cloudflare` / `auto`).
///
/// Creds manquantes (`openai-http`/`cloudflare`/`auto`) ⇒ `panic!` : la
/// signature est infaillible et l'embedder est construit une fois au démarrage,
/// une config invalide doit faire échouer le boot, pas une requête.
///
/// `auto` mode: the local hosts of `embed_hosts` in priority order, each behind
/// its own breaker, then Cloudflare Workers AI once every host is down. The
/// Cloudflare credentials are required because they back the fallback.
pub fn build_query_embedder(settings: &Settings) -> AnyEmbedder {
    match settings.embed_backend.as_str() {
        "dummy" => AnyEmbedder::Dummy(DummyEmbedder::default()),
        "openai-http" => {
            let [host] = settings.embed_hosts.as_slice() else {
                panic!("embed_backend=openai-http takes one LIBREJUSTICE_EMBED_URL, a list needs auto.")
            };
            AnyEmbedder::OpenAiHttp(OpenAIHttpEmbedder::new(
                host.url.clone(),
                host.api_key.clone(),
                "",
            ))
        }
        "cloudflare" => {
            let (account_id, token) = require_cloudflare(
                settings,
                "embed_backend=cloudflare nécessite \
                 LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID + \
                 LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN.",
            );
            AnyEmbedder::Cloudflare(CloudflareWorkersAIEmbedder::new(
                account_id,
                token,
                CloudflareWorkersAIEmbedder::DEFAULT_MODEL,
            ))
        }
        "auto" => {
            // `auto` valide les creds Cloudflare (fallback) avant de construire
            // le backend local primaire.
            let (account_id, token) = require_cloudflare(
                settings,
                "embed_backend=auto nécessite \
                 LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID + \
                 LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN pour le fallback cloudflare.",
            );
            let cloudflare = CloudflareWorkersAIEmbedder::new(
                account_id,
                token,
                CloudflareWorkersAIEmbedder::DEFAULT_MODEL,
            );
            AnyEmbedder::Auto {
                locals: settings.embed_hosts.iter().map(LocalHost::new).collect(),
                cloudflare,
            }
        }
        other => panic!("embed_backend inconnu : {other:?}"),
    }
}

/// Retourne `(account_id, backend_token)` ou `panic!` avec `msg` si l'un manque.
fn require_cloudflare<'a>(settings: &'a Settings, msg: &str) -> (&'a str, &'a str) {
    match (
        settings.cloudflare_account_id.as_deref(),
        settings.cloudflare_backend_token.as_deref(),
    ) {
        (Some(id), Some(token)) if !id.is_empty() && !token.is_empty() => (id, token),
        _ => panic!("{msg}"),
    }
}
