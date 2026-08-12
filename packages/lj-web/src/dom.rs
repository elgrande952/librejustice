//! Helpers DOM bindés à des shims JS, pour ce que les features web-sys héritées
//! de leptos n'exposent pas. Compile UNIQUEMENT sous `hydrate` (le module est
//! vide en SSR — les appels sont gatés `#[cfg(feature = "hydrate")]` côté
//! appelant, comme `spawn_local` dans `activity_page` / `DecisionActions`).
#![cfg(feature = "hydrate")]

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/js/clipboard.js")]
extern "C" {
    #[wasm_bindgen(js_name = "copyText")]
    fn js_copy_text(text: &str) -> js_sys::Promise;
}

/// Écrit `text` dans le presse-papier. `true` au succès, `false` si refusé.
pub async fn copy_text(text: &str) -> bool {
    JsFuture::from(js_copy_text(text)).await.is_ok()
}

#[wasm_bindgen(module = "/js/nav-store.js")]
extern "C" {
    /// Graine de navigation persistée pour cette décision, `None` si l'onglet
    /// n'en a pas ou si le stockage est refusé.
    #[wasm_bindgen(js_name = "navSeedGet")]
    pub fn nav_seed_get(id: &str) -> Option<String>;

    /// Persiste la graine de cette décision pour la durée de l'onglet.
    #[wasm_bindgen(js_name = "navSeedSet")]
    pub fn nav_seed_set(id: &str, json: &str);
}
