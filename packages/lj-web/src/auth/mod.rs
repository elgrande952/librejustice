//! Ops d'auth du front (`bindings`) + `AuthGuard`.
//!
//! La session est un cookie `HttpOnly` : le front ne la lit pas, il demande
//! `/internal/me`. Le SSR ne fait donc aucune de ces requetes : le gate effectif
//! se joue cote client apres hydratation.

mod bindings;
mod guard;

use leptos::prelude::*;

pub use guard::AuthGuard;

#[cfg(feature = "hydrate")]
pub use bindings::{current_email, has_session, sign_out};

/// Etat d'authentification reactif partage : email de la session courante,
/// `None` si anonyme. Toujours `None` au SSR (pas de session locale) ; peuple et
/// tenu a jour cote client par le pilote d'auth (`app.rs::provide_auth_runtime`).
/// Lu par la top-bar (avatar + menu compte).
#[derive(Clone, Copy)]
pub struct AuthState {
    pub email: RwSignal<Option<String>>,
    /// Incremente a chaque changement d'etat de session produit par le front,
    /// c'est-a-dire la seule deconnexion : la connexion est un formulaire natif,
    /// donc un rechargement complet. Le pilote d'auth le suit et relit l'identite.
    pub revision: RwSignal<u32>,
}

impl AuthState {
    /// Signale que la session vient de changer sous nos pieds.
    pub fn bump(&self) {
        self.revision.update(|r| *r = r.wrapping_add(1));
    }
}

/// Pose un `AuthState` vide dans le contexte. Le pilotage (lecture de session,
/// suivi des changements) vit dans `app.rs::provide_auth_runtime`, racine de
/// composition qui orchestre email + cache + theme depuis une source unique.
pub fn provide_auth_state() {
    provide_context(AuthState {
        email: RwSignal::new(None::<String>),
        revision: RwSignal::new(0),
    });
}

/// Accede a l'`AuthState` du contexte (pose par `provide_auth_state`).
pub fn use_auth() -> AuthState {
    expect_context::<AuthState>()
}
