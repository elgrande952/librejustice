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
    FacetChoice, LegalInstrumentFacet, QueryMode, SearchFacets, SearchHit, SearchRequest,
    SearchResponse, SortOrder,
};

use adaptive::compute_adaptive_weights;
use hydrate::{
    assemble_page, display_title, fetch_pub_ids_and_facets, hydrate_decisions, DecisionMeta,
};
use legs::{
    ann_leg, bm25_leg, bm25_parse_leg, fuse_ranks, pool_max_per_decision, title_leg,
    truncate_fused, LegHit,
};
use query::{detect_query_mode, is_boolean_query, query_lacks_searchable_terms, translate_boolean};

pub(crate) use dates::{parse_search_date, DateError, DATE_GE, DATE_LE};
pub use query::{body_query_for_arm, BodyArm};

// ── API bancs offline (lj-bench rank-arms / arm-latency / rank-bsweep) ────────

/// Récupération lexicale pure pour l'A/B de ranking offline (`lj-bench
/// rank-arms`) : jambe BM25 body (corps construit selon `arm`) + jambe titre,
/// fusion RRF aux poids de base, troncature relative. Renvoie les `decision_id` en
/// ordre de pertinence.
pub async fn lexical_rank_for_arm(
    conn: &Client,
    query: &str,
    arm: BodyArm,
    leg_limit: i64,
) -> anyhow::Result<Vec<i64>> {
    let req: SearchRequest = serde_json::from_value(serde_json::json!({ "query": query }))?;
    let body_query = body_query_for_arm(query, arm);
    let bm25_scores = bm25_parse_leg(conn, &req, &body_query, leg_limit).await?;
    let title_scores = title_leg(conn, &req, leg_limit).await?;
    let empty: HashMap<i64, LegHit> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &empty,
        &title_scores,
        signals::BASE_WEIGHTS.bm25,
        signals::BASE_WEIGHTS.ann,
        signals::BASE_WEIGHTS.title,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Récupération hybride pour l'A/B de ranking offline (`lj-bench rank-arms --mode
/// hybrid`) : embedding requête + jambes body / ANN / titre, poids RRF adaptatifs,
/// troncature relative. Seule la jambe BM25 varie entre bras. Renvoie les
/// `decision_id` en ordre de pertinence RRF.
pub async fn hybrid_rank_for_arm(
    state: &AppState,
    query: &str,
    arm: BodyArm,
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
        title_leg(&title_conn, &req, leg_limit),
    )?;
    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &ann_best,
        &title_scores,
        weights.bm25,
        weights.ann,
        weights.title,
    ));
    Ok(fused.into_iter().map(|(d, _, _)| d).collect())
}

/// Recherche complète SANS le cache résultats moka (banc `lj-bench load`) :
/// exécute toujours récupération + RRF + facettes + hydratation + assemblage de
/// page, en conservant le permit d'admission et le cache embeddings. `ai_mode`
/// ignoré.
pub async fn search_nocache(state: &AppState, req: &SearchRequest) -> Result<SearchResponse> {
    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let query_mode = effective_query_mode(req);
    let t0 = std::time::Instant::now();
    let (ranked, retrieval_ms) = {
        let _db_permit = state
            .search_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
        let fused = if query_lacks_searchable_terms(&req.query) {
            Vec::new()
        } else {
            match query_mode {
                QueryMode::Lexical => retrieve_boolean(state, req, leg_limit).await?,
                QueryMode::Hybrid => retrieve_hybrid(state, req, leg_limit, vchord_probes).await?,
            }
        };
        let retrieval_ms = t0.elapsed().as_secs_f64() * 1000.0;
        (
            std::sync::Arc::new(rank_results(state, fused, query_mode).await?),
            retrieval_ms,
        )
    };
    let hydrate_ms = t0.elapsed().as_secs_f64() * 1000.0 - retrieval_ms;
    let t1 = std::time::Instant::now();
    let resp = paginate(state, req, &ranked, None).await?;
    tracing::info!(
        target: "lj_bench_timing",
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

/// Latence isolée de la jambe BM25 body pour un bras (banc `lj-bench
/// arm-latency`) — la seule jambe qui varie entre bras. Renvoie `(nb_hits, durée
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

#[tracing::instrument(skip(state, req), fields(leg_limit))]
async fn retrieve_boolean(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
) -> Result<Vec<(i64, f64, LegHit)>> {
    let bm25_conn = client(state).await?;
    let title_conn = client(state).await?;
    let body_query = if is_boolean_query(&req.query) {
        translate_boolean(&req.query)
    } else {
        req.query.clone()
    };
    let (bm25_scores, title_scores) = if is_boolean_query(&req.query) {
        tokio::try_join!(
            bm25_parse_leg(&bm25_conn, req, &body_query, leg_limit),
            title_leg(&title_conn, req, leg_limit),
        )
    } else {
        tokio::try_join!(
            bm25_leg(&bm25_conn, req, leg_limit),
            title_leg(&title_conn, req, leg_limit),
        )
    }
    .map_err(ApiError::Store)?;

    let empty: HashMap<i64, LegHit> = HashMap::new();
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &empty,
        &title_scores,
        signals::BASE_WEIGHTS.bm25,
        signals::BASE_WEIGHTS.ann,
        signals::BASE_WEIGHTS.title,
    ));
    Ok(fused)
}

#[tracing::instrument(skip(state, req), fields(leg_limit, vchord_probes))]
async fn retrieve_hybrid(
    state: &AppState,
    req: &SearchRequest,
    leg_limit: i64,
    vchord_probes: u32,
) -> Result<Vec<(i64, f64, LegHit)>> {
    let query_arr = match state.embedding_cache.get(&req.query).await {
        Some(vec) => vec,
        None => {
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

    // Les 3 connexions des jambes sont relâchées dès le `try_join` fini, AVANT
    // `compute_adaptive_weights` qui en reprend 2 : le pic reste à
    // `PEAK_CONNS_PER_SEARCH`.
    let (bm25_scores, ann_hits, title_scores) = {
        let bm25_conn = client(state).await?;
        let mut ann_conn = client(state).await?;
        let title_conn = client(state).await?;
        tokio::try_join!(
            bm25_leg(&bm25_conn, req, leg_limit),
            ann_leg(&mut ann_conn, req, &query_vec, leg_limit, vchord_probes),
            title_leg(&title_conn, req, leg_limit),
        )
        .map_err(ApiError::Store)?
    };

    let ann_best = pool_max_per_decision(&ann_hits, leg_limit as usize);
    let weights = compute_adaptive_weights(state, &req.query, &bm25_scores, &ann_hits).await?;
    let fused = truncate_fused(fuse_ranks(
        &bm25_scores,
        &ann_best,
        &title_scores,
        weights.bm25,
        weights.ann,
        weights.title,
    ));
    Ok(fused)
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
    if state.rerank_budget.take(budget_key).await == crate::ratelimit::Verdict::Exhausted {
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
    facets: SearchFacets,
    /// `decision_id → public_id` de tous les hits.
    pub_ids: HashMap<i64, String>,
    total: i64,
    /// Tier 2 — pages déjà highlightées, mémoïsées par `(ai_mode, sort, offset,
    /// limit)`. `ai_mode` est dans la clé car le rerank change l'ordre, donc le contenu
    /// de la page. `std::sync::Mutex` : verrou jamais tenu à travers un `await`.
    page_memo: std::sync::Mutex<HashMap<String, std::sync::Arc<Vec<SearchHit>>>>,
}

impl RankedResults {
    /// Poids approximatif de l'entrée en octets, pour le `weigher` du cache moka.
    /// Dominé par la metadata hydratée. Le `page_memo` n'est pas pesé — moka ne
    /// ré-évalue pas le poids après insertion.
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
        (fused + meta + ids + facets_weight(&self.facets)).min(u32::MAX as usize) as u32
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
fn facets_weight(f: &SearchFacets) -> usize {
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
    choices(&f.jurisdiction)
        + choices(&f.legal_domain)
        + choices(&f.solution)
        + choices(&f.significance)
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
fn ranked_cache_key(req: &SearchRequest, query_mode: QueryMode) -> String {
    let mut norm = req.clone();
    norm.offset = 0;
    norm.limit = 0;
    norm.sort = lj_dtos::SortOrder::Relevance;
    norm.ai_mode = false;
    format!(
        "{query_mode:?}|{}",
        serde_json::to_string(&norm).unwrap_or_default()
    )
}

/// Récupération (RRF) + facettes + hydratation → bundle [`RankedResults`], la
/// partie coûteuse mise en cache et ai_mode-indépendante.
async fn rank_results(
    state: &AppState,
    fused: Vec<(i64, f64, LegHit)>,
    query_mode: QueryMode,
) -> Result<RankedResults> {
    let all_ids: Vec<i64> = fused.iter().map(|(d, _, _)| *d).collect();
    let total = fused.len() as i64;

    // `fetch_pub_ids_and_facets` et `hydrate_all` sont indépendants : lancés en
    // parallèle. Pic de connexions 1 + 1 = 2 ≤ `PEAK_CONNS_PER_SEARCH`.
    let refs = referential(state).await?;
    let facets_conn = client(state).await?;
    let ((pub_ids, facets), meta) = tokio::try_join!(
        async {
            fetch_pub_ids_and_facets(&facets_conn, &all_ids, &refs)
                .await
                .map_err(ApiError::Store)
        },
        hydrate_all(state, &fused),
    )?;

    Ok(RankedResults {
        query_mode,
        fused,
        meta,
        facets,
        pub_ids,
        total,
        page_memo: std::sync::Mutex::new(HashMap::new()),
    })
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

    Ok(SearchResponse {
        query: req.query.clone(),
        total: ranked.total,
        hits: page,
        query_mode: ranked.query_mode,
        facets: Some(ranked.facets.clone()),
        all_hit_ids,
    })
}

/// Mode de recherche effectif (parité du `match` de dispatch) : `Lexical` si la
/// requête est explicitement lexicale, ou détectée booléenne en `auto` ; sinon
/// `Hybrid`.
fn effective_query_mode(req: &SearchRequest) -> QueryMode {
    match req.mode {
        lj_dtos::SearchMode::Lexical => QueryMode::Lexical,
        lj_dtos::SearchMode::Auto if detect_query_mode(&req.query) == QueryMode::Lexical => {
            QueryMode::Lexical
        }
        _ => QueryMode::Hybrid,
    }
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
    query_mode: QueryMode,
    leg_limit: i64,
    vchord_probes: u32,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<std::sync::Arc<RankedResults>> {
    let key = ranked_cache_key(req, query_mode);
    let mut own_err: Option<ApiError> = None;
    let hit = state
        .search_cache
        .optionally_get_with(key, async {
            match rank_uncached(state, req, query_mode, leg_limit, vchord_probes, budget_key).await
            {
                Ok(ranked) => Some(ranked),
                Err(err) => {
                    own_err = Some(err);
                    None
                }
            }
        })
        .await;
    match (hit, own_err) {
        (Some(ranked), _) => Ok(ranked),
        (None, Some(err)) => Err(err),
        (None, None) => {
            rank_uncached(state, req, query_mode, leg_limit, vchord_probes, budget_key).await
        }
    }
}

/// Le calcul du bundle lui-même — budget, permit DB, récupération, ranking.
/// Séparé de [`ranked_results_cached`] pour que le single-flight ne porte que sur
/// le cache.
async fn rank_uncached(
    state: &AppState,
    req: &SearchRequest,
    query_mode: QueryMode,
    leg_limit: i64,
    vchord_probes: u32,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<std::sync::Arc<RankedResults>> {
    if state.search_budget.take(budget_key).await == crate::ratelimit::Verdict::Exhausted
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
    let fused = if query_lacks_searchable_terms(&req.query) {
        Vec::new()
    } else {
        match query_mode {
            QueryMode::Lexical => retrieve_boolean(state, req, leg_limit).await?,
            QueryMode::Hybrid => retrieve_hybrid(state, req, leg_limit, vchord_probes).await?,
        }
    };
    Ok(std::sync::Arc::new(
        rank_results(state, fused, query_mode).await?,
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
    librejustice.search.session = tracing::field::Empty,
    librejustice.search.context = tracing::field::Empty,
    librejustice.search.page = tracing::field::Empty,
    librejustice.search.filters = tracing::field::Empty,
    librejustice.search.results_count = tracing::field::Empty,
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

    let leg_limit = state.settings.leg_limit as i64;
    let vchord_probes = state.settings.vchord_probes;
    let query_mode = effective_query_mode(req);
    let ranked =
        ranked_results_cached(state, req, query_mode, leg_limit, vchord_probes, budget_key).await?;
    let rerank_ids = reranked_order(state, req, &ranked, budget_key).await?;
    let resp = paginate(
        state,
        req,
        &ranked,
        rerank_ids.as_deref().map(Vec::as_slice),
    )
    .await?;

    caller.record_outcome(ENGINE, req.query.as_str(), shape, resp.total);
    Ok(resp)
}
