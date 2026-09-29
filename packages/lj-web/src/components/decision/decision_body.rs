//! Corps structuré de la décision (sections + paragraphes) + toolbar flottante
//! de sélection de texte.
use leptos::prelude::*;
use lj_dtos::DecisionDetail;

use crate::components::decision::selection::{SelState, SelectionToolbar};
use crate::components::search::compact_search::highlight::CitedParagraph;
use crate::pages::decision_page::reference::build_decision_references;
use crate::pages::decision_page::sections::RenderSection;

#[component]
pub fn DecisionBody(detail: DecisionDetail, sections: Vec<RenderSection>) -> impl IntoView {
    let has_paragraphs = sections.iter().any(|s| !s.paragraphs.is_empty());

    if !has_paragraphs {
        return view! {
            <p class="text-sm italic text-[var(--color-ink-subtle)]">
                "Texte intégral non disponible."
            </p>
        }
        .into_any();
    }

    let references = StoredValue::new(build_decision_references(&detail));
    // Date de lecture propagée aux hover cards d'articles : elles servent la
    // version en vigueur à la date de la décision.
    let at_date = detail.date_lecture.clone();
    let article_ref = NodeRef::<leptos::html::Article>::new();
    let selection_state: RwSignal<Option<SelState>> = RwSignal::new(None);

    #[cfg(feature = "hydrate")]
    crate::components::decision::selection::watch_selection(article_ref, selection_state);

    let sections_view = sections
        .into_iter()
        .map(|section| {
            let RenderSection {
                id,
                title,
                paragraphs,
                paragraph_spans,
            } = section;
            // Cible d'ancre : `scroll-mt-20` (scroll-margin-top 80px) décale
            // l'atterrissage du jump natif #hash sous la top-bar sticky.
            // Chaque paragraphe est rendu avec ses citations cliquables (overlay
            // composé server-side) ; spans vides ⇒ texte brut.
            let at_date = at_date.clone();
            let paragraphs_view = paragraphs
                .into_iter()
                .zip(paragraph_spans)
                .map(|(p, spans)| {
                    view! { <CitedParagraph text=p spans=spans at_date=at_date.clone() /> }
                })
                .collect_view();
            // One wordless break, on the first moyens section alone: the court
            // heads its own later branches on 98,8 % of 400 cassation decisions.
            let rule = (id == "moyens").then(|| {
                view! { <div class="mt-10 mb-6 h-px bg-[var(--color-rule)]" aria-hidden="true" /> }
            });
            view! {
                <section aria-label=title.clone()>
                    <div id=id class="scroll-mt-20" aria-hidden="true" />
                    {rule}
                    {paragraphs_view}
                </section>
            }
        })
        .collect_view();

    view! {
        <article node_ref=article_ref class="prose-decision">
            {sections_view}
        </article>
        <Show when=move || selection_state.get().is_some()>
            {move || {
                let state = selection_state.get().expect("présence garantie par `when`");
                view! {
                    <SelectionToolbar
                        state=state
                        reference=Some(references.get_value().full)
                        selection_state=selection_state
                    />
                }
            }}
        </Show>
    }
    .into_any()
}
