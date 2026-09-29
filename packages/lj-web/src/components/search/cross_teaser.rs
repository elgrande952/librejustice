//! Ponts croisés entre les deux moteurs : bloc « Dans les textes » dans le
//! rail de `/decision-search`, « Dans les décisions » dans celui de `/norm-search`.
//!
//! Le fetch part en parallèle de la recherche principale, depuis un `Effect`
//! vers un signal. Zéro résultat ou erreur ⇒ le bloc disparaît. Le signal porte
//! la requête à laquelle il répond : tant que la requête courante n'est pas
//! celle-là, le bloc est en attente — il ne montre jamais la question
//! précédente.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{
    ArticleSearchResponse, SearchContext, SearchMode, SearchRequest, SearchResponse, SortOrder,
};

use crate::api::{ApiClient, PageParams, TextesFilters};
use crate::components::ui::Skeleton;
use crate::helpers::{encode_query, format_article_num, group_thousands};

use super::compact_search::highlight::Highlighted;

/// Hits affichés par teaser.
const TEASER_ROWS: usize = 3;

/// "Dans les textes" block of the `/decision-search` rail: top 3 articles of the
/// text engine for the same query + a link to `/norm-search?q=…`.
#[component]
pub fn TextesTeaser(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let title = "Dans les textes";
    let answer = RwSignal::new(None::<(String, Option<ArticleSearchResponse>)>);
    Effect::new(move |_| {
        let q = query.get();
        if q.is_empty() {
            answer.set(None);
            return;
        }
        leptos::task::spawn_local(async move {
            let resp = ApiClient::from_context()
                .search_textes(
                    &q,
                    TextesFilters::default(),
                    PageParams {
                        limit: TEASER_ROWS as u32,
                        offset: 0,
                    },
                    // A teaser ranks a decisions query; it designates nothing.
                    lj_dtos::SearchMode::Lexical,
                    SearchContext::Teaser,
                )
                .await;
            if query.get_untracked() != q {
                return;
            }
            answer.set(Some((q, resp.ok())));
        });
    });

    move || {
        let q = query.get();
        if q.is_empty() {
            return ().into_any();
        }
        let Some(answered) = answer.get().filter(|(to, _)| *to == q) else {
            return teaser_skeleton(title);
        };
        let Some(resp) = answered.1 else {
            return ().into_any();
        };
        if resp.hits.is_empty() {
            return ().into_any();
        }
        let total = resp.total;
        let rows = resp
            .hits
            .into_iter()
            .take(TEASER_ROWS)
            .map(|h| {
                // `numKey` vide : hit « texte entier » → page du texte ; sinon page de
                // l'article.
                let text = lj_vocab::norm_path(&h.jurisdiction, &h.code);
                let (href, label) = if h.num_key.is_empty() {
                    (text, h.code_title)
                } else {
                    (
                        format!("{text}/{}", h.num_key),
                        format!("Art. {} · {}", format_article_num(&h.num), h.code_title),
                    )
                };
                teaser_row(href, label)
            })
            .collect_view();
        let more_label = if total == 1 {
            "Voir le texte".to_string()
        } else {
            format!("Voir les {} textes", group_thousands(total))
        };
        teaser_block(
            title,
            rows.into_any(),
            format!("/norm-search?q={}", encode_query(&q)),
            more_label,
        )
    }
}

/// "Dans les décisions" block of the `/norm-search` rail: top 3 decisions of the
/// case law engine for the same query + a link to `/decision-search?q=…`.
#[component]
pub fn DecisionsTeaser(#[prop(into)] query: Signal<String>) -> impl IntoView {
    let title = "Dans les décisions";
    let answer = RwSignal::new(None::<(String, Option<SearchResponse>)>);
    Effect::new(move |_| {
        let q = query.get();
        if q.is_empty() {
            answer.set(None);
            return;
        }
        leptos::task::spawn_local(async move {
            let request = teaser_search_request(&q);
            let resp = ApiClient::from_context()
                .search(&request, SearchContext::Teaser)
                .await;
            if query.get_untracked() != q {
                return;
            }
            answer.set(Some((q, resp.ok())));
        });
    });

    move || {
        let q = query.get();
        if q.is_empty() {
            return ().into_any();
        }
        let Some(answered) = answer.get().filter(|(to, _)| *to == q) else {
            return teaser_skeleton(title);
        };
        let Some(resp) = answered.1 else {
            return ().into_any();
        };
        if resp.hits.is_empty() {
            return ().into_any();
        }
        let rows = resp
            .hits
            .into_iter()
            .take(TEASER_ROWS)
            .map(|h| {
                view! {
                    <DecisionTeaserRow
                        href=lj_dtos::decision_path(h.jurisdiction_type, &h.id)
                        title_html=h.title_html
                        snippet=h.best_chunk.snippet
                    />
                }
            })
            .collect_view();
        // Sans nombre : le décompte exact vaut une seconde d'agrégat, que le pont
        // ne paie pas pour trois lignes de rail.
        teaser_block(
            title,
            rows.into_any(),
            format!("/decision-search?q={}", encode_query(&q)),
            "Voir les décisions".to_string(),
        )
    }
}

/// Ligne du teaser décisions : titre puis extrait du texte. L'extrait peut être
/// vide, et un nœud conditionnel parmi les enfants d'un `collect_view()` n'émet
/// son marqueur que dans une branche : la frontière de composant est ce qui le
/// rend hydratable.
#[component]
fn DecisionTeaserRow(href: String, title_html: String, snippet: String) -> impl IntoView {
    view! {
        <A href=href attr:class="block no-underline">
            <span class="block truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors hover:text-[var(--color-ink)]">
                <Highlighted text=title_html />
            </span>
            {Some(snippet)
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    view! {
                        <span class="mt-0.5 line-clamp-2 block text-[12px] leading-snug text-[var(--color-ink-subtle)]">
                            <Highlighted text=s />
                        </span>
                    }
                })}
        </A>
    }
}

/// Requête décisions du teaser : top-3, tri pertinence, sans filtre ni mode IA.
/// Mode lexical forcé : le pont sert un extrait surligné, que produit la jambe
/// BM25.
fn teaser_search_request(q: &str) -> SearchRequest {
    SearchRequest {
        query: q.to_string(),
        jurisdiction_type: None,
        solution: None,
        procedure: None,
        office: None,
        legal_domain: None,
        jurisdiction_code: None,
        seat: None,
        legal_instrument: None,
        legal_article: None,
        publication: None,
        date_from: None,
        date_to: None,
        mode: SearchMode::Lexical,
        sort: SortOrder::Relevance,
        limit: TEASER_ROWS as u32,
        offset: 0,
        ai_mode: false,
    }
}

/// Gabarit commun des deux blocs.
fn teaser_block(
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

/// Bloc en attente : le titre, qui ne dépend pas de la réponse, et des lignes
/// muettes à la place des hits de la question précédente.
fn teaser_skeleton(title: &'static str) -> AnyView {
    view! {
        <div class="flex flex-col gap-1.5 border-t border-[var(--color-rule)] pt-4">
            <p class="pb-1 text-[11px] uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            <Skeleton class="h-4 w-full" />
            <Skeleton class="h-4 w-5/6" />
            <Skeleton class="h-4 w-2/3" />
            <Skeleton class="mt-1 h-4 w-32" />
        </div>
    }
    .into_any()
}

/// Ligne d'un hit texte (label plein, tronqué à une ligne).
fn teaser_row(href: String, label: String) -> impl IntoView {
    view! {
        <A
            href=href
            attr:class="block truncate text-[13px] leading-snug text-[var(--color-ink-muted)] transition-colors hover:text-[var(--color-ink)]"
        >
            {label}
        </A>
    }
}
