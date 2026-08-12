//! `ClientOnly` : rend ses enfants uniquement côté client.
//!
//! Au SSR et au 1er rendu d'hydratation, rend le `fallback` ; une fois le
//! document hydraté, bascule sur les enfants, montés frais.
//!
//! Le gate est global et à sens unique (`Hydrated`, fourni par `App`), pas
//! par-instance : après la bascule, un (re)montage via nav SPA monte les
//! enfants directement, sans re-flasher le squelette.
//!
//! Réservé aux routes non-SEO : le contenu n'est pas dans le HTML SSR, donc
//! invisible aux crawlers.

use leptos::prelude::*;

/// Vrai une fois le document hydraté, jamais re-faux. Fourni au contexte
/// racine par [`provide_hydrated`].
#[derive(Clone, Copy)]
pub struct Hydrated(pub RwSignal<bool>);

/// À appeler une fois dans `App`. Fournit le signal [`Hydrated`] (`false` au
/// SSR et au 1er rendu client), puis le bascule `true` via un `Effect` client.
pub fn provide_hydrated() {
    let hydrated = RwSignal::new(false);
    provide_context(Hydrated(hydrated));
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| hydrated.set(true));
}

/// Monte `children` une fois le document hydraté ; rend `fallback` avant.
#[component]
pub fn ClientOnly(
    children: ChildrenFn,
    /// Rendu au SSR et au 1er rendu client : squelette statique, sans état
    /// dépendant de `window`/`localStorage`.
    #[prop(optional, into)]
    fallback: ViewFn,
) -> impl IntoView {
    let Hydrated(hydrated) = expect_context::<Hydrated>();
    let children = StoredValue::new(children);
    move || {
        if hydrated.get() {
            children.with_value(|c| c()).into_any()
        } else {
            fallback.run().into_any()
        }
    }
}
