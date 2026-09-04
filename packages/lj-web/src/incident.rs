//! Balise d'incident client, vers `window.__ljIncident` posé par le shell
//! (`app::SONDE_CLIENT`), qui poste sur `/internal/client/incident`.

#[cfg(feature = "hydrate")]
pub fn signaler(motif: &str, detail: &wasm_bindgen::JsValue) {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::JsValue;

    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(balise) = js_sys::Reflect::get(&window, &JsValue::from_str("__ljIncident")) else {
        return;
    };
    if let Ok(balise) = balise.dyn_into::<js_sys::Function>() {
        let _ = balise.call2(&window, &JsValue::from_str(motif), detail);
    }
}

/// Le 429 sort du compte : c'est le budget de l'appelant, pas un désaccord de
/// contrat, et l'utilisateur le résout en attendant.
#[cfg(feature = "hydrate")]
pub fn signaler_recherche_refusee(status: u16) {
    use wasm_bindgen::JsValue;

    if !(400..500).contains(&status) || status == 429 {
        return;
    }
    let detail = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &detail,
        &JsValue::from_str("status"),
        &JsValue::from_f64(f64::from(status)),
    );
    signaler("recherche_refusee", detail.as_ref());
}
