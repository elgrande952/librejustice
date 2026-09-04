//! Ops d'auth du front, sur le client `/internal` same-origin. Compile UNIQUEMENT
//! sous `hydrate` (cible wasm).
//!
//! Le cookie de session est `HttpOnly` : il n'est jamais lu ici, le navigateur
//! l'envoie et le reçoit seul. Les écrans de compte sont des formulaires natifs
//! sans JS — ne restent donc que la sonde de session et la déconnexion.
#![cfg(feature = "hydrate")]

use crate::api::ApiClient;

/// Email de la session courante, ou `None` (anonyme, ou cookie refusé — le
/// serveur l'efface alors lui-même).
pub async fn current_email() -> Option<String> {
    ApiClient::from_context().fetch_me().await.ok()?.email
}

/// `true` si une session existe.
pub async fn has_session() -> bool {
    ApiClient::from_context().fetch_me().await.is_ok()
}

/// Déconnexion de ce navigateur.
pub async fn sign_out() {
    let _ = ApiClient::from_context().logout().await;
}
