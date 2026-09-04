//! Extracteurs d'identité des routes REST : le cookie de session, et rien
//! d'autre.
//!
//! Le MCP a sa propre porte (`mcp.rs`, jetons opaques de `mcp_tokens`) : elle ne
//! passe pas par ici.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// Extracteur axum : `user_id` ou `None`.
#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<String>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        // La surface publique a déjà nommé son appelant : elle n'a pas de cookie
        // à présenter, et la clé vaut la session pour tout ce qui suit.
        if let Some(PorteurDeCle(sub)) = parts.extensions.get::<PorteurDeCle>() {
            return Ok(OptionalUser(Some(sub.clone())));
        }
        let Some((cookie, legacy)) = crate::session::from_parts(parts) else {
            return Ok(OptionalUser(None));
        };
        use crate::session::Verdict;
        // Troisième état : le cookie existe et ne vaut rien. Journalisé ICI, sur
        // toutes les routes — y compris celles qui se contentent de dégrader en
        // anonyme, où le refus serait sinon muet. Le motif est dans le corps du
        // message : un filtre de ligne Loki ne voit que lui.
        match crate::session::resolve(state, &cookie).await {
            Verdict::Porteur { sub, refresh } => {
                // Réémission : quand l'échéance a glissé en base, pour que le
                // navigateur la suive ; et quand le cookie est arrivé sous le nom
                // hérité, pour qu'il passe au nom préfixé sans attendre.
                if refresh || legacy {
                    if let Some(slot) = parts.extensions.get::<crate::session::CookieRefresh>() {
                        slot.set(crate::session::set_cookie(&cookie));
                    }
                }
                Ok(OptionalUser(Some(sub)))
            }
            Verdict::Indisponible => {
                tracing::warn!("session indéterminée : la base n'a pas répondu");
                Ok(OptionalUser(None))
            }
            refus => {
                let motif = match refus {
                    Verdict::FormeInvalide => "forme_invalide",
                    _ => "inconnue",
                };
                tracing::warn!("session refusée : {motif}");
                parts.extensions.insert(SessionRejected);
                Ok(OptionalUser(None))
            }
        }
    }
}

/// Marque de requête : un cookie de session a été présenté et refusé.
#[derive(Debug, Clone, Copy)]
pub struct SessionRejected;

/// Compte nommé par une clé d'API, posé par [`crate::api_keys::garde`] sur la
/// surface publique. Une clé y vaut la session : même compte, même budget.
#[derive(Debug, Clone)]
pub struct PorteurDeCle(pub String);

/// Extracteur axum : `user_id` requis, sinon 401 `auth_required` — ou
/// `session_rejected` si un cookie a été présenté et refusé.
#[derive(Debug, Clone)]
pub struct RequiredUser(pub String);

impl FromRequestParts<AppState> for RequiredUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Ok(OptionalUser(user)) = OptionalUser::from_request_parts(parts, state).await;
        user.map(RequiredUser).ok_or_else(|| {
            if parts.extensions.get::<SessionRejected>().is_some() {
                ApiError::SessionRejected
            } else {
                ApiError::Unauthorized
            }
        })
    }
}
