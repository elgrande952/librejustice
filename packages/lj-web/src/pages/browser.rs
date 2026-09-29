//! The page origin, a browser primitive with no Leptos equivalent. Compiles
//! under `hydrate` only (wasm target).
#![cfg(feature = "hydrate")]

/// Origine de la page (`https://librejustice.fr`), pour composer une URL
/// absolue à montrer. Vide si le contexte n'a pas de fenêtre.
pub fn location_origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}
