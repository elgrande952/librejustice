//! Page `/norm/{juris}/{code}/{num}[/{date}]` (LawArticlePage, SEO-critique), et
//! l'aiguillage des segments qui suivent le slug d'un texte (NormNodePage).
//!
//! - Article (en-tête, méta, corps, timeline) : `Resource::new_blocking` ⇒ le SSR
//!   attend la résolution avant d'émettre le HTML (`<Title>`/`<Meta>`/JSON-LD
//!   crawlables). Le `LawArticleResponse` porte sa propre timeline (`versions`).
//! - Décisions citantes : `Resource` non bloquante + `<Suspense>` ⇒ streamées
//!   après le shell.
//! - `Cache-Control` (SSR) posé selon le statut (200 → 7 j CDN, 404/400/422 →
//!   5 min, 5xx → no-store).

mod actions;
pub(crate) mod cited_by;
pub mod data;
pub(crate) mod top_cited;

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Script, Title};
use leptos_router::components::A;
use lj_dtos::{
    Applicability, ApplicabilityStatus, ArticleModification, ArticleNeighbor, CoCitedArticle,
    LawArticleResponse, LawArticleVersion, LinkedTextRef, ModificationItem, PlaceGroup, PlaceNode,
    PlaceRef, ReadablePlace, StateElsewhere, UndeterminedReason, DEFAULT_READER_PLACE,
};

use actions::ArticleActions;
use cited_by::CitedBySection;

use crate::components::decision::DecisionLayout;
use crate::components::own_page_description;
use crate::components::search::compact_search::highlight::CitedBlock;
use crate::components::ui::{GroupedSelect, ProvenanceBlock, SelectGroup, SelectOption};
use crate::helpers::{format_article_num, format_iso_date};
use crate::seo::law::{article_canonical_url, article_meta_description, build_article_json_ld};
use crate::seo::OG_IMAGE;

use crate::pages::law_compare_page::LawComparePage;
use crate::pages::law_section_page::{LawSectionLoaded, LawSectionPage};
use data::{
    article_key, chrono_date, fetch_article, fetch_article_or_section, fetch_citing, fetch_related,
    fetch_section_citing, reader_place, reader_suffix, sendable, ArticleKey, ArticleOrSection,
    CitingResult, NormNode, PageError, RelatedResult,
};

const RELATED_LIMIT: usize = 5;

/// `/norm/{juris}/{code}/{*rest}`: the page the segments after the slug name.
#[component]
pub fn NormNodePage() -> impl IntoView {
    let node = data::norm_node();
    let kind = Memo::new(move |_| NodeKind::of(&node.get()));
    move || match kind.get() {
        NodeKind::Article => view! { <LawArticlePage /> }.into_any(),
        NodeKind::Compare => view! { <LawComparePage /> }.into_any(),
        NodeKind::Section => view! { <LawSectionPage /> }.into_any(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Article,
    Compare,
    Section,
}

impl NodeKind {
    fn of(node: &NormNode) -> Self {
        match node {
            NormNode::Article { .. } => NodeKind::Article,
            NormNode::Compare { .. } => NodeKind::Compare,
            NormNode::Section(_) => NodeKind::Section,
        }
    }
}

#[component]
pub fn LawArticlePage() -> impl IntoView {
    own_page_description();

    let key = article_key();

    let section_date = chrono_date();
    let node = Resource::new_blocking(
        move || (key.get(), section_date.get()),
        |(key, date)| sendable(fetch_article_or_section(key, date)),
    );
    // Awaits the node, so an article page never asks for a section's citing decisions.
    let section_citing = Resource::new(
        move || (key.get(), section_date.get()),
        move |(key, date)| {
            sendable(async move {
                match node.await {
                    Ok(ArticleOrSection::Section(_)) => {
                        fetch_section_citing(key.juris, key.code, key.num, date).await
                    }
                    _ => CitingResult {
                        hits: Vec::new(),
                        error: None,
                    },
                }
            })
        },
    );
    // Décisions citantes bornées à la fenêtre de validité de la version servie
    // (clé = code + num + date).
    let citing = Resource::new(
        move || {
            let k = key.get();
            (k.juris, k.code, k.num, k.date, k.place)
        },
        |(juris, code, num, date, place)| sendable(fetch_citing(juris, code, num, date, place)),
    );
    // Articles co-cités (« souvent cité avec ») — indépendants de la version
    // servie (co-citation au grain `num_key`).
    let related = Resource::new(
        move || {
            let k = key.get();
            (k.juris, k.code, k.num)
        },
        |(juris, code, num)| sendable(fetch_related(juris, code, num)),
    );

    view! {
        <Suspense fallback=LawSkeleton>
            {move || Suspend::new(async move {
                match node.await {
                    Ok(ArticleOrSection::Article(article)) => {
                        // L'API résout par clé d'identité : le slug renommé et la
                        // forme non canonique du numéro rendent le même article,
                        // sous une seule URL indexable.
                        let k = key.get_untracked();
                        let segment = lj_vocab::url_segment(&article.jurisdiction);
                        if k.code != article.code || k.num != article.num_key
                            || k.juris != segment
                        {
                            redirection_permanente(
                                &article_url(
                                    &article.jurisdiction,
                                    &article.code,
                                    &article.num_key,
                                    k.date.as_deref(),
                                    k.place.as_deref(),
                                ),
                            );
                        }
                        set_cache_control(200);
                        Either::Left(
                            view! { <LawArticleLoaded article=article citing=citing related=related /> },
                        )
                    }
                    Ok(ArticleOrSection::Section(section)) => {
                        set_cache_control(200);
                        Either::Right(Either::Left(
                            view! {
                                <LawSectionLoaded
                                    section=section
                                    citing=section_citing
                                    date=section_date.get()
                                />
                            },
                        ))
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        let k = key.get_untracked();
                        // Why a dated read finds nothing is in the timeline the
                        // undated read carries: one extra fetch, on the 404 leg only.
                        let frise = if err.status == 404 && k.date.is_some() {
                            sendable(fetch_article(ArticleKey { date: None, ..k.clone() }))
                                .await
                                .ok()
                        } else {
                            None
                        };
                        Either::Right(Either::Right(
                            view! {
                                <LawError
                                    err=err
                                    juris=k.juris
                                    code=k.code
                                    num=k.num
                                    date=k.date
                                    place=k.place
                                    frise=frise
                                />
                            },
                        ))
                    }
                }
            })}
        </Suspense>
    }
}

/// URL d'article, `num` et date de la route préservés — seul le slug bouge.
fn article_url(
    juris: &str,
    code: &str,
    num: &str,
    date: Option<&str>,
    place: Option<&str>,
) -> String {
    let text = lj_vocab::norm_path(juris, code);
    let path = match date {
        Some(d) => format!("{text}/{num}/{d}"),
        None => format!("{text}/{num}"),
    };
    format!("{path}{}", reader_suffix(place))
}

/// Pose un 301 vers l'URL canonique : un slug retiré par un renommage lègue son
/// indexation au nouveau. No-op côté hydrate, le client a déjà suivi.
#[cfg(feature = "ssr")]
pub(crate) fn redirection_permanente(cible: &str) {
    use axum::http::{header::LOCATION, HeaderValue, StatusCode};
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        resp.set_status(StatusCode::MOVED_PERMANENTLY);
        if let Ok(hv) = HeaderValue::from_str(cible) {
            resp.insert_header(LOCATION, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
pub(crate) fn redirection_permanente(_cible: &str) {}

/// Pose le statut HTTP et le `Cache-Control` de la réponse document SSR. No-op
/// côté hydrate (200 → 7 j ; erreurs client → 5 min ; 5xx → no-store).
#[cfg(feature = "ssr")]
pub(crate) fn set_cache_control(status: u16) {
    use axum::http::{header::CACHE_CONTROL, HeaderValue, StatusCode};
    let value = match status {
        200 => "public, max-age=0, s-maxage=604800, stale-while-revalidate=86400",
        404 | 400 | 422 => "public, max-age=0, s-maxage=300",
        _ => "no-store",
    };
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        if status != 200 {
            if let Ok(code) = StatusCode::from_u16(status) {
                resp.set_status(code);
            }
        }
        if let Ok(hv) = HeaderValue::from_str(value) {
            resp.insert_header(CACHE_CONTROL, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
pub(crate) fn set_cache_control(_status: u16) {}

/// Page d'erreur (référence invalide / 404 / autre). Pose `robots noindex`.
///
/// A dated read that finds nothing has found the article and missed the version,
/// and 26 % of the dated reads served on 08/09/2026 landed here: « article
/// introuvable » sends the reader away from a page that exists.
#[component]
fn LawError(
    err: PageError,
    juris: String,
    code: String,
    num: String,
    date: Option<String>,
    place: Option<String>,
    frise: Option<LawArticleResponse>,
) -> impl IntoView {
    let suffix = reader_suffix(place.as_deref());
    let text = lj_vocab::norm_path(&juris, &code);
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    let title = "Article introuvable - LibreJustice";
    let (titre, recours) = match (err.status, date.as_deref()) {
        (404, Some(d)) => (
            format!(
                "Aucune version de cet article au {}",
                lj_dtos::format_date_fr(d)
            ),
            Some(format!("{text}/{num}{suffix}")),
        ),
        (422, _) => (
            "Cette adresse ne désigne ni un lieu ni une date que nous connaissons".to_string(),
            Some(format!("{text}/{num}")),
        ),
        _ => (err.message, None),
    };
    let raison = date
        .as_deref()
        .zip(frise.as_ref())
        .and_then(|(d, article)| raison_du_trou(&article.versions, d));
    view! {
        <Title text=title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                {eyebrow}
            </p>
            <h1 class="mt-2 font-sans text-3xl text-[var(--color-ink)]">{titre}</h1>
            {raison
                .map(|(phrase, bornes)| {
                    view! {
                        <p class="mt-4 text-sm leading-relaxed text-[var(--color-ink-muted)]">
                            {phrase}
                        </p>
                        <ul class="mt-4 flex flex-col gap-1">
                            {bornes
                                .into_iter()
                                .map(|(libelle, jour)| {
                                    view! {
                                        <li>
                                            <a
                                                href=format!("{text}/{num}/{jour}{suffix}")
                                                class="text-sm text-[var(--color-accent)] underline-offset-4 hover:underline"
                                            >
                                                {libelle}
                                            </a>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                })}
            {recours
                .map(|href| {
                    view! {
                        <a
                            href=href
                            class="mt-6 self-start text-sm text-[var(--color-accent)] underline-offset-4 hover:underline"
                        >
                            "Voir la version en vigueur"
                        </a>
                    }
                })}
        </div>
    }
}

/// French wording of the gap a dated read fell into, and the two versions that
/// bound it. `lj_dtos::gap_explique` decides whether the status says why, the
/// unexplained case being ours to hold and named as such.
fn raison_du_trou(
    versions: &[lj_dtos::LawArticleVersion],
    date: &str,
) -> Option<(String, Vec<(String, String)>)> {
    let bornes: Vec<_> = versions
        .iter()
        .map(|v| (Some(v.date_debut.as_str()), v.date_fin.as_deref()))
        .collect();
    let (i, j) = lj_dtos::version_gap(&bornes, date)?;
    let (avant, apres) = (versions.get(i)?, versions.get(j)?);
    let ferme = lj_dtos::format_date_fr(avant.date_fin.as_deref()?);
    let reprise = lj_dtos::format_date_fr(&apres.date_debut);
    let phrase = if lj_dtos::gap_explique(&avant.etat) {
        format!(
            "{} le {ferme}, l'article est à nouveau présent le {reprise} : il n'existait pas sous ce numéro à la date demandée.",
            status_label(&avant.etat)
        )
    } else {
        format!(
            "L'article court de part et d'autre de cette date, mais nous ne détenons aucune version entre le {ferme} et le {reprise}."
        )
    };
    Some((
        phrase,
        vec![
            (
                format!(
                    "Version qui précède, du {}",
                    lj_dtos::format_date_fr(avant.date_debut.as_str())
                ),
                avant.date_debut.clone(),
            ),
            (
                format!("Version qui suit, du {reprise}"),
                apres.date_debut.clone(),
            ),
        ],
    ))
}

/// Squelette de chargement (en-tête + corps + colonnes).
#[component]
fn LawSkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <Skeleton class="h-4 w-1/3" />
            <Skeleton class="h-9 w-2/3" />
            <Skeleton class="h-4 w-1/2" />
            <div class="mt-4 flex flex-col gap-3">
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-4 w-11/12" />
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-4 w-3/5" />
            </div>
        </div>
    }
}

/// Titre humain d'un article (« Article 1240 du Code civil », « Article 2 de
/// l'Ordonnance n° 2016-131… »).
pub(crate) fn article_title(article: &LawArticleResponse) -> String {
    format!(
        "Article {} {}",
        format_article_num(&article.num),
        lj_dtos::instrument_with_de(&code_display_name(article))
    )
}

/// Nom affichable du code : le titre humain (`code_title`, ex. « Code de la
/// famille sénégalais ») quand l'API le fournit ; à défaut, un libellé dérivé du
/// slug `code` (tirets → espaces), forcément approximatif.
fn code_display_name(article: &LawArticleResponse) -> String {
    article
        .code_title
        .clone()
        .unwrap_or_else(|| article.code.replace('-', " "))
}

/// Page chargée : SEO (title/meta/OG/canonical/JSON-LD) + en-tête, méta, corps,
/// timeline, décisions citantes.
#[component]
fn LawArticleLoaded(
    article: LawArticleResponse,
    citing: Resource<CitingResult>,
    related: Resource<RelatedResult>,
) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    Effect::new(|_| crate::reads::report(lj_dtos::ReadKind::Article));
    let title = article_title(&article);
    let description = article_meta_description(&article, &title);
    let url = article_canonical_url(&article.jurisdiction, &article.code, &article.num_key);
    let jsonld = serde_json::to_string(&build_article_json_ld(&article, &title, &description))
        .unwrap_or_else(|_| "{}".to_string());
    let page_title = format!("{title} - LibreJustice");

    // L'API vide `dateDebut` pour les articles modificatifs (sentinelle LEGI
    // 2999) : pas de « en vigueur depuis le … ». Version servie future : le bandeau
    // « entrera en vigueur » la porte.
    let served_is_upcoming = !article.date_debut.is_empty()
        && article.upcoming_version_date.as_deref() == Some(article.date_debut.as_str());
    // `expired` and the version window are read on the article's own timeline,
    // which is the metropolitan one. A place that keeps a wording the metropole
    // has replaced makes that timeline speak of somewhere else: the wording is
    // then named by its date, and the window is the reader's.
    // A matter passed to the reader's place keeps the wording in force on the
    // day of the transfer, which is the version served.
    let frozen_on = article
        .applicability
        .as_ref()
        .filter(|a| a.status == ApplicabilityStatus::Undetermined(UndeterminedReason::Frozen))
        .and_then(|a| a.frozen_on.clone());
    // An article is not shown from a place the reader picked where it does not
    // apply; from the default place it stays, under its banner.
    let withheld = article
        .applicability
        .as_ref()
        .is_some_and(|a| a.status == ApplicabilityStatus::NotApplicable)
        && reader_place()
            .get_untracked()
            .is_some_and(|p| p != DEFAULT_READER_PLACE);
    let reader_holds = frozen_on.is_some()
        || article.expired
            && article.applicability.as_ref().is_some_and(|a| {
                a.status == ApplicabilityStatus::Applicable && a.ends_on.is_none()
            });
    let validity =
        (!withheld && !article.date_debut.is_empty() && !served_is_upcoming).then(|| {
            let label = if let Some(d) = frozen_on.as_deref() {
                format!("Rédaction en vigueur le {}", format_iso_date(Some(d)))
            } else if reader_holds {
                format!(
                    "Rédaction du {}",
                    format_iso_date(Some(&article.date_debut))
                )
            } else {
                validity_label(
                    &article.date_debut,
                    article.date_fin.as_deref(),
                    article.expired,
                )
            };
            view! { <p class="text-sm text-[var(--color-ink-muted)]">{label}</p> }
        });
    // L'API sert la version en vigueur quand la route ne porte pas de date : une
    // version close en réponse dit alors qu'aucune ne s'applique plus.
    let expired_notice =
        (!withheld && article.expired && !reader_holds && !article.date_debut.is_empty()).then(
            || {
                expired_banner(match article_key().get_untracked().date {
                    None => "Cet article n'est plus en vigueur",
                    Some(_) => "Cette version n'est plus en vigueur",
                })
            },
        );
    let applicability = applicability_view(&article, article_key().get_untracked().date.is_some());
    let code_href = lj_vocab::norm_path(&article.jurisdiction, &article.code);
    let code_name = code_display_name(&article);
    // Bandeau temporel : une version future de l'article existe. Si la version
    // SERVIE est cette version future, le bandeau dit l'entrée en vigueur, pas une
    // modification.
    let upcoming = article
        .upcoming_version_date
        .as_deref()
        .filter(|_| !withheld && frozen_on.is_none())
        .map(|d| {
            let subject = if article.date_debut == d {
                "Cet article entrera en vigueur"
            } else {
                "Cet article sera modifié"
            };
            upcoming_banner(subject, d)
        });

    let nav_view = view! { <ArticleSideNav article=article.clone() /> }.into_any();
    let neighbor_nav = neighbor_nav(&article);
    // « Cité par » : décisions citantes streamées, textes citants portés par la
    // réponse bloquante.
    let cited_by_view = view! {
        <CitedBySection
            texts=article.cited_by.clone()
            citing=citing
            search_query=title.clone()
            code=article.code.clone()
            num_key=article.num_key.clone()
        />
    };
    let main_view = view! {
        <article class="flex flex-col gap-10">
            <header class="flex flex-col gap-2">
                <A
                    href=code_href
                    attr:class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)] underline-offset-4 hover:text-[var(--color-accent)]"
                >
                    {code_name}
                </A>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{title.clone()}</h1>
                {validity}
                {applicability}
                {expired_notice}
                {upcoming}
                {neighbor_nav}
            </header>
            {(!withheld).then(|| view! { <ArticleBody article=article.clone() /> })}
            {travaux_section(article.travaux_parlementaires.clone())}
            {cited_by_view}
            <crate::components::decision::DecisionCommentaires commentaires=article
                .commentaires
                .clone() />
            <ArticleMeta article=article.clone() />
            <ProvenanceBlock rows=article_provenance_rows(&article) />
        </article>
    }
    .into_any();
    let neighbors_view =
        view! { <ArticleNeighbors article=article.clone() related=related /> }.into_any();

    view! {
        <Title text=page_title />
        <Meta name="description" content=description.clone() />
        <Meta property="og:type" content="article" />
        <Meta property="og:site_name" content="LibreJustice" />
        <Meta property="og:title" content=title.clone() />
        <Meta property="og:description" content=description.clone() />
        <Meta property="og:url" content=url.clone() />
        <Meta property="og:locale" content="fr_FR" />
        <Meta property="og:image" content=OG_IMAGE />
        <Meta name="twitter:card" content="summary_large_image" />
        {withheld.then(|| view! { <Meta name="robots" content="noindex" /> })}
        <Link rel="canonical" href=url />
        <Script type_="application/ld+json">{jsonld}</Script>
        <DecisionLayout toc=nav_view main=main_view similar=neighbors_view />
    }
}

/// Article précédent / suivant (voisins immédiats du contexte de chapitre),
/// mobile uniquement — sur desktop la colonne « Dans le même chapitre » porte
/// déjà cette navigation.
fn neighbor_nav(article: &LawArticleResponse) -> Option<impl IntoView> {
    let ctx = &article.context;
    let idx = ctx.iter().position(|n| n.current)?;
    let suffix = reader_suffix(reader_place().get_untracked().as_deref());
    let text = lj_vocab::norm_path(&article.jurisdiction, &article.code);
    let link = |n: &ArticleNeighbor, arrow_left: bool| {
        let href = format!("{text}/{}{suffix}", n.num_key);
        let label = format!("Article {}", format_article_num(&n.num));
        view! {
            <A
                href=href
                attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-accent)] no-underline hover:underline"
            >
                {arrow_left.then(|| view! { <span aria-hidden="true">"←"</span> })}
                {label}
                {(!arrow_left).then(|| view! { <span aria-hidden="true">"→"</span> })}
            </A>
        }
    };
    let prev = idx
        .checked_sub(1)
        .and_then(|i| ctx.get(i))
        .map(|n| link(n, true));
    let next = ctx.get(idx + 1).map(|n| link(n, false));
    if prev.is_none() && next.is_none() {
        return None;
    }
    Some(view! {
        <nav
            aria-label="Articles voisins"
            class="flex items-center justify-between gap-4 pt-1 lg:hidden"
        >
            <span>{prev}</span>
            <span>{next}</span>
        </nav>
    })
}

/// Colonne gauche : agir sur l'article (copier sa référence) puis sa
/// dimension temporelle — sélecteur Chronolégi et versions en points sur rail
/// fin (le point plein accent marque la version courante). Scrollable
/// indépendamment quand l'historique dépasse la fenêtre.
#[component]
fn ArticleSideNav(article: LawArticleResponse) -> impl IntoView {
    // Chronolégi : la date demandée vient de la route
    // (`/norm/{juris}/{code}/{num}/{date}`), le picker navigue en segment de path.
    let base = format!(
        "{}/{}",
        lj_vocab::norm_path(&article.jurisdiction, &article.code),
        article.num_key
    );
    let key = article_key();
    let reference = article_title(&article);
    let requested_date = Signal::derive(move || key.get().date);
    let places = article
        .applicability
        .as_ref()
        .map(|a| readable_groups(&a.readable))
        .unwrap_or_default();
    view! {
        <div class="flex flex-col gap-6 lg:sticky lg:top-20 lg:max-h-[calc(100vh-6rem)] lg:self-start lg:overflow-y-auto">
            <ArticleActions reference=reference />
            <nav aria-label="Versions de l'article" class="flex flex-col gap-8">
                <ChronoDatePicker base=base.clone() date=requested_date path_segment=true />
                <PlacePicker base=base groups=places date=requested_date />
                <Timeline article=article />
            </nav>
        </div>
    }
}

/// Sélecteur Chronolégi « À la date du … » : navigue vers la même ressource à
/// la date choisie — segment de path pour l'article (`{base}/{date}`), query
/// `?date=` pour sommaire et section. Vider le champ (ou « version en vigueur »)
/// revient à `base`.
#[component]
pub(crate) fn ChronoDatePicker(
    base: String,
    #[prop(into)] date: Signal<Option<String>>,
    #[prop(optional)] path_segment: bool,
) -> impl IntoView {
    use leptos_router::hooks::use_navigate;
    let navigate = use_navigate();
    let place = reader_place().get_untracked();
    let href_for = StoredValue::new(move |picked: &str| {
        let mut href = if picked.is_empty() {
            base.clone()
        } else if path_segment {
            format!("{base}/{picked}")
        } else {
            format!("{base}?date={picked}")
        };
        if let Some(l) = &place {
            href.push(if href.contains('?') { '&' } else { '?' });
            href.push_str(&format!("place={l}"));
        }
        href
    });
    let reset = move || {
        date.get().map(|_| {
            let navigate = use_navigate();
            view! {
                <button
                    type="button"
                    class="w-fit text-xs text-[var(--color-accent)] underline-offset-4 hover:underline"
                    on:click=move |_| navigate(&href_for.get_value()(""), Default::default())
                >
                    "Revenir à la version en vigueur"
                </button>
            }
        })
    };
    view! {
        <div class="flex flex-col gap-1.5">
            <label class="flex flex-col gap-1">
                <span class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "À la date du"
                </span>
                <input
                    type="date"
                    prop:value=move || date.get().unwrap_or_default()
                    class="h-9 w-fit rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2 text-sm text-[var(--color-ink)]"
                    on:change=move |ev| {
                        let picked = event_target_value(&ev);
                        navigate(&href_for.get_value()(&picked), Default::default());
                    }
                />
            </label>
            {reset}
        </div>
    }
}

/// Bloc à points sur rail (idiome sommaire/chronologie) : en-tête petites
/// capitales + liste, chaque entrée ancrée au rail par un point (plein accent
/// = entrée courante).
pub(crate) fn rail_block(title: &'static str, items: Vec<AnyView>) -> AnyView {
    view! {
        <div class="flex flex-col gap-3">
            <p class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                {title}
            </p>
            <div class="relative">
                <div
                    aria-hidden="true"
                    class="absolute bottom-2 left-[7px] top-2 w-[2px] rounded-full bg-[var(--color-rule)]"
                />
                <ol class="flex flex-col gap-3">{items}</ol>
            </div>
        </div>
    }
    .into_any()
}

/// Entrée d'un bloc à rail : point + contenu.
pub(crate) fn rail_item(current: bool, body: AnyView) -> AnyView {
    let dot_class = if current {
        "absolute left-[3px] top-[5px] h-[10px] w-[10px] rounded-full border-2 border-[var(--color-accent)] bg-[var(--color-accent)]"
    } else {
        "absolute left-[3px] top-[5px] h-[10px] w-[10px] rounded-full border-2 border-[var(--color-rule)] bg-[var(--color-parchment)]"
    };
    view! {
        <li class="relative pl-7">
            <span aria-hidden="true" class=dot_class />
            {body}
        </li>
    }
    .into_any()
}

/// (href, libellé) de la section descriptive de la source sur `/sources`.
/// Ancre par provenance ; fallback page `/sources` sans ancre.
fn source_descriptor(source: &str) -> (String, &'static str) {
    let (anchor, label) = match source {
        "legifrance" => ("dila", "Base LEGI · DILA"),
        "jorf" => ("dila", "Journal officiel · DILA"),
        _ => ("", "Données & sources"),
    };
    let href = if anchor.is_empty() {
        "/sources".to_string()
    } else {
        format!("/sources#{anchor}")
    };
    (href, label)
}

/// Libellé lisible des sources nommées, hôte de `source_url` sinon : les
/// libellés machine (`official-fr`, `eu-law`, `ecoi`) y renvoient tous.
fn diffuser_name(article: &LawArticleResponse) -> String {
    let named = match article.source.as_str() {
        "legifrance" | "kali" | "jorf" => Some("Légifrance"),
        "bofip" => Some("BOFiP-Impôts"),
        "eur-lex" | "eu-law" => Some("EUR-Lex"),
        _ => None,
    };
    named.map(str::to_string).unwrap_or_else(|| {
        article
            .source_url
            .as_deref()
            .and_then(url_host)
            .unwrap_or_else(|| article.source.clone())
    })
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.split(':').next()?;
    (!host.is_empty()).then(|| host.strip_prefix("www.").unwrap_or(host).to_string())
}

fn diffuser_link_label(name: &str) -> String {
    let elide = name
        .chars()
        .next()
        .is_some_and(|c| "aeiouAEIOUéÉèÈ".contains(c));
    let de = if elide { "d'" } else { "de " };
    format!("Voir sur le site {de}{name}")
}

/// Pendant de [`lj_dtos::provenance_rows`] pour un article.
fn article_provenance_rows(article: &LawArticleResponse) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Diffuseur", diffuser_name(article))];
    if let Some(url) = article.source_url.as_deref().filter(|u| !u.is_empty()) {
        let label = if article.source_official {
            "Page officielle"
        } else {
            "Page du diffuseur"
        };
        rows.push((label, url.to_string()));
    }
    if let Some(url) = article
        .source_upstream_url
        .as_deref()
        .filter(|u| !u.is_empty())
    {
        rows.push(("Source d'origine", url.to_string()));
    }
    rows.push((
        "Permalien",
        article_canonical_url(&article.jurisdiction, &article.code, &article.num_key),
    ));
    rows
}

/// Bloc méta : état, dates de validité, identifiant LEGIARTI, lien Légifrance
/// versionné.
#[component]
fn ArticleMeta(article: LawArticleResponse) -> impl IntoView {
    let date_fin = article
        .date_fin
        .as_deref()
        .map(|d| format_iso_date(Some(d)))
        .unwrap_or_else(|| "en vigueur".to_string());
    let (source_href, source_label) = source_descriptor(&article.source);
    let freshness = article
        .source_asof
        .as_deref()
        .map(|d| format_iso_date(Some(d)));
    let source_authority = article.source_authority.clone();
    let diffuser_link = diffuser_link_label(&diffuser_name(&article));
    let source_url = article.source_url.clone().filter(|u| !u.is_empty());
    let source_upstream = article
        .source_upstream_url
        .clone()
        .filter(|u| !u.is_empty());

    view! {
        <section
            aria-label="Métadonnées"
            class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6"
        >
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Métadonnées"</h2>
            <dl class="mt-4 grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
                <MetaField label="État" value=status_label(&article.etat) mono=false />
                <MetaField
                    label="Début de validité"
                    value=format_iso_date(Some(&article.date_debut))
                    mono=false
                />
                <MetaField label="Fin de validité" value=date_fin mono=false />
                <MetaField label="Identifiant" value=article.legiarti mono=true />
                {freshness
                    .map(|f| view! { <MetaField label="Fraîcheur de la source" value=f mono=false /> })}
                <MetaField label="Autorité" value=source_authority mono=false />
            </dl>
            <div class="mt-5 flex flex-wrap items-center gap-x-6 gap-y-2 border-t border-[var(--color-rule)] pt-4">
                {source_url
                    .map(|u| {
                        view! {
                            <a
                                href=u
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                            >
                                {diffuser_link}
                                <span aria-hidden="true">"↗"</span>
                            </a>
                        }
                    })}
                {source_upstream
                    .map(|u| {
                        view! {
                            <a
                                href=u
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink-subtle)] underline-offset-4 hover:text-[var(--color-accent)]"
                            >
                                "Source d'origine"
                                <span aria-hidden="true">"↗"</span>
                            </a>
                        }
                    })}
                <A
                    href=source_href
                    attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink-muted)] underline-offset-4 hover:text-[var(--color-accent)]"
                >
                    {format!("Source : {source_label}")}
                </A>
            </div>
        </section>
    }
}

/// Lecture en contexte : voisins de l'article dans sa division (ou fenêtre),
/// l'article courant au point plein. Masqué si pas de contexte (≤ 1 entrée =
/// seulement l'article lui-même).
#[component]
fn ArticleContext(article: LawArticleResponse) -> impl IntoView {
    let context = article.context;
    if context.len() <= 1 {
        return ().into_any();
    }
    let text = lj_vocab::norm_path(&article.jurisdiction, &article.code);
    let suffix = reader_suffix(reader_place().get_untracked().as_deref());
    let items = context
        .into_iter()
        .map(|n| context_item(n, &text, &suffix))
        .collect();
    rail_block("Dans le même chapitre", items)
}

fn context_item(neighbor: ArticleNeighbor, text: &str, suffix: &str) -> AnyView {
    let label = format!("Article {}", format_article_num(&neighbor.num));
    let abrogated = neighbor.etat != "VIGUEUR";
    if neighbor.current {
        let body = view! {
            <span aria-current="page" class="block text-sm font-medium leading-snug text-[var(--color-ink)]">
                {label}
            </span>
        }
        .into_any();
        return rail_item(true, body);
    }
    let href = format!("{text}/{}{suffix}", neighbor.num_key);
    let cls = if abrogated {
        "block text-sm leading-snug text-[var(--color-ink-subtle)] line-through no-underline hover:text-[var(--color-accent)]"
    } else {
        "block text-sm leading-snug text-[var(--color-accent)] no-underline hover:underline"
    };
    let body = view! { <A href=href attr:class=cls>{label}</A> }.into_any();
    rail_item(false, body)
}

#[component]
fn MetaField(label: &'static str, value: String, mono: bool) -> impl IntoView {
    let dd_class = if mono {
        "font-mono text-sm text-[var(--color-ink)] break-all"
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

/// Corps de l'article : texte principal (+ texte original en accordéon si
/// trad.) et, sous le texte, le `nota` = apparat éditorial : « Nota » officielle
/// Légifrance (entrée en vigueur, QPC) ou jurisprudence/doctrine + renvois « voir
/// aussi » des éditions annotées. Rendu dans un encart distinct, non normatif.
#[component]
fn ArticleBody(article: LawArticleResponse) -> impl IntoView {
    let texte = article
        .texte
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Texte indisponible.".to_string());
    // Badge de provenance : on signale une traduction non officielle ou
    // automatique ; le texte officiel ne porte pas de badge.
    let badge = match article.translation.as_str() {
        "non_officiel" => Some(("Traduction non officielle", false)),
        "automatique" => Some(("Traduction automatique — non vérifiée", true)),
        _ => None,
    }
    .map(|(label, warn)| {
        let tone = if warn {
            "border-[var(--color-accent)] text-[var(--color-accent)]"
        } else {
            "border-[var(--color-rule)] text-[var(--color-ink-subtle)]"
        };
        view! {
            <span class=format!(
                "inline-flex w-fit items-center rounded-full border px-2.5 py-0.5 text-xs {tone}",
            )>{label}</span>
        }
    });
    let lang_orig = article.lang_original.clone();
    let original = article
        .texte_original
        .filter(|t| !t.trim().is_empty())
        .map(|orig| {
            let dir = if lang_orig.as_deref() == Some("ar") { "rtl" } else { "ltr" };
            let label = match lang_orig.as_deref() {
                Some("ar") => "Texte original (arabe)".to_string(),
                Some(l) => format!("Texte original ({l})"),
                None => "Texte original".to_string(),
            };
            view! {
                <details class="mt-6 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-3">
                    <summary class="cursor-pointer text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                        {label}
                    </summary>
                    <div
                        dir=dir
                        class="mt-2 whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]"
                    >
                        {orig}
                    </div>
                </details>
            }
        });
    // Fil d'Ariane : segments TOC cliquables vers la vue-lecture de section quand
    // la structure est ingérée ; sinon le `titre_text` plat.
    let fil = if article.breadcrumb.is_empty() {
        article
            .titre_text
            .filter(|t| !t.trim().is_empty())
            .map(|fil| {
                view! {
                    <p class="text-xs text-[var(--color-ink-subtle)]">{fil}</p>
                }
                .into_any()
            })
    } else {
        let last = article.breadcrumb.len() - 1;
        let segments = article
            .breadcrumb
            .into_iter()
            .enumerate()
            .map(|(i, seg)| {
                let sep = (i < last).then(|| {
                    view! { <span aria-hidden="true">"›"</span> }
                });
                let inner = match seg.href {
                    Some(href) => view! {
                        <A
                            href=href
                            attr:class="no-underline underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                        >
                            {seg.label}
                        </A>
                    }
                    .into_any(),
                    None => view! { <span>{seg.label}</span> }.into_any(),
                };
                view! {
                    {inner}
                    {sep}
                }
            })
            .collect_view();
        Some(
            view! {
                <nav
                    aria-label="Fil d'Ariane"
                    class="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-xs text-[var(--color-ink-subtle)]"
                >
                    {segments}
                </nav>
            }
            .into_any(),
        )
    };
    let nota = article
        .nota
        .filter(|t| !t.trim().is_empty())
        .map(|n| {
            view! {
                <aside class="mt-2 rounded-md border-l-2 border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-3">
                    <p class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                        "Notes"
                    </p>
                    <div class="mt-2 whitespace-pre-line text-sm leading-relaxed text-[var(--color-ink-subtle)]">
                        {n}
                    </div>
                </aside>
            }
        });

    // Article modificatif (ordonnance/loi de réforme) : l'API a parsé le résumé de
    // liens LEGI en groupes liés (`modifications`), rendus au lieu du corps brut.
    // Corps normal : les renvois du texte sont cliquables.
    let body = if article.modifications.is_empty() {
        view! {
            <CitedBlock
                text=texte
                spans=article.texte_spans
                class="whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]"
            />
        }
        .into_any()
    } else {
        view! { <ModificationsList groups=article.modifications /> }.into_any()
    };

    view! {
        <article class="flex flex-col gap-4">
            {fil}
            {badge}
            {body}
            {original}
            {nota}
        </article>
    }
}

/// Liste des dispositions d'un article modificatif : un bloc par (action, code),
/// en-tête action + code lié, puis les cibles — articles en chips cliquables vers
/// `/norm/{juris}/{code}/{num}`, sections en libellés.
#[component]
fn ModificationsList(groups: Vec<ArticleModification>) -> impl IntoView {
    let blocks = groups
        .into_iter()
        .map(|g| view! { <ModificationGroup group=g /> })
        .collect_view();
    view! { <div class="flex flex-col gap-5">{blocks}</div> }
}

#[component]
fn ModificationGroup(group: ArticleModification) -> impl IntoView {
    let ArticleModification {
        action,
        code,
        code_href,
        items,
    } = group;
    let action = match action.as_str() {
        "modifie" => "Modifie".to_string(),
        "cree" => "Crée".to_string(),
        "abroge" => "Abroge".to_string(),
        _ => action,
    };
    let code_view = match code_href {
        Some(href) => view! {
            <A
                href=href
                attr:class="text-[var(--color-accent)] no-underline hover:underline"
            >
                {code}
            </A>
        }
        .into_any(),
        None => view! { <span>{code}</span> }.into_any(),
    };
    let items = items.into_iter().map(modification_item).collect_view();
    view! {
        <section class="flex flex-col gap-2">
            <p class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {action}" · "{code_view}
            </p>
            <div class="flex flex-wrap gap-x-3 gap-y-1.5">{items}</div>
        </section>
    }
}

/// Une cible : article en chip lié (numéro), section en libellé discret, texte
/// entier en libellé lié vers son sommaire.
fn modification_item(item: ModificationItem) -> AnyView {
    if item.kind == "section" {
        return view! {
            <span class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                {item.label}
            </span>
        }
        .into_any();
    }
    let label = if item.kind == "article" {
        format!("art. {}", item.label)
    } else {
        item.label
    };
    match item.href {
        Some(href) => view! {
            <A
                href=href
                attr:class="text-sm leading-relaxed text-[var(--color-accent)] no-underline hover:underline"
            >
                {label}
            </A>
        }
        .into_any(),
        None => view! {
            <span class="text-sm leading-relaxed text-[var(--color-ink)]">
                {label}
            </span>
        }
        .into_any(),
    }
}

/// Accordéon « Travaux parlementaires » : une ligne par loi modificatrice de
/// l'article, lien externe composé vers la page Légifrance de la loi au JO
/// (bloc « Travaux préparatoires » + dossiers législatifs). Rien si aucune loi
/// modificatrice.
fn travaux_section(refs: Vec<LinkedTextRef>) -> Option<AnyView> {
    if refs.is_empty() {
        return None;
    }
    let count = refs.len();
    let items = refs
        .into_iter()
        .map(|r| {
            view! {
                <li class="text-sm leading-relaxed">
                    <a
                        href=r.href.unwrap_or_default()
                        rel="external noopener"
                        target="_blank"
                        class="text-[var(--color-accent)] no-underline hover:underline"
                    >
                        {r.label}
                    </a>
                    <span class="text-[var(--color-ink-subtle)]">
                        " — travaux préparatoires sur Légifrance"
                    </span>
                </li>
            }
        })
        .collect_view();
    Some(
        view! {
            <details class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-3">
                <summary class="cursor-pointer text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    "Travaux parlementaires · "
                    {count}
                </summary>
                <ul class="mt-2 flex flex-col gap-1.5">{items}</ul>
            </details>
        }
        .into_any(),
    )
}

fn place_labels(places: &[PlaceRef]) -> String {
    places
        .iter()
        .map(|p| p.label.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The first three places by name, the rest counted.
fn place_summary(places: &[PlaceRef]) -> String {
    if places.len() <= 3 {
        return place_labels(places);
    }
    format!(
        "{} et {} autres lieux",
        place_labels(&places[..3]),
        places.len() - 3
    )
}

/// A clause cut on a word, an ellipsis saying so.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    let cut = head.rfind(' ').unwrap_or(head.len());
    format!("{}…", head[..cut].trim_end())
}

fn state_links(states: &[StateElsewhere], path: &str, with_version: bool) -> AnyView {
    let mut targets: Vec<(String, String)> = Vec::new();
    for e in states {
        for p in &e.places {
            let href = if with_version {
                format!("{path}/{}?place={}", e.version, p.uid)
            } else {
                format!("{path}?place={}", p.uid)
            };
            targets.push((href, p.label.clone()));
        }
    }
    targets
        .into_iter()
        .map(|(href, label)| {
            view! {
                <A href=href attr:class="underline underline-offset-4">
                    {label}
                </A>
            }
        })
        .collect_view()
        .into_any()
}

/// The line under the title that says where the served version applies, or the
/// banner that says what the reader's place makes of the article. Nothing while
/// the text has no compiled state.
fn applicability_view(article: &LawArticleResponse, dated: bool) -> Option<AnyView> {
    let a: &Applicability = article.applicability.as_ref()?;
    let when = if dated {
        "à cette date"
    } else {
        "aujourd'hui"
    };
    let key = article_key().get_untracked();
    let path = format!(
        "{}/{}",
        lj_vocab::norm_path(&article.jurisdiction, &article.code),
        article.num_key
    );
    let dated_path = match key.date.as_deref() {
        Some(d) => format!("{path}/{d}"),
        None => path.clone(),
    };
    let banner_class = "mt-1 w-fit rounded-md border border-[var(--color-accent)] bg-[var(--color-accent-soft)] px-3 py-1.5 text-sm text-[var(--color-ink)]";
    let from_reader = format!("Lu depuis {} : ", a.reader.label);
    let lead = match a.status {
        ApplicabilityStatus::Applicable => return Some(applicable_view(a, &path, when)),
        ApplicabilityStatus::NotApplicable => match a.frozen_on.as_deref() {
            Some(d) => format!(
                "{from_reader}cet article ne s'y applique pas : il n'était pas en vigueur le {}, jour où la matière est passée à la collectivité.",
                format_iso_date(Some(d))
            ),
            None => format!("{from_reader}cet article ne s'y applique pas {when}."),
        },
        ApplicabilityStatus::Undetermined(UndeterminedReason::NoMention) => format!(
            "{from_reader}une loi ne s'y applique que si un texte le dit, et aucune mention qui étende cet article n'a été relevée."
        ),
        ApplicabilityStatus::Undetermined(UndeterminedReason::Frozen) => format!(
            "{from_reader}la matière relève de la collectivité depuis le {}. Le texte national y reste dans sa rédaction de cette date tant que la collectivité ne l'a pas remplacé : voici cette rédaction.",
            format_iso_date(a.frozen_on.as_deref())
        ),
        ApplicabilityStatus::OutsideWindow => format!(
            "Aucun état connu de cet article ne couvre {} {when}.",
            a.reader.label
        ),
    };
    let lists_elsewhere = matches!(
        a.status,
        ApplicabilityStatus::NotApplicable
            | ApplicabilityStatus::Undetermined(UndeterminedReason::NoMention)
    );
    let elsewhere = (lists_elsewhere && !a.elsewhere.is_empty()).then(|| {
        let links = state_links(&a.elsewhere, &dated_path, false);
        view! {
            " Il s'applique en : "
            <span class="inline-flex flex-wrap gap-x-2">{links}</span>
        }
    });
    let rule = a.rule_source.as_deref().map(|s| {
        let short = shorten(s, 240);
        view! {
            <p class="text-xs text-[var(--color-ink-subtle)]">
                {format!("Selon la règle : « {short} »")}
            </p>
        }
    });
    Some(
        view! {
            <p class=banner_class>
                <span aria-hidden="true" class="mr-1.5">"⚠"</span>
                {lead}
                {elsewhere}
            </p>
            {rule}
        }
        .into_any(),
    )
}

/// The line of an article a state covers where the reader stands, the clause
/// that dates it, and the other states the source left covering them.
fn applicable_view(a: &Applicability, path: &str, when: &str) -> AnyView {
    let banner_class = "mt-1 w-fit rounded-md border border-[var(--color-accent)] bg-[var(--color-accent-soft)] px-3 py-1.5 text-sm text-[var(--color-ink)]";
    let from_reader = if a.reader.uid == DEFAULT_READER_PLACE {
        String::new()
    } else {
        format!("Lu depuis {} : ", a.reader.label)
    };
    let mut line = format!("{from_reader}S'applique en {}", place_summary(&a.places));
    match a.starts_on.as_deref() {
        Some(d) => line.push_str(&format!(" depuis le {}", format_iso_date(Some(d)))),
        None => line.push_str(" (début non daté par la source)"),
    }
    if let Some(f) = a.ends_on.as_deref() {
        line.push_str(&format!(
            ", cesse de s'appliquer le {}",
            format_iso_date(Some(f))
        ));
        if a.ends_on_origin.as_deref() == Some("computed") {
            line.push_str(" (fin déduite de l'état suivant)");
        }
    }
    let all_places = place_labels(&a.places);
    let clause = a
        .starts_on_source
        .as_deref()
        .or(a.ends_on_source.as_deref())
        .map(|c| {
            let short = shorten(c, 240);
            view! {
                <p class="text-xs text-[var(--color-ink-subtle)]">
                    {format!("Selon la source : « {short} »")}
                </p>
            }
        });
    let competing = (!a.competing.is_empty()).then(|| {
        let links = state_links(&a.competing, path, true);
        view! {
            <p class=banner_class>
                <span aria-hidden="true" class="mr-1.5">"⚠"</span>
                {format!("Plusieurs états couvrent {} {when} et la source ne les départage pas. S'applique aussi, dans une autre version : ", a.reader.label)}
                <span class="inline-flex flex-wrap gap-x-2">{links}</span>
            </p>
        }
    });
    view! {
        <p class="text-sm text-[var(--color-ink-muted)]" title=all_places>{line}</p>
        {clause}
        {competing}
    }
    .into_any()
}

/// The heading a place is listed under in « Lu depuis ».
pub(crate) fn place_group_label(group: PlaceGroup) -> &'static str {
    match group {
        PlaceGroup::Metropole => "Métropole",
        PlaceGroup::OverseasIdentity => "Outre-mer, identité législative",
        PlaceGroup::OverseasSpecialty => "Outre-mer, spécialité législative",
        PlaceGroup::OtherStates => "Autres États",
    }
}

/// The places the article can be read from, under their headings, in the
/// order the API sorts them.
fn readable_groups(readable: &[ReadablePlace]) -> Vec<(String, Vec<PlaceRef>)> {
    let mut groups: Vec<(PlaceGroup, Vec<PlaceRef>)> = Vec::new();
    for p in readable {
        match groups.last_mut() {
            Some((group, members)) if *group == p.group => members.push(p.place.clone()),
            _ => groups.push((p.group, vec![p.place.clone()])),
        }
    }
    groups
        .into_iter()
        .map(|(group, members)| (place_group_label(group).to_string(), members))
        .collect()
}

/// « Lu depuis » : a place picked under its heading, navigated as `?place=`
/// on `base` at the date. Nothing when no place is listed.
#[component]
pub fn PlacePicker(
    base: String,
    groups: Vec<(String, Vec<PlaceRef>)>,
    #[prop(into)] date: Signal<Option<String>>,
) -> impl IntoView {
    use leptos_router::hooks::use_navigate;
    if groups.is_empty() {
        return ().into_any();
    }
    let place = reader_place();
    let current = Signal::derive(move || {
        place
            .get()
            .unwrap_or_else(|| DEFAULT_READER_PLACE.to_string())
    });
    let groups = groups
        .into_iter()
        .map(|(label, members)| SelectGroup {
            label,
            options: members
                .into_iter()
                .map(|p| SelectOption {
                    value: p.uid,
                    label: p.label,
                })
                .collect(),
        })
        .collect();
    let navigate = use_navigate();
    let on_change = Callback::new(move |picked: String| {
        let path = match date.get_untracked() {
            Some(d) => format!("{base}/{d}"),
            None => base.clone(),
        };
        let href = if picked == DEFAULT_READER_PLACE {
            path
        } else {
            format!("{path}?place={picked}")
        };
        navigate(&href, Default::default());
    });
    view! {
        <div class="flex flex-col gap-1">
            <span class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "Lu depuis"
            </span>
            <GroupedSelect
                value=current
                on_change=on_change
                groups=groups
                aria_label="Lu depuis"
                filter_placeholder="Filtrer (ex. Mayotte…)"
                empty_text="Aucun lieu ne correspond."
            />
        </div>
    }
    .into_any()
}

/// The vocabulary as a select shows it: the French places under their
/// headings, a State with subdivisions as its own group, the other States
/// together. France itself is left out, its places splitting it whole.
pub(crate) fn place_groups(places: &[PlaceNode]) -> Vec<(String, Vec<PlaceRef>)> {
    let place_ref = |p: &PlaceNode| PlaceRef {
        uid: p.uid.clone(),
        label: p.label.clone(),
    };
    let mut headed: std::collections::BTreeMap<PlaceGroup, Vec<PlaceRef>> = Default::default();
    for p in places.iter().filter(|p| p.group != PlaceGroup::OtherStates) {
        headed.entry(p.group).or_default().push(place_ref(p));
    }
    let mut groups: Vec<(String, Vec<PlaceRef>)> = headed
        .into_iter()
        .map(|(group, members)| (place_group_label(group).to_string(), members))
        .collect();
    let mut states: Vec<PlaceRef> = Vec::new();
    for root in places
        .iter()
        .filter(|p| p.parent.is_none() && p.group == PlaceGroup::OtherStates)
    {
        let mut below: Vec<&PlaceNode> = Vec::new();
        let mut stack = vec![root.uid.as_str()];
        while let Some(parent) = stack.pop() {
            for child in places
                .iter()
                .filter(|p| p.parent.as_deref() == Some(parent))
            {
                below.push(child);
                stack.push(&child.uid);
            }
        }
        let own: Vec<PlaceRef> = below
            .iter()
            .filter(|p| p.group == PlaceGroup::OtherStates)
            .map(|p| place_ref(p))
            .collect();
        if below.is_empty() {
            states.push(place_ref(root));
        } else if !own.is_empty() {
            let mut members = vec![place_ref(root)];
            members.extend(own);
            groups.push((root.label.clone(), members));
        }
    }
    if !states.is_empty() {
        groups.push((
            place_group_label(PlaceGroup::OtherStates).to_string(),
            states,
        ));
    }
    groups
}

/// Bandeau temporel : « {sujet} sera modifié le … » — la sentinelle
/// `2222-02-22` (date inconnue DILA) devient « à une date à déterminer ».
pub(crate) fn upcoming_banner(subject: &str, date: &str) -> AnyView {
    let when = if date == "2222-02-22" {
        "à une date à déterminer".to_string()
    } else {
        format!("le {}", format_iso_date(Some(date)))
    };
    view! {
        <p class="mt-1 w-fit rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/60 px-3 py-1.5 text-sm text-[var(--color-ink-muted)]">
            <span aria-hidden="true" class="mr-1.5">"⏳"</span>
            {format!("{subject} {when}")}
        </p>
    }
    .into_any()
}

/// Fenêtre de validité de la version servie : bornée dès qu'elle a cessé de
/// s'appliquer, ouverte tant qu'elle court.
fn validity_label(date_debut: &str, date_fin: Option<&str>, expired: bool) -> String {
    let debut = format_iso_date(Some(date_debut));
    match date_fin.filter(|_| expired) {
        Some(fin) => format!("En vigueur du {debut} au {}", format_iso_date(Some(fin))),
        None => format!("En vigueur depuis le {debut}"),
    }
}

/// Bandeau : la version servie a cessé de s'appliquer.
fn expired_banner(subject: &'static str) -> AnyView {
    view! {
        <p class="mt-1 w-fit rounded-md border border-[var(--color-accent)] bg-[var(--color-accent-soft)] px-3 py-1.5 text-sm text-[var(--color-ink)]">
            <span aria-hidden="true" class="mr-1.5">"⚠"</span>
            {subject}
        </p>
    }
    .into_any()
}

/// Libellé FR d'un état DILA ; état inconnu rendu brut.
pub(crate) fn status_label(etat: &str) -> String {
    lj_dtos::article_status_label(etat)
        .map(str::to_string)
        .unwrap_or_else(|| etat.to_string())
}

/// Versions de l'article, du rail : chaque entrée lie vers `…/{date_debut}`,
/// la version servie au point plein.
#[component]
fn Timeline(article: LawArticleResponse) -> impl IntoView {
    if article.versions.is_empty() {
        return ().into_any();
    }
    let text = lj_vocab::norm_path(&article.jurisdiction, &article.code);
    let num = article.num_key.clone();
    let current = article.legiarti.clone();
    let suffix = reader_suffix(reader_place().get_untracked().as_deref());
    // Comparateur : version courante vs sa précédente (ou la suivante si la
    // courante ouvre la timeline), dès 2 versions.
    let compare = compare_href(&article.versions, &text, &num, &current).map(|href| {
        view! {
            <A
                href=href
                attr:class="mt-1 w-fit text-sm text-[var(--color-accent)] underline-offset-4 hover:underline"
            >
                "Comparer les versions"
            </A>
        }
    });
    let modified_by = article.modified_by;
    let rows = article
        .versions
        .into_iter()
        .map(|v| timeline_item(v, &text, &num, &current, &modified_by, &suffix))
        .collect();
    view! {
        <div class="flex flex-col gap-1">
            {rail_block("Versions", rows)}
            {compare}
        </div>
    }
    .into_any()
}

/// URL du comparateur pour la version courante face à sa voisine (précédente,
/// sinon suivante — la timeline est triée par `date_debut`). `None` s'il n'y a
/// qu'une version.
fn compare_href(
    versions: &[LawArticleVersion],
    text: &str,
    num: &str,
    current: &str,
) -> Option<String> {
    let idx = versions.iter().position(|v| v.legiarti == current)?;
    let other = if idx > 0 {
        versions.get(idx - 1)
    } else {
        versions.get(idx + 1)
    }?;
    let (from, to) = if idx > 0 {
        (other, &versions[idx])
    } else {
        (&versions[idx], other)
    };
    Some(format!(
        "{text}/{num}/compare/{}/{}",
        crate::pages::law_compare_page::version_url_key(from),
        crate::pages::law_compare_page::version_url_key(to),
    ))
}

fn timeline_item(
    version: LawArticleVersion,
    text: &str,
    num: &str,
    current: &str,
    modified_by: &[LinkedTextRef],
    suffix: &str,
) -> AnyView {
    let is_current = version.legiarti == current;
    // `date_debut` vide = sentinelle absorbée (article modificatif) : pas de
    // « depuis le … ».
    let span = match (version.date_debut.is_empty(), version.date_fin.as_deref()) {
        (true, _) => "Version initiale".to_string(),
        (false, Some(fin)) => format!(
            "{} – {}",
            format_iso_date(Some(&version.date_debut)),
            format_iso_date(Some(fin))
        ),
        (false, None) => format!("depuis le {}", format_iso_date(Some(&version.date_debut))),
    };
    // « à venir » : version pas encore entrée en vigueur, calculé côté API
    // (l'horloge vit côté serveur).
    let a_venir = version.upcoming.then(|| {
        view! {
            <span class="ml-1.5 rounded-sm border border-[var(--color-rule)] px-1 py-px text-[10px] uppercase tracking-wide text-[var(--color-ink-muted)]">
                "à venir"
            </span>
        }
    });
    let etat = view! {
        <span class="block text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
            {status_label(&version.etat)}
            {a_venir}
        </span>
    };
    if is_current {
        // Textes à l'origine de la version servie (liens entrants du graphe), portés
        // par l'entrée de frise.
        let sources = modified_by
            .iter()
            .map(|r| {
                let label = r.label.clone();
                let inner = match r.href.clone() {
                    Some(href) => view! {
                        <A
                            href=href
                            attr:class="text-[var(--color-accent)] no-underline hover:underline"
                        >
                            {label}
                        </A>
                    }
                    .into_any(),
                    None => view! { <span>{label}</span> }.into_any(),
                };
                view! {
                    <span class="block text-xs leading-snug text-[var(--color-ink-muted)]">
                        "par "
                        {inner}
                    </span>
                }
            })
            .collect_view();
        let body = view! {
            <span aria-current="true" class="block">
                <span class="block text-sm font-medium leading-snug text-[var(--color-ink)]">
                    {span}
                </span>
                {etat}
                {sources}
            </span>
        }
        .into_any();
        return rail_item(true, body);
    }
    let href = format!("{text}/{num}/{}{suffix}", version.date_debut);
    let body = view! {
        <A href=href attr:class="group block no-underline">
            <span class="block text-sm leading-snug text-[var(--color-accent)] group-hover:underline">
                {span}
            </span>
            {etat}
        </A>
    }
    .into_any();
    rail_item(false, body)
}

/// Colonne droite : rebondir vers un autre article du corpus — les voisins du
/// chapitre puis les articles co-cités (streamés). Aucun contenu porteur : ce qui
/// cite l'article vit au centre.
#[component]
fn ArticleNeighbors(
    article: LawArticleResponse,
    related: Resource<RelatedResult>,
) -> impl IntoView {
    view! {
        <aside
            aria-label="Articles liés"
            class="flex flex-col gap-4 lg:sticky lg:top-20 lg:self-start"
        >
            <ArticleContext article=article />
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move { related_view(related.await) })}
            </Suspense>
        </aside>
    }
}

/// Bloc « Souvent cité avec » : articles co-cités dans les décisions citantes,
/// boilerplate procédural exclu côté repo, plafonné à `RELATED_LIMIT`. Rien à
/// afficher = pas de bloc.
fn related_view(resolved: RelatedResult) -> AnyView {
    if resolved.items.is_empty() {
        return ().into_any();
    }
    let items = resolved
        .items
        .into_iter()
        .take(RELATED_LIMIT)
        .map(related_item)
        .collect_view();
    view! {
        <h2 class="font-sans text-base text-[var(--color-ink)]">"Souvent cité avec"</h2>
        <ul class="flex flex-col gap-2">{items}</ul>
    }
    .into_any()
}

fn related_item(item: CoCitedArticle) -> AnyView {
    use crate::components::hover_preview::{preview_kind, HoverPreview, PreviewKind};
    let label = format!("Article {}", format_article_num(&item.num_key));
    let count = view! {
        <span class="shrink-0 text-xs tabular-nums text-[var(--color-ink-subtle)]">
            {format!("{} déc.", item.count)}
        </span>
    };
    let title = view! {
        <span class="line-clamp-2 text-xs text-[var(--color-ink-subtle)]">
            {item.text_title}
        </span>
    };
    let Some(href) = item.href else {
        return view! {
            <li class="flex items-baseline justify-between gap-2">
                <span class="text-sm text-[var(--color-ink)]">{label} {title}</span>
                {count}
            </li>
        }
        .into_any();
    };
    let place = reader_place().get_untracked();
    let mut kind = preview_kind(&href, None);
    if let Some(PreviewKind::Article { place: p, .. }) = kind.as_mut() {
        *p = place.clone();
    }
    let href = format!("{href}{}", reader_suffix(place.as_deref()));
    let lien = view! {
        <A
            href=href
            attr:class="text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
        >
            {label}
        </A>
        {title}
    };
    let corps = match kind {
        Some(kind) => view! { <HoverPreview kind=kind>{lien}</HoverPreview> }.into_any(),
        None => lien.into_any(),
    };
    view! {
        <li class="flex items-baseline justify-between gap-2">{corps}{count}</li>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::validity_label;

    /// KALI laisse `status` à `VIGUEUR` sur des versions dépassées : c'est la
    /// fenêtre, et elle seule, qui dit ce qui s'applique.
    #[test]
    fn une_fenetre_close_ne_sannonce_pas_en_vigueur() {
        assert_eq!(
            validity_label("2010-06-01", Some("2010-07-19"), true),
            "En vigueur du 1 juin 2010 au 19 juillet 2010"
        );
        assert_eq!(
            validity_label("2010-06-01", Some("2027-12-31"), false),
            "En vigueur depuis le 1 juin 2010"
        );
        assert_eq!(
            validity_label("2010-06-01", None, false),
            "En vigueur depuis le 1 juin 2010"
        );
    }
}
