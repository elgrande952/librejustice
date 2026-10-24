//! Trait commun des embedders.

use crate::error::{EmbedError, Result};
use crate::hosts::EmbedHost;
use crate::openai_http::OpenAIHttpEmbedder;
use ndarray::Array2;
use opentelemetry::KeyValue;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::instrument;

pub const LOG_TARGET: &str = "lj_llm::backend";

/// Dimension cible des embeddings.
pub const EMBEDDING_DIM: usize = 1024;

/// Instruction Qwen3 asymétrique pour les requêtes. Doit rester identique
/// octet-pour-octet : elle entre dans le texte effectivement embedé.
pub const LEGAL_QUERY_INSTRUCTION: &str =
    "Given a French-language legal question, retrieve passages from \
French court decisions that best answer it.";

/// Préfixe une requête de l'instruction (format Qwen3-Embedding asymétrique).
///
/// Forme : `Instruct: {instruction}\nQuery: {text}`.
pub fn format_query(text: &str, instruction: &str) -> String {
    format!("Instruct: {instruction}\nQuery: {text}")
}

/// Backend d'embedding (passages vs requêtes, asymétrique). `async` car I/O HTTP.
#[allow(async_fn_in_trait)]
pub trait Embedder {
    /// Embed des passages (documents). Renvoie `(n_texts, EMBEDDING_DIM)`.
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>>;
    /// Embed des requêtes utilisateur (avec instruction).
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>>;
}

/// Dispatcher statique des backends (évite `dyn Embedder`, non object-safe à
/// cause des `async fn`). Construit par `auto`/`cloudflare`/`openai-http`.
pub enum AnyEmbedder {
    /// Explicitly unavailable: no vectors, model, or network call.
    Disabled,
    Dummy(DummyEmbedder),
    Cloudflare(crate::cloudflare::CloudflareWorkersAIEmbedder),
    OpenAiHttp(OpenAIHttpEmbedder),
    /// Local hosts in priority order, then Cloudflare once every host is down.
    Auto {
        locals: Vec<LocalHost>,
        cloudflare: crate::cloudflare::CloudflareWorkersAIEmbedder,
    },
}

/// One local host of the `auto` chain, with its own breaker.
pub struct LocalHost {
    embedder: OpenAIHttpEmbedder,
    label: String,
    degraded: AtomicBool,
}

impl LocalHost {
    pub fn new(host: &EmbedHost) -> Self {
        Self {
            embedder: OpenAIHttpEmbedder::new(host.url.clone(), host.api_key.clone(), ""),
            label: host.label.clone(),
            degraded: AtomicBool::new(false),
        }
    }
}

type Leg<'a> = Pin<Box<dyn Future<Output = Result<Array2<f32>>> + Send + 'a>>;
type Chain<'a> = Pin<Box<dyn Future<Output = (Result<Array2<f32>>, Served)> + Send + 'a>>;

/// Who served an `auto` embedding. `Refused` is an invalid input, which says
/// nothing about the health of any backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Served {
    Local(usize),
    Cloudflare,
    Nobody,
    Refused,
}

impl Served {
    fn backend(self) -> &'static str {
        match self {
            Served::Local(_) => "local",
            Served::Cloudflare => "cloudflare",
            Served::Nobody => "aucun",
            Served::Refused => "refus",
        }
    }
}

/// First answer of a latched host's race against the rest of the chain.
enum Winner {
    Host(Result<Array2<f32>>),
    Rest((Result<Array2<f32>>, Served)),
}

/// Breaker of the `auto` mode, shared between passages and queries through two
/// future factories: `local(i)` for host `i`, `cloudflare()` for the fallback.
///
/// When degraded, several legs emit the same HTTP trace: the span's `backend`
/// and `host` fields are the only ones naming the leg that answered.
#[instrument(skip_all, fields(
    librejustice.embed.degraded = tracing::field::Empty,
    librejustice.embed.backend = tracing::field::Empty,
    librejustice.embed.host = tracing::field::Empty,
))]
async fn auto_embed<'a>(
    hosts: &'a [LocalHost],
    local: &'a (dyn Fn(usize) -> Leg<'a> + Sync),
    cloudflare: &'a (dyn Fn() -> Leg<'a> + Sync),
) -> Result<Array2<f32>> {
    let span = tracing::Span::current();
    let any_degraded = hosts.iter().any(|h| h.degraded.load(Ordering::Relaxed));
    span.record("librejustice.embed.degraded", any_degraded);

    let (out, served) = race(hosts, 0, local, cloudflare).await;

    span.record("librejustice.embed.backend", served.backend());
    let mut attributes = vec![KeyValue::new("backend", served.backend())];
    if let Served::Local(i) = served {
        span.record("librejustice.embed.host", hosts[i].label.as_str());
        attributes.push(KeyValue::new("host", hosts[i].label.clone()));
    }
    lj_telemetry::meter()
        .u64_counter("embed.auto.served")
        .with_description("Embeddings du mode auto, par backend ayant rendu la réponse.")
        .build()
        .add(1, &attributes);
    out
}

/// Breaker of host `i`, whose fallback is the rest of the chain: the hosts after
/// it, then Cloudflare. A healthy host is called alone and its first failure
/// latches its breaker. A latched host races the rest of the chain, the losing
/// call being the recovery probe: winning is what rearms it.
fn race<'a>(
    hosts: &'a [LocalHost],
    i: usize,
    local: &'a (dyn Fn(usize) -> Leg<'a> + Sync),
    cloudflare: &'a (dyn Fn() -> Leg<'a> + Sync),
) -> Chain<'a> {
    Box::pin(async move {
        let Some(host) = hosts.get(i) else {
            return match cloudflare().await {
                Ok(v) => (Ok(v), Served::Cloudflare),
                Err(exc) => (Err(exc), Served::Nobody),
            };
        };
        if !host.degraded.load(Ordering::Relaxed) {
            return match local(i).await {
                Ok(v) => (Ok(v), Served::Local(i)),
                Err(exc) if is_request_fault(&exc) => (Err(exc), Served::Refused),
                Err(exc) => {
                    host.degraded.store(true, Ordering::Relaxed);
                    tracing::warn!(host = %host.label, error = %exc, "local host failed, degraded: falling back");
                    race(hosts, i + 1, local, cloudflare).await
                }
            };
        }
        let mut rest = race(hosts, i + 1, local, cloudflare);
        let winner = tokio::select! {
            r = local(i) => Winner::Host(r),
            r = &mut rest => Winner::Rest(r),
        };
        match winner {
            Winner::Host(Ok(v)) => {
                host.degraded.store(false, Ordering::Relaxed);
                tracing::info!(host = %host.label, "local host won the race again, rearmed");
                (Ok(v), Served::Local(i))
            }
            Winner::Host(Err(exc)) if is_request_fault(&exc) => (Err(exc), Served::Refused),
            Winner::Host(Err(_)) => rest.await,
            Winner::Rest(served) => served,
        }
    })
}

/// L'erreur porte-t-elle sur la **requête** plutôt que sur la santé du backend ?
///
/// Le disjoncteur n'a de sens que pour l'indisponibilité : machine éteinte,
/// connexion refusée, timeout. Un refus qui dit « cette entrée-là est invalide »
/// n'apprend rien sur le backend — le second backend la refusera pareil — et le
/// latch qu'il provoquerait dégraderait tout le run.
///
/// `InputTooLong` en est un : le chunk char-heuristique peut dépasser 8192 tokens
/// sur un texte dense. `embed_prepared` sait le rattraper (re-chunk BPE exact puis
/// retry), mais seulement si l'erreur lui **parvient**.
fn is_request_fault(err: &EmbedError) -> bool {
    matches!(
        err,
        EmbedError::InputTooLong | EmbedError::Invalid(_) | EmbedError::TokenBudgetExceeded { .. }
    )
}

impl Embedder for AnyEmbedder {
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>> {
        match self {
            AnyEmbedder::Disabled => Err(EmbedError::Invalid("embeddings are disabled in lexical-only mode".into())),
            AnyEmbedder::Dummy(e) => e.embed_passages(texts).await,
            AnyEmbedder::Cloudflare(e) => e.embed_passages(texts).await,
            AnyEmbedder::OpenAiHttp(e) => e.embed_passages(texts).await,
            AnyEmbedder::Auto { locals, cloudflare } => {
                auto_embed(
                    locals,
                    &|i| -> Leg<'_> { Box::pin(locals[i].embedder.embed_passages(texts)) },
                    &|| -> Leg<'_> { Box::pin(cloudflare.embed_passages(texts)) },
                )
                .await
            }
        }
    }
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>> {
        match self {
            AnyEmbedder::Disabled => Err(EmbedError::Invalid("embeddings are disabled in lexical-only mode".into())),
            AnyEmbedder::Dummy(e) => e.embed_query(texts).await,
            AnyEmbedder::Cloudflare(e) => e.embed_query(texts).await,
            AnyEmbedder::OpenAiHttp(e) => e.embed_query(texts).await,
            AnyEmbedder::Auto { locals, cloudflare } => {
                auto_embed(
                    locals,
                    &|i| -> Leg<'_> { Box::pin(locals[i].embedder.embed_query(texts)) },
                    &|| -> Leg<'_> { Box::pin(cloudflare.embed_query(texts)) },
                )
                .await
            }
        }
    }
}

/// Embedder déterministe par hachage (tests / mode hors-ligne).
///
/// Produit un vecteur gaussien déterministe par texte, L2-normalisé (PRNG
/// splitmix64 + Box-Muller). Utilisé en tests / hors-ligne uniquement.
#[derive(Debug, Clone, Copy)]
pub struct DummyEmbedder {
    pub dim: usize,
}

impl Default for DummyEmbedder {
    fn default() -> Self {
        Self { dim: EMBEDDING_DIM }
    }
}

impl DummyEmbedder {
    fn hash_embed(&self, texts: &[String]) -> Array2<f32> {
        let dim = self.dim;
        let mut out = Array2::<f32>::zeros((texts.len(), dim));
        for (i, text) in texts.iter().enumerate() {
            let seed = seed_from_text(text);
            let mut rng = SplitMix64::new(seed);
            let mut v: Vec<f32> = (0..dim).map(|_| rng.next_gaussian() as f32).collect();
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v {
                    *x /= norm;
                }
            }
            for (j, x) in v.into_iter().enumerate() {
                out[[i, j]] = x;
            }
        }
        out
    }
}

impl Embedder for DummyEmbedder {
    async fn embed_passages(&self, texts: &[String]) -> Result<Array2<f32>> {
        Ok(self.hash_embed(texts))
    }
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>> {
        Ok(self.hash_embed(texts))
    }
}

/// Graine 64 bits dérivée du texte (FNV-1a 64) : déterministe, pas
/// cryptographique.
fn seed_from_text(text: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &b in text.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// PRNG splitmix64 — déterministe, sans dépendance externe.
struct SplitMix64 {
    state: u64,
    spare: Option<f64>,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self {
            state: seed,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Flottant uniforme dans `(0, 1)`.
    fn next_f64(&mut self) -> f64 {
        // 53 bits de mantisse → (0,1), borne basse écartée de zéro.
        let bits = self.next_u64() >> 11;
        (bits as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Tirage gaussien centré-réduit (Box-Muller, avec spare).
    fn next_gaussian(&mut self) -> f64 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let u1 = self.next_f64();
        let u2 = self.next_f64();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_query_shape() {
        let q = format_query("ma question", "instruction X");
        assert_eq!(q, "Instruct: instruction X\nQuery: ma question");
    }

    #[test]
    fn legal_instruction_value() {
        assert_eq!(
            LEGAL_QUERY_INSTRUCTION,
            "Given a French-language legal question, retrieve passages from \
French court decisions that best answer it."
        );
    }

    #[tokio::test]
    async fn dummy_is_deterministic_and_normalized() {
        let e = DummyEmbedder::default();
        let a = e.embed_passages(&["bonjour".to_string()]).await.unwrap();
        let b = e.embed_passages(&["bonjour".to_string()]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a.shape(), &[1, EMBEDDING_DIM]);
        let norm: f32 = a.row(0).iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "norm={norm}");
    }

    #[tokio::test]
    async fn dummy_distinct_texts_differ() {
        let e = DummyEmbedder { dim: 16 };
        let a = e.embed_query(&["a".to_string()]).await.unwrap();
        let b = e.embed_query(&["b".to_string()]).await.unwrap();
        assert_ne!(a, b);
    }

    /// Hosts of the chain, each latched or not.
    fn hosts(latched: &[bool]) -> Vec<LocalHost> {
        latched
            .iter()
            .enumerate()
            .map(|(i, &latched)| {
                let url = format!("http://host{i}:8400/v1/embeddings");
                let host = LocalHost::new(&crate::hosts::parse_hosts(&url, None).unwrap()[0]);
                host.degraded.store(latched, Ordering::Relaxed);
                host
            })
            .collect()
    }

    fn latched(hosts: &[LocalHost]) -> Vec<bool> {
        hosts
            .iter()
            .map(|h| h.degraded.load(Ordering::Relaxed))
            .collect()
    }

    /// Leg that answers `out` at once.
    fn ready(out: Result<Array2<f32>>) -> Leg<'static> {
        Box::pin(std::future::ready(out))
    }

    /// Jambe qui ne répond jamais. `tokio::select!` tire l'ordre de ses branches
    /// au hasard : deux jambes prêtes en même temps donnent un gagnant aléatoire,
    /// et seule celle-ci fait perdre l'autre à coup sûr.
    fn leg_lente() -> Leg<'static> {
        Box::pin(std::future::pending())
    }

    fn ok() -> Result<Array2<f32>> {
        Ok(Array2::zeros((1, 2)))
    }
    fn panne() -> Result<Array2<f32>> {
        Err(EmbedError::Middleware(
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("backend injoignable")),
        ))
    }
    fn entree_invalide() -> Result<Array2<f32>> {
        Err(EmbedError::InputTooLong)
    }

    #[tokio::test]
    async fn nominal_sert_le_local_sans_armer_le_disjoncteur() {
        let hosts = hosts(&[false]);
        let (out, served) = race(&hosts, 0, &|_| ready(ok()), &|| ready(panne())).await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Local(0));
        assert_eq!(latched(&hosts), [false]);
    }

    #[tokio::test]
    async fn panne_du_local_arme_le_disjoncteur_et_sert_cloudflare() {
        let hosts = hosts(&[false]);
        let (out, served) = race(&hosts, 0, &|_| ready(panne()), &|| ready(ok())).await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Cloudflare);
        assert_eq!(latched(&hosts), [true]);
    }

    #[tokio::test]
    async fn les_deux_en_panne_ne_designent_aucun_backend() {
        let hosts = hosts(&[false]);
        let (out, served) = race(&hosts, 0, &|_| ready(panne()), &|| ready(panne())).await;
        assert!(out.is_err());
        assert_eq!(served, Served::Nobody);
    }

    /// Une entrée invalide n'apprend rien sur la santé du backend : elle remonte
    /// telle quelle, sans repli et sans armer le disjoncteur.
    #[tokio::test]
    async fn entree_invalide_ne_degrade_pas() {
        let hosts = hosts(&[false]);
        let (out, served) = race(&hosts, 0, &|_| ready(entree_invalide()), &|| ready(ok())).await;
        assert!(matches!(out, Err(EmbedError::InputTooLong)));
        assert_eq!(served, Served::Refused);
        assert_eq!(latched(&hosts), [false]);
    }

    #[tokio::test]
    async fn le_local_qui_regagne_la_course_rearme_le_mode_nominal() {
        let hosts = hosts(&[true]);
        let (out, served) = race(&hosts, 0, &|_| ready(ok()), &|| leg_lente()).await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Local(0));
        assert_eq!(latched(&hosts), [false]);
    }

    /// En dégradé, un local toujours KO laisse Cloudflare servir et le verrou
    /// fermé : sans ça chaque requête repaierait le stall du primaire.
    #[tokio::test]
    async fn degrade_avec_le_local_toujours_ko_reste_degrade() {
        let hosts = hosts(&[true]);
        let (out, served) = race(&hosts, 0, &|_| ready(panne()), &|| ready(ok())).await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Cloudflare);
        assert_eq!(latched(&hosts), [true]);
    }

    #[tokio::test]
    async fn a_failed_primary_hands_over_to_the_next_host_before_cloudflare() {
        let hosts = hosts(&[false, false]);
        let (out, served) = race(
            &hosts,
            0,
            &|i| if i == 0 { ready(panne()) } else { ready(ok()) },
            &|| ready(ok()),
        )
        .await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Local(1));
        assert_eq!(latched(&hosts), [true, false]);
    }

    #[tokio::test]
    async fn cloudflare_serves_only_once_every_host_is_down() {
        let hosts = hosts(&[false, false]);
        let (out, served) = race(&hosts, 0, &|_| ready(panne()), &|| ready(ok())).await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Cloudflare);
        assert_eq!(latched(&hosts), [true, true]);
    }

    #[tokio::test]
    async fn a_latched_primary_that_answers_first_is_rearmed() {
        let hosts = hosts(&[true, false]);
        let (out, served) = race(
            &hosts,
            0,
            &|i| if i == 0 { ready(ok()) } else { leg_lente() },
            &|| leg_lente(),
        )
        .await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Local(0));
        assert_eq!(latched(&hosts), [false, false]);
    }

    /// Cloudflare never answers here: reaching it would hang the test.
    #[tokio::test]
    async fn a_primary_still_down_leaves_the_second_host_serving() {
        let hosts = hosts(&[true, false]);
        let (out, served) = race(
            &hosts,
            0,
            &|i| if i == 0 { ready(panne()) } else { ready(ok()) },
            &|| leg_lente(),
        )
        .await;
        assert!(out.is_ok());
        assert_eq!(served, Served::Local(1));
        assert_eq!(latched(&hosts), [true, false]);
    }

    #[tokio::test]
    async fn dummy_batches_rows() {
        let e = DummyEmbedder { dim: 8 };
        let out = e
            .embed_passages(&["x".to_string(), "y".to_string(), "z".to_string()])
            .await
            .unwrap();
        assert_eq!(out.shape(), &[3, 8]);
    }
}
