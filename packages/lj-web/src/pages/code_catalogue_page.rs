//! Page `/normes/codes` (CodeCataloguePage) — catalogue des codes et
//! constitutions du corpus (`legal_text`), groupé par ordre/origine (FR / UE /
//! international / codes étrangers), chaque code liant son sommaire
//! `/texte/{code}`. Fond `codes` de l'arborescence des normes : SSR pur, sans
//! logique client — les autres fonds (lois, décrets…) se parcourent par année
//! sous `/normes/{fond}`.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use lj_dtos::CodeCatalogueEntry;

use crate::api::ApiClient;
use crate::components::own_page_description;
use crate::pages::juridictions_page::{
    breadcrumb_jsonld, set_cache_control, HubBreadcrumb, HubError, HubSkeleton,
};
use crate::pages::law_page::data::{sendable, PageError};

#[component]
pub fn CodeCataloguePage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    // Catalogue complet du fond, bloquant SSR (dans le document initial pour le SEO).
    let entries = Resource::new_blocking(|| (), |_| sendable(fetch_catalogue()));

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match entries.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <CatalogueLoaded entries=r.entries /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <HubError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

/// Charge le catalogue (codes + constitutions). Bloquant SSR.
async fn fetch_catalogue() -> Result<lj_dtos::CodeCatalogueResponse, PageError> {
    ApiClient::from_context()
        .fetch_codes_catalogue()
        .await
        .map_err(PageError::from)
}

/// Groupe d'affichage d'une valeur `jurisdiction` du corpus : `(rang,
/// libellé)`. Les juridictions étrangères (codes pays) fusionnent dans un seul
/// groupe « Droit étranger ».
fn jurisdiction_group(jurisdiction: &str) -> (u8, &'static str) {
    match jurisdiction {
        "FR" => (0, "France"),
        "UE" => (1, "Union européenne"),
        "INTL" => (2, "International"),
        _ => (3, "Droit étranger"),
    }
}

/// Sous-catégorie d'affichage d'un texte à partir de sa `nature` brute
/// (taxonomie du corpus) : `(rang, libellé)`. Le fond ne porte que ces deux
/// familles (codes, constitutions).
fn nature_category(nature: &str) -> (u8, &'static str) {
    let n = nature.to_ascii_uppercase();
    if n == "CONSTITUTION" {
        (1, "Constitutions")
    } else {
        (0, "Codes")
    }
}

#[component]
fn CatalogueLoaded(entries: Vec<CodeCatalogueEntry>) -> impl IntoView {
    let title = "Codes : textes consolidés - LibreJustice";
    let description =
        "Catalogue des codes et constitutions consolidés : droit français, droit de l'Union \
         européenne, conventions internationales et codes étrangers. Versions à date, articles liés.";
    let url = "https://librejustice.fr/normes/codes";
    let jsonld = breadcrumb_jsonld(&[
        ("Accueil", "https://librejustice.fr/".to_string()),
        ("Normes", "https://librejustice.fr/normes".to_string()),
        ("Codes", url.to_string()),
    ]);
    let count_line = format!("{} textes.", entries.len());

    // Groupes ordonnés (FR, UE, INTL, étranger), sous-groupés par nature.
    let mut groups: Vec<(u8, &'static str, Vec<CodeCatalogueEntry>)> = Vec::new();
    for entry in entries {
        let (rank, label) = jurisdiction_group(&entry.jurisdiction);
        if let Some(g) = groups.iter_mut().find(|(r, _, _)| *r == rank) {
            g.2.push(entry);
        } else {
            groups.push((rank, label, vec![entry]));
        }
    }
    groups.sort_by_key(|(r, _, _)| *r);

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <HubBreadcrumb items=vec![
                    ("Accueil".to_string(), Some("/".to_string())),
                    ("Normes".to_string(), Some("/normes".to_string())),
                    ("Codes".to_string(), None),
                ] />
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Textes & codes"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">"Codes"</h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">{count_line}</p>
            </header>

            {groups
                .into_iter()
                .map(|(_, label, codes)| {
                    view! { <CatalogueSection label=label codes=codes /> }
                })
                .collect_view()}
        </div>
    }
}

/// Une section d'ordre/origine : en-tête + sous-sections par nature
/// (Codes / Constitutions).
#[component]
fn CatalogueSection(label: &'static str, codes: Vec<CodeCatalogueEntry>) -> impl IntoView {
    // Sous-groupes (catégorie de nature) ordonnés Codes → Constitutions ;
    // chaque sous-groupe garde l'ordre alphabétique d'arrivée.
    let mut categories: Vec<(u8, &'static str, Vec<CodeCatalogueEntry>)> = Vec::new();
    for entry in codes {
        let (rank, label) = nature_category(&entry.nature);
        if let Some(g) = categories.iter_mut().find(|(r, _, _)| *r == rank) {
            g.2.push(entry);
        } else {
            categories.push((rank, label, vec![entry]));
        }
    }
    categories.sort_by_key(|(r, _, _)| *r);

    view! {
        <section class="flex flex-col gap-4">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {label}
            </h2>
            {categories
                .into_iter()
                .map(|(_, label, codes)| view! { <CatalogueCategory label=label codes=codes /> })
                .collect_view()}
        </section>
    }
}

/// Une sous-section par nature (Codes / Constitutions) dans un ordre de
/// juridiction.
#[component]
fn CatalogueCategory(label: &'static str, codes: Vec<CodeCatalogueEntry>) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-2">
            <h3 class="text-[0.7rem] uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]/70">
                {label}
            </h3>
            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                {codes
                    .into_iter()
                    .map(|entry| view! { <CatalogueRow entry=entry /> })
                    .collect_view()}
            </ul>
        </div>
    }
}

#[component]
fn CatalogueRow(entry: CodeCatalogueEntry) -> impl IntoView {
    let href = format!("/texte/{}", entry.code);
    let count = format!(
        "{} article{}",
        entry.article_count,
        if entry.article_count > 1 { "s" } else { "" }
    );
    view! {
        <li class="py-3">
            <A href=href attr:class="group flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                <span class="font-sans text-base text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                    {entry.title}
                </span>
                <span class="tabular-nums text-sm text-[var(--color-ink-subtle)]">{count}</span>
            </A>
        </li>
    }
}
