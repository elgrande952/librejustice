//! Redirection trailing-slash.
//!
//! Si le chemin demandé ne matche aucune route mais que le chemin avec le slash
//! final basculé (ajouté/retiré) matche, on renvoie un **307** vers le chemin
//! canonique (méthode + corps préservés) via le fallback du routeur racine.
//!
//! Le fallback ne se déclenche QUE sur un miss de routage (aucune route ne
//! matche le chemin). Un 404 *retourné par un handler* (ex. décision absente)
//! ne passe pas par ici.
//!
//! Détection sans introspection du routeur (non exposée par axum) : table des
//! gabarits servis, tenue en miroir de `create_app` / `oauth` / `mcp`. Match au
//! niveau du chemin uniquement.

use axum::extract::OriginalUri;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

/// The prefixes carrying the nest, so a template below is written once. The
/// public surface serves a subset of them, and a slash redirect toward a path
/// it does not serve lands on its own refusal, which is the right answer.
const NEST_PREFIXES: [&str; 2] = ["/api", "/internal"];

/// Templates served under each nest prefix, written without it. Mirror of the
/// routes mounted in [`crate::routes::create_app`]: a new route is added here to
/// get the slash redirect.
const NESTED_TEMPLATES: &[&str] = &[
    "/health",
    "/search",
    "/search-textes",
    "/decision/{p}",
    "/decision/{p}/similar",
    "/decision/{p}/download.pdf",
    "/me",
    "/me/activity-tracking",
    "/me/bookmarks",
    "/me/bookmarks/{p}",
    "/me/search-history",
    "/me/search-history/{p}",
    "/me/decision-views",
    "/me/decision-views/{p}",
];

/// Templates served at the root, mirror of [`crate::oauth`] and [`crate::mcp`].
const ROOT_TEMPLATES: &[&str] = &[
    "/oauth/register",
    "/oauth/authorize",
    "/oauth/approve",
    "/oauth/token",
    "/.well-known/oauth-authorization-server",
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/mcp",
    "/mcp/.well-known/oauth-authorization-server",
    "/mcp/.well-known/oauth-protected-resource",
    // Served with and without the slash, see `mcp::mcp_router`.
    "/mcp/",
    "/mcp",
];

/// Un gabarit (`/a/{x}/b`) matche-t-il un chemin concret ? Même nombre de
/// segments ; `{...}` matche n'importe quel segment non vide, sinon égalité.
fn template_matches(template: &str, path: &str) -> bool {
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    if t.len() != p.len() {
        return false;
    }
    t.iter().zip(&p).all(|(ts, ps)| {
        if ts.starts_with('{') && ts.ends_with('}') {
            !ps.is_empty()
        } else {
            ts == ps
        }
    })
}

fn route_exists(path: &str) -> bool {
    if ROOT_TEMPLATES.iter().any(|t| template_matches(t, path)) {
        return true;
    }
    NEST_PREFIXES.iter().any(|prefixe| {
        path.strip_prefix(prefixe)
            .is_some_and(|reste| NESTED_TEMPLATES.iter().any(|t| template_matches(t, reste)))
    })
}

/// Bascule le slash final : `/x/` → `/x`, `/x` → `/x/`. `None` pour la racine.
fn toggle_trailing_slash(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    match path.strip_suffix('/') {
        Some(stripped) => Some(stripped.to_string()),
        None => Some(format!("{path}/")),
    }
}

/// Fallback racine : 307 vers la variante de slash si elle matche une route,
/// sinon 404. La query string est préservée dans la `Location`.
pub async fn slash_redirect_fallback(OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    if let Some(alt) = toggle_trailing_slash(path) {
        if route_exists(&alt) {
            let target = match uri.query() {
                Some(q) => format!("{alt}?{q}"),
                None => alt,
            };
            return Redirect::temporary(&target).into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_param_matches_any_single_segment() {
        assert!(template_matches(
            "/api/decision/{p}",
            "/api/decision/abc123"
        ));
        assert!(!template_matches(
            "/api/decision/{p}",
            "/api/decision/abc/extra"
        ));
        assert!(!template_matches("/api/decision/{p}", "/api/decision/"));
    }

    #[test]
    fn route_exists_covers_health_and_mcp() {
        assert!(route_exists("/api/health"));
        assert!(route_exists("/mcp/"));
        assert!(route_exists("/mcp"));
        assert!(route_exists("/api/me/bookmarks/xyz"));
        assert!(!route_exists("/api/health/"));
    }

    #[test]
    fn le_nid_est_servi_sous_les_deux_prefixes() {
        for prefixe in NEST_PREFIXES {
            assert!(route_exists(&format!("{prefixe}/health")));
            assert!(route_exists(&format!("{prefixe}/me/bookmarks/xyz")));
        }
        assert!(!route_exists("/internal/health/"));
        assert!(!route_exists("/apihealth"));
    }

    #[test]
    fn toggle_adds_and_strips_one_slash() {
        assert_eq!(
            toggle_trailing_slash("/api/health/").as_deref(),
            Some("/api/health")
        );
        assert_eq!(toggle_trailing_slash("/mcp").as_deref(), Some("/mcp/"));
        assert_eq!(toggle_trailing_slash("/"), None);
    }
}
