//! Recherche hybride (BM25 ParadeDB `@@@` + ANN `<=>`).
//!
//! 1. `embed_query` produit le vecteur requête (chemin hybride).
//! 2. En parallèle : BM25 body (`decisions_bm25` sur `full_text`, phrase-combo ou
//!    traduction booléenne), BM25 titre (`search_title`), et ANN VectorChord sur
//!    `decision_chunks` — la seule jambe encore au grain chunk (max-pool par
//!    décision).
//! 3. Fusion RRF au grain décision, pondération adaptative, cap et seuil relatif.
//! 4. Hydratation métadonnées + facettes + snippets.
//!
//! Le tri stable tie-break par `source_uid`/id vient de l'ordre déterministe des
//! `ORDER BY` SQL et du tri final sur le score puis l'id.

mod adaptive;
mod aggregate;
mod browse;
mod dates;
mod filters;
mod hydrate;
mod legs;
mod query;
pub mod suggest;

use std::collections::HashMap;

use deadpool_postgres::Client;
use lj_llm::backend::Embedder;
use lj_store::repository::DecisionRepository;

use crate::error::{ApiError, Result};
use crate::referential::referential;
use crate::rerank::{
    cached_order, rerank_shortlist, DocScores, RerankItem, RERANK_BUDGET, RERANK_K,
};
use crate::signals;
use crate::state::AppState;

use lj_dtos::{
    FacetChoice, LegalInstrumentFacet, PinnedDecision, QueryMode, SearchFacets, SearchHit,
    SearchRequest, SearchResponse, SortOrder,
};

use adaptive::compute_adaptive_weights;
use aggregate::aggregate_facets;
use hydrate::{
    assemble_page, display_title, facets_from_counters, fetch_pub_ids, hydrate_decisions,
    DecisionMeta,
};
use legs::{
    ann_leg, bm25_leg, bm25_parse_leg, docket_leg, fuse_ranks, pool_max_per_decision, title_leg,
    truncate_fused, LegHit, Legs,
};
use query::{
    detect_query_mode, has_boolean_operator, is_boolean_query, query_lacks_searchable_terms,
    translate_boolean,
};

pub(crate) use browse::has_filters;
pub(crate) use dates::{parse_search_date, DateError, DATE_GE, DATE_LE};
pub use query::{body_query_for_arm, BodyArm};

// ── API des bancs offline ────────────────────────────────────────────────────

/// Whether the offline benches run the title leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleLeg {
    On,
    Off,
}

/// Pure lexical retrieval for the offline ranking A/B: BM25 body leg (body built
/// per `arm`), title leg, RRF at base weights, relative truncation.
pub async fn lexical_rank_for_arm(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
    title: TitleLeg,
) -> anyhow::Result<Vec<i64>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let bm25_scores = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    let title_scores = match title {
        TitleLeg::On => title_leg(conn, &req, leg_limit).await?,
        TitleLeg::Off => HashMap::new(),
    };
    let empty: HashMap<i64, LegHit> = HashMap::new();
    let no_docket: HashMap<i64, f64> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &empty,
            title: &title_scores,
            docket: &no_docket,
        },
        signals::BASE_WEIGHTS,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Hybrid retrieval for the offline ranking A/B: query embedding, body / ANN /
/// title legs, adaptive RRF weights, relative truncation.
pub async fn hybrid_rank_for_arm(
    state: &AppState,
    query: &str,
    arm: BodyArm,
    title: TitleLeg,
) -> anyhow::Result<Vec<i64>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;

    let query_vec = embed_query_cached(state, &req.query).await?;

    let body_query = body_query_for_arm(query, arm);
    let bm25_conn = client(state).await?;
    let mut ann_conn = client(state).await?;
    let title_conn = client(state).await?;
    let (bm25_scores, ann_hits, title_scores) = tokio::try_join!(
        bm25_parse_leg(&bm25_conn, &req, &body_query, leg_limit),
        ann_leg(&mut ann_conn, &req, &query_vec, leg_limit, vchord_probes),
        async {
            match title {
                TitleLeg::On => title_leg(&title_conn, &req, leg_limit).await,
                TitleLeg::Off => Ok(HashMap::new()),
            }
        },
    )?;
    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let no_docket: HashMap<i64, f64> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &ann_best,
            title: &title_scores,
            docket: &no_docket,
        },
        weights,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Ce qu'un balayage de plancher ANN observe ([`hybrid_rank_ann_floor_sweep`]).
pub struct AnnFloorSweep {
    /// Similarité cosinus du chunk gagnant, par décision atteinte par la jambe.
    pub ann_similarity: HashMap<i64, f64>,
    /// Similarité du dernier chunk rendu, avant plancher. Sous un plancher, la
    /// jambe a épuisé ses appariements et son décompte devient exact.
    pub ann_worst_similarity: Option<f64>,
    /// Classement fusionné, un par plancher, dans l'ordre des `floors` reçus.
    pub rankings: Vec<Vec<i64>>,
}

/// Balayage offline d'un plancher sur la jambe ANN : une seule récupération, une
/// fusion par plancher, poids adaptatifs recalculés sur la jambe filtrée.
/// `floors` en similarité cosinus minimale, `f64::NEG_INFINITY` valant prod.
pub async fn hybrid_rank_ann_floor_sweep(
    state: &AppState,
    query: &str,
    floors: &[f64],
    ann_limit: i64,
) -> anyhow::Result<AnnFloorSweep> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let leg_limit = state.settings.leg_limit as i64;

    let query_vec = embed_query_cached(state, &req.query).await?;
    let bm25_conn = client(state).await?;
    let mut ann_conn = client(state).await?;
    let title_conn = client(state).await?;
    let (bm25_scores, ann_hits, title_scores) = tokio::try_join!(
        bm25_leg(&bm25_conn, &req, leg_limit),
        ann_leg(
            &mut ann_conn,
            &req,
            &query_vec,
            ann_limit,
            state.settings.vchord_probes
        ),
        title_leg(&title_conn, &req, leg_limit),
    )?;
    let no_docket: HashMap<i64, f64> = HashMap::new();

    let mut rankings = Vec::with_capacity(floors.len());
    for floor in floors {
        let kept: Vec<LegHit> = ann_hits
            .iter()
            .filter(|h| h.raw_score >= *floor)
            .cloned()
            .collect();
        let ann_best = pool_max_per_decision(&kept, ann_limit as usize);
        let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &kept).await?;
        let fused = truncate_fused(fuse_ranks(
            Legs {
                bm25: &bm25_scores,
                ann: &ann_best,
                title: &title_scores,
                docket: &no_docket,
            },
            weights,
        ));
        rankings.push(fused.into_iter().map(|(d, _, _)| d).collect());
    }

    Ok(AnnFloorSweep {
        ann_similarity: pool_max_per_decision(&ann_hits, ann_limit as usize)
            .into_iter()
            .map(|(d, h)| (d, h.raw_score))
            .collect(),
        ann_worst_similarity: ann_hits
            .iter()
            .map(|h| h.raw_score)
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
        rankings,
    })
}

/// Recherche complète SANS le cache résultats moka (banc de charge, sonde
/// profonde) : exécute toujours récupération + RRF + hydratation + assemblage de
/// page, en conservant le permit d'admission et le cache embeddings. `ai_mode`
/// ignoré. Le décompte n'en fait pas partie, comme sur le chemin prod.
pub async fn search_nocache(state: &AppState, req: &SearchRequest) -> Result<SearchResponse> {
    nocache(state, req, false).await
}

/// La même chose, décompte résolu sur la MÊME récupération : ce qui permet de
/// comparer le total annoncé à la liste servie.
pub async fn search_nocache_counted(
    state: &AppState,
    req: &SearchRequest,
) -> Result<SearchResponse> {
    nocache(state, req, true).await
}

async fn nocache(
    state: &AppState,
    req: &SearchRequest,
    avec_decompte: bool,
) -> Result<SearchResponse> {
    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let t0 = std::time::Instant::now();
    let (ranked, retrieval_ms) = {
        let _db_permit = state
            .search_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
        let resolution = resolve(state, req).await?;
        let (retrieval, mode) = retrieve(state, req, resolution, leg_limit, vchord_probes).await?;
        let retrieval_ms = t0.elapsed().as_secs_f64() * 1000.0;
        (
            std::sync::Arc::new(rank_results(state, retrieval, mode).await?),
            retrieval_ms,
        )
    };
    let hydrate_ms = t0.elapsed().as_secs_f64() * 1000.0 - retrieval_ms;
    if avec_decompte {
        comptage(state, req, &ranked).await?;
    }
    let t1 = std::time::Instant::now();
    let resp = paginate(state, req, &ranked, None).await?;
    tracing::info!(
        target: "lj_api::timing",
        retrieval_ms,
        hydrate_ms,
        paginate_ms = t1.elapsed().as_secs_f64() * 1000.0,
        "search_nocache phases"
    );
    Ok(resp)
}

/// Embedding requête via le cache in-process : un texte → un vecteur stable,
/// partagé entre les bras et les jambes d'une même requête.
async fn embed_query_cached(state: &AppState, text: &str) -> anyhow::Result<Vec<f32>> {
    let arr = match state.embedding_cache.get(text).await {
        Some(vec) => vec,
        None => {
            let owned = text.to_string();
            let arr = state
                .embedder
                .embed_query(std::slice::from_ref(&owned))
                .await
                .map_err(|e| anyhow::anyhow!("embed: {e}"))?;
            let vec = std::sync::Arc::new(arr.row(0).to_owned());
            state
                .embedding_cache
                .set(text, std::sync::Arc::clone(&vec))
                .await;
            vec
        }
    };
    Ok(arr.iter().copied().collect())
}

/// Latence isolée de la jambe BM25 body pour un bras — la seule jambe qui varie entre bras. Renvoie `(nb_hits, durée
/// d'UN appel)` ; l'appelant boucle pour la statistique.
pub async fn time_body_leg_for_arm(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
) -> anyhow::Result<(usize, std::time::Duration)> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let t = std::time::Instant::now();
    let hits = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    Ok((hits.len(), t.elapsed()))
}

/// Ranking de la seule jambe body BM25, sans jambe titre ni fusion RRF, pour un
/// bras de construction de requête. Renvoie `(decision_id, score)` trié score
/// décroissant puis id croissant.
pub async fn body_leg_ranks(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
) -> anyhow::Result<Vec<(i64, f64)>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let scores = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    let mut ranked: Vec<(i64, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    Ok(ranked)
}

/// Latence isolée de la jambe titre (banc `arm-latency`). Renvoie (nb_hits,
/// durée).
pub async fn time_title_leg(
    conn: &Client,
    query: &str,
    leg_limit: i64,
) -> anyhow::Result<(usize, std::time::Duration)> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let t = std::time::Instant::now();
    let scores = title_leg(conn, &req, leg_limit).await?;
    Ok((scores.len(), t.elapsed()))
}

/// Latence isolée de la jambe ANN (banc `arm-latency`), embedding hors chrono.
/// Renvoie (nb_hits, durée de la requête ANN seule).
pub async fn time_ann_leg(
    state: &AppState,
    conn: &mut Client,
    query: &str,
    leg_limit: i64,
) -> anyhow::Result<(usize, std::time::Duration)> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let query_vec = embed_query_cached(state, &req.query).await?;
    let probes = state.settings.vchord_probes;
    let t = std::time::Instant::now();
    let hits = ann_leg(conn, &req, &query_vec, leg_limit, probes).await?;
    Ok((hits.len(), t.elapsed()))
}

// ── Orchestration ──────────────────────────────────────────────────────────

/// Hydratation DB de tous les hits rankés (≤100) : metadata seule, aucun
/// highlight. C'est le Tier 1 du cache (cf. [`RankedResults`]) ; le highlight est
/// par-page ([`assemble_page`]).
async fn hydrate_all(
    state: &AppState,
    fused: &[(i64, f64, LegHit)],
) -> Result<HashMap<i64, DecisionMeta>> {
    let decision_ids: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
    let meta_conn = client(state).await?;
    hydrate_decisions(&meta_conn, &decision_ids)
        .await
        .map_err(ApiError::Store)
}

async fn client(state: &AppState) -> Result<Client> {
    state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))
}

/// Hydrate des `SearchHit` complets pour une liste ordonnée d'ids de décisions,
/// sans requête de recherche (score 0, snippet = tête du texte). Renvoie
/// `(decision_id, hit)` dans l'ordre d'entrée ; les ids sans metadata affichable
/// sont silencieusement omis.
pub(crate) async fn hits_for_decision_ids(
    conn: &Client,
    decision_ids: &[i64],
    refs: &crate::referential::Referential,
) -> Result<Vec<(i64, SearchHit)>> {
    let meta = hydrate_decisions(conn, decision_ids)
        .await
        .map_err(ApiError::Store)?;
    let page: Vec<(i64, f64, LegHit)> = decision_ids
        .iter()
        .filter(|id| meta.contains_key(id))
        .map(|id| (*id, 0.0, LegHit::synthetic_title_only(*id)))
        .collect();
    let hits = assemble_page(conn, &page, &meta, "", refs).await?;
    Ok(page.iter().map(|(id, _, _)| *id).zip(hits).collect())
}

/// Ordre relevance → ordre d'affichage : trie une vue de la liste fused par date
/// via la metadata hydratée, sans tie-break. `sort_by` stable + `order` déjà en
/// pertinence ⇒ égalité de date = ordre de pertinence préservé.
fn sort_fused(
    order: &mut [&(i64, f64, LegHit)],
    meta: &HashMap<i64, DecisionMeta>,
    sort: SortOrder,
) {
    let date_of = |d: i64| {
        meta.get(&d)
            .and_then(|m| m.date_lecture.clone())
            .unwrap_or_default()
    };
    match sort {
        SortOrder::Relevance => {}
        SortOrder::DateDesc => order.sort_by_key(|x| std::cmp::Reverse(date_of(x.0))),
        SortOrder::DateAsc => order.sort_by_key(|x| date_of(x.0)),
    }
}

/// Retrieval output: the fused list, and the card above it when the resolution
/// carries one.
struct Retrieval {
    fused: Vec<(i64, f64, LegHit)>,
    lexicales: std::collections::HashSet<i64>,
    pinned: Vec<i64>,
}

/// How many decisions the card above a content list holds.
const PINNED_MAX: usize = 3;

/// What `auto` decides for a query, or what a stated mode collapses to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Reference,
    Lexical,
    Semantic,
    LexicalWithCard,
    SemanticWithCard,
}

impl Resolution {
    fn query_mode(self) -> QueryMode {
        match self {
            Resolution::Reference => QueryMode::Reference,
            Resolution::Lexical | Resolution::LexicalWithCard => QueryMode::Lexical,
            Resolution::Semantic | Resolution::SemanticWithCard => QueryMode::Hybrid,
        }
    }

    fn with_card(self) -> bool {
        matches!(
            self,
            Resolution::LexicalWithCard | Resolution::SemanticWithCard
        )
    }
}

/// What a content query designates besides what it asks: a docket number, or a
/// case name the corpus knows, by its matched key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Designation {
    Number,
    Name(String),
}

impl Designation {
    /// The designation is the whole query: nothing is left to embed.
    fn consumes(&self, query: &str) -> bool {
        match self {
            Designation::Number => lj_core::docket_key::is_bare_number(query),
            Designation::Name(key) => lj_core::case_names::case_name_key(query) == *key,
        }
    }

    /// The rest of the query cites the designation, a court named: the query
    /// is a reference. A number takes every word carrying a digit with it.
    fn cited(&self, query: &str) -> bool {
        let folded = lj_core::case_names::case_name_key(query);
        let words: Vec<&str> = folded.split(' ').collect();
        let around: Vec<&str> = match self {
            Designation::Number => words
                .into_iter()
                .filter(|w| !w.chars().any(|c| c.is_ascii_digit()))
                .collect(),
            Designation::Name(key) => lj_core::case_names::words_around(&words, key),
        };
        lj_core::case_names::remainder_cites(&around)
    }
}

/// A stated mode is served as stated, without a card; `auto` settles the
/// declared reference on the query shape and leaves the rest to [`resolve_content`].
fn resolve_shape(req: &SearchRequest) -> Option<Resolution> {
    match req.mode {
        lj_dtos::SearchMode::Lexical => Some(Resolution::Lexical),
        lj_dtos::SearchMode::Semantic => Some(Resolution::Semantic),
        lj_dtos::SearchMode::Auto => {
            (lj_core::docket_key::declares_reference_explicitly(&req.query)
                && !has_boolean_operator(&req.query))
            .then_some(Resolution::Reference)
        }
    }
}

/// The content mode, and a card above it when the query designates a decision.
/// A designation cited with its court is the reference; one that consumes the
/// query leaves nothing for the ANN: lexical.
fn resolve_content(query: &str, designation: Option<Designation>) -> Resolution {
    let lexical = detect_query_mode(query) == QueryMode::Lexical;
    match designation {
        None if lexical => Resolution::Lexical,
        None => Resolution::Semantic,
        Some(d) if d.cited(query) && !has_boolean_operator(query) => Resolution::Reference,
        Some(d) if lexical || d.consumes(query) => Resolution::LexicalWithCard,
        Some(_) => Resolution::SemanticWithCard,
    }
}

async fn designation(state: &AppState, query: &str) -> Result<Option<Designation>> {
    if !lj_core::docket_key::query_docket_keys(query).is_empty() {
        return Ok(Some(Designation::Number));
    }
    let keys = lj_core::case_names::candidate_keys(query);
    if !crate::referential::referential(state)
        .await?
        .names_a_case(&keys)
    {
        return Ok(None);
    }
    let conn = client(state).await?;
    Ok(DecisionRepository::new(&conn)
        .case_name_match(&keys)
        .await
        .map_err(ApiError::Store)?
        .map(|m| Designation::Name(m.key)))
}

async fn resolve(state: &AppState, req: &SearchRequest) -> Result<Resolution> {
    if let Some(resolution) = resolve_shape(req) {
        return Ok(resolution);
    }
    let designation = designation(state, &req.query).await?;
    Ok(resolve_content(&req.query, designation))
}

/// The legs a resolution runs, and the mode served. A card outcome runs the
/// reference retrieval after the content one, connections released in between,
/// and keeps its top [`PINNED_MAX`]; an empty content list is replaced by it.
async fn retrieve(
    state: &AppState,
    req: &SearchRequest,
    resolution: Resolution,
    leg_limit: i64,
    vchord_probes: u32,
) -> Result<(Retrieval, QueryMode)> {
    let mode = resolution.query_mode();
    if query_lacks_searchable_terms(&req.query) {
        let empty = Retrieval {
            fused: Vec::new(),
            lexicales: std::collections::HashSet::new(),
            pinned: Vec::new(),
        };
        return Ok((empty, mode));
    }
    let mut retrieval = match mode {
        QueryMode::Reference => retrieve_reference(state, req, leg_limit).await?,
        QueryMode::Hybrid => retrieve_hybrid(state, req, leg_limit, vchord_probes).await?,
        _ => retrieve_boolean(state, req, leg_limit).await?,
    };
    if resolution.with_card() {
        let card = retrieve_reference(state, req, leg_limit).await?;
        if retrieval.fused.is_empty() && !card.fused.is_empty() {
            return Ok((card, QueryMode::Reference));
        }
        retrieval.pinned = card
            .fused
            .iter()
            .take(PINNED_MAX)
            .map(|(d, _, _)| *d)
            .collect();
    }
    Ok((retrieval, mode))
}

/// Exact key sub-leg of the reference retrieval: the bearers of the query's
/// docket keys, or of the numbers of the decisions its case name resolves to.
async fn key_leg(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
) -> Result<HashMap<i64, f64>> {
    let numbers = lj_core::docket_key::query_docket_keys(&req.query);
    let name_keys = match numbers.is_empty() {
        true => lj_core::case_names::candidate_keys(&req.query),
        false => Vec::new(),
    };
    if numbers.is_empty()
        && !crate::referential::referential(state)
            .await?
            .names_a_case(&name_keys)
    {
        return Ok(HashMap::new());
    }
    let conn = client(state).await?;
    let (keys, within) = match numbers.is_empty() {
        false => (numbers, None),
        true => {
            let Some(named) = DecisionRepository::new(&conn)
                .case_name_match(&name_keys)
                .await
                .map_err(ApiError::Store)?
            else {
                return Ok(HashMap::new());
            };
            (
                named
                    .decisions
                    .iter()
                    .flat_map(|d| &d.docket_numbers)
                    .map(|n| lj_core::docket_key::docket_key(n))
                    .collect(),
                Some(named.decisions.iter().map(|d| d.id).collect::<Vec<i64>>()),
            )
        }
    };
    docket_leg(&conn, req, &keys, within.as_deref(), leg_limit)
        .await
        .map_err(ApiError::Store)
}

/// Retrieval for a query that designates a decision: exact key leg plus title leg,
/// the only two that index a decision's identity. No embedding and no ANN, so an
/// absent reference answers with an empty list instead of its neighbours.
#[tracing::instrument(skip(state, req), fields(leg_limit))]
async fn retrieve_reference(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
) -> Result<Retrieval> {
    let title_conn = client(state).await?;
    let title_scores = title_leg(&title_conn, req, leg_limit)
        .await
        .map_err(ApiError::Store)?;
    let key_scores = key_leg(state, req, leg_limit).await?;
    let weights = signals::Weights {
        docket: signals::PURE_REFERENCE_DOCKET_WEIGHT,
        ..signals::BASE_WEIGHTS
    };
    let empty: HashMap<i64, LegHit> = HashMap::new();
    let no_bm25: HashMap<i64, f64> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &no_bm25,
            ann: &empty,
            title: &title_scores,
            docket: &key_scores,
        },
        weights,
    ));
    Ok(Retrieval {
        fused,
        lexicales: title_scores.keys().copied().collect(),
        pinned: Vec::new(),
    })
}

#[tracing::instrument(skip(state, req), fields(leg_limit))]
async fn retrieve_boolean(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
) -> Result<Retrieval> {
    let bm25_conn = client(state).await?;
    let body_query = if is_boolean_query(&req.query) {
        translate_boolean(&req.query)
    } else {
        req.query.clone()
    };
    let bm25_scores = if is_boolean_query(&req.query) {
        bm25_parse_leg(&bm25_conn, req, &body_query, leg_limit).await
    } else {
        bm25_leg(&bm25_conn, req, leg_limit).await
    }
    .map_err(ApiError::Store)?;
    let empty: HashMap<i64, LegHit> = HashMap::new();
    let none: HashMap<i64, f64> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &empty,
            title: &none,
            docket: &none,
        },
        signals::BASE_WEIGHTS,
    ));
    Ok(Retrieval {
        fused,
        lexicales: bm25_scores.keys().copied().collect(),
        pinned: Vec::new(),
    })
}

#[tracing::instrument(skip(state, req), fields(leg_limit, vchord_probes))]
async fn retrieve_hybrid(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
) -> Result<Retrieval> {
    let query_arr = match state.embedding_cache.get(&req.query).await {
        Some(vec) => {
            crate::cache_metrics::record_access("embedding", true);
            vec
        }
        None => {
            crate::cache_metrics::record_access("embedding", false);
            let arr = state
                .embedder
                .embed_query(std::slice::from_ref(&req.query))
                .await
                .map_err(|e| ApiError::Internal(format!("embed: {e}")))?;
            let vec = std::sync::Arc::new(arr.row(0).to_owned());
            state
                .embedding_cache
                .set(&req.query, std::sync::Arc::clone(&vec))
                .await;
            vec
        }
    };
    let query_vec: Vec<f32> = query_arr.iter().copied().collect();

    // Les connexions des jambes sont relâchées dès le `try_join` fini, AVANT
    // `compute_adaptive_weights` qui en reprend 2 : le pic reste à
    // `PEAK_CONNS_PER_SEARCH`.
    let (bm25_scores, ann_hits) = {
        let bm25_conn = client(state).await?;
        let mut ann_conn = client(state).await?;
        tokio::try_join!(
            bm25_leg(&bm25_conn, req, leg_limit),
            ann_leg(&mut ann_conn, req, &query_vec, leg_limit, vchord_probes),
        )
        .map_err(ApiError::Store)?
    };

    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let none: HashMap<i64, f64> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        Legs {
            bm25: &bm25_scores,
            ann: &ann_best,
            title: &none,
            docket: &none,
        },
        weights,
    ));
    Ok(Retrieval {
        fused,
        lexicales: bm25_scores.keys().copied().collect(),
        pinned: Vec::new(),
    })
}

/// Clé du cache rerank : `(query, ensemble TRIÉ du top-[`RERANK_K`])`. Trié parce
/// que la sortie du rerank ne dépend que du set de documents et de la query — deux
/// récupérations au même top-50 partagent l'ordre LLM.
fn rerank_cache_key(query: &str, top_ids: &[i64]) -> String {
    let mut ids = top_ids.to_vec();
    ids.sort_unstable();
    let joined = ids
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("{query}|{joined}")
}

/// Applique l'ordre LLM `new_ids` sur le head (top-[`RERANK_K`]) de `base` et
/// renvoie la séquence complète d'ids en ordre relevance reranké : head réordonné,
/// ids du head non couverts remis en queue de head dans l'ordre RRF, puis la tail
/// intacte. Conserve tous les ids de `base`.
fn rerank_id_order(base: &[(i64, f64, LegHit)], new_ids: &[i64]) -> Vec<i64> {
    let head_len = RERANK_K.min(base.len());
    let head_ids: Vec<i64> = base[..head_len].iter().map(|(d, _, _)| *d).collect();
    let mut remaining: std::collections::HashSet<i64> = head_ids.iter().copied().collect();
    let mut out: Vec<i64> = Vec::with_capacity(base.len());
    for d in new_ids {
        if remaining.remove(d) {
            out.push(*d);
        }
    }
    for d in &head_ids {
        if remaining.remove(d) {
            out.push(*d);
        }
    }
    out.extend(base[head_len..].iter().map(|(d, _, _)| *d));
    out
}

/// Ordre LLM du top-[`RERANK_K`] si le mode IA s'applique, sinon `None` (ordre RRF
/// naturel). No-op silencieux si : `ai_mode` désactivé, retrieval lexical, sort ≠
/// relevance, clé Mistral absente, shortlist < 3, budget de rerank épuisé, ou échec
/// du rerank.
///
/// Deux caches en série : l'ordre complet, keyé `(query, top-ids)` ; puis les
/// notes, keyées `(query, document)`, indépendantes de la shortlist. Seuls les
/// documents sous [`TARGET_SCORES`] repartent au LLM.
///
/// Body-source = `bundle.meta`, donc zéro requête DB ici.
async fn reranked_order(
    state: &AppState,
    req: &SearchRequest,
    bundle: &RankedResults,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<Option<std::sync::Arc<Vec<i64>>>> {
    if !req.ai_mode
        || bundle.query_mode == QueryMode::Lexical
        || req.sort != SortOrder::Relevance
        || bundle.fused.len() < 3
    {
        return Ok(None);
    }

    let head_len = RERANK_K.min(bundle.fused.len());
    let top_ids: Vec<i64> = bundle.fused[..head_len]
        .iter()
        .map(|(d, _, _)| *d)
        .collect();

    let refs = referential(state).await?;
    let items: Vec<RerankItem> = top_ids
        .iter()
        .filter_map(|d| {
            let m = bundle.meta.get(d)?;
            let summary = m.summary.clone().filter(|s| !s.is_empty())?;
            Some(RerankItem {
                decision_id: *d,
                title: display_title(m, &refs),
                summary,
            })
        })
        .collect();
    if items.len() < 3 {
        return Ok(None);
    }

    // Single-flight : `optionally_get_with` ne laisse qu'un appelant exécuter le
    // rerank pour une clé donnée. Le `None` — mode IA renoncé, échec, budget épuisé —
    // n'est pas mis en cache.
    let key = rerank_cache_key(&req.query, &top_ids);
    let order = state
        .rerank_cache
        .optionally_get_with(key, llm_order(state, req, items, budget_key))
        .await;
    Ok(order)
}

/// Ordre LLM pour une shortlist qui n'est pas déjà dans le cache d'ordre : notes
/// en cache d'abord, appels Mistral pour le reste. `None` = renoncement, et
/// l'appelant sert l'ordre RRF.
async fn llm_order(
    state: &AppState,
    req: &SearchRequest,
    items: Vec<RerankItem>,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Option<std::sync::Arc<Vec<i64>>> {
    let qkey: std::sync::Arc<str> = std::sync::Arc::from(req.query.as_str());
    let mut scores: HashMap<i64, DocScores> = HashMap::new();
    for it in &items {
        if let Some(v) = state
            .rerank_scores_cache
            .get(&(std::sync::Arc::clone(&qkey), it.decision_id))
            .await
        {
            scores.insert(it.decision_id, v);
        }
    }
    if let Some(order) = cached_order(&items, &scores) {
        return Some(std::sync::Arc::new(order));
    }

    let Some(client) = state.mistral.as_ref() else {
        tracing::warn!(
            "ai_mode requested but LIBREJUSTICE_MISTRAL_API_KEYS missing — skipping rerank"
        );
        return None;
    };
    {
        let conn = match state.pool.get().await {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!(error = %err, "checkout connexion pour les clés Mistral — skipping rerank");
                return None;
            }
        };
        let disabled = match DecisionRepository::new(&conn)
            .disabled_mistral_key_fingerprints()
            .await
        {
            Ok(disabled) => disabled,
            Err(err) => {
                tracing::warn!(error = %err, "lecture de mistral_key_status — skipping rerank");
                return None;
            }
        };
        client.set_dead_fingerprints(&disabled);
        if client.live_key_count() == 0 {
            tracing::warn!(
                "toutes les clés Mistral sont désactivées (mistral_key_status) — skipping rerank"
            );
            return None;
        }
    }

    // Un jeton = un rerank, quel que soit le nombre d'appels qu'il déclenche : le
    // cache de notes le fait varier de ~4 à 15. Le jeton est calé sur le pire cas.
    if state
        .budgets
        .take(crate::ratelimit::Cost::Rerank, budget_key)
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return None;
    }

    let sink: crate::rerank::ScoreSink = {
        let cache = state.rerank_scores_cache.clone();
        let qkey = std::sync::Arc::clone(&qkey);
        std::sync::Arc::new(move |batch: HashMap<i64, f64>| {
            let cache = cache.clone();
            let qkey = std::sync::Arc::clone(&qkey);
            tokio::spawn(async move {
                let now = tokio::time::Instant::now();
                for (decision_id, score) in batch {
                    cache
                        .entry((std::sync::Arc::clone(&qkey), decision_id))
                        .and_upsert_with(|old| async move {
                            DocScores::merged(old.map(|e| e.into_value()), &[score], now)
                        })
                        .await;
                }
            });
        })
    };

    let result = rerank_shortlist(
        items,
        &req.query,
        std::sync::Arc::clone(client),
        Some(tokio::time::Instant::now() + RERANK_BUDGET),
        &scores,
        sink,
    )
    .await;

    let spent = client.drain_spent_fingerprints();
    if !spent.is_empty() {
        match state.pool.get().await {
            Ok(conn) => {
                let repo = DecisionRepository::new(&conn);
                for fp in &spent {
                    if let Err(err) = repo.mark_mistral_key_disabled(fp, 401, "rerank").await {
                        tracing::warn!(fingerprint = %fp, error = %err, "mark_mistral_key_disabled failed");
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "checkout connexion pour mark_mistral_key_disabled")
            }
        }
    }

    match result {
        Ok(new_ids) => Some(std::sync::Arc::new(new_ids)),
        Err(err) => {
            tracing::warn!(error = %err, "rerank_shortlist failed — falling back to retrieval order");
            None
        }
    }
}

/// Bundle de recherche mis en cache : récupération (BM25/ANN, RRF), facettes et
/// hydratation complète des hits (metadata + summary), AVANT rerank, tri par date
/// et pagination. Partagé via `Arc` dans le cache moka de l'[`AppState`], clé
/// ai_mode-indépendante. Le `full_text` n'est pas caché : il est fetché par-page
/// par [`assemble_page`].
pub struct RankedResults {
    query_mode: QueryMode,
    /// Liste fused en ordre RRF (pré-rerank, pré-tri). Porte le `chunk_id` du chunk
    /// gagnant ANN et les scores.
    fused: Vec<(i64, f64, LegHit)>,
    /// Metadata hydratée de tous les hits (≤100), `summary` inclus. Sert au tri par
    /// date, à l'assemblage de page et de texte-source au rerank. Pas de highlight.
    meta: HashMap<i64, DecisionMeta>,
    /// `decision_id → public_id` de tous les hits.
    pub_ids: HashMap<i64, String>,
    /// Servies que la question n'apparie pas, portées ici pour que le décompte
    /// décrive la récupération qui a servi CETTE liste.
    hors_question: Vec<i64>,
    comptage: tokio::sync::OnceCell<(i64, SearchFacets)>,
    /// Decisions the query designates, hydrated once with the bundle.
    pinned: Vec<PinnedDecision>,
    /// Tier 2 — pages déjà highlightées, mémoïsées par `(ai_mode, sort, offset,
    /// limit)`. `ai_mode` est dans la clé car le rerank change l'ordre, donc le contenu
    /// de la page. `std::sync::Mutex` : verrou jamais tenu à travers un `await`.
    page_memo: std::sync::Mutex<HashMap<String, std::sync::Arc<Vec<SearchHit>>>>,
}

impl RankedResults {
    /// Poids approximatif de l'entrée en octets, pour le `weigher` du cache moka.
    /// Dominé par la metadata hydratée. Ni le `page_memo` ni le décompte ne sont
    /// pesés, tous deux remplis après insertion : moka ne ré-évalue pas le poids.
    pub fn approx_weight(&self) -> u32 {
        let fused: usize = self
            .fused
            .iter()
            .map(|(_, _, c)| {
                std::mem::size_of::<LegHit>() + c.snippet.as_ref().map_or(0, String::len)
            })
            .sum();
        let meta: usize = self.meta.values().map(meta_weight).sum();
        let ids: usize = self
            .pub_ids
            .values()
            .map(|s| std::mem::size_of::<i64>() + std::mem::size_of::<String>() + s.len())
            .sum();
        (fused + meta + ids).min(u32::MAX as usize) as u32
    }
}

/// Octets approximatifs d'une `DecisionMeta` (size_of inline + champs texte sur le tas).
fn meta_weight(m: &DecisionMeta) -> usize {
    fn opt(o: &Option<String>) -> usize {
        o.as_ref().map_or(0, String::len)
    }
    fn vec_s(v: &[String]) -> usize {
        v.iter()
            .map(|x| std::mem::size_of::<String>() + x.len())
            .sum()
    }
    std::mem::size_of::<DecisionMeta>()
        + m.public_id.len()
        + m.jurisdiction_type.len()
        + opt(&m.jurisdiction_code)
        + opt(&m.date_lecture)
        + opt(&m.solution_uid)
        + opt(&m.procedure_uid)
        + opt(&m.office_uid)
        + opt(&m.legal_domain_uid)
        + vec_s(&m.publication_codes)
        + m.docket_numbers.as_deref().map_or(0, vec_s)
        + opt(&m.summary)
}

/// Octets approximatifs d'un bloc de facettes (libellés + overhead des buckets).
pub(crate) fn facets_weight(f: &SearchFacets) -> usize {
    fn choices(v: &[FacetChoice]) -> usize {
        v.iter()
            .map(|c| {
                std::mem::size_of::<FacetChoice>()
                    + c.value.len()
                    + c.label.len()
                    + c.parent.as_ref().map_or(0, String::len)
            })
            .sum()
    }
    choices(&f.seat)
        + choices(&f.office)
        + choices(&f.legal_domain)
        + choices(&f.solution)
        + choices(&f.procedure)
        + choices(&f.publication)
        + choices(&f.date_lecture_year)
        + f.legal_instrument
            .iter()
            .map(|l| {
                std::mem::size_of::<LegalInstrumentFacet>()
                    + l.value.len()
                    + l.label.len()
                    + choices(&l.articles)
            })
            .sum::<usize>()
}

/// Clé de cache du bundle retrieval : query, filtres, mode. Exclut
/// `offset`/`limit`, `sort` et `ai_mode` — une seule entrée sert les deux modes,
/// toutes les pages et tous les tris.
fn ranked_cache_key(req: &SearchRequest) -> String {
    let mut norm = req.clone();
    norm.offset = 0;
    norm.limit = 0;
    norm.sort = lj_dtos::SortOrder::Relevance;
    norm.ai_mode = false;
    serde_json::to_string(&norm).unwrap_or_default()
}

/// Récupération (RRF) + hydratation → bundle [`RankedResults`], la partie
/// coûteuse mise en cache et ai_mode-indépendante. Le décompte n'en fait pas
/// partie : [`comptage`] le calcule à la demande dans le bundle.
async fn rank_results(
    state: &AppState,
    retrieval: Retrieval,
    query_mode: QueryMode,
) -> Result<RankedResults> {
    let Retrieval {
        fused,
        lexicales,
        pinned,
    } = retrieval;
    let all_ids: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
    let hors_question: Vec<i64> = all_ids
        .iter()
        .copied()
        .filter(|d| !lexicales.contains(d))
        .collect();

    // Les deux sont indépendants : lancés en parallèle. Pic de connexions
    // 1 + 1 = 2 ≤ `PEAK_CONNS_PER_SEARCH`.
    let cite_conn = client(state).await?;
    let (pub_ids, meta) = tokio::try_join!(
        async {
            fetch_pub_ids(&cite_conn, &all_ids)
                .await
                .map_err(ApiError::Store)
        },
        hydrate_all(state, &fused),
    )?;
    let pinned = pinned_decisions(state, &pinned).await?;

    Ok(RankedResults {
        query_mode,
        fused,
        meta,
        pub_ids,
        hors_question,
        comptage: tokio::sync::OnceCell::new(),
        pinned,
        page_memo: std::sync::Mutex::new(HashMap::new()),
    })
}

/// Title and public id of the designated decisions, in the leg's order.
async fn pinned_decisions(state: &AppState, ids: &[i64]) -> Result<Vec<PinnedDecision>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let conn = client(state).await?;
    let meta = hydrate_decisions(&conn, ids)
        .await
        .map_err(ApiError::Store)?;
    let refs = referential(state).await?;
    Ok(ids
        .iter()
        .filter_map(|d| meta.get(d))
        .map(|m| PinnedDecision {
            id: m.public_id.clone(),
            title: display_title(m, &refs),
        })
        .collect())
}

/// Total exact et facettes du bundle, calculés une seule fois : les demandeurs
/// suivants attendent le même résultat, et aucun ne relance la récupération.
async fn comptage<'a>(
    state: &AppState,
    req: &SearchRequest,
    ranked: &'a RankedResults,
) -> Result<&'a (i64, SearchFacets)> {
    ranked
        .comptage
        .get_or_try_init(|| async {
            let conn = client(state).await?;
            let refs = referential(state).await?;
            let (total, counters) = aggregate_facets(&conn, req, &ranked.hors_question)
                .await
                .map_err(|e| ApiError::Internal(format!("facettes agrégées : {e}")))?;
            Ok((total, facets_from_counters(&counters, &refs)))
        })
        .await
}

/// Tier 2 : applique l'ordre relevance effectif (reranké si mode IA, sinon RRF du
/// bundle), puis tri + slice de la page (zéro DB) et highlight des ≤ `limit` hits,
/// mémoïsés par `(ai_mode, sort, offset, limit)`. `all_hit_ids` est recomposé dans
/// l'ordre relevance effectif.
async fn paginate(
    state: &AppState,
    req: &SearchRequest,
    ranked: &RankedResults,
    rerank_ids: Option<&[i64]>,
) -> Result<SearchResponse> {
    let relevance_ids: Vec<i64> = match rerank_ids {
        Some(ids) => rerank_id_order(&ranked.fused, ids),
        None => ranked.fused.iter().map(|(d, _, _)| *d).collect(),
    };
    let all_hit_ids: Vec<String> = relevance_ids
        .iter()
        .filter_map(|d| ranked.pub_ids.get(d).cloned())
        .collect();

    let page_key = format!(
        "{}|{:?}|{}|{}",
        req.ai_mode, req.sort, req.offset, req.limit
    );

    let cached_page = ranked.page_memo.lock().unwrap().get(&page_key).cloned();
    let page = if let Some(hits) = cached_page {
        (*hits).clone()
    } else {
        let by_id: HashMap<i64, &(i64, f64, LegHit)> =
            ranked.fused.iter().map(|row| (row.0, row)).collect();
        let mut order: Vec<&(i64, f64, LegHit)> = relevance_ids
            .iter()
            .filter_map(|d| by_id.get(d).copied())
            .filter(|(d, _, _)| ranked.meta.contains_key(d))
            .collect();
        sort_fused(&mut order, &ranked.meta, req.sort);

        let offset = req.offset as usize;
        let slice: Vec<(i64, f64, LegHit)> = if offset >= order.len() {
            Vec::new()
        } else {
            let end = (offset + req.limit as usize).min(order.len());
            order[offset..end].iter().map(|x| (**x).clone()).collect()
        };

        let conn = client(state).await?;
        let refs = referential(state).await?;
        let hits = assemble_page(&conn, &slice, &ranked.meta, &req.query, &refs).await?;
        ranked
            .page_memo
            .lock()
            .unwrap()
            .insert(page_key, std::sync::Arc::new(hits.clone()));
        hits
    };

    let comptage = ranked.comptage.get();
    Ok(SearchResponse {
        query: req.query.clone(),
        total: comptage.map(|(total, _)| *total),
        hits: page,
        query_mode: ranked.query_mode,
        facets: comptage.map(|(_, facets)| facets.clone()),
        all_hit_ids,
        reranked: rerank_ids.is_some(),
        pinned: ranked.pinned.clone(),
    })
}

/// Bundle retrieval depuis le cache moka, sinon calculé (récupération + RRF +
/// facettes + hydratation, sans rerank) puis caché. Clé ai_mode-indépendante.
///
/// Single-flight sur le chemin qui réussit : `optionally_get_with` ne laisse qu'un
/// appelant calculer. L'échec n'est pas partagé — un suiveur tombé sur un échec
/// recalcule pour obtenir son erreur typée.
async fn ranked_results_cached(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<std::sync::Arc<RankedResults>> {
    let key = ranked_cache_key(req);
    let mut own_err: Option<ApiError> = None;
    let mut computed = false;
    let hit = state
        .search_cache
        .optionally_get_with(key, async {
            computed = true;
            match rank_uncached(state, req, leg_limit, vchord_probes, budget_key).await {
                Ok(ranked) => Some(ranked),
                Err(err) => {
                    own_err = Some(err);
                    None
                }
            }
        })
        .await;
    crate::cache_metrics::record_access("recherche", !computed);
    match (hit, own_err) {
        (Some(ranked), _) => Ok(ranked),
        (None, Some(err)) => Err(err),
        (None, None) => rank_uncached(state, req, leg_limit, vchord_probes, budget_key).await,
    }
}

/// Budget, DB permit, resolution, retrieval, ranking. Kept apart from
/// [`ranked_results_cached`] so the single flight covers the cache alone.
async fn rank_uncached(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<std::sync::Arc<RankedResults>> {
    if state
        .budgets
        .take(crate::ratelimit::Cost::Search, budget_key)
        .await
        == crate::ratelimit::Verdict::Exhausted
        && state.settings.search_budget_enforce
    {
        return Err(ApiError::TooManyRequests);
    }
    let _db_permit = state
        .search_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
    let resolution = resolve(state, req).await?;
    let (retrieval, mode) = retrieve(state, req, resolution, leg_limit, vchord_probes).await?;
    Ok(std::sync::Arc::new(
        rank_results(state, retrieval, mode).await?,
    ))
}

/// Exécute une recherche (lexicale ou hybride selon `mode`) et assemble les hits +
/// facettes. Opérateurs `@@@` (BM25) / `<=>` (cosine) en SQL brut. Le bundle
/// retrieval passe par le cache moka ; le rerank LLM ([`reranked_order`], cache
/// dédié) n'est qu'un réordonnancement appliqué par-dessus.
///
/// Émet le signal d'usage du moteur `decisions` (cf. [`crate::usage`]). La
/// provenance vient du handler appelant ; l'identité de l'utilisateur ne va jamais
/// dans les traces, seulement `identity` = `authenticated` | `anonymous`.
///
/// `budget_key` est dans le `skip` pour cette raison : `instrument` enregistre par
/// défaut tout paramètre non skippé via son `Debug`, et celui de
/// [`crate::ratelimit::CallerKey`] porte le `sub` du compte ou l'IP du client.
#[tracing::instrument(skip(state, req, caller, budget_key), fields(
    librejustice.search.engine = tracing::field::Empty,
    librejustice.search.source = tracing::field::Empty,
    librejustice.search.query = tracing::field::Empty,
    librejustice.search.mode = tracing::field::Empty,
    librejustice.search.identity = tracing::field::Empty,
    librejustice.search.context = tracing::field::Empty,
    librejustice.search.origin = tracing::field::Empty,
    librejustice.search.caller = tracing::field::Empty,
    user_agent.original = tracing::field::Empty,
    librejustice.search.visit = tracing::field::Empty,
    librejustice.search.page = tracing::field::Empty,
    librejustice.search.filters = tracing::field::Empty,
    librejustice.search.results_count = tracing::field::Empty,
    librejustice.search.hit_ids = tracing::field::Empty,
    librejustice.search.retrieval_ms = tracing::field::Empty,
    librejustice.search.rerank_ms = tracing::field::Empty,
    librejustice.search.paginate_ms = tracing::field::Empty,
))]
pub async fn search(
    state: &AppState,
    req: &SearchRequest,
    caller: crate::usage::Caller,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<SearchResponse> {
    const ENGINE: lj_dtos::SearchEngine = lj_dtos::SearchEngine::Decisions;
    let filters = crate::search_history::facet_filters(req).to_string();
    let shape = crate::usage::Shape {
        page: crate::search_history::page_number(i64::from(req.offset), i64::from(req.limit))
            .unwrap_or(1),
        filters: &filters,
    };
    caller.record_span(ENGINE, req.query.as_str(), shape);
    tracing::Span::current().record("librejustice.search.mode", tracing::field::debug(req.mode));

    if req.query.trim().is_empty() {
        let resp = browse::browse(state, req).await?;
        caller.record_outcome(
            ENGINE,
            req.query.as_str(),
            shape,
            resp.total.unwrap_or_default(),
        );
        return Ok(resp);
    }

    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let t = std::time::Instant::now();
    let ranked = ranked_results_cached(state, req, leg_limit, vchord_probes, budget_key).await?;
    let retrieval = t.elapsed();
    let t = std::time::Instant::now();
    let rerank_ids = reranked_order(state, req, &ranked, budget_key).await?;
    let rerank = t.elapsed();
    let t = std::time::Instant::now();
    let resp = paginate(
        state,
        req,
        &ranked,
        rerank_ids.as_deref().map(Vec::as_slice),
    )
    .await?;

    let span = tracing::Span::current();
    span.record(
        "librejustice.search.retrieval_ms",
        retrieval.as_millis() as u64,
    );
    span.record("librejustice.search.rerank_ms", rerank.as_millis() as u64);
    span.record(
        "librejustice.search.paginate_ms",
        t.elapsed().as_millis() as u64,
    );
    // Served order up to the reranker horizon: the rank of the decision a caller
    // opens next reads nowhere else, and it is what says whether the window is
    // too narrow or too wide.
    span.record(
        "librejustice.search.hit_ids",
        resp.all_hit_ids
            .iter()
            .take(crate::rerank::RERANK_K)
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
            .as_str(),
    );
    // Le total exact ne se paie plus ici : `results_count` porte le nombre servi.
    caller.record_outcome(
        ENGINE,
        req.query.as_str(),
        shape,
        resp.all_hit_ids.len() as i64,
    );
    Ok(resp)
}

/// Décompte exact et facettes, pour les mêmes paramètres que la recherche : le
/// bundle vient du cache que celle-ci vient de remplir, et l'agrégat ne tourne
/// qu'une fois par bundle. N'émet aucun événement d'usage : une recherche en
/// deux appels reste une recherche.
pub async fn count(
    state: &AppState,
    req: &SearchRequest,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<lj_dtos::SearchCountResponse> {
    if req.query.trim().is_empty() {
        let refs = referential(state).await?;
        let counts = browse::totals_and_facets(state, req, &refs).await?;
        return Ok(lj_dtos::SearchCountResponse {
            total: counts.0,
            facets: counts.1.clone(),
        });
    }
    let ranked = ranked_results_cached(
        state,
        req,
        state.settings.leg_limit as i64,
        state.settings.vchord_probes,
        budget_key,
    )
    .await?;
    let (total, facets) = comptage(state, req, &ranked).await?;
    Ok(lj_dtos::SearchCountResponse {
        total: *total,
        facets: facets.clone(),
    })
}

/// Recherche d'un appelant qui ne reviendra pas chercher son décompte : MCP,
/// dont le schéma de sortie exige `total` et `facets`.
pub async fn search_with_count(
    state: &AppState,
    req: &SearchRequest,
    caller: crate::usage::Caller,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<SearchResponse> {
    let mut resp = search(state, req, caller, budget_key).await?;
    if resp.total.is_none() {
        let compte = count(state, req, budget_key).await?;
        resp.total = Some(compte.total);
        resp.facets = Some(compte.facets);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(query: &str, mode: &str) -> SearchRequest {
        serde_json::from_value(serde_json::json!({ "query": query, "mode": mode })).unwrap()
    }

    #[test]
    fn la_requete_qui_designe_une_decision_part_en_reference() {
        for q in [
            "\"18-25.770\"",
            "n° 250747",
            "n° 1702092",
            "21PA01295",
            "31399/23",
        ] {
            assert_eq!(
                resolve_shape(&req(q, "auto")),
                Some(Resolution::Reference),
                "{q}"
            );
        }
        for q in [
            "\"12-11.666\" OU valeur des parts de l'associé retrayant",
            "\"12-15.373\" ET \"cour d'appel de Douai\"",
            "21PA01295 SAUF référé",
            "387763 Czabaj délai raisonnable",
            "75013",
            "Danthony",
        ] {
            assert_eq!(resolve_shape(&req(q, "auto")), None, "{q}");
        }
    }

    #[test]
    fn le_mode_demande_est_servi_tel_quel_sans_carte() {
        for q in ["\"18-25.770\"", "Danthony", "75013", "expulsion locative"] {
            assert_eq!(
                resolve_shape(&req(q, "lexical")),
                Some(Resolution::Lexical),
                "{q}"
            );
            assert_eq!(
                resolve_shape(&req(q, "semantic")),
                Some(Resolution::Semantic),
                "{q}"
            );
        }
    }

    #[test]
    fn la_designation_dans_une_requete_de_contenu_pose_une_carte() {
        let number = || Some(Designation::Number);
        let danthony = || Some(Designation::Name("danthony".into()));
        let beziers = || Some(Designation::Name("commune de beziers".into()));
        let beziers_court = || Some(Designation::Name("beziers".into()));
        for (q, designation, attendu) in [
            ("expulsion locative", None, Resolution::Semantic),
            ("Conseil d'État 2024 expulsion", None, Resolution::Semantic),
            ("congés ET payés", None, Resolution::Lexical),
            ("\"force majeure\"", None, Resolution::Lexical),
            ("75013", number(), Resolution::LexicalWithCard),
            (" 1702092 ", number(), Resolution::LexicalWithCard),
            (
                "387763 Czabaj délai raisonnable",
                number(),
                Resolution::SemanticWithCard,
            ),
            (
                "barème 500000 accident de la circulation",
                number(),
                Resolution::SemanticWithCard,
            ),
            (
                "\"12-11.666\" OU valeur des parts",
                number(),
                Resolution::LexicalWithCard,
            ),
            ("Danthony", danthony(), Resolution::LexicalWithCard),
            ("\"Danthony\"", danthony(), Resolution::LexicalWithCard),
            (
                "Danthony vice de procédure",
                danthony(),
                Resolution::SemanticWithCard,
            ),
            ("Commune de Béziers", beziers(), Resolution::LexicalWithCard),
            (
                "Commune de Béziers absence de transmission",
                beziers(),
                Resolution::SemanticWithCard,
            ),
            ("Béziers", beziers_court(), Resolution::LexicalWithCard),
            (
                "Béziers 2001",
                beziers_court(),
                Resolution::SemanticWithCard,
            ),
            (
                "jurisprudence Béziers II",
                beziers_court(),
                Resolution::SemanticWithCard,
            ),
            (
                "Béziers conseil d'État",
                beziers_court(),
                Resolution::Reference,
            ),
            ("Béziers CE 2001", beziers_court(), Resolution::Reference),
            ("Danthony CE", danthony(), Resolution::Reference),
            ("362324 conseil d'État", number(), Resolution::Reference),
            (
                "Béziers OU Danthony CE",
                beziers_court(),
                Resolution::LexicalWithCard,
            ),
            (
                "Béziers conseil d'État loyauté contractuelle",
                beziers_court(),
                Resolution::SemanticWithCard,
            ),
        ] {
            assert_eq!(resolve_content(q, designation), attendu, "{q}");
        }
    }
}
