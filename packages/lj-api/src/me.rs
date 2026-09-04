//! Endpoints `/me/*` — lecture, mise à jour et suppression du profil
//! utilisateur.
//!
//! Les handlers axum vivent dans `routes.rs` ; ce module porte la logique
//! d'accès aux données, exposée comme fonctions de service prenant le pool /
//! l'`AppState`.

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use lj_dtos::UserProfile;

use crate::error::ApiError;
use crate::state::AppState;

/// Formate un timestamp Postgres `TIMESTAMPTZ` en RFC 3339.
pub(crate) fn ts_to_rfc3339(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Lit le profil utilisateur.
///
/// Renvoie [`ApiError::NotFound`] si la ligne est absente (théoriquement
/// impossible : [`RequiredUser`] vient de provisionner —
/// `detail=profile_missing`).
pub async fn fetch_profile(pool: &Pool, sub: &str) -> Result<UserProfile, ApiError> {
    let conn = pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let row = conn
        .query_opt(
            "SELECT sub, email, display_name, created_at, last_seen_at, track_activity \
             FROM users WHERE sub = $1",
            &[&sub],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("fetch_profile: {e}")))?;
    let row = row.ok_or(ApiError::NotFound)?;
    Ok(UserProfile {
        sub: row.get(0),
        email: row.get(1),
        display_name: row.get(2),
        created_at: ts_to_rfc3339(row.get(3)),
        last_seen_at: ts_to_rfc3339(row.get(4)),
        track_activity: row.get(5),
    })
}

/// `GET /me`.
pub async fn get_me(state: &AppState, user_sub: &str) -> Result<UserProfile, ApiError> {
    fetch_profile(&state.pool, user_sub).await
}

/// `PUT /me/activity-tracking` — active/désactive l'enregistrement d'activité.
/// Désactiver = mode ZDR : on coupe le tracking **et** on purge tout l'existant
/// (recherches, lectures, signets) dans une seule transaction.
pub async fn set_activity_tracking(
    state: &AppState,
    user_sub: &str,
    enabled: bool,
) -> Result<UserProfile, ApiError> {
    {
        let mut conn = state
            .pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
        let tx = conn
            .transaction()
            .await
            .map_err(|e| ApiError::Internal(format!("tx begin: {e}")))?;
        tx.execute(
            "UPDATE users SET track_activity = $1 WHERE sub = $2",
            &[&enabled, &user_sub],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("set_activity_tracking: {e}")))?;
        if !enabled {
            tx.execute(
                "DELETE FROM user_search_history WHERE user_sub = $1",
                &[&user_sub],
            )
            .await
            .map_err(|e| ApiError::Internal(format!("purge history: {e}")))?;
            tx.execute(
                "DELETE FROM user_decision_views WHERE user_sub = $1",
                &[&user_sub],
            )
            .await
            .map_err(|e| ApiError::Internal(format!("purge views: {e}")))?;
            tx.execute(
                "DELETE FROM user_bookmarks WHERE user_sub = $1",
                &[&user_sub],
            )
            .await
            .map_err(|e| ApiError::Internal(format!("purge bookmarks: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| ApiError::Internal(format!("tx commit: {e}")))?;
    }
    fetch_profile(&state.pool, user_sub).await
}

/// `DELETE /me` — suppression définitive du compte.
///
/// Un seul `DELETE FROM users`, qui cascade sur bookmarks / history /
/// decision_views / identités / sessions web / mcp_tokens / mcp_auth_codes
/// (ON DELETE CASCADE).
pub async fn delete_me(state: &AppState, user_sub: &str) -> Result<(), ApiError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute("DELETE FROM users WHERE sub = $1", &[&user_sub])
        .await
        .map_err(|e| ApiError::Internal(format!("delete_me: {e}")))?;
    Ok(())
}
