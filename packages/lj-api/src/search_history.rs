//! Endpoints `/me/search-history` + hook d'insertion depuis `/search`.

use deadpool_postgres::Pool;
use lj_dtos::{ActivitySource, SearchEngine, SearchHistoryEntry, SearchRequest};

use crate::error::ApiError;
use crate::me::ts_to_rfc3339;
use crate::state::AppState;

/// Borne haute de pagination de l'historique.
pub const MAX_ITEMS: i64 = 100;

/// Représentation chaîne d'un canal d'activité (`web` | `api` | `mcp`), pour les
/// colonnes TEXT.
pub(crate) fn source_value(source: ActivitySource) -> &'static str {
    match source {
        ActivitySource::Web => "web",
        ActivitySource::Api => "api",
        ActivitySource::Mcp => "mcp",
    }
}

/// Désérialise un TEXT `web`/`api`/`mcp` lu en base vers l'enum.
fn parse_source(raw: &str) -> Result<ActivitySource, ApiError> {
    match raw {
        "web" => Ok(ActivitySource::Web),
        "api" => Ok(ActivitySource::Api),
        "mcp" => Ok(ActivitySource::Mcp),
        other => Err(ApiError::Internal(format!("source invalide {other:?}"))),
    }
}

/// Représentation chaîne du moteur interrogé (`decisions` | `textes` |
/// `annonces`), pour la colonne TEXT.
pub(crate) fn engine_value(engine: SearchEngine) -> &'static str {
    match engine {
        SearchEngine::Decisions => "decisions",
        SearchEngine::Textes => "textes",
        SearchEngine::Annonces => "annonces",
    }
}

/// Désérialise un TEXT `decisions`/`textes`/`annonces` lu en base vers l'enum.
fn parse_engine(raw: &str) -> Result<SearchEngine, ApiError> {
    match raw {
        "decisions" => Ok(SearchEngine::Decisions),
        "textes" => Ok(SearchEngine::Textes),
        "annonces" => Ok(SearchEngine::Annonces),
        other => Err(ApiError::Internal(format!("engine invalide {other:?}"))),
    }
}

/// Numéro de page 1-based de la fenêtre `(offset, limit)` demandée, `None` sur
/// la première page. Clé `page` de l'URL de la page de recherche (`?page=N`),
/// pas la fenêtre brute : c'est le grain que l'utilisateur manipule.
pub(crate) fn page_number(offset: i64, limit: i64) -> Option<i64> {
    if offset == 0 {
        return None;
    }
    Some(offset / limit + 1)
}

/// Sélection de facettes seule : la `SearchRequest` sans la query ni la fenêtre
/// (les `Option` à `None` sont déjà omis par les `skip_serializing_if` du DTO).
/// C'est ce que le span d'usage porte, la profondeur allant dans son propre
/// attribut `page`.
pub(crate) fn facet_filters(req: &SearchRequest) -> serde_json::Value {
    let mut value = serde_json::to_value(req).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.remove("query");
        obj.remove("limit");
        obj.remove("offset");
    }
    value
}

/// Filtres JSONB de l'historique : [`facet_filters`] plus un `page` 1-based posé
/// au-delà de la première page — dépouiller une recherche page à page donne donc
/// des entrées distinctes.
pub(crate) fn filters_from_request(req: &SearchRequest) -> serde_json::Value {
    let mut value = facet_filters(req);
    if let Some(obj) = value.as_object_mut() {
        if let Some(page) = page_number(i64::from(req.offset), i64::from(req.limit)) {
            obj.insert("page".to_string(), page.into());
        }
    }
    value
}

/// Insère une entrée d'historique. Échec silencieux (best-effort). Les appelants
/// décisions sérialisent leurs filtres via [`filters_from_request`] ; les
/// appelants textes posent leurs filtres propres.
///
/// Gaté par `track_activity` directement dans l'INSERT (`WHERE EXISTS`) : aucune
/// ligne insérée si l'utilisateur a coupé l'enregistrement.
pub async fn record_search(
    pool: &Pool,
    user_sub: &str,
    query: &str,
    filters: serde_json::Value,
    source: ActivitySource,
    engine: SearchEngine,
) {
    let src = source_value(source);
    let eng = engine_value(engine);
    let res: Result<(), ApiError> = async {
        let conn = pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
        conn.execute(
            "INSERT INTO user_search_history (user_sub, query, filters, source, engine) \
             SELECT $1, $2, $3, $4, $5 \
             WHERE EXISTS (SELECT 1 FROM users WHERE sub = $6 AND track_activity)",
            &[&user_sub, &query, &filters, &src, &eng, &user_sub],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("record_search: {e}")))?;
        Ok(())
    }
    .await;
    if let Err(exc) = res {
        tracing::warn!(error = %exc, "user_search_history insert failed");
    }
}

/// Page de recherches (plus récentes d'abord) + total complet. `total` via
/// `COUNT(*) OVER()`. `limit` `None` → tout.
pub async fn fetch_history(
    pool: &Pool,
    user_sub: &str,
    limit: Option<i64>,
    offset: i64,
) -> Result<(Vec<SearchHistoryEntry>, i64), ApiError> {
    let conn = pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let rows = conn
        .query(
            "SELECT id, query, filters, source, engine, created_at, COUNT(*) OVER() AS total \
             FROM user_search_history \
             WHERE user_sub = $1 \
             ORDER BY created_at DESC \
             LIMIT $2 OFFSET $3",
            &[&user_sub, &limit, &offset],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("fetch_history: {e}")))?;

    let total = rows.first().map(|r| r.get::<_, i64>(6)).unwrap_or(0);
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        let filters: Option<serde_json::Value> = row.get(2);
        items.push(SearchHistoryEntry {
            id: row.get(0),
            query: row.get(1),
            filters: filters.unwrap_or_else(|| serde_json::json!({})),
            source: parse_source(row.get::<_, &str>(3))?,
            engine: parse_engine(row.get::<_, &str>(4))?,
            created_at: ts_to_rfc3339(row.get(5)),
        });
    }
    Ok((items, total))
}

/// `GET /me/search-history`. `limit` est borné
/// `[1, MAX_ITEMS]` à la frontière HTTP (validation côté handler axum).
pub async fn list_history(
    state: &AppState,
    user_sub: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<SearchHistoryEntry>, i64), ApiError> {
    fetch_history(&state.pool, user_sub, Some(limit), offset).await
}

/// `DELETE /me/search-history/{entry_id}`.
/// `NotFound` (`entry_not_found`) si la ligne n'appartient pas à l'utilisateur.
pub async fn delete_entry(state: &AppState, user_sub: &str, entry_id: i64) -> Result<(), ApiError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let affected = conn
        .execute(
            "DELETE FROM user_search_history WHERE id = $1 AND user_sub = $2",
            &[&entry_id, &user_sub],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("delete_entry: {e}")))?;
    if affected == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// `DELETE /me/search-history` — purge complète.
pub async fn clear_history(state: &AppState, user_sub: &str) -> Result<(), ApiError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute(
        "DELETE FROM user_search_history WHERE user_sub = $1",
        &[&user_sub],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("clear_history: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::{SearchMode, SortOrder};

    fn base_request() -> SearchRequest {
        SearchRequest {
            query: "expulsion locataire".to_string(),
            jurisdiction_type: None,
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            jurisdiction_code: None,
            seat: None,
            legal_instrument: None,
            legal_article: None,
            publication: None,
            date_from: None,
            date_to: None,
            mode: SearchMode::Auto,
            sort: SortOrder::Relevance,
            limit: 20,
            offset: 0,
            ai_mode: false,
        }
    }

    #[test]
    fn filters_drop_query_limit_offset() {
        let filters = filters_from_request(&base_request());
        let obj = filters.as_object().unwrap();
        assert!(!obj.contains_key("query"));
        assert!(!obj.contains_key("limit"));
        assert!(!obj.contains_key("offset"));
        assert_eq!(obj.get("mode").and_then(|v| v.as_str()), Some("auto"));
        assert_eq!(obj.get("sort").and_then(|v| v.as_str()), Some("relevance"));
        assert_eq!(obj.get("aiMode").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn filters_carry_page_beyond_first() {
        let first = filters_from_request(&base_request());
        assert!(!first.as_object().unwrap().contains_key("page"));

        let mut req = base_request();
        req.offset = 40;
        let third = filters_from_request(&req);
        assert_eq!(
            third
                .as_object()
                .unwrap()
                .get("page")
                .and_then(|v| v.as_i64()),
            Some(3)
        );
        assert_ne!(first, third);
    }

    #[test]
    fn filters_omit_none_options() {
        let filters = filters_from_request(&base_request());
        let obj = filters.as_object().unwrap();
        assert!(!obj.contains_key("jurisdictionType"));
        assert!(!obj.contains_key("dateFrom"));
    }

    #[test]
    fn filters_keep_set_options_camelcase() {
        let mut req = base_request();
        req.jurisdiction_code = Some(vec!["ce".to_string()]);
        let filters = filters_from_request(&req);
        let obj = filters.as_object().unwrap();
        assert!(obj.contains_key("jurisdictionCode"));
    }

    #[test]
    fn source_value_lowercase() {
        assert_eq!(source_value(ActivitySource::Web), "web");
        assert_eq!(source_value(ActivitySource::Api), "api");
        assert_eq!(source_value(ActivitySource::Mcp), "mcp");
    }
}
