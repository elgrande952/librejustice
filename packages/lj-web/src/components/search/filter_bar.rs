//! `DecisionsFilterBar` — barre de filtres horizontale sous la barre de
//! recherche : `Date ▾ · Juridiction ▾ · Publication ▾ · Matière ▾ ·
//! Textes cités ▾ · Plus de filtres`. « Plus de filtres » étend la barre en
//! place avec les dropdowns Office, Solution et Procédure (pas de modal) ;
//! ils restent épinglés tant qu'un de leurs filtres est actif.
//!
//! Sync bidirectionnelle facettes ↔ query params, mutations via `query_state`
//! (replace=true, drop `page`).

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use lj_dtos::SearchFacets;

use super::compact_search::{query_state, DraftQuery};
use super::date_range_picker::DateRangePicker;
use super::facet_widgets::{
    build_tree, prune_tree, CheckboxOption, FacetChecklist, FacetNode, FacetTree,
    FilterSearchInput, Nav, TreeRow,
};
use super::filter_dropdown::{FilterDropdown, OpenDropdown};
use super::legal_instrument_filter::LegalInstrumentPanel;

#[component]
pub fn DecisionsFilterBar(#[prop(into)] facets: Signal<Option<SearchFacets>>) -> impl IntoView {
    let query_map = use_query_map();
    let nav = Nav::new();
    // Un seul dropdown ouvert à la fois dans la barre.
    provide_context(OpenDropdown(RwSignal::new(None)));

    let get_all = move |key: &str| query_map.get().get_all(key).unwrap_or_default();

    // Map courante avec `q` réécrit par le texte (non soumis) de la barre : toute
    // mutation de filtre applique CE texte, pour ajuster requête + filtres d'un
    // seul geste. Absent le provider (composant isolé), on garde le `q` de l'URL.
    let draft = use_context::<DraftQuery>().map(|d| d.0);
    let effective_map = move || {
        let mut map = query_map.get_untracked();
        if let Some(draft) = draft {
            let q = draft.get_untracked().trim().to_string();
            if q.is_empty() {
                map.remove("q");
            } else {
                map.replace("q".to_string(), q);
            }
        }
        map
    };

    let jur_filter = RwSignal::new(String::new());

    // ── Callbacks de mutation (`nav` est `Copy`, capturé par copie) ──────────
    let toggle = move |key: String, value: String| {
        nav.go(query_state::toggle_multi(&effective_map(), &key, &value));
    };
    let on_dates = move |(f, t): (String, String)| {
        nav.go(query_state::with_dates(
            &effective_map(),
            Some(&f),
            Some(&t),
        ));
    };

    // ── Sélections courantes (URL) ────────────────────────────────────────────
    let offices = Signal::derive(move || get_all("office"));
    let seats = Signal::derive(move || get_all("seat"));
    let domaines = Signal::derive(move || get_all("legalDomain"));
    let solutions = Signal::derive(move || get_all("solution"));
    let procedures = Signal::derive(move || get_all("procedure"));
    let pubs = Signal::derive(move || get_all("publication"));
    let lis = Signal::derive(move || get_all("legalInstrument"));
    let las = Signal::derive(move || get_all("legalArticle"));
    let date_from = Signal::derive(move || query_map.get().get("dateFrom").unwrap_or_default());
    let date_to = Signal::derive(move || query_map.get().get("dateTo").unwrap_or_default());

    // ── Juridiction : the tree takes the depth the facet serves. `Memo` read by
    // the per-row signals: computed once per change of facets or of input.
    let jurisdiction_choices = Memo::new(move |_| facets.get().map(|f| f.seat).unwrap_or_default());
    // `build_tree` hangs off the facets alone, so a keystroke replays only the
    // pruning, which also says which rows the search unfolds.
    let jurisdiction_roots = Memo::new(move |_| build_tree(&jurisdiction_choices.get()));
    let jurisdiction_pruned = Memo::new(move |_| {
        let needle = jur_filter.get().trim().to_lowercase();
        prune_tree(jurisdiction_roots.get(), &needle)
    });
    let jurisdiction_tree = Memo::new(move |_| jurisdiction_pruned.get().0);
    let jur_unfold = Signal::derive(move || jurisdiction_pruned.get().1);
    // Sélections absentes de la facette : lignes orphelines plates
    // `(clé d'URL, valeur)` en fin de liste.
    let juridiction_orphans = Memo::new(move |_| {
        let choices = jurisdiction_choices.get();
        seats
            .get()
            .into_iter()
            .filter(|s| !choices.iter().any(|c| &c.value == s))
            .collect::<Vec<String>>()
    });

    // ── Domaine : arbre facette (valeurs = suffixes `legal_domain:*`, racine et
    // feuille se filtrent par la même clé).
    let domaine_choices =
        Memo::new(move |_| facets.get().map(|f| f.legal_domain).unwrap_or_default());

    let office_choices = Signal::derive(move || facets.get().map(|f| f.office).unwrap_or_default());
    let solution_choices =
        Signal::derive(move || facets.get().map(|f| f.solution).unwrap_or_default());
    let procedure_choices =
        Signal::derive(move || facets.get().map(|f| f.procedure).unwrap_or_default());
    let publication_choices =
        Signal::derive(move || facets.get().map(|f| f.publication).unwrap_or_default());

    // ── Compteurs actifs des boutons ─────────────────────────────────────────
    let jur_count = Signal::derive(move || seats.get().len());
    let office_count = Signal::derive(move || offices.get().len());
    let domaine_count = Signal::derive(move || domaines.get().len());
    let dates_count = Signal::derive(move || {
        usize::from(!date_from.get().is_empty() || !date_to.get().is_empty())
    });
    let textes_count = Signal::derive(move || lis.get().len() + las.get().len());
    let solution_count = Signal::derive(move || solutions.get().len());
    let procedure_count = Signal::derive(move || procedures.get().len());
    let publication_count = Signal::derive(move || pubs.get().len());
    let more_count = Signal::derive(move || {
        offices.get().len() + solutions.get().len() + procedures.get().len() + pubs.get().len()
    });

    // « Plus de filtres » déplie Office + Solution + Procédure EN PLACE dans la
    // barre ; un filtre actif dans ce groupe les épingle (on ne cache jamais un
    // filtre appliqué) et le bouton de bascule disparaît.
    let more_expanded = RwSignal::new(false);
    let show_more = Signal::derive(move || more_expanded.get() || more_count.get() > 0);

    view! {
        // Pas de grisage pendant un refetch : les dropdowns gardent les
        // facettes précédentes et restent pleinement interactifs (on peut
        // enchaîner les coches).
        <div
            role="toolbar"
            aria-label="Filtres de recherche"
            class="flex flex-wrap items-center gap-2 pb-1"
        >
            <FilterDropdown id="date" label="Date" active_count=dates_count>
                <DateRangePicker
                    from=date_from
                    to=date_to
                    on_change=Callback::new(on_dates)
                />
            </FilterDropdown>

            <FilterDropdown id="juridiction" label="Juridiction" active_count=jur_count>
                <FilterSearchInput value=jur_filter placeholder="Filtrer (ex. Lyon…)" />
                <div
                    class="filter-scroll flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto overscroll-contain"
                >
                    // `<For>` keyé par uid racine : `each` ne dépend que des facettes
                    // + de la saisie — JAMAIS de la sélection. Un clic de filtre ne
                    // reconstruit donc pas la liste : seuls les signaux par ligne
                    // (`selected`, compteurs) se mettent à jour en place.
                    <For
                        each=move || jurisdiction_tree.get()
                        key=|n: &FacetNode| n.choice.value.clone()
                        children=move |n: FacetNode| {
                            view! {
                                <TreeRow
                                    siblings=jurisdiction_tree
                                    value=n.choice.value
                                    selected=seats
                                    unfold=jur_unfold
                                    on_toggle=Callback::new(move |value: String| toggle(
                                        "seat".to_string(),
                                        value,
                                    ))
                                />
                            }
                        }
                    />
                    <For
                        each=move || juridiction_orphans.get()
                        key=|value: &String| value.clone()
                        children=move |value: String| {
                            let sel_value = value.clone();
                            let is_sel =
                                Signal::derive(move || seats.get().contains(&sel_value));
                            let toggle_orphan = toggle;
                            let toggle_value = value.clone();
                            view! {
                                <CheckboxOption
                                    label=value.clone()
                                    count=Signal::derive(|| 0)
                                    selected=is_sel
                                    on_toggle=Callback::new(move |_| toggle_orphan(
                                        "seat".to_string(),
                                        toggle_value.clone(),
                                    ))
                                    indent=false
                                />
                            }
                        }
                    />
                </div>
            </FilterDropdown>

            <FilterDropdown id="publication" label="Publication" active_count=publication_count>
                <FacetChecklist
                    choices=publication_choices
                    selected=pubs
                    on_toggle=Callback::new(move |value: String| toggle(
                        "publication".to_string(),
                        value,
                    ))
                />
            </FilterDropdown>

            // Masqué quand la facette ne remonte rien (couverture domaine
            // partielle du corpus) — sauf sélection active, qu'on ne cache pas.
            <Show when=move || {
                !domaine_choices.get().is_empty() || domaine_count.get() > 0
            }>
            <FilterDropdown id="legalDomain" label="Matière" active_count=domaine_count>
                <FacetTree
                    choices=domaine_choices
                    selected=domaines
                    on_toggle=Callback::new(move |value: String| toggle(
                        "legalDomain".to_string(),
                        value,
                    ))
                />
            </FilterDropdown>
            </Show>

            // Masqué quand la facette ne remonte rien : le mode parcours ne la sert
            // pas. Sauf sélection active, qu'on ne cache pas.
            <Show when=move || {
                facets.get().is_some_and(|f| !f.legal_instrument.is_empty())
                    || textes_count.get() > 0
            }>
            <FilterDropdown id="textes" label="Textes cités" active_count=textes_count>
                <LegalInstrumentPanel
                    facets=facets
                    selected_instruments=lis
                    selected_articles=las
                    on_toggle_instrument=Callback::new(move |value: String| toggle(
                        "legalInstrument".to_string(),
                        value,
                    ))
                    on_toggle_article=Callback::new(move |value: String| toggle(
                        "legalArticle".to_string(),
                        value,
                    ))
                />
            </FilterDropdown>
            </Show>

            <Show when=move || show_more.get()>
                // Office du juge (JEX, JAF, JCP, JLD, magistrat désigné…) : axe
                // séparé de la juridiction, en miroir du filtre `office_uid`.
                // Masqué quand la facette est vide, sauf sélection.
                <Show when=move || {
                    !office_choices.get().is_empty() || office_count.get() > 0
                }>
                <FilterDropdown id="office" label="Office" active_count=office_count>
                    <FacetChecklist
                        choices=office_choices
                        selected=offices
                        on_toggle=Callback::new(move |value: String| toggle(
                            "office".to_string(),
                            value,
                        ))
                    />
                </FilterDropdown>
                </Show>

                <FilterDropdown id="solution" label="Solution" active_count=solution_count>
                    <FacetChecklist
                        choices=solution_choices
                        selected=solutions
                        on_toggle=Callback::new(move |value: String| toggle(
                            "solution".to_string(),
                            value,
                        ))
                    />
                </FilterDropdown>

                // Voie procédurale (référé, QPC, tri R. 222-1, renvoi
                // préjudiciel…) : une valeur absente est la procédure
                // contentieuse ordinaire, qui n'est donc pas un choix.
                <Show when=move || {
                    !procedure_choices.get().is_empty() || procedure_count.get() > 0
                }>
                <FilterDropdown id="procedure" label="Procédure" active_count=procedure_count>
                    <FacetChecklist
                        choices=procedure_choices
                        selected=procedures
                        on_toggle=Callback::new(move |value: String| toggle(
                            "procedure".to_string(),
                            value,
                        ))
                    />
                </FilterDropdown>
                </Show>

            </Show>

            <Show when=move || { more_count.get() == 0 }>
                <button
                    type="button"
                    on:click=move |_| more_expanded.update(|v| *v = !*v)
                    class="flex shrink-0 items-center gap-1.5 rounded border border-[var(--color-rule)] px-2.5 py-1 text-xs text-[var(--color-ink)] transition-colors hover:border-[var(--color-ink)]"
                >
                    <span class="whitespace-nowrap">
                        {move || {
                            if more_expanded.get() { "Moins de filtres" } else { "Plus de filtres" }
                        }}
                    </span>
                </button>
            </Show>
        </div>
    }
}
