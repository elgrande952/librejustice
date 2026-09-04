//! Reclassement LLM : état porté par l'URL seule. **Param absent = actif** ;
//! seul un OFF explicite s'écrit (`?aiMode=false`), et rien n'est mémorisé
//! entre visites, la soumission recopiant le choix dans l'URL.
//!
//! Ce fichier est rattaché au module via `#[path]` depuis `compact_search`.

use leptos::prelude::*;

/// État du reclassement pour une valeur de query param : **absent ⇒ actif**
/// (défaut), `false`/`0` ⇒ OFF explicite, tout le reste ⇒ actif.
pub fn ai_mode_from_param(value: Option<&str>) -> bool {
    !matches!(value, Some("false") | Some("0"))
}

/// Hook : état `aiMode` synchronisé avec le query param. Renvoie
/// `(getter, setter)`. L'init lit l'URL seule, donc SSR et client partent de la
/// même valeur et l'hydratation ne diverge pas.
pub fn use_ai_mode() -> (Signal<bool>, Callback<bool>) {
    use leptos_router::hooks::use_query_map;

    let query = use_query_map();
    let initial = ai_mode_from_param(query.get_untracked().get("aiMode").as_deref());
    let state = RwSignal::new(initial);

    #[cfg(feature = "hydrate")]
    {
        // Sync URL vers state (back/forward). Param retiré = retour au défaut.
        Effect::new(move |_| {
            let map = query.get();
            state.set(ai_mode_from_param(map.get("aiMode").as_deref()));
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
                let map = query.get_untracked();
                if map.get("q").is_some() {
                    // OFF explicite dans l'URL (l'absence vaut actif) ; actif =
                    // retrait du param, l'URL partagée reste propre.
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
