//! Deux primitives du navigateur qui n'ont pas d'équivalent Leptos : l'origine
//! de la page et la confirmation modale native. Compile UNIQUEMENT sous
//! `hydrate` (cible wasm).
#![cfg(feature = "hydrate")]

/// Origine de la page (`https://librejustice.fr`), pour composer une URL
/// absolue à montrer. Vide si le contexte n'a pas de fenêtre.
pub fn location_origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

/// Confirmation modale native. `false` si l'utilisateur refuse, et si le
/// navigateur supprime la boîte — un geste destructeur ne part pas sans un oui.
pub fn confirm_native(message: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(message).ok())
        .unwrap_or(false)
}
