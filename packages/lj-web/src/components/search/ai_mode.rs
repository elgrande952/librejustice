//! Toggle « mode IA » persistant.
//!
//! Source de vérité : l'URL. **Param absent = mode IA ON** ; seul un OFF
//! explicite s'écrit (`?aiMode=false`). Le localStorage mémorise le choix entre
//! sessions, y compris le OFF. Hook Leptos : `use_ai_mode()` renvoie
//! `(Signal<bool>, Callback<bool>)` synchronisé avec le query param `aiMode` ET
//! le localStorage.
//!
//! Ce fichier est rattaché au module via `#[path]` depuis `compact_search`.

use leptos::prelude::*;

/// Clé localStorage de la préférence mode IA (lue/écrite côté client seulement).
#[cfg(feature = "hydrate")]
const STORAGE_KEY: &str = "librejustice.aiMode";

/// État du mode IA pour une valeur de query param : **absent ⇒ ON** (défaut),
/// `false`/`0` ⇒ OFF explicite, tout le reste ⇒ ON.
pub fn ai_mode_from_param(value: Option<&str>) -> bool {
    !matches!(value, Some("false") | Some("0"))
}

/// Préférence stockée : `Some(bool)` si l'utilisateur a tranché, `None` s'il n'a
/// jamais touché au toggle (→ le défaut s'applique). Accès via `js_sys::Reflect`
/// sur `window.localStorage` (la feature web-sys `Storage` n'est pas activée
/// dans le workspace).
#[cfg(feature = "hydrate")]
pub fn read_stored() -> Option<bool> {
    match local_storage_get(STORAGE_KEY).as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// SSR : pas de localStorage, donc aucune préférence connue.
#[cfg(feature = "ssr")]
pub fn read_stored() -> Option<bool> {
    None
}

/// Écrit la préférence. Les deux sens sont mémorisés (`"1"` / `"0"`) : le OFF
/// est un choix, pas une absence de choix.
#[cfg(feature = "hydrate")]
pub fn write_stored(value: bool) {
    local_storage_set(STORAGE_KEY, if value { "1" } else { "0" });
}

#[cfg(feature = "ssr")]
pub fn write_stored(_value: bool) {}

// ── Pont localStorage via js_sys (sans feature web-sys Storage) ──────────────

#[cfg(feature = "hydrate")]
fn local_storage() -> Option<js_sys::Object> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window()?;
    let storage = js_sys::Reflect::get(&window, &"localStorage".into()).ok()?;
    storage.dyn_into::<js_sys::Object>().ok()
}

#[cfg(feature = "hydrate")]
fn local_storage_get(key: &str) -> Option<String> {
    use wasm_bindgen::JsCast;
    let storage = local_storage()?;
    let get = js_sys::Reflect::get(&storage, &"getItem".into()).ok()?;
    let func: js_sys::Function = get.dyn_into().ok()?;
    let value = func.call1(&storage, &key.into()).ok()?;
    value.as_string()
}

#[cfg(feature = "hydrate")]
fn local_storage_set(key: &str, value: &str) {
    use wasm_bindgen::JsCast;
    if let Some(storage) = local_storage() {
        if let Ok(set) = js_sys::Reflect::get(&storage, &"setItem".into()) {
            if let Ok(func) = set.dyn_into::<js_sys::Function>() {
                let _ = func.call2(&storage, &key.into(), &value.into());
            }
        }
    }
}

/// Hook : état `aiMode` synchronisé avec le query param `aiMode` et le
/// localStorage. Renvoie `(getter, setter)`.
///
/// - Init : l'URL seule (absente ⇒ défaut ON), identique SSR et client.
/// - Effect (hydrate) : sync URL -> state au back/forward.
/// - Effect mount (hydrate) : si l'URL n'a pas `aiMode` mais le store dit OFF et
///   `q` est présent, patcher l'URL (`replace`) pour servir un résultat sans IA.
/// - `set` : maj state + `write_stored` ; si `q` présent, refléter dans l'URL.
pub fn use_ai_mode() -> (Signal<bool>, Callback<bool>) {
    use leptos_router::hooks::use_query_map;

    let query = use_query_map();
    // Init aligné sur le SSR : l'URL seule, jamais le localStorage. Lire le store
    // ici désynchroniserait le SSR (pas de `window`) du client (préférence
    // mémorisée) ; Leptos adopte alors le DOM SSR à l'hydratation et ne re-patche
    // PAS l'attribut tant que le signal ne CHANGE pas. La préférence localStorage
    // est donc appliquée post-hydratation par l'effet de mount, un vrai changement
    // de valeur. Depuis le défaut ON, le seul cas à rattraper est le OFF mémorisé.
    let initial = ai_mode_from_param(query.get_untracked().get("aiMode").as_deref());
    let state = RwSignal::new(initial);

    #[cfg(feature = "hydrate")]
    {
        use leptos_router::hooks::use_navigate;
        use leptos_router::NavigateOptions;

        // Sync URL -> state (back/forward). Param retiré = retour au défaut ON.
        Effect::new(move |_| {
            let map = query.get();
            state.set(ai_mode_from_param(map.get("aiMode").as_deref()));
        });

        // Au mount (post-hydratation) : applique la préférence localStorage. Le
        // store OFF sans `aiMode` dans l'URL fait passer `state` de true (défaut
        // SSR) à false, ce qui re-patche `aria-checked`. Si `q` est présent on
        // patche aussi l'URL (replace) pour servir un résultat sans IA dès la 1re
        // requête ; sans `q` (landing) le toggle s'affiche OFF, l'URL est posée au
        // submit par le caller.
        let navigate = use_navigate();
        Effect::new(move |prev: Option<()>| {
            if prev.is_some() {
                return;
            }
            let map = query.get_untracked();
            if map.get("aiMode").is_none() && read_stored() == Some(false) {
                state.set(false);
                if map.get("q").is_some() {
                    let qs = super::query_state::with_param(&map, "aiMode", Some("false"), false);
                    navigate(
                        &super::query_state::search_href(&qs),
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
            }
        });
    }

    let setter = {
        #[cfg(feature = "hydrate")]
        {
            use leptos_router::hooks::use_navigate;
            use leptos_router::NavigateOptions;
            let navigate = use_navigate();
            Callback::new(move |value: bool| {
                state.set(value);
                write_stored(value);
                let map = query.get_untracked();
                if map.get("q").is_some() {
                    // OFF explicite dans l'URL (l'absence vaut ON) ; ON = retrait
                    // du param, l'URL partagée reste propre.
                    let qs = super::query_state::with_param(
                        &map,
                        "aiMode",
                        if value { None } else { Some("false") },
                        false,
                    );
                    navigate(
                        &super::query_state::search_href(&qs),
                        NavigateOptions {
                            replace: true,
                            ..Default::default()
                        },
                    );
                }
            })
        }
        #[cfg(feature = "ssr")]
        {
            Callback::new(move |value: bool| {
                state.set(value);
            })
        }
    };

    (state.into(), setter)
}
