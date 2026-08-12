//! Shim d'authentification : bindings wasm-bindgen vers `js/auth.js` +
//! `AuthGuard`.
//!
//! Cote SSR (pas de `localStorage`/session), toutes les fns sont des no-op
//! anonymes : `get_access_token() -> None`, l'AuthGuard ne redirige pas (le
//! gate effectif se joue cote client apres hydratation).

mod bindings;
mod guard;

use leptos::prelude::*;

pub use guard::AuthGuard;

#[cfg(feature = "hydrate")]
pub use bindings::{
    current_email, has_session, on_auth_state_change, reset_password, sign_in_oauth,
    sign_in_password, sign_out, sign_up,
};

/// Jeton d'acces de la session courante (header `Authorization: Bearer`).
///
/// Cote SSR : toujours `None` (anonyme — pas de session locale). Cote wasm :
/// delegue au shim JS (Supabase `getSession`).
#[cfg(feature = "ssr")]
pub async fn get_access_token() -> Option<String> {
    None
}

/// Jeton d'acces de la session courante (header `Authorization: Bearer`).
#[cfg(feature = "hydrate")]
pub async fn get_access_token() -> Option<String> {
    bindings::get_access_token().await
}

/// `true` si une session vit dans le storage local. Lu quand
/// [`get_access_token`] rend `None` : la session est la, mais son jeton n'a pas
/// pu etre rafraichi dans le delai — la requete part en anonyme et le client
/// HTTP le signale par [`lj_dtos::SESSION_DEGRADED_HEADER`].
///
/// Cote SSR : toujours `false` (pas de storage local).
#[cfg(feature = "ssr")]
pub fn session_in_storage() -> bool {
    false
}

/// `true` si une session vit dans le storage local.
#[cfg(feature = "hydrate")]
pub fn session_in_storage() -> bool {
    bindings::session_in_storage()
}

/// Etat d'authentification reactif partage : email de la session courante,
/// `None` si anonyme. Toujours `None` au SSR (pas de session locale) ; peuple et
/// tenu a jour cote client par le pilote d'auth (`app.rs::provide_auth_runtime`).
/// Lu par la top-bar (avatar + menu compte).
#[derive(Clone, Copy)]
pub struct AuthState {
    pub email: RwSignal<Option<String>>,
}

/// Pose un `AuthState` vide dans le contexte. Le pilotage (lecture de session,
/// abonnement aux changements) vit dans `app.rs::provide_auth_runtime`, racine de
/// composition qui orchestre email + cache + theme depuis une source unique.
pub fn provide_auth_state() {
    provide_context(AuthState {
        email: RwSignal::new(None::<String>),
    });
}

/// Accede a l'`AuthState` du contexte (pose par `provide_auth_state`).
pub fn use_auth() -> AuthState {
    expect_context::<AuthState>()
}
