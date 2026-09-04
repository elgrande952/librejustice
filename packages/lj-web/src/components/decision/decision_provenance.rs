//! Bloc « Source » d'une décision : provenance/audit en pied de page. Le
//! contenu vient de [`lj_dtos::provenance_rows`], que l'export PDF lit aussi.
use crate::components::ui::ProvenanceBlock;
use leptos::prelude::*;
use lj_dtos::DecisionDetail;

#[component]
pub fn DecisionProvenance(detail: DecisionDetail) -> impl IntoView {
    view! { <ProvenanceBlock rows=lj_dtos::provenance_rows(&detail) /> }
}
