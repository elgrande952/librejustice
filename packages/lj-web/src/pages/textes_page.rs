//! Page de recherche de textes `/textes` (lois et règlements) — page distincte
//! de `/decisions` : le corpus articles est servi par un moteur BM25 seul, avec
//! ses propres filtres ; aucune recherche transverse.
//!
//! Rendue côté client comme `/decisions` : route non indexée, mêmes îlots
//! interactifs (barre, dropdowns). `<Title>` dynamique reflétant la requête `q`
//! de l'URL ; hérite de la description racine.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use crate::components::client_only::ClientOnly;
use crate::components::layout::{CONTENT_COL, GRID_3COL, SHELL};
use crate::components::search::compact_search::DraftQuery;
use crate::components::search::TextesView;

#[component]
pub fn TextesPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    let query_map = use_query_map();
    // `leptos_meta` garde cette clôture dans un contexte de durée de vie
    // applicative et la réévalue APRÈS le démontage de la route, pour restaurer
    // le titre — `query_map`, propriété de la route, est alors déjà libéré. Un
    // `get()` y panique ; `try_get` rend `None` et on sert le titre nu.
    let title = move || {
        let q = query_map
            .try_get()
            .and_then(|params| params.get("q"))
            .unwrap_or_default();
        let q = q.trim();
        if q.is_empty() {
            "Textes - LibreJustice".to_string()
        } else {
            format!("{q} - Textes - LibreJustice")
        }
    };
    view! {
        <Title text=title />
        <ClientOnly fallback=|| {
            view! { <TextesPageSkeleton /> }
        }>
            <TextesPageBody />
        </ClientOnly>
    }
}

/// Squelette statique du fallback SSR/1er rendu : même grille que le corps
/// (rail vide + colonne contenu, gabarit `/decisions`), barre inerte +
/// placeholder de liste. Aucun signal dépendant de `window`/`localStorage` →
/// hydratation sans divergence.
#[component]
fn TextesPageSkeleton() -> impl IntoView {
    view! {
        <div class=SHELL>
            <h1 class="sr-only">"Recherche de textes"</h1>
            <div class=GRID_3COL>
                <div class="hidden lg:block"></div>
                <div class=CONTENT_COL>
                    <div class="h-12 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/40"></div>
                    <div class="flex flex-col gap-4">
                        {(0..4)
                            .map(|_| {
                                view! {
                                    <div class="h-20 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/30"></div>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn TextesPageBody() -> impl IntoView {
    let query_map = use_query_map();
    // Texte de la barre partagé avec les filtres (même mécanique que
    // `/decisions`) : une mutation de filtre applique le texte non soumis.
    provide_context(DraftQuery(RwSignal::new(
        query_map.get_untracked().get("q").unwrap_or_default(),
    )));

    view! {
        <div class=SHELL>
            <h1 class="sr-only">"Recherche de textes"</h1>
            <TextesView />
        </div>
    }
}
