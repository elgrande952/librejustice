//! Trait commun des embedders.

use crate::error::{EmbedError, Result};
use ndarray::Array2;
use opentelemetry::KeyValue;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::instrument;

/// Cible des évènements du disjoncteur. Les deux transitions se lisent en paire
/// ou pas du tout : côté `lj-api`, l'export OTLP plafonne à WARN, donc le
/// réarmement (INFO) n'atteint Loki que si la cible y est inscrite.
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
    Dummy(DummyEmbedder),
    Cloudflare(crate::cloudflare::CloudflareWorkersAIEmbedder),
    OpenAiHttp(crate::openai_http::OpenAIHttpEmbedder),
    Auto {
        local: crate::openai_http::OpenAIHttpEmbedder,
        cloudflare: crate::cloudflare::CloudflareWorkersAIEmbedder,
        degraded: AtomicBool,
    },
}

/// Disjoncteur du mode `auto`, factorisé entre passages et requêtes via deux
/// fabriques de futures (une par backend), ré-appelables (`Fn`).
///
/// Once `degraded` latches, both backends run at once and the losing local call
/// is the recovery probe: winning the race is what re-arms `degraded=false`.
///
/// En dégradé les deux jambes émettent la même trace HTTP : le champ `backend`
/// du span est le seul à dire laquelle a rendu la réponse.
#[instrument(skip_all, fields(
    librejustice.embed.degraded = tracing::field::Empty,
    librejustice.embed.backend = tracing::field::Empty,
))]
async fn auto_embed<VF, CF, VFut, CFut>(
    degraded: &AtomicBool,
    local: VF,
    cloudflare: CF,
) -> Result<Array2<f32>>
where
    VF: Fn() -> VFut,
    CF: Fn() -> CFut,
    VFut: std::future::Future<Output = Result<Array2<f32>>>,
    CFut: std::future::Future<Output = Result<Array2<f32>>>,
{
    let span = tracing::Span::current();
    let entry_degraded = degraded.load(Ordering::Relaxed);
    span.record("librejustice.embed.degraded", entry_degraded);

    let (out, backend) = race(degraded, entry_degraded, local, cloudflare).await;

    span.record("librejustice.embed.backend", backend);
    lj_telemetry::meter()
        .u64_counter("embed.auto.served")
        .with_description("Embeddings du mode auto, par backend ayant rendu la réponse.")
        .build()
        .add(1, &[KeyValue::new("backend", backend)]);
    out
}

/// Corps du disjoncteur : rend la réponse et le backend qui l'a produite —
/// `local`, `cloudflare`, `aucun` (les deux en échec) ou `refus` (entrée
/// invalide, qui ne renseigne sur la santé d'aucun backend).
async fn race<VF, CF, VFut, CFut>(
    degraded: &AtomicBool,
    entry_degraded: bool,
    local: VF,
    cloudflare: CF,
) -> (Result<Array2<f32>>, &'static str)
where
    VF: Fn() -> VFut,
    CF: Fn() -> CFut,
    VFut: std::future::Future<Output = Result<Array2<f32>>>,
    CFut: std::future::Future<Output = Result<Array2<f32>>>,
{
    if !entry_degraded {
        return match local().await {
            Ok(r) => (Ok(r), "local"),
            Err(exc) if is_request_fault(&exc) => (Err(exc), "refus"),
            Err(exc) => {
                degraded.store(true, Ordering::Relaxed);
                tracing::warn!(error = %exc, "local KO → dégradé, repli Cloudflare");
                fallback_served(cloudflare().await)
            }
        };
    }
    tokio::select! {
        r = local() => match r {
            Ok(v) => {
                degraded.store(false, Ordering::Relaxed);
                tracing::info!("local a regagné la course → mode nominal réarmé");
                (Ok(v), "local")
            }
            Err(exc) if is_request_fault(&exc) => (Err(exc), "refus"),
            Err(_) => fallback_served(cloudflare().await),
        },
        r = cloudflare() => fallback_served(r),
    }
}

/// Étiquette la sortie du repli : `aucun` quand Cloudflare échoue à son tour,
/// donc qu'aucun des deux backends n'a servi.
fn fallback_served(r: Result<Array2<f32>>) -> (Result<Array2<f32>>, &'static str) {
    let backend = if r.is_ok() { "cloudflare" } else { "aucun" };
    (r, backend)
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
            AnyEmbedder::Dummy(e) => e.embed_passages(texts).await,
            AnyEmbedder::Cloudflare(e) => e.embed_passages(texts).await,
            AnyEmbedder::OpenAiHttp(e) => e.embed_passages(texts).await,
            AnyEmbedder::Auto {
                local,
                cloudflare,
                degraded,
            } => {
                auto_embed(
                    degraded,
                    || local.embed_passages(texts),
                    || cloudflare.embed_passages(texts),
                )
                .await
            }
        }
    }
    async fn embed_query(&self, texts: &[String]) -> Result<Array2<f32>> {
        match self {
            AnyEmbedder::Dummy(e) => e.embed_query(texts).await,
            AnyEmbedder::Cloudflare(e) => e.embed_query(texts).await,
            AnyEmbedder::OpenAiHttp(e) => e.embed_query(texts).await,
            AnyEmbedder::Auto {
                local,
                cloudflare,
                degraded,
            } => {
                auto_embed(
                    degraded,
                    || local.embed_query(texts),
                    || cloudflare.embed_query(texts),
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

    /// Fabrique de jambe : rend toujours `out`, immédiatement.
    fn leg(
        out: fn() -> Result<Array2<f32>>,
    ) -> impl Fn() -> std::future::Ready<Result<Array2<f32>>> {
        move || std::future::ready(out())
    }

    /// Jambe qui ne répond jamais. `tokio::select!` tire l'ordre de ses branches
    /// au hasard : deux jambes prêtes en même temps donnent un gagnant aléatoire,
    /// et seule celle-ci fait perdre l'autre à coup sûr.
    fn leg_lente() -> impl Fn() -> std::future::Pending<Result<Array2<f32>>> {
        std::future::pending
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
        let degraded = AtomicBool::new(false);
        let (out, backend) = race(&degraded, false, leg(ok), leg(panne)).await;
        assert!(out.is_ok());
        assert_eq!(backend, "local");
        assert!(!degraded.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn panne_du_local_arme_le_disjoncteur_et_sert_cloudflare() {
        let degraded = AtomicBool::new(false);
        let (out, backend) = race(&degraded, false, leg(panne), leg(ok)).await;
        assert!(out.is_ok());
        assert_eq!(backend, "cloudflare");
        assert!(degraded.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn les_deux_en_panne_ne_designent_aucun_backend() {
        let degraded = AtomicBool::new(false);
        let (out, backend) = race(&degraded, false, leg(panne), leg(panne)).await;
        assert!(out.is_err());
        assert_eq!(backend, "aucun");
    }

    /// Une entrée invalide n'apprend rien sur la santé du backend : elle remonte
    /// telle quelle, sans repli et sans armer le disjoncteur.
    #[tokio::test]
    async fn entree_invalide_ne_degrade_pas() {
        let degraded = AtomicBool::new(false);
        let (out, backend) = race(&degraded, false, leg(entree_invalide), leg(ok)).await;
        assert!(matches!(out, Err(EmbedError::InputTooLong)));
        assert_eq!(backend, "refus");
        assert!(!degraded.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn le_local_qui_regagne_la_course_rearme_le_mode_nominal() {
        let degraded = AtomicBool::new(true);
        let (out, backend) = race(&degraded, true, leg(ok), leg_lente()).await;
        assert!(out.is_ok());
        assert_eq!(backend, "local");
        assert!(!degraded.load(Ordering::Relaxed));
    }

    /// En dégradé, un local toujours KO laisse Cloudflare servir et le verrou
    /// fermé : sans ça chaque requête repaierait le stall du primaire.
    #[tokio::test]
    async fn degrade_avec_le_local_toujours_ko_reste_degrade() {
        let degraded = AtomicBool::new(true);
        let (out, backend) = race(&degraded, true, leg(panne), leg(ok)).await;
        assert!(out.is_ok());
        assert_eq!(backend, "cloudflare");
        assert!(degraded.load(Ordering::Relaxed));
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
