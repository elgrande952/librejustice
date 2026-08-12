//! Page de recherche de décisions `/decisions` (gabarit du secteur) :
//! CompactSearch + barre de filtres horizontale + résultats. La recherche de
//! textes vit sur sa propre page `/textes` (moteurs et filtres distincts,
//! aucune recherche transverse). État dans les query params ; données via
//! `leptos-fetch` (SWR) ; DTO `lj-dtos`.
//!
//! Anatomie alignée sur la page décision (`DecisionLayout`) : même conteneur
//! `max-w-[92rem]` et même grille `240px | minmax(0,1fr) | 280px` dès `lg` —
//! colonne gauche = rail de facettes (`SearchRail`), colonne DROITE (280px) =
//! volumétrie + aide à la recherche + pont croisé « Dans les textes ». La
//! colonne de contenu (barre, filtres, liste) prend le reste et se borne à
//! `max-w-3xl`.
//!
//! `<Title>` dynamique reflétant la requête `q` de l'URL ; hérite de la
//! description racine.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;
use lj_dtos::{QueryMode, SearchResponse};

use crate::components::client_only::ClientOnly;
use crate::components::layout::{CONTENT_COL, GRID_3COL, SHELL, SIDE_RAIL};
use crate::components::search::compact_search::search_data::{
    cached_search, search_is_loading, search_resource, SeenSearchKeys,
};
use crate::components::search::compact_search::search_state::{
    key_from_map, SearchKey, SEARCH_LIMIT,
};
use crate::components::search::compact_search::DraftQuery;
use crate::components::search::{
    corpus_stats, ActiveFilterChips, CompactSearch, DecisionsCorpusLine, DecisionsCorpusSummary,
    DecisionsExamples, DecisionsFilterBar, JuridictionsRail, Pagination, ResultEmpty, ResultError,
    ResultList, ResultSkeleton, SearchRail, SortSelect, SyntaxHint, TextesCorpusTeaser,
    TextesTeaser,
};
use crate::helpers::{format_results_count, total_pages};

/// `/decisions` est **rendue côté client** : route non indexée, la plus coûteuse
/// en SSR (elle rend CompactSearch + cartes + facettes côté serveur) et la seule
/// porteuse des mismatches d'hydratation (toggle IA `localStorage`, date-picker,
/// nav). `ClientOnly` rend un squelette statique au SSR et au 1er rendu, puis
/// monte le corps réactif après hydratation. Les routes SEO (décision, landing,
/// légal, mcp-guide) restent SSR.
#[component]
pub fn SearchPage() -> impl IntoView {
    let query_map = use_query_map();
    // `leptos_meta` réévalue cette clôture après le démontage de la route, quand
    // `query_map` est déjà libéré : `try_get` au lieu de `get`.
    let title = move || {
        let q = query_map
            .try_get()
            .and_then(|params| params.get("q"))
            .unwrap_or_default();
        let q = q.trim();
        if q.is_empty() {
            "Décisions - LibreJustice".to_string()
        } else {
            format!("{q} - LibreJustice")
        }
    };
    view! {
        <Title text=title />
        <ClientOnly fallback=|| {
            view! { <SearchPageSkeleton /> }
        }>
            <SearchPageBody />
        </ClientOnly>
    }
}

/// Squelette statique du fallback SSR/1er rendu : même grille que le corps
/// (rail vide + colonne contenu), barre inerte + skeleton de résultats. Aucun
/// signal dépendant de `window`/`localStorage` → hydratation sans divergence.
#[component]
fn SearchPageSkeleton() -> impl IntoView {
    view! {
        <div class=SHELL>
            <h1 class="sr-only">"Recherche"</h1>
            <div class=GRID_3COL>
                <div class="hidden lg:block"></div>
                <div class=CONTENT_COL>
                    <div class="h-12 w-full animate-pulse rounded-lg bg-[var(--color-rule)]/40"></div>
                    <ResultSkeleton />
                </div>
            </div>
        </div>
    }
}

#[component]
fn SearchPageBody() -> impl IntoView {
    let query_map = use_query_map();
    // Texte de la barre partagé avec les filtres : cocher un filtre l'applique
    // comme `q` sans passer par « Rechercher ». Initialisé sur le `q` de l'URL.
    provide_context(DraftQuery(RwSignal::new(
        query_map.get_untracked().get("q").unwrap_or_default(),
    )));

    view! {
        <div class=SHELL>
            <h1 class="sr-only">"Recherche"</h1>
            <DecisionsView />
        </div>
    }
}

/// Corps décisions : rail de synthèse à gauche (colonne alignée sur le
/// sommaire de la page décision), colonne contenu à droite — barre de
/// recherche, filtres horizontaux, chips, tri et résultats (gabarit du secteur).
/// Sans requête, la page garde la même anatomie : les emplacements de
/// volumétrie, de rail et de pont croisé sont remplis par du global corpus
/// (`corpus_intro`), donc l'aide à la recherche reste à la même place ; seuls
/// les filtres et la liste, qui n'existent que pour une requête, cèdent la
/// place à l'invite.
#[component]
fn DecisionsView() -> impl IntoView {
    let query_map = use_query_map();
    let query = Signal::derive(move || {
        query_map
            .get()
            .get("q")
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    });
    let key = Signal::derive(move || key_from_map(&query_map.get()));
    let resource = search_resource(key);
    // Facettes et volumétrie remontées de la réponse par `Effect` : ne jamais lire
    // la `Resource` dans le rendu du rail ou de la barre. Mises à jour sur réponse
    // Ok SEULEMENT — pendant un refetch, rail et barre gardent les valeurs
    // précédentes, interactives.
    let facets = RwSignal::new(None::<lj_dtos::SearchFacets>);
    let volume = RwSignal::new(None::<(i64, QueryMode)>);
    Effect::new(move |_| {
        if let Some(Ok(resp)) = resource.get().flatten() {
            volume.set(Some((resp.total, resp.query_mode)));
            facets.set(resp.facets);
        }
    });
    // Skeleton : clé en premier chargement (`is_loading` leptos-fetch) ET rien en
    // cache pour elle. La lecture SYNCHRONE du cache donne les deux comportements :
    // pas de flash au retour depuis `/decision` (cache présent → rendu direct), et
    // skeleton bien affiché quand une mutation de filtre retombe sur une clé vue
    // mais évincée du cache.
    let is_loading = search_is_loading(key);
    let show_skeleton =
        Signal::derive(move || is_loading.get() && cached_search(&key.get()).is_none());
    let has_query = Signal::derive(move || !query.get().is_empty());
    let stats = corpus_stats();

    view! {
        <div class=GRID_3COL>
            <aside aria-label="Synthèse des résultats" class=SIDE_RAIL>
                <Show when=move || has_query.get() fallback=move || view! { <JuridictionsRail /> }>
                    <SearchRail facets=facets />
                </Show>
            </aside>
            <div class=CONTENT_COL>
                <div class="flex flex-col gap-3">
                    <CompactSearch />
                    // Ligne compacte mobile, rendue dans les DEUX états juste sous la
                    // barre : volumétrie des résultats ou volumétrie du corpus, et
                    // l'aide calée à droite — elle ne se déplace donc pas entre l'avant
                    // et l'après recherche. Sur desktop la volumétrie vit dans la
                    // colonne droite (`lg:hidden` ici).
                    <div class="flex items-baseline gap-3 lg:hidden">
                        <Show
                            when=move || has_query.get()
                            fallback=move || view! { <DecisionsCorpusLine stats=stats /> }
                        >
                            <MobileSummary volume=volume loading=show_skeleton />
                        </Show>
                        <SyntaxHint class="ml-auto shrink-0" />
                    </div>
                </div>
                <Show when=move || has_query.get() fallback=move || view! { <PromptEmpty /> }>
                    <div class="flex min-w-0 flex-col gap-4">
                        // Tri sur la ligne des filtres, calé à droite (gabarit secteur
                        // `… · Plus de filtres + Trier par ▾`) ; la barre wrappe dans sa
                        // colonne, le tri reste en haut à droite.
                        <div class="flex items-start justify-between gap-4">
                            <div class="min-w-0 flex-1">
                                <DecisionsFilterBar facets=facets />
                            </div>
                            <SortSelect />
                        </div>
                        <ActiveFilterChips facets=facets />
                        <SearchResults query=query resource=resource key=key show_skeleton=show_skeleton />
                    </div>
                </Show>
            </div>
            // Colonne droite : volumétrie, aide à la recherche, puis pont croisé
            // vers le moteur textes (la même requête posée à l'autre moteur). Les
            // trois emplacements sont remplis requête vide par leur pendant corpus,
            // dans le même gabarit : la page vide garde l'anatomie de la page de
            // résultats. Sous lg la colonne disparaît.
            <aside aria-label="Voir aussi" class=SIDE_RAIL>
                <div class="flex flex-col gap-4">
                    <Show
                        when=move || has_query.get()
                        fallback=move || view! { <DecisionsCorpusSummary stats=stats /> }
                    >
                        <SearchSummary query=query volume=volume loading=show_skeleton />
                    </Show>
                    <div class="flex self-start">
                        <SyntaxHint />
                    </div>
                    <Show
                        when=move || has_query.get()
                        fallback=move || view! { <TextesCorpusTeaser stats=stats /> }
                    >
                        <TextesTeaser query=query />
                    </Show>
                </div>
            </aside>
        </div>
    }
}

/// Volumétrie desktop, en tête de la colonne droite : « N résultats / pour
/// « q » / Recherche <mode> », ou l'état d'attente pendant un chargement à
/// cache froid.
#[component]
fn SearchSummary(
    #[prop(into)] query: Signal<String>,
    #[prop(into)] volume: Signal<Option<(i64, QueryMode)>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    move || {
        if loading.get() {
            return view! {
                <h2 class="font-sans text-xl text-[var(--color-ink-subtle)]">
                    "Recherche en cours pour "
                    <em class="not-italic">"«\u{00A0}"{move || query.get()}"\u{00A0}»"</em>
                    "…"
                </h2>
            }
            .into_any();
        }
        let Some((total, mode)) = volume.get() else {
            return ().into_any();
        };
        let mode_label = match mode {
            QueryMode::Lexical => "lexicale",
            QueryMode::Hybrid => "sémantique",
        };
        view! {
            <div class="flex flex-col gap-1">
                <h2 class="font-sans text-xl text-[var(--color-ink)]">
                    {format_results_count(total)}
                </h2>
                <p class="text-sm leading-snug text-[var(--color-ink-subtle)]">
                    "pour "
                    <em class="text-[var(--color-ink)]">"«\u{00A0}"{move || query.get()}"\u{00A0}»"</em>
                </p>
                <p class="text-xs text-[var(--color-ink-subtle)]">
                    {format!("Recherche {mode_label}")}
                </p>
            </div>
        }
        .into_any()
    }
}

/// Volumétrie compacte, rendue dans la ligne mobile qui porte aussi l'aide à la
/// recherche (sur desktop la volumétrie vit dans la colonne droite).
#[component]
fn MobileSummary(
    #[prop(into)] volume: Signal<Option<(i64, QueryMode)>>,
    #[prop(into)] loading: Signal<bool>,
) -> impl IntoView {
    move || {
        if loading.get() {
            return view! {
                <p class="text-sm text-[var(--color-ink-subtle)]">"Recherche en cours…"</p>
            }
            .into_any();
        }
        let Some((total, mode)) = volume.get() else {
            return ().into_any();
        };
        let mode_label = match mode {
            QueryMode::Lexical => "lexicale",
            QueryMode::Hybrid => "sémantique",
        };
        view! {
            <p class="text-sm text-[var(--color-ink-subtle)]">
                <span class="font-medium text-[var(--color-ink)]">{format_results_count(total)}</span>
                {format!(" · Recherche {mode_label}")}
            </p>
        }
        .into_any()
    }
}

/// Corps : skeleton (chargement à cache froid) / erreur / résultats, lus depuis
/// la `Resource`. Le skeleton est piloté par `show_skeleton` (calculé dans
/// `DecisionsView`) ; un `<Transition>` garde les résultats précédents pendant
/// qu'une clé déjà en cache résout.
#[component]
fn SearchResults(
    #[prop(into)] query: Signal<String>,
    resource: Resource<Option<Result<SearchResponse, String>>>,
    #[prop(into)] key: Signal<crate::components::search::compact_search::search_state::SearchKey>,
    #[prop(into)] show_skeleton: Signal<bool>,
) -> impl IntoView {
    // Anime la liste à chaque NOUVELLE recherche — clé jamais vue cette session,
    // c'est-à-dire changement de requête, filtre, tri, page ou mode IA. Set de clés
    // vues PARTAGÉ via contexte (fourni par `App`) : il survit au remontage de
    // `SearchPage` au retour depuis `/decision`, donc une recherche déjà vue ne
    // rejoue pas l'animation. Fallback local hors provider.
    let seen_keys = use_context::<SeenSearchKeys>()
        .map(|s| s.0)
        .unwrap_or_else(|| StoredValue::new(std::collections::HashSet::<SearchKey>::new()));

    view! {
        <section aria-label="Résultats de recherche" class="min-w-0">
            <Show
                when=move || show_skeleton.get()
                fallback=move || {
                    // Au retour depuis une décision la donnée est déjà en cache : on la
                    // lit de façon SYNCHRONE et on rend la liste dans la frame de
                    // montage. Cache froid (premier chargement) → <Transition> + Suspend
                    // (streaming SSR ; le skeleton est piloté en amont par le <Show>
                    // `show_skeleton`).
                    move || {
                        let current_key = key.get();
                        match cached_search(&current_key) {
                            Some(Ok(result)) => {
                                seen_keys.update_value(|seen| {
                                    seen.insert(current_key);
                                });
                                view! {
                                    <ResultsBody
                                        result=result
                                        query=query
                                        key=key
                                        animate=false
                                    />
                                }
                                    .into_any()
                            }
                            Some(Err(message)) => view! { <ResultError message=message /> }
                                .into_any(),
                            None => {
                                view! {
                                    <Transition fallback=|| ()>
                                        {move || Suspend::new(async move {
                                            let resolved = resource.await;
                                            // 1ʳᵉ fois qu'on voit cette clé cette session →
                                            // anime ; revisite (cache) → pas d'animation.
                                            let current_key = key.get_untracked();
                                            let animate = !seen_keys
                                                .with_value(|seen| seen.contains(&current_key));
                                            seen_keys.update_value(move |seen| {
                                                seen.insert(current_key);
                                            });
                                            match resolved {
                                                None => ().into_any(),
                                                Some(Err(message)) => {
                                                    view! { <ResultError message=message /> }
                                                        .into_any()
                                                }
                                                Some(Ok(result)) => {
                                                    view! {
                                                        <ResultsBody
                                                            result=result
                                                            query=query
                                                            key=key
                                                            animate=animate
                                                        />
                                                    }
                                                        .into_any()
                                                }
                                            }
                                        })}
                                    </Transition>
                                }
                                    .into_any()
                            }
                        }
                    }
                }
            >
                <ResultSkeleton />
            </Show>
        </section>
    }
}

/// Corps des résultats : vide / liste + pagination.
#[component]
fn ResultsBody(
    result: SearchResponse,
    #[prop(into)] query: Signal<String>,
    #[prop(into)] key: Signal<crate::components::search::compact_search::search_state::SearchKey>,
    animate: bool,
) -> impl IntoView {
    if result.hits.is_empty() {
        return view! { <ResultEmpty query=query /> }.into_any();
    }
    // Restaure le scroll de la liste au retour depuis une décision : posé par le
    // bouton retour de la barre décision, consommé ICI au montage. One-shot.
    #[cfg(feature = "hydrate")]
    {
        let restore = crate::components::decision_bar::use_restore_scroll();
        Effect::new(move |_| {
            let Some(y) = restore.get_untracked() else {
                return;
            };
            restore.set(None);
            let scroll = move || {
                if let Some(w) = web_sys::window() {
                    w.scroll_to_with_x_and_y(0.0, y);
                }
            };
            // La liste se monte sous le <Suspend> dans la tâche de cet effet ; ses
            // nœuds ne sont attachés au DOM vivant qu'en fin de tâche. On applique le
            // scroll à trois échéances croissantes, toutes idempotentes, pour viser
            // le tout premier paint sans saut :
            //   1. synchrone — atteint la cible si le DOM est déjà à pleine hauteur ;
            //   2. micro-tâche (`spawn_local`) — s'exécute après l'attache des nœuds
            //      mais AVANT le paint (checkpoint micro-tâches < étape de rendu),
            //      donc la frame de montage peint déjà à la bonne position ;
            //   3. rAF — filet de sécurité si la liste se monte une frame plus tard.
            scroll();
            leptos::task::spawn_local(async move { scroll() });
            request_animation_frame(scroll);
        });
    }
    let total = result.total;
    let pages = total_pages(total, SEARCH_LIMIT as i64, MAX_PAGES_DISPLAY) as u32;
    let hits = result.hits;
    let all_hit_ids = result.all_hit_ids;
    let page = key.get_untracked().page;
    // Vue par défaut des cartes = le mode de recherche RÉSOLU par le serveur, pas
    // le toggle IA : une recherche booléenne/lexicale s'ouvre sur l'extrait
    // surligné (c'est le passage qui matche les opérateurs saisis), une recherche
    // sémantique sur le résumé. Le toggle, lui, ne gouverne que le reclassement
    // LLM — no-op en lexical — et n'est jamais décoché à la place de
    // l'utilisateur.
    let open_on_summary = result.query_mode == QueryMode::Hybrid;

    view! {
        <div class="flex min-w-0 flex-col gap-6">
            <ResultList
                hits=hits
                all_hit_ids=all_hit_ids
                page=page
                total=total
                page_size=SEARCH_LIMIT
                auto_load_summary=open_on_summary
                animate=animate
            />
            // `current_page`/`total_pages` STATIQUES (pas d'abonnement réactif) :
            // `ResultsBody` est reconstruit en entier à chaque nouvelle recherche, donc
            // la page courante est fixe pour une instance. Dériver de `key` (signal de
            // page) ferait ré-évaluer la pagination depuis un scope que le `<Suspense>`
            // dispose au changement de clé → panic « reactive value already disposed ».
            <Pagination
                current_page=Signal::derive(move || page)
                total_pages=Signal::derive(move || pages)
            />
        </div>
    }
    .into_any()
}

/// Plafond de pages affichées : `RESULTS_CAP=400` borne déjà à 40 pages, le cap
/// URL `MAX_PAGES=10` borne la navigation.
const MAX_PAGES_DISPLAY: i64 = 40;

/// Invite initiale (requête vide).
#[component]
fn PromptEmpty() -> impl IntoView {
    view! {
        <div class="flex flex-col items-start gap-3">
            <h2 class="font-sans text-2xl text-[var(--color-ink)]">
                "Saisissez une requête pour commencer."
            </h2>
            <p class="max-w-prose text-[var(--color-ink-muted)]">
                "La recherche fonctionne en langage naturel ou avec des opérateurs booléens. La detection du mode est automatique."
            </p>
            <DecisionsExamples />
        </div>
    }
}
