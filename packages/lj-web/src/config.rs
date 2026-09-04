//! Config centralisee cote serveur : pas de `std::env::var` disperse, tout
//! passe par `Settings`. C'est l'UNIQUE endroit ou lj-web touche
//! l'environnement. Le port front + les assets vivent dans la config
//! cargo-leptos (`[[workspace.metadata.leptos]]`) + `LeptosOptions`, pas ici.
#![cfg(feature = "ssr")]

/// Reglages serveur du front. Prefix d'environnement obligatoire : `LIBREJUSTICE_`.
pub struct Settings {
    /// `cargo leptos watch` (posé par cargo-leptos, pas par nous). Le composant
    /// `AutoReload` n'émet son script de rechargement que dans ce mode, et ce
    /// script ouvre une WebSocket vers le port de reload : la CSP doit l'y
    /// autoriser, sinon `mise run dev` perd le hot-reload.
    pub watch: bool,
}

impl Settings {
    pub fn from_env() -> Self {
        Self {
            watch: std::env::var("LEPTOS_WATCH").is_ok(),
        }
    }
}
