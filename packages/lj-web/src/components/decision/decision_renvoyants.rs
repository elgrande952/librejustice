//! Bloc « Citée par les analyses du Conseil d'État ». Le rapprochement est écrit
//! par le service de documentation, pas par le juge : sur 9 702 renvois, 59 sont
//! aussi une citation du corps.

use leptos::prelude::*;
use lj_dtos::Renvoyant;

use crate::components::hover_preview::{HoverPreview, PreviewKind};
use crate::helpers::format_iso_date;

#[component]
pub fn DecisionRenvoyants(renvoyants: Vec<Renvoyant>) -> impl IntoView {
    if renvoyants.is_empty() {
        return ().into_any();
    }
    let title = "Citée par les analyses du Conseil d'État";
    let lignes = renvoyants
        .into_iter()
        .map(|r| view! { <RenvoyantLigne renvoyant=r /> })
        .collect_view();

    view! {
        <section
            aria-label=title
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6"
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">{title}</h2>
            <ul class="mt-4 flex flex-col gap-2">{lignes}</ul>
        </section>
    }
    .into_any()
}

/// Une décision citante. Rendue par un composant pour que les enfants du
/// `collect_view()` restent uniformes, le noeud optionnel vivant derrière la
/// frontière (règle wasm).
#[component]
fn RenvoyantLigne(renvoyant: Renvoyant) -> impl IntoView {
    let Renvoyant {
        id,
        label,
        date,
        docket_numbers,
    } = renvoyant;
    let href = format!("/decision/{id}");
    let intitule = match date.as_deref() {
        Some(d) => format!("{label}, {}", format_iso_date(Some(d))),
        None => label,
    };
    let numeros = docket_numbers
        .filter(|n| !n.is_empty())
        .map(|n| format!("n° {}", n.join(", ")));

    view! {
        <li class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
            <HoverPreview kind=PreviewKind::Decision { id }>
                <a
                    href=href
                    class="text-sm text-[var(--color-accent)] underline-offset-4 hover:underline"
                >
                    {intitule}
                </a>
            </HoverPreview>
            {numeros
                .map(|n| {
                    view! { <span class="text-xs text-[var(--color-ink-subtle)]">{n}</span> }
                })}
        </li>
    }
}
