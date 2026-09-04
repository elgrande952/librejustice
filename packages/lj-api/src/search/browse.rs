//! Mode parcours : sans requête textuelle, le fonds filtré est listé par date,
//! et total comme facettes portent sur lui en entier, jamais sur une page.
//!
//! « Textes cités » est servie ici comme les autres axes : ses 3,02 s étaient
//! celles du SQL, l'index la rend dans la passe des autres sans coût mesurable.

use std::sync::Arc;

use deadpool_postgres::Client;
use lj_dtos::{QueryMode, SearchFacets, SearchHit, SearchRequest, SearchResponse, SortOrder};

use crate::error::{ApiError, Result};
use crate::referential::{referential, Referential};
use crate::state::AppState;

use super::filters::{as_refs, Params};
use super::hydrate::{facets_from_counters, FacetCounters};

/// La requête porte-t-elle au moins un filtre ? Sans question ni filtre, la
/// page est l'arrivée sur `/decisions` et non une recherche.
pub(crate) fn has_filters(req: &SearchRequest) -> bool {
    fn some<T>(v: &Option<Vec<T>>) -> bool {
        v.as_deref().is_some_and(|s| !s.is_empty())
    }
    some(&req.jurisdiction_type)
        || some(&req.solution)
        || some(&req.procedure)
        || some(&req.office)
        || some(&req.legal_domain)
        || some(&req.jurisdiction_code)
        || some(&req.seat)
        || some(&req.legal_instrument)
        || some(&req.legal_article)
        || some(&req.publication)
        || req.date_from.is_some()
        || req.date_to.is_some()
}

/// Sans requête, la pertinence vaut la date décroissante.
fn order_by(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::DateAsc => "d.date_lecture ASC NULLS LAST, d.id",
        _ => "d.date_lecture DESC NULLS LAST, d.id",
    }
}

/// Largeur de la liste que la navigation latérale parcourt, ici comme en
/// recherche : les flèches traversent la page au lieu de mourir à son bord.
const NAV_WINDOW: i64 = 100;

/// Fenêtre `[start, start + len)` ancrée sur un multiple de [`NAV_WINDOW`], et
/// élargie quand la page demandée déborderait de son ancrage.
fn nav_window(offset: i64, limit: i64) -> (i64, i64) {
    let start = offset - offset % NAV_WINDOW;
    (start, NAV_WINDOW.max(offset + limit - start))
}

/// Identifiants et `public_id` de la fenêtre `[start, start + len)`, qui couvre
/// la page demandée. Sous filtre, l'index trie et tronque, par le
/// prédicat qui sert déjà les compteurs : le tas remonte son index de date en
/// jetant au filtre, 2,1 s sur un office rare et 6,7 s sur un texte que personne
/// ne cite, contre 20 à 50 ms quelle que soit la sélectivité. Sans filtre il
/// reste le plus court, 0,1 ms en index seul.
async fn window_ids(
    conn: &Client,
    req: &SearchRequest,
    start: i64,
    len: i64,
) -> Result<Vec<(i64, String)>> {
    let mut params: Params = Vec::new();
    let predicate = if has_filters(req) {
        let mut idx = 1usize;
        format!(
            "d.id @@@ {}",
            super::aggregate::build_predicate(req, &mut idx, &mut params)
        )
    } else {
        "d.deleted_at IS NULL".to_string()
    };
    let (li, oi) = (params.len() + 1, params.len() + 2);
    let mut refs = as_refs(&params);
    refs.push(&len);
    refs.push(&start);
    let sql = format!(
        "SELECT d.id, d.public_id FROM decisions d WHERE {predicate} \
         ORDER BY {} LIMIT ${li} OFFSET ${oi}",
        order_by(req.sort)
    );
    let rows = conn
        .query(&sql, &refs)
        .await
        .map_err(|e| ApiError::Internal(format!("browse page: {e}")))?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

/// Total et facettes du fonds filtré, comptés dans l'index par le même agrégat
/// que la recherche textuelle : 0,61 s sous le filtre `CA` contre 2,5 s pour le
/// `GROUPING SETS` pleine table qu'il remplace.
async fn live_totals_and_facets(
    state: &AppState,
    req: &SearchRequest,
    refs: &Referential,
) -> Result<(i64, SearchFacets)> {
    let conn = super::client(state).await?;
    let (total, counters) = super::aggregate::aggregate_facets(&conn, req, &[])
        .await
        .map_err(|e| ApiError::Internal(format!("browse facettes: {e}")))?;
    Ok((total, facets_from_counters(&counters, refs)))
}

/// Sans aucun filtre, l'état du fonds ne change qu'une fois par nuit : il se lit
/// dans l'instantané que recompte la chaîne, pas en balayant 3,7 M de lignes.
async fn snapshot_totals_and_facets(
    state: &AppState,
    refs: &Referential,
) -> Result<(i64, SearchFacets)> {
    let conn = super::client(state).await?;
    let rows = conn
        .query(
            "SELECT c.dimension, c.value, c.parent, c.n, lt.title, lt.slug \
             FROM corpus_counts c \
             LEFT JOIN legal_text lt \
               ON c.dimension = 'legal_instrument' AND lt.text_uid = c.value",
            &[],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("browse snapshot: {e}")))?;
    let mut total = 0i64;
    let mut c = FacetCounters::default();
    for row in &rows {
        let dimension: String = row.get(0);
        let value: String = row.get(1);
        let parent: Option<String> = row.get(2);
        let n: i64 = row.get(3);
        match (dimension.as_str(), parent) {
            ("total", _) => total = n,
            ("seat", _) => drop(c.seats.insert(value, n)),
            ("office", _) => drop(c.office.insert(value, n)),
            ("legal_domain", _) => drop(c.domain.insert(value, n)),
            ("solution", _) => drop(c.solution.insert(value, n)),
            ("procedure", _) => drop(c.procedure.insert(value, n)),
            ("year", _) => drop(c.year.insert(value, n)),
            ("publication_codes", _) => {
                let codes: Vec<String> = value.split(',').map(str::to_string).collect();
                for key in lj_core::publication::publication_keys(&codes) {
                    *c.publication
                        .entry(format!("publication:{key}"))
                        .or_insert(0) += n;
                }
            }
            ("legal_instrument", _) => {
                if let Some(title) = row.get::<_, Option<String>>(4) {
                    c.instrument.labels.insert(value.clone(), title);
                }
                c.instrument.slugs.insert(value.clone(), row.get(5));
                c.instrument.counts.insert(value, n);
            }
            ("legal_article", Some(uid)) => drop(
                c.instrument
                    .articles
                    .entry(uid)
                    .or_default()
                    .insert(value, n),
            ),
            _ => {}
        }
    }
    Ok((total, facets_from_counters(&c, refs)))
}

/// Clé du cache parcours : les filtres seuls. Ni page, ni tri, ni mode, dont le
/// total et les facettes ne dépendent pas.
fn cache_key(req: &SearchRequest) -> String {
    let mut norm = req.clone();
    norm.offset = 0;
    norm.limit = 0;
    norm.sort = SortOrder::Relevance;
    norm.mode = lj_dtos::SearchMode::Auto;
    norm.ai_mode = false;
    serde_json::to_string(&norm).unwrap_or_default()
}

/// Total et facettes, du cache ou calculés. Single-flight : `optionally_get_with`
/// ne laisse qu'un appelant payer le calcul.
pub(crate) async fn totals_and_facets(
    state: &AppState,
    req: &SearchRequest,
    refs: &Referential,
) -> Result<Arc<(i64, SearchFacets)>> {
    let mut own_err: Option<ApiError> = None;
    let mut paye = false;
    let hit = state
        .browse_cache
        .optionally_get_with(cache_key(req), async {
            paye = true;
            let computed = if has_filters(req) {
                live_totals_and_facets(state, req, refs).await
            } else {
                snapshot_totals_and_facets(state, refs).await
            };
            match computed {
                Ok(v) => Some(Arc::new(v)),
                Err(err) => {
                    own_err = Some(err);
                    None
                }
            }
        })
        .await;
    crate::cache_metrics::record_access("parcours", !paye);
    match (hit, own_err) {
        (Some(v), _) => Ok(v),
        (None, Some(err)) => Err(err),
        (None, None) => Err(ApiError::Internal("browse: cache sans valeur".into())),
    }
}

pub(crate) async fn browse(state: &AppState, req: &SearchRequest) -> Result<SearchResponse> {
    let refs = referential(state).await?;

    let (offset, limit) = (i64::from(req.offset), i64::from(req.limit));
    let (window_start, window_len) = nav_window(offset, limit);

    let list_conn = super::client(state).await?;
    let (window, counts) = tokio::try_join!(
        window_ids(&list_conn, req, window_start, window_len),
        totals_and_facets(state, req, &refs),
    )?;
    let (total, facets) = (counts.0, counts.1.clone());

    let page: Vec<i64> = window
        .iter()
        .skip((offset - window_start) as usize)
        .take(limit as usize)
        .map(|(id, _)| *id)
        .collect();

    let hits_conn = super::client(state).await?;
    let hits: Vec<SearchHit> = super::hits_for_decision_ids(&hits_conn, &page, &refs)
        .await?
        .into_iter()
        .map(|(_, hit)| hit)
        .collect();
    let all_hit_ids: Vec<String> = window.into_iter().map(|(_, public_id)| public_id).collect();

    Ok(SearchResponse {
        query: String::new(),
        total: Some(total),
        hits,
        query_mode: QueryMode::Browse,
        facets: Some(facets),
        all_hit_ids,
        reranked: false,
        pinned: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_window_couvre_toujours_la_page_demandee() {
        for (offset, limit) in [(0, 10), (40, 10), (140, 10), (60, 60), (0, 100), (250, 25)] {
            let (start, len) = nav_window(offset, limit);
            assert!(start <= offset, "fenetre {start} apres la page {offset}");
            assert!(
                offset + limit <= start + len,
                "page {offset}+{limit} hors de la fenetre {start}+{len}"
            );
            assert!(len >= NAV_WINDOW);
        }
        assert_eq!(nav_window(0, 10), (0, NAV_WINDOW));
        assert_eq!(nav_window(140, 10), (100, NAV_WINDOW));
    }
}
