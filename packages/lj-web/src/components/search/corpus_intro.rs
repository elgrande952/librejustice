//! État sans requête des pages de recherche (`/decisions`, `/textes`).
//!
//! Le principe : chaque emplacement partagé avec la page de résultats est
//! rempli dans les DEUX états, au même endroit — volumétrie (colonne droite
//! desktop + ligne compacte mobile), rail gauche, pont croisé. L'aide à la
//! recherche, qui suit la volumétrie, garde donc la même position avant et
//! après recherche.
//!
//! Ce qui remplit ces emplacements sans requête est du **global corpus**
//! (compteurs `/api/corpus-stats`, catalogues juridictions et fonds de normes),
//! pas un échantillon de décisions : les volumes affichés sont ceux du fond
//! entier.
//!
//! Fetch depuis un `Effect` vers un signal, comme les ponts croisés : jamais de
//! `Resource` lue dans le rendu d'un rail, et une erreur replie le bloc.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{CorpusStatsResponse, JurisdictionCatalogueResponse, NormCatalogueResponse};

use crate::api::ApiClient;
use crate::helpers::{encode_query, group_thousands};

/// Lignes affichées par bloc de rail (aligné sur les blocs de facettes).
const RAIL_ROWS: usize = 5;

/// Compteurs corpus, chargés une fois par page (cache 12 h côté API). Erreur ⇒
/// `None` : les blocs qui en dépendent disparaissent.
pub fn corpus_stats() -> Signal<Option<CorpusStatsResponse>> {
    let data = RwSignal::new(None::<CorpusStatsResponse>);
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            data.set(ApiClient::from_context().fetch_corpus_stats().await.ok());
        });
    });
    data.into()
}

/// Volumétrie corpus de la colonne droite de `/decisions` sans requête : même
/// gabarit trois lignes que la volumétrie de résultats (`SearchSummary`).
#[component]
pub fn DecisionsCorpusSummary(
    #[prop(into)] stats: Signal<Option<CorpusStatsResponse>>,
) -> impl IntoView {
    move || {
        stats.get().map(|s| {
            summary_block(
                format!("{} décisions", group_thousands(s.decisions_count)),
                "jurisprudence administrative et judiciaire".to_string(),
                "Recherche lexicale ou sémantique",
            )
        })
    }
}

/// Volumétrie corpus de la colonne droite de `/textes` sans requête.
#[component]
pub fn TextesCorpusSummary(
    #[prop(into)] stats: Signal<Option<CorpusStatsResponse>>,
) -> impl IntoView {
    move || {
        stats.get().map(|s| {
            summary_block(
                format!("{} textes", group_thousands(s.texts_count)),
                format!("{} articles en vigueur", group_thousands(s.articles_count)),
                "Codes, lois, règlements, doctrine administrative",
            )
        })
    }
}

/// Ligne compacte mobile de `/decisions` sans requête, à la place qu'occupe la
/// volumétrie des résultats. Le volume seul : la ligne partage sa hauteur avec
/// l'aide à la recherche, et la précision (mode, articles) la ferait passer à la
/// ligne sous l'aide.
#[component]
pub fn DecisionsCorpusLine(
    #[prop(into)] stats: Signal<Option<CorpusStatsResponse>>,
) -> impl IntoView {
    move || {
        stats
            .get()
            .map(|s| summary_line(format!("{} décisions", group_thousands(s.decisions_count))))
    }
}

/// Ligne compacte mobile de `/textes` sans requête.
#[component]
pub fn TextesCorpusLine(#[prop(into)] stats: Signal<Option<CorpusStatsResponse>>) -> impl IntoView {
    move || {
        stats
            .get()
            .map(|s| summary_line(format!("{} textes", group_thousands(s.texts_count))))
    }
}

/// Bloc « Juridictions » du rail gauche de `/decisions` sans requête : les
/// juridictions les plus volumineuses du fond, chacune vers son hub.
#[component]
pub fn JuridictionsRail() -> impl IntoView {
    let data = RwSignal::new(None::<JurisdictionCatalogueResponse>);
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            data.set(ApiClient::from_context().fetch_juridictions().await.ok());
        });
    });

    move || {
        let Some(catalogue) = data.get() else {
            return ().into_any();
        };
        let mut entries: Vec<_> = catalogue
            .groups
            .into_iter()
            .flat_map(|g| g.jurisdictions)
            .collect();
        let total = entries.len() as i64;
        entries.sort_by_key(|e| std::cmp::Reverse(e.decision_count));
        let rows = entries
            .into_iter()
            .take(RAIL_ROWS)
            .map(|e| {
                count_row(
                    format!("/juridiction/{}", e.code),
                    e.label,
                    e.decision_count,
                )
            })
            .collect_view()
            .into_any();
        rail_head(rail_block(
            "Juridictions",
            rows,
            "/juridictions".to_string(),
            format!("Voir les {total} juridictions"),
        ))
    }
}

/// Bloc « Normes » du rail gauche de `/textes` sans requête : les premiers fonds
/// du catalogue, chacun vers son hub — pendant exact de `JuridictionsRail` sur
/// `/decisions`. Le fond `codes` n'est qu'un fond parmi dix.
///
/// Ordre éditorial de l'API (la pyramide des normes : codes, lois, décrets,
/// arrêtés…), pas le volume comme côté juridictions : `text_count` compte les
/// textes, or un code est UN texte pour des milliers d'articles.
#[component]
pub fn NormesRail() -> impl IntoView {
    let data = RwSignal::new(None::<NormCatalogueResponse>);
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            data.set(ApiClient::from_context().fetch_normes().await.ok());
        });
    });

    move || {
        let Some(catalogue) = data.get() else {
            return ().into_any();
        };
        let total = catalogue.fonds.len() as i64;
        let rows = catalogue
            .fonds
            .into_iter()
            .take(RAIL_ROWS)
            .map(|e| count_row(format!("/normes/{}", e.fond), e.label, e.text_count))
            .collect_view()
            .into_any();
        rail_head(rail_block(
            "Normes",
            rows,
            "/normes".to_string(),
            format!("Voir les {total} fonds"),
        ))
    }
}

/// Pont « Dans les textes » de `/decisions` sans requête : le volume du fond
/// normatif et l'entrée de son arborescence, à la place du teaser de requête.
#[component]
pub fn TextesCorpusTeaser(
    #[prop(into)] stats: Signal<Option<CorpusStatsResponse>>,
) -> impl IntoView {
    move || {
        stats.get().map(|s| {
            rail_block(
                "Dans les textes",
                view! {
                    <p class="text-[13px] leading-snug text-[var(--color-ink-muted)]">
                        {format!(
                            "{} textes · {} articles en vigueur",
                            group_thousands(s.texts_count),
                            group_thousands(s.articles_count),
                        )}
                    </p>
                }
                .into_any(),
                "/normes".to_string(),
                "Parcourir les textes".to_string(),
            )
        })
    }
}

/// Pont « Dans les décisions » de `/textes` sans requête.
#[component]
pub fn DecisionsCorpusTeaser(
    #[prop(into)] stats: Signal<Option<CorpusStatsResponse>>,
) -> impl IntoView {
    move || {
        stats.get().map(|s| {
            rail_block(
                "Dans les décisions",
                view! {
                    <p class="text-[13px] leading-snug text-[var(--color-ink-muted)]">
                        {format!(
                            "{} décisions, deux ordres de juridiction",
                            group_thousands(s.decisions_count),
                        )}
                    </p>
                }
                .into_any(),
                "/juridictions".to_string(),
                "Parcourir les juridictions".to_string(),
            )
        })
    }
}

/// Requêtes d'exemple de `/decisions`, cliquables : la zone des résultats porte
/// de quoi lancer une recherche au lieu d'un grand vide.
#[component]
pub fn DecisionsExamples() -> impl IntoView {
    examples(
        "/decisions",
        &[
            "responsabilité du fait des choses",
            "trouble anormal de voisinage",
            "bail commercial ET déplafonnement",
            "permis de construire SAUF référé",
            "licenciement PROCHE5 harcèlement",
        ],
    )
}

/// Requêtes d'exemple de `/textes`, cliquables.
#[component]
pub fn TextesExamples() -> impl IntoView {
    examples(
        "/textes",
        &[
            "responsabilité du fait des choses",
            "article 1240 code civil",
            "préavis de démission",
            "trouble anormal de voisinage",
        ],
    )
}

/// Volumétrie trois lignes (titre volumétrique, précision, mode) — gabarit
/// commun à la volumétrie de résultats et à celle du corpus.
fn summary_block(head: String, line: String, foot: &'static str) -> AnyView {
    view! {
        <div class="flex flex-col gap-1">
            <h2 class="font-sans text-xl text-[var(--color-ink)]">{head}</h2>
            <p class="text-sm leading-snug text-[var(--color-ink-subtle)]">{line}</p>
            <p class="text-xs text-[var(--color-ink-subtle)]">{foot}</p>
        </div>
    }
    .into_any()
}

/// Ligne compacte du volume (mobile).
fn summary_line(head: String) -> AnyView {
    view! {
        <p class="text-sm text-[var(--color-ink-subtle)]">
            <span class="font-medium text-[var(--color-ink)]">{head}</span>
        </p>
    }
    .into_any()
}

/// Tête de rail gauche : le premier bloc d'une colonne perd son filet haut,
/// comme les blocs de facettes de la page de résultats.
fn rail_head(block: AnyView) -> AnyView {
    view! {
        <div class="[&>div]:border-t-0 [&>div]:pt-0">{block}</div>
    }
    .into_any()
}

/// Bloc de rail : filet haut, titre en petites capitales, lignes, lien « voir
/// tout » — même anatomie que les blocs de facettes et les ponts croisés.
fn rail_block(
    title: &'static str,
    rows: AnyView,
    more_href: String,
    more_label: String,
) -> AnyView {
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            {rows}
            <A
                href=more_href
                attr:class="pt-1 text-[13px] text-[var(--color-ink-muted)] underline-offset-2 transition-colors hover:text-[var(--color-accent)] hover:underline"
            >
                {more_label}
                " →"
            </A>
        </div>
    }
    .into_any()
}

/// Ligne `libellé … compte` d'un bloc de rail, en lien (les lignes de facettes
/// sont des boutons de filtre ; sans requête, elles mènent au catalogue).
fn count_row(href: String, label: String, count: i64) -> impl IntoView {
    let title = label.clone();
    view! {
        <A
            href=href
            attr:class="group flex w-full items-baseline justify-between gap-3"
            attr:title=title
        >
            <span class="truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors group-hover:text-[var(--color-ink)]">
                {label}
            </span>
            <span class="shrink-0 font-mono text-[11px] tabular-nums text-[var(--color-ink-subtle)]">
                {group_thousands(count)}
            </span>
        </A>
    }
}

/// Liste de requêtes d'exemple vers un moteur.
fn examples(engine: &'static str, queries: &'static [&'static str]) -> impl IntoView {
    let rows = queries
        .iter()
        .map(|q| {
            view! {
                <A
                    href=format!("{engine}?q={}", encode_query(q))
                    attr:class="text-[var(--color-ink-muted)] underline-offset-2 transition-colors hover:text-[var(--color-accent)] hover:underline"
                >
                    "«\u{00A0}"
                    {*q}
                    "\u{00A0}»"
                </A>
            }
        })
        .collect_view();
    view! {
        <div class="flex flex-col gap-2 pt-2">
            <p class="text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "Exemples"
            </p>
            <div class="flex flex-col gap-1 text-sm">{rows}</div>
        </div>
    }
}
