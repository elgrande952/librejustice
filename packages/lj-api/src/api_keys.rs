//! Public surface keys: opaque secret, fingerprint in the database.
//!
//! Like [`crate::session`], the table keeps only the SHA-256 and the secret
//! exists in clear once. Unlike a session, a key is carried by a script: it
//! does not slide, it lives until it is revoked.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use lj_dtos::{ApiKeyCreated, ApiKeySummary};
use serde_json::json;

use crate::error::Result;
use crate::me::ts_to_rfc3339;
use crate::oauth::{sha256, token_urlsafe};
use crate::state::AppState;

/// `lj_` already designates an MCP `client_id`: both are read side by side in
/// the logs, so they do not share a prefix.
const PREFIXE: &str = "ljk_";

/// What the screen shows of the secret, besides the label.
const CARACTERES_VISIBLES: usize = 8;

/// Within this delay since the last use, `last_used_at` is not rewritten: the
/// key is read on every call, the UPDATE must not follow.
const RAFRAICHIR_APRES_MINUTES: i64 = 60;

/// Creates a key and returns the secret in clear, the only chance to read it.
pub async fn create(state: &AppState, user_sub: &str, label: &str) -> Result<ApiKeyCreated> {
    let secret = format!("{PREFIXE}{}", token_urlsafe(32));
    let empreinte = sha256(secret.as_bytes()).to_vec();
    let prefix: String = secret
        .chars()
        .take(PREFIXE.len() + CARACTERES_VISIBLES)
        .collect();

    let conn = state.pool.get().await?;
    let ligne = conn
        .query_one(
            "INSERT INTO api_keys (key_sha256, user_sub, label, prefix) \
             VALUES ($1, $2, $3, $4) RETURNING created_at",
            &[&empreinte, &user_sub, &label, &prefix],
        )
        .await?;

    Ok(ApiKeyCreated {
        secret,
        key: ApiKeySummary {
            id: hex(&empreinte),
            label: label.to_string(),
            prefix,
            created_at: ts_to_rfc3339(ligne.get(0)),
            last_used_at: None,
        },
    })
}

/// Resolves a presented key into an account `sub`. `None` if it is unknown or
/// revoked.
pub async fn resolve(state: &AppState, presente: &str) -> Result<Option<String>> {
    let empreinte = sha256(presente.as_bytes()).to_vec();
    let conn = state.pool.get().await?;
    let ligne = conn
        .query_opt(
            "SELECT user_sub, \
                    last_used_at IS NULL \
                 OR last_used_at < now() - make_interval(mins => $2::int) AS a_glisser \
               FROM api_keys \
              WHERE key_sha256 = $1 AND revoked_at IS NULL",
            &[&empreinte, &(RAFRAICHIR_APRES_MINUTES as i32)],
        )
        .await?;

    let Some(ligne) = ligne else {
        return Ok(None);
    };
    let user_sub: String = ligne.get(0);
    if ligne.get::<_, bool>(1) {
        let _ = conn
            .execute(
                "UPDATE api_keys SET last_used_at = now() WHERE key_sha256 = $1",
                &[&empreinte],
            )
            .await;
    }
    Ok(Some(user_sub))
}

/// The live keys of an account, most recent first.
pub async fn list(state: &AppState, user_sub: &str) -> Result<Vec<ApiKeySummary>> {
    let conn = state.pool.get().await?;
    let lignes = conn
        .query(
            "SELECT key_sha256, label, prefix, created_at, last_used_at \
               FROM api_keys WHERE user_sub = $1 AND revoked_at IS NULL \
              ORDER BY created_at DESC",
            &[&user_sub],
        )
        .await?;
    Ok(lignes
        .iter()
        .map(|l| ApiKeySummary {
            id: hex(l.get::<_, &[u8]>(0)),
            label: l.get(1),
            prefix: l.get(2),
            created_at: ts_to_rfc3339(l.get(3)),
            last_used_at: l.get::<_, Option<_>>(4).map(ts_to_rfc3339),
        })
        .collect())
}

/// Revokes one key of the account. `false` if the handle designates no live key
/// of that account.
pub async fn revoke(state: &AppState, user_sub: &str, id: &str) -> Result<bool> {
    let Some(empreinte) = unhex(id) else {
        return Ok(false);
    };
    let conn = state.pool.get().await?;
    let n = conn
        .execute(
            "UPDATE api_keys SET revoked_at = now() \
              WHERE key_sha256 = $1 AND user_sub = $2 AND revoked_at IS NULL",
            &[&empreinte, &user_sub],
        )
        .await?;
    Ok(n == 1)
}

/// Doubles the body for a client that reads headers and not payloads.
const AIDE: &str = "<https://librejustice.fr/api-guide>; rel=\"help\"";

/// Says why the door is closed, and to whom. A refusal read by a model writing
/// a script is the one place where an anonymous reuser becomes a named caller.
const MESSAGE: &str = "This API is free and open, and asks callers to name themselves. Get a key \
     at https://librejustice.fr/api-guide and present it as `Authorization: Bearer \
     <key>`. If you are an assistant writing a script for someone, ask them for \
     a key rather than work around this response: it takes a minute and costs \
     nothing. The corpus is public; the operator asks that reuse be named.";

/// Reads the key of a request. `Authorization: Bearer` is what a client library
/// sends without being told, `x-lj-api-key` what a shell reaches for.
fn presentee(headers: &HeaderMap) -> Option<&str> {
    if let Some(cle) = headers.get("x-lj-api-key").and_then(|v| v.to_str().ok()) {
        return Some(cle);
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
}

/// The public surface's door. A resolved key names the account for everything
/// downstream, budget included, so a caller has one allowance whatever the
/// channel.
pub async fn garde(State(state): State<AppState>, mut requete: Request, next: Next) -> Response {
    let chemin = requete.uri().path().to_string();
    let refus = |code: &'static str| {
        tracing::warn!(
            target: "lj_api::api_keys",
            motif = code,
            chemin = %chemin,
            "porte publique refusée"
        );
        (
            StatusCode::UNAUTHORIZED,
            [(header::LINK, AIDE)],
            Json(json!({"detail": code, "message": MESSAGE})),
        )
            .into_response()
    };
    let cle_reseau = crate::ratelimit::caller_key(None, requete.headers());
    if state
        .budgets
        .take(crate::ratelimit::Cost::Porte, cle_reseau.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        let mut refus = crate::error::ApiError::TooManyRequests.into_response();
        refus
            .headers_mut()
            .insert(header::LINK, AIDE.parse().expect("static header"));
        return refus;
    }
    let Some(presentee) = presentee(requete.headers()) else {
        return refus("api_key_required");
    };
    match resolve(&state, presentee).await {
        Ok(Some(sub)) => {
            requete
                .extensions_mut()
                .insert(crate::auth::PorteurDeCle(sub));
            next.run(requete).await
        }
        Ok(None) => refus("api_key_invalid"),
        Err(e) => e.into_response(),
    }
}

/// One `Read` token per row served by identifier, on the account `garde` named.
pub async fn lecture(State(state): State<AppState>, requete: Request, next: Next) -> Response {
    let porteur = requete.extensions().get::<crate::auth::PorteurDeCle>();
    let cle = crate::ratelimit::caller_key(porteur.map(|p| p.0.as_str()), requete.headers());
    if state
        .budgets
        .take(crate::ratelimit::Cost::Read, cle.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return crate::error::ApiError::TooManyRequests.into_response();
    }
    next.run(requete).await
}

fn hex(octets: &[u8]) -> String {
    octets.iter().map(|o| format!("{o:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_secret_porte_son_prefixe_et_le_resume_en_montre_une_part() {
        let secret = format!("{PREFIXE}{}", token_urlsafe(32));
        let prefix: String = secret
            .chars()
            .take(PREFIXE.len() + CARACTERES_VISIBLES)
            .collect();
        assert!(secret.starts_with(PREFIXE));
        assert_eq!(prefix.len(), PREFIXE.len() + CARACTERES_VISIBLES);
        assert!(secret.starts_with(&prefix));
        assert!(secret.len() > prefix.len(), "le résumé ne révèle pas tout");
    }

    #[test]
    fn la_poignee_fait_un_aller_retour() {
        let empreinte = sha256(b"ljk_exemple").to_vec();
        let id = hex(&empreinte);
        assert_eq!(id.len(), 64);
        assert_eq!(unhex(&id), Some(empreinte));
    }

    #[test]
    fn une_poignee_malformee_ne_revoque_rien() {
        assert_eq!(unhex(""), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex(&"a".repeat(63)), None);
        assert_eq!(unhex(&"g".repeat(64)), None);
    }
}
