//! Bloc « Synthèse » : résumé + champs structurés + références juridiques.
use leptos::prelude::*;
use lj_dtos::{DecisionDetail, FacetTag, LegalReference};

use leptos_router::components::A;

use crate::components::decision::selection::{SelState, SelectionToolbar};
use crate::components::hover_preview::{HoverPreview, PreviewKind};
use crate::helpers::{format_iso_date, format_juridiction};
use crate::pages::decision_page::labels::publication_label;

/// Champ de synthèse (label + valeur, `mono` = police monospace).
#[derive(Clone)]
struct MetaField {
    label: &'static str,
    value: String,
    mono: bool,
}

fn build_fields(detail: &DecisionDetail) -> Vec<MetaField> {
    let mut fields: Vec<MetaField> = Vec::new();
    // 1. Juridiction (toujours).
    fields.push(MetaField {
        label: "Juridiction",
        value: detail
            .jurisdiction_name
            .clone()
            .unwrap_or_else(|| format_juridiction(detail.jurisdiction_type).to_string()),
        mono: false,
    });
    // Nom d'usage : c'est par lui que la décision se cite.
    if let Some(nom) = detail.case_name.clone() {
        fields.push(MetaField {
            label: "Nom d'usage",
            value: nom,
            mono: false,
        });
    }
    // 2. Numéros de dossier (si non vide).
    if let Some(dockets) = detail.docket_numbers.as_ref().filter(|d| !d.is_empty()) {
        fields.push(MetaField {
            label: "Numéros de dossier",
            value: dockets.join(", "),
            mono: true,
        });
    }
    // 3. Date de la décision.
    if let Some(date) = detail.date_lecture.as_deref() {
        fields.push(MetaField {
            label: detail.jurisdiction_type.date_label(),
            value: format_iso_date(Some(date)),
            mono: false,
        });
    }
    // 4-7. Tags référentiels résolus par l'API : rendu direct des libellés
    // servis. `voie` absente = procédure ordinaire (pas de champ).
    let tags: [(&'static str, &Option<FacetTag>); 3] = [
        ("Solution", &detail.solution),
        ("Procédure", &detail.procedure),
        ("Domaine", &detail.legal_domain),
    ];
    for (label, tag) in tags {
        if let Some(tag) = tag {
            fields.push(MetaField {
                label,
                value: tag.label.clone(),
                mono: false,
            });
        }
    }
    // 9. Siège (position de chambre qualifiée).
    if let Some(seat) = detail.seat.clone() {
        fields.push(MetaField {
            label: "Formation",
            value: seat,
            mono: false,
        });
    }
    // 10. Date d'audience.
    if let Some(date) = detail.date_audience.as_deref() {
        fields.push(MetaField {
            label: "Date d'audience",
            value: format_iso_date(Some(date)),
            mono: false,
        });
    }
    // 11. Publication + portée : lecture normalisée des codes au rang le plus
    // fort ; indéterminée = pas de champ.
    if let Some(pub_label) = publication_label(&detail.publication_codes) {
        fields.push(MetaField {
            label: "Publication",
            value: pub_label,
            mono: false,
        });
    }
    // 12. Nomenclature (nac, code brut).
    if let Some(nac) = detail.nac.clone() {
        fields.push(MetaField {
            label: "Nomenclature",
            value: nac,
            mono: true,
        });
    }
    fields
}

#[component]
pub fn DecisionMeta(
    detail: DecisionDetail,
    #[prop(optional, into)] section_id: Option<String>,
) -> impl IntoView {
    let section_id = section_id.unwrap_or_else(|| "synthese".to_string());
    let fields = build_fields(&detail);
    let themes = detail.themes.clone();
    let legal_references = detail
        .legal_references
        .clone()
        .filter(|r| !r.is_empty())
        .unwrap_or_default();

    // Résumé garanti en base et porté par le détail : rendu direct, aucun fetch.
    // Absent (cas résiduel) ⇒ bloc masqué.
    let summary_ref = NodeRef::<leptos::html::P>::new();
    let selection_state: RwSignal<Option<SelState>> = RwSignal::new(None);
    #[cfg(feature = "hydrate")]
    crate::components::decision::selection::watch_selection(summary_ref, selection_state);
    let summary_view = detail.summary.clone().map(|text| {
        view! {
            <p
                node_ref=summary_ref
                class="mt-3 text-sm leading-relaxed text-[var(--color-ink-muted)]"
            >
                {text}
            </p>
        }
    });

    let fields_view = fields
        .into_iter()
        .map(|f| view! { <Field label=f.label value=f.value mono=f.mono /> })
        .collect_view();

    // Chips de matière (themes Judilibre).
    let themes_view = (!themes.is_empty()).then(|| {
        let chips = themes
            .into_iter()
            .map(|theme| {
                view! {
                    <li class="rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2.5 py-1 text-xs text-[var(--color-ink-muted)]">
                        {theme}
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="mt-5 border-t border-[var(--color-rule)] pt-4">
                <h3 class="font-sans text-sm font-medium text-[var(--color-ink)]">"Matières"</h3>
                <ul class="mt-3 flex flex-wrap gap-2">{chips}</ul>
            </div>
        }
    });

    let refs_view = (!legal_references.is_empty()).then(|| {
        // Date de lecture propagée aux hover cards d'articles : version en
        // vigueur à la date de la décision.
        let at_date = detail.date_lecture.clone();
        let place = detail.court_place.clone();
        let rows = legal_references
            .into_iter()
            .map(|r| {
                view! { <LegalRefRow reference=r at_date=at_date.clone() place=place.clone() /> }
            })
            .collect_view();
        view! {
            <div class="mt-5 border-t border-[var(--color-rule)] pt-4">
                <h3 class="font-sans text-sm font-medium text-[var(--color-ink)]">
                    "Références juridiques"
                </h3>
                <ul class="mt-3 flex flex-col gap-2">{rows}</ul>
            </div>
        }
    });

    view! {
        // Never printed: the sheet goes into a case file, where only the court's own text counts.
        <section
            aria-label="Synthèse"
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6 print:hidden"
        >
            <div id=section_id class="relative -top-24" aria-hidden="true" />
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Synthèse"</h2>
            {summary_view}
            <dl class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">{fields_view}</dl>
            {themes_view}
            {refs_view}
        </section>
        <Show when=move || selection_state.get().is_some()>
            {move || {
                let state = selection_state.get().expect("présence garantie par `when`");
                view! {
                    <SelectionToolbar state=state reference=None selection_state=selection_state />
                }
            }}
        </Show>
    }
}

#[component]
fn Field(label: &'static str, value: String, mono: bool) -> impl IntoView {
    let dd_class = if mono {
        "font-mono text-sm text-[var(--color-ink)]"
    } else {
        "text-sm text-[var(--color-ink)]"
    };
    view! {
        <div class="flex flex-col gap-1">
            <dt class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {label}
            </dt>
            <dd class=dd_class>{value}</dd>
        </div>
    }
}

#[component]
fn LegalRefRow(
    reference: LegalReference,
    /// Date de la décision hôte, transmise aux hover cards d'articles.
    #[prop(optional_no_strip)]
    at_date: Option<String>,
    /// The court's place, which the links carry as the body's do.
    #[prop(optional_no_strip)]
    place: Option<String>,
) -> impl IntoView {
    let LegalReference {
        instrument,
        slug,
        jurisdiction,
        articles,
    } = reference;
    // Each article becomes a `/norm/{juris}/{slug}/{numKey}/{at_date}`
    // link once the citation is **resolved** against the catalogue: the
    // `legal_text` slug plus the canonical `numKey`, both set on the DTO.
    // Otherwise plain text, separated by a comma.
    let text = slug
        .as_deref()
        .map(|s| lj_vocab::norm_path(&jurisdiction, s));
    let segment = lj_vocab::url_segment(&jurisdiction);
    let suffix = crate::pages::law_page::data::reader_suffix(place.as_deref());
    let articles_view = (!articles.is_empty()).then(|| {
        let last = articles.len() - 1;
        let items = articles
            .into_iter()
            .enumerate()
            .map(|(i, article)| {
                let sep = (i < last).then_some(", ");
                let target = text
                    .as_ref()
                    .zip(slug.as_ref())
                    .filter(|_| !article.num_key.is_empty())
                    .map(|(path, s)| {
                        let href = match at_date.as_deref() {
                            Some(d) => format!("{path}/{}/{d}{suffix}", article.num_key),
                            None => format!("{path}/{}{suffix}", article.num_key),
                        };
                        (
                            href,
                            PreviewKind::Article {
                                juris: segment.clone(),
                                code: s.to_string(),
                                num: article.num_key.clone(),
                                date: at_date.clone(),
                                place: place.clone(),
                            },
                        )
                    });
                let num = article.num;
                let body = match target {
                    // Hover card d'article sur le lien.
                    Some((href, kind)) => view! {
                        <HoverPreview kind=kind>
                            <A href=href attr:class="underline underline-offset-4 hover:text-[var(--color-accent)]">
                                {num}
                            </A>
                        </HoverPreview>
                    }
                    .into_any(),
                    None => view! { <span>{num}</span> }.into_any(),
                };
                view! {
                    {body}
                    {sep}
                }
            })
            .collect_view();
        // `art.&nbsp;` : espace insécable U+00A0.
        view! {
            <span class="text-[var(--color-ink-muted)]">"art.\u{00A0}"{items}</span>
        }
    });
    // The instrument name links to the whole text as soon as the citation
    // resolves against the catalogue, the same doctrine as the body, where a
    // bare mention leads to the summary.
    let instrument_view = match text {
        Some(path) => view! {
            <A
                href=format!("{path}{suffix}")
                attr:class="font-medium text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
            >
                {instrument}
            </A>
        }
        .into_any(),
        None => view! {
            <span class="font-medium text-[var(--color-ink)]">{instrument}</span>
        }
        .into_any(),
    };
    view! {
        <li class="flex flex-wrap items-baseline gap-2 text-sm">
            {instrument_view}
            {articles_view}
        </li>
    }
}
