//! `Skeleton` : placeholder anime.

use leptos::prelude::*;

use crate::helpers::cn;

/// Bloc skeleton. `class` regle la taille (h-…/w-…).
///
/// Le rayon a son propre prop et ne se surcharge PAS par `class` : `cn` concatène
/// sans dédoublonner, et Tailwind émet `.rounded-sm` après tous les autres rayons
/// — à spécificité égale, un `rounded-md`/`rounded-full` glissé dans `class`
/// serait silencieusement perdu.
#[component]
pub fn Skeleton(
    #[prop(optional, into)] class: String,
    #[prop(default = "rounded-sm")] rounded: &'static str,
) -> impl IntoView {
    let classes = cn(["lj-skeleton bg-[var(--color-vellum)]", rounded, &class]);
    view! { <div class=classes></div> }
}
