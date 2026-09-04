//! Référentiel de droit positif — law-at-date, timeline, citations, sommaire de
//! code.
//!
//! Handlers purs `(&AppState, …) -> Result<DTO>` (même forme que
//! [`crate::decisions`]) ; l'adaptation axum (path/query, headers de cache) vit
//! dans [`crate::routes`]. Le `code` d'URL est résolu en `text_uid` par lookup
//! **exact** sur le slug ([`DecisionRepository::referential_ref`], caché in-process) ; rien
//! ne matche ⇒ 404 ([`ApiError::NotFound`]). Le `num` d'URL **est déjà** la clé
//! canonique (`num_key`) : nos liens internes la portent (DTO `numKey`), aucune
//! normalisation au runtime serve.

use chrono::{Datelike, NaiveDate};
use lj_core::source_authority::{is_live_authoritative, source_authority};
use lj_dtos::{
    ArticleModification, ArticleNeighbor, ArticleSearchFacets, ArticleSearchHit,
    ArticleSearchResponse, CitationSpan, CitationTarget, CitingDecisionHit, CoCitedArticle,
    CodeCatalogueEntry, CodeCatalogueResponse, CodeTocResponse, Commentaire, FacetChoice,
    JurisdictionType, LawArticleResponse, LawArticleVersion, LawCodeSummary, LawCompareOp,
    LawCompareResponse, LawCompareRow, LawCompareRowKind, LawCompareSegment, LawSectionItem,
    LawSectionRef, LawSectionResponse, LinkedTextRef, ModificationItem, SectionCitingText,
    TocEntry, TocNode, TopCitedArticle,
};
use lj_store::repository::{
    ArticleNeighborRow, ArticleSearchRow, ArticleSearchStats, CitingDecisionRow,
    DecisionRepository, FacetCount, LawVersionRow, LegalArticleRow, LegalTextCatalogRow,
    ResolvedLegalLink, TocArticleRow, TocReadingRow, TocTreeRow,
};
use std::sync::Arc;
use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::state::AppState;
use crate::titles::decision_title;

/// URL Légifrance absolue et versionnée d'un article **natif** Légifrance
/// (`source_uid` = LEGIARTI…) : `/codes/article_lc/{LEGIARTI}/{date}`, `date` ISO
/// `YYYY-MM-DD` étant la date de consultation (date demandée pour la
/// version-à-date, sinon `date_debut` de la version servie). `None` pour une
/// source curée (droit étranger, traités : `source_uid` synthétique
/// `text_uid#…`) qui porte déjà sa propre `source_url`. Sert à *remplir*
/// `source_url` quand elle est absente.
fn legifrance_source_url(source_uid: &str, date: &str) -> Option<String> {
    if source_uid.contains('#') {
        return None;
    }
    Some(format!(
        "https://www.legifrance.gouv.fr/codes/article_lc/{source_uid}/{date}"
    ))
}

/// Rend une `date_debut` en ISO `YYYY-MM-DD`, en mappant la sentinelle de borne
/// ouverte (`0001-01-01`, posée par l'upsert pour les versions sans début connu)
/// vers la chaîne vide (DTO `dateDebut` vide = pas de début). Un `None` est traité
/// de même.
fn date_debut_display(date_debut: Option<NaiveDate>) -> String {
    match date_debut {
        Some(d) if d.year() != 1 => d.to_string(),
        _ => String::new(),
    }
}

/// Date de début pour l'AFFICHAGE (DTO `dateDebut`) : comme [`date_debut_display`]
/// mais absorbe aussi la sentinelle LEGI `2999-01-01`, posée sur `date_debut` des
/// articles **modificatifs** sans date propre. L'URL Légifrance garde 2999 (elle
/// la résout) via la `consult_date`, calculée à part par [`date_debut_display`].
fn display_start_date(date_debut: Option<NaiveDate>) -> String {
    match date_debut {
        Some(d) if d.year() != 1 && d.year() < 2999 => d.to_string(),
        _ => String::new(),
    }
}

/// Partie « nom du texte » d'un libellé de lien DILA (« Code civil - art. 1302
/// (V) » → « Code civil »).
fn label_prefix(label: &str) -> &str {
    label.split(" - ").next().unwrap_or(label).trim()
}

/// Partie « cible » d'un libellé de lien DILA (« Code civil - Chapitre III :
/// Les autres sources » → « Chapitre III : Les autres sources »).
fn label_suffix(label: &str) -> &str {
    label
        .split_once(" - ")
        .map(|(_, rest)| rest.trim())
        .unwrap_or(label)
}

/// Une arête du graphe en référence liée : lien article garanti quand la cible
/// est résolue, ancre de section au sommaire (`#{cid}`) pour une cible section,
/// sinon sommaire du texte porteur, sinon libellé nu.
fn linked_text_ref(l: ResolvedLegalLink) -> LinkedTextRef {
    let href = match (l.resolved_slug, l.resolved_num_key) {
        (Some(slug), Some(num_key)) => Some(format!("/texte/{slug}/{num_key}")),
        _ => match (&l.target_text_slug, &l.resolved_section_cid) {
            (Some(slug), Some(cid)) => Some(format!("/texte/{slug}#{cid}")),
            _ => l.target_text_slug.map(|s| format!("/texte/{s}")),
        },
    };
    LinkedTextRef {
        label: l.target_label,
        href,
        date: l.target_date.map(|d| d.to_string()),
    }
}

/// Dérive les blocs de la page article depuis ses arêtes `legal_link` :
/// dispositions modifiées (sortant modifie/crée/abroge, groupé **globalement**
/// par action × texte cible dans l'ordre de première apparition — le XML
/// entrelace les actions section par section), « Modifié par » (entrant
/// modificatoire, trié par date), « Cité par » (CITATION entrant, dédupliqué —
/// la réciprocité DILA peut répéter une cible). Le sortant CITATION n'est pas
/// rendu : la citation se lit dans le texte, sur son span.
fn link_blocks(
    links: Vec<ResolvedLegalLink>,
) -> (
    Vec<ArticleModification>,
    Vec<LinkedTextRef>,
    Vec<LinkedTextRef>,
) {
    let mut modifications: Vec<ArticleModification> = Vec::new();
    let mut group_index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    let mut modified_by: Vec<LinkedTextRef> = Vec::new();
    let mut cited_by: Vec<LinkedTextRef> = Vec::new();
    for l in links {
        if l.target_label.is_empty() {
            continue;
        }
        let incoming = l.direction == "incoming";
        match l.verb.as_str() {
            "modifie" | "cree" | "abroge" if !incoming => {
                let action = l.verb.clone();
                let code = l
                    .target_text_title
                    .clone()
                    .unwrap_or_else(|| label_prefix(&l.target_label).to_string());
                let code_href = l.target_text_slug.clone().map(|s| format!("/texte/{s}"));
                let item = match l.target_kind.as_str() {
                    "article" => ModificationItem {
                        href: match (&l.resolved_slug, &l.resolved_num_key) {
                            (Some(slug), Some(num_key)) => Some(format!("/texte/{slug}/{num_key}")),
                            _ => None,
                        },
                        kind: "article".to_string(),
                        label: l
                            .target_num
                            .unwrap_or_else(|| label_suffix(&l.target_label).to_string()),
                    },
                    // Cible section : ancre au sommaire du texte porteur quand
                    // le cid est résolu via l'arbre structurel.
                    "section" => ModificationItem {
                        kind: "section".to_string(),
                        label: label_suffix(&l.target_label).to_string(),
                        href: match (&l.target_text_slug, &l.resolved_section_cid) {
                            (Some(slug), Some(cid)) => Some(format!("/texte/{slug}#{cid}")),
                            _ => None,
                        },
                    },
                    _ => ModificationItem {
                        kind: "texte".to_string(),
                        label: l.target_label.clone(),
                        href: l.target_text_slug.map(|s| format!("/texte/{s}")),
                    },
                };
                match group_index.get(&(action.clone(), code.clone())) {
                    Some(&i) => modifications[i].items.push(item),
                    None => {
                        group_index.insert((action.clone(), code.clone()), modifications.len());
                        modifications.push(ArticleModification {
                            action,
                            code,
                            code_href,
                            items: vec![item],
                        });
                    }
                }
            }
            "modifie" | "cree" | "abroge" => modified_by.push(linked_text_ref(l)),
            "cite" if !incoming => {}
            "cite" => cited_by.push(linked_text_ref(l)),
            // codification, concordance, transfert… : stockés, non exposés.
            _ => {}
        }
    }
    // « Modifié par » chronologique (ISO trie lexicalement) ; dates absentes en fin.
    modified_by.sort_by(|a, b| match (&a.date, &b.date) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    for list in [&mut modified_by, &mut cited_by] {
        let mut seen = std::collections::HashSet::new();
        list.retain(|r| seen.insert((r.label.clone(), r.href.clone())));
    }
    (modifications, modified_by, cited_by)
}

/// Convertit une ligne de timeline du repo en DTO version (dates ISO telles
/// quelles). `dateDebut` du DTO est requise : une borne ouverte (sentinelle
/// `0001-01-01` posée par l'upsert, ou source non versionnée) est rendue en
/// chaîne vide ; `dateFin` absente ⇒ pas de fin. `source_uid` = identifiant natif
/// de la version (LEGIARTI…).
fn version_dto(row: LawVersionRow) -> LawArticleVersion {
    // Sentinelles LEGI de borne ouverte / date absente : `0001-01-01` (borne
    // ouverte upsert) et `2999-01-01` (article modificatif sans date propre).
    let date_debut = match row.date_debut {
        Some(d) if d != "0001-01-01" && d != "2999-01-01" => d,
        _ => String::new(),
    };
    // Version future = pas encore entrée en vigueur. Comparaison lexicale ISO ;
    // la sentinelle 2999 est déjà absorbée ci-dessus, 2222 (date inconnue) reste
    // marquée « à venir ».
    let upcoming =
        !date_debut.is_empty() && date_debut > chrono::Utc::now().date_naive().to_string();
    LawArticleVersion {
        legiarti: row.source_uid,
        etat: row.status,
        date_debut,
        date_fin: row.date_fin,
        upcoming,
    }
}

/// Assemble la réponse article (version servie + timeline) à partir d'une ligne
/// `legal_article` et de la timeline. `consult_date` est la date de consultation
/// pour l'URL Légifrance (date demandée, sinon `date_debut` de la version, sinon
/// chaîne vide pour une borne ouverte). `legiarti` = identifiant natif de la
/// version (`source_uid`, LEGIARTI…) ; `legitext` = identité du texte
/// (`text_uid`).
#[allow(clippy::too_many_arguments)]
async fn article_response(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    text: &TextRef,
    article: LegalArticleRow,
    consult_date: &str,
    href_date: Option<NaiveDate>,
    versions: Vec<LawVersionRow>,
    context: Vec<ArticleNeighborRow>,
) -> Result<LawArticleResponse> {
    let links = repo
        .article_links(&article.text_uid, &article.num_key, article.date_debut)
        .await?;
    // Renvois du corps : spans de la version servie, hrefs internes — datés quand
    // la lecture l'est (`href_date` = date demandée de la route). Article ciblé →
    // `/texte/{slug}/{numKey}`, mention nue → sommaire seulement si le texte a des
    // articles, sinon pas de span du tout.
    let texte_spans: Vec<CitationSpan> = repo
        .article_citation_spans(
            &article.text_uid,
            &article.num_key,
            // La borne ouverte est `None` dans la ligne lue mais stockée
            // sentinelle `0001-01-01` côté `text_legal_citation` (clé owner).
            article
                .date_debut
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(1, 1, 1).unwrap()),
        )
        .await?
        .into_iter()
        .filter_map(|s| {
            let slug = s.ref_slug?;
            let href = match s.ref_num_key.as_deref().filter(|k| !k.is_empty()) {
                Some(k) => match href_date {
                    Some(d) => format!("/texte/{slug}/{k}/{d}"),
                    None => format!("/texte/{slug}/{k}"),
                },
                None if s.ref_readable => match href_date {
                    Some(d) => format!("/texte/{slug}?date={d}"),
                    None => format!("/texte/{slug}"),
                },
                None => return None,
            };
            Some(CitationSpan {
                start: s.char_start.max(0) as usize,
                end: s.char_end.max(0) as usize,
                targets: vec![CitationTarget {
                    href: Some(href),
                    label: s.ref_title,
                }],
            })
        })
        .collect();
    let travaux_parlementaires = travaux_refs(&links);
    let (modifications, modified_by, cited_by) = link_blocks(links);
    // Commentaires de norme : bundles ancrés sur (texte, article) et sur le texte
    // entier, forme `commentaires[]` partagée avec la décision. Ancre = le slug
    // canonique : la clé que la page porte, pas l'identité physique du texte —
    // celle-ci bouge en source.
    let commentaires = match repo
        .article_commentaires(&text.slug, &article.num_key)
        .await?
    {
        Some(v) => serde_json::from_value::<Vec<Commentaire>>(v)
            .map_err(|e| ApiError::Internal(format!("commentaires invalides: {e}")))?,
        None => Vec::new(),
    };
    let date_debut = display_start_date(article.date_debut);
    let expired = article
        .date_fin
        .is_some_and(|f| f < chrono::Utc::now().date_naive());
    let date_fin = article.date_fin.map(|d| d.to_string());
    // Lien « source » unique : l'URL curée si présente (droit étranger, traités),
    // sinon — pour un article natif Légifrance — la page Légifrance versionnée.
    let source_url = article
        .source_url
        .clone()
        .or_else(|| legifrance_source_url(&article.source_uid, consult_date));
    // Fraîcheur effective : `source_asof` par ligne (curé / bulk / jafbase), sinon
    // — pour les sources live re-synchronisées (legifrance/kali) — la date du
    // dernier sync (`ingest_freshness`). Axe distinct de `translation`.
    let source_asof = match article.source_asof {
        Some(d) => Some(d.to_string()),
        None if is_live_authoritative(&article.source) => {
            ingest_freshness(state, repo, &article.source)
                .await?
                .map(|d| d.to_string())
        }
        None => None,
    };
    let authority = source_authority(&article.source);
    // Titre humain du texte pour l'en-tête « Article N du … » : le `code` d'URL est un
    // slug. Porté par la résolution du slug — même ligne `legal_text`, déjà lue.
    let code_title = text.title.clone();
    // Frise + bandeau « sera modifié le … » : versions converties une fois,
    // première date future extraite.
    let versions: Vec<LawArticleVersion> = versions.into_iter().map(version_dto).collect();
    let upcoming_version_date = versions
        .iter()
        .filter(|v| v.upcoming)
        .map(|v| v.date_debut.clone())
        .min();
    // Fil d'Ariane TOC cliquable : divisions enclosantes de la version servie,
    // hrefs vers la vue-lecture de section. Vide hors TOC (JORF, étranger) — le
    // front retombe sur `titre_text`.
    let breadcrumb = repo
        .article_toc_breadcrumb(&article.text_uid, &article.source_uid)
        .await?
        .into_iter()
        .map(|(label, cid)| LinkedTextRef {
            label,
            href: cid.map(|c| format!("/texte/{}/section/{c}", text.slug)),
            date: None,
        })
        .collect();
    Ok(LawArticleResponse {
        legiarti: article.source_uid,
        legitext: article.text_uid,
        code: text.slug.clone(),
        code_title,
        num: article.num,
        num_key: article.num_key,
        etat: article.status,
        date_debut,
        source: article.source,
        titre_text: article.title_path,
        breadcrumb,
        date_fin,
        expired,
        texte: article.texte,
        texte_spans,
        texte_original: article.texte_original,
        lang_original: article.lang_original,
        translation: article.translation,
        source_asof,
        source_authority: authority.label().to_string(),
        source_official: authority.is_official(),
        source_url,
        source_upstream_url: article.source_upstream_url,
        nota: article.nota,
        upcoming_version_date,
        versions,
        context: context.into_iter().map(neighbor_dto).collect(),
        modifications,
        modified_by,
        cited_by,
        commentaires,
        travaux_parlementaires,
    })
}

/// Travaux parlementaires : une ligne par **loi** modificatrice de l'article
/// (arêtes entrantes modifie/crée/abroge, nature LOI, texte JORFTEXT),
/// dédupliquée par texte, triée par date décroissante. `href` composé vers la
/// page Légifrance de la loi au JO, qui porte le bloc « Travaux préparatoires »
/// et les liens de dossiers législatifs.
fn travaux_refs(links: &[ResolvedLegalLink]) -> Vec<LinkedTextRef> {
    let mut seen = std::collections::HashSet::new();
    let mut refs: Vec<LinkedTextRef> = links
        .iter()
        .filter(|l| {
            l.direction == "incoming"
                && matches!(l.verb.as_str(), "modifie" | "cree" | "abroge")
                && l.target_nature.as_deref() == Some("LOI")
        })
        .filter_map(|l| {
            let uid = l.target_text_uid.as_deref()?;
            if !uid.starts_with("JORFTEXT") || !seen.insert(uid.to_string()) {
                return None;
            }
            Some(LinkedTextRef {
                // Le libellé DILA pointe l'article de la loi (« LOI n°2018-287
                // du 20 avril 2018 - art. 16 ») ; la ligne vise la loi.
                label: label_prefix(&l.target_label).to_string(),
                href: Some(format!("https://www.legifrance.gouv.fr/jorf/id/{uid}")),
                date: l.target_date.map(|d| d.to_string()),
            })
        })
        .collect();
    refs.sort_by(|a, b| b.date.cmp(&a.date));
    refs
}

fn neighbor_dto(row: ArticleNeighborRow) -> ArticleNeighbor {
    ArticleNeighbor {
        num: row.num,
        num_key: row.num_key,
        etat: row.status,
        current: row.current,
    }
}

/// Identité d'un texte de référentiel résolue depuis son slug d'URL : le
/// `text_uid` qui clé toutes les requêtes en aval, le **slug canonique** que
/// portent les DTO, et le titre humain de l'en-tête. Valeur du cache
/// [`AppState::text_ref_cache`], partagée par `Arc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRef {
    pub text_uid: String,
    pub slug: String,
    pub title: Option<String>,
}

/// Résout le `code` d'URL en identité de texte de référentiel. Le lookup suit
/// les renommages de slug : le DTO rend alors le slug d'aujourd'hui,
/// et la page SSR, qui lit l'écart avec l'URL demandée, répond 301. 404 si le
/// slug n'est ni vivant ni redirigé.
///
/// Servi depuis le cache in-process (TTL 1 h) : le référentiel ne bouge qu'à
/// l'ingest.
async fn resolve_text(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    code: &str,
) -> Result<Arc<TextRef>> {
    if let Some(hit) = state.text_ref_cache.get(code).await {
        return hit.ok_or(ApiError::NotFound);
    }
    let found = repo
        .referential_ref(code)
        .await?
        .map(|(text_uid, slug, title)| {
            Arc::new(TextRef {
                text_uid,
                slug,
                title,
            })
        });
    state
        .text_ref_cache
        .insert(code.to_string(), found.clone())
        .await;
    found.ok_or(ApiError::NotFound)
}

/// Fraîcheur « as-of » d'une source live, servie depuis le cache in-process
/// (TTL 1 h).
async fn ingest_freshness(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    source: &str,
) -> Result<Option<NaiveDate>> {
    if let Some(hit) = state.ingest_freshness_cache.get(source).await {
        return Ok(hit);
    }
    let asof = repo.get_ingest_freshness(source).await?;
    state
        .ingest_freshness_cache
        .insert(source.to_string(), asof)
        .await;
    Ok(asof)
}

/// Version d'un article à une date, `None` valant « en vigueur », clé compacte
/// rattrapée (`l80a` pour `l80-a`) et motif à la place du `not_found` muet.
///
/// Rend la clé servie : timeline, contexte et URL doivent porter la même, sinon
/// la réponse mélange deux articles.
async fn served_version(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    num_key: &str,
    date: Option<NaiveDate>,
) -> Result<(String, LegalArticleRow)> {
    if let Some(article) = repo.law_article_at_date(text_uid, num_key, date).await? {
        return Ok((num_key.to_string(), article));
    }
    let separee = lj_core::article_key::letter_suffix_separated(num_key);
    if let Some(cle) = &separee {
        if let Some(article) = repo.law_article_at_date(text_uid, cle, date).await? {
            tracing::info!(demande = num_key, servi = %cle, "article : clé rattrapée");
            return Ok((cle.clone(), article));
        }
    }
    if let Some(cle) = lj_core::article_key::without_spoken_word(num_key) {
        if let Some(article) = repo.law_article_at_date(text_uid, cle, date).await? {
            tracing::info!(demande = num_key, servi = %cle, "article : clé rattrapée");
            return Ok((cle.to_string(), article));
        }
    }
    let mut cle = num_key.to_string();
    let mut versions = repo.law_article_versions(text_uid, num_key).await?;
    if versions.is_empty() {
        if let Some(c) = &separee {
            versions = repo.law_article_versions(text_uid, c).await?;
            cle = c.clone();
        }
    }
    // Asymétrie voulue : après la dernière fenêtre, le texte tel qu'il a cessé de
    // s'appliquer répond. Avant la première, servir la plus ancienne version
    // l'appliquerait à des faits qu'elle ne régissait pas, donc erreur.
    if let (Some(d), Some(fin)) = (date, derniere_fin(&versions)) {
        if d > fin {
            if let Some(article) = repo
                .law_article_last_version_before(text_uid, &cle, d)
                .await?
            {
                return Ok((cle, article));
            }
        }
    }
    Err(no_version_error(&versions, date))
}

/// Fin de la dernière fenêtre de validité, `None` si une version court toujours
/// ou si aucune n'est bornée.
fn derniere_fin(versions: &[LawVersionRow]) -> Option<NaiveDate> {
    if versions.iter().any(|v| v.date_fin.is_none()) {
        return None;
    }
    versions
        .iter()
        .filter_map(|v| v.date_fin.as_deref())
        .filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .max()
}

/// Motif d'une lecture qui ne rend rien, à partir du seul jeu de versions.
///
/// Une version `VIGUEUR*` sans couverture de la date est une incohérence de
/// notre stock, pas une abrogation : sur les 8 échecs mesurés de la semaine du
/// 22/08/2026, 5 étaient de cette forme. Le message n'y affirme donc rien du
/// droit.
fn no_version_error(versions: &[LawVersionRow], date: Option<NaiveDate>) -> ApiError {
    if versions.is_empty() {
        return ApiError::NotFound;
    }
    let debut = versions
        .iter()
        .filter_map(|v| v.date_debut.as_deref())
        .min();
    let fin = versions.iter().filter_map(|v| v.date_fin.as_deref()).max();
    if versions.iter().any(|v| v.status.starts_with("VIGUEUR")) {
        // Date DANS l'intervalle couvert : le trou est chez nous. Hors bornes, il
        // n'y a rien à réparer, et la lecture en vigueur n'arrive jamais ici,
        // faute de test de fenêtre.
        if let (Some(d), Some(a), Some(b)) = (date.map(|d| d.to_string()), debut, fin) {
            if d.as_str() >= a && d.as_str() <= b {
                tracing::warn!(
                    date = %d,
                    debut = a,
                    fin = b,
                    "article : version en vigueur sans couverture"
                );
            }
        }
        return ApiError::NoVersionAtDate(match (debut, fin) {
            (Some(a), Some(b)) => format!(
                "law article exists but no version covers that date \
                 (known versions span {a} to {b})"
            ),
            _ => "law article exists but no version covers that date".to_string(),
        });
    }
    ApiError::NoVersionAtDate(match fin {
        Some(b) => format!(
            "law article repealed: no version in force at that date, the last one ran to {b}"
        ),
        None => "law article repealed: no version in force at that date".to_string(),
    })
}

/// Version en vigueur d'un article (`GET /api/texte/{code}/{num}`), avec sa
/// timeline. 404 si le code ou l'article n'est pas en vigueur.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num))]
pub async fn article_in_force(
    state: &AppState,
    code: &str,
    num: &str,
) -> Result<LawArticleResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_core::article_key::identity_key(num);

    let (num_key, article) = served_version(&repo, text_uid, num_key, None).await?;
    let consult_date = date_debut_display(article.date_debut);
    let versions = repo.law_article_versions(text_uid, &num_key).await?;
    let context = repo.article_context(text_uid, &num_key, None).await?;
    article_response(
        state,
        &repo,
        &text,
        article,
        &consult_date,
        None,
        versions,
        context,
    )
    .await
}

/// Version d'un article à une date (`GET /api/texte/{code}/{num}/{date}`), avec sa
/// timeline. 404 si le code est inconnu ou si aucune version ne couvre la date.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, date = %date))]
pub async fn article_at_date(
    state: &AppState,
    code: &str,
    num: &str,
    date: NaiveDate,
) -> Result<LawArticleResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_core::article_key::identity_key(num);

    let (num_key, article) = served_version(&repo, text_uid, num_key, Some(date)).await?;
    let consult_date = date.to_string();
    let versions = repo.law_article_versions(text_uid, &num_key).await?;
    let context = repo.article_context(text_uid, &num_key, Some(date)).await?;
    article_response(
        state,
        &repo,
        &text,
        article,
        &consult_date,
        Some(date),
        versions,
        context,
    )
    .await
}

/// Variante de [`article_at_date`] acceptant une date ISO `YYYY-MM-DD` en
/// chaîne, pour les appelants in-process (client SSR `lj-web`) qui ne
/// manipulent pas `chrono`. Format invalide ⇒ 422
/// ([`ApiError::Unprocessable`]).
pub async fn article_at_date_str(
    state: &AppState,
    code: &str,
    num: &str,
    date: &str,
) -> Result<LawArticleResponse> {
    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(crate::error::validation::date_parsing(
            &["path", "date"],
            date,
            "invalid date",
        ))
    })?;
    article_at_date(state, code, num, parsed).await
}

/// Parse la date Chronolégi optionnelle des appelants in-process (client SSR,
/// qui ne manipule pas `chrono`). Format invalide ⇒ 422.
fn parse_opt_date(date: Option<&str>) -> Result<Option<NaiveDate>> {
    date.map(|d| {
        NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| {
            ApiError::Unprocessable(crate::error::validation::date_parsing(
                &["query", "date"],
                d,
                "invalid date",
            ))
        })
    })
    .transpose()
}

/// Variante de [`code_toc`] à date ISO chaîne optionnelle (client SSR).
pub async fn code_toc_str(
    state: &AppState,
    code: &str,
    date: Option<&str>,
) -> Result<CodeTocResponse> {
    code_toc(state, code, parse_opt_date(date)?).await
}

/// Variante de [`law_section`] à date ISO chaîne optionnelle (client SSR).
pub async fn law_section_str(
    state: &AppState,
    code: &str,
    cid: &str,
    date: Option<&str>,
) -> Result<LawSectionResponse> {
    law_section(state, code, cid, parse_opt_date(date)?).await
}

/// Borne du comparateur de versions : date ISO tombant dans la fenêtre de la
/// version visée, ou `initiale` pour la version à borne ouverte (sentinelle
/// `0001-01-01`). Format invalide ⇒ 422.
fn parse_compare_bound(raw: &str, segment: &'static str) -> Result<NaiveDate> {
    if raw == "initiale" {
        return Ok(NaiveDate::from_ymd_opt(1, 1, 1).expect("date sentinelle"));
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(crate::error::validation::date_parsing(
            &["path", segment],
            raw,
            "invalid date",
        ))
    })
}

/// Métadonnées de version d'une ligne `legal_article` servie, avec la même
/// normalisation de sentinelles que la timeline ([`version_dto`]).
fn row_version_dto(row: &LegalArticleRow) -> LawArticleVersion {
    version_dto(LawVersionRow {
        source_uid: row.source_uid.clone(),
        status: row.status.clone(),
        date_debut: row.date_debut.map(|d| d.to_string()),
        date_fin: row.date_fin.map(|d| d.to_string()),
    })
}

/// Comparaison de deux rédactions d'un article :
/// `GET /api/texte/{code}/{num}/compare/{de}/{a}`. Chaque borne résout sa version
/// par la fenêtre de dates (law-at-date) ; 404 si le code est inconnu ou si une
/// borne ne couvre aucune version. Le diff est calculé côté serveur
/// (`lj_core::compare`, mot Unicode dans les blocs remplacés).
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, de = %de, a = %a))]
pub async fn article_compare(
    state: &AppState,
    code: &str,
    num: &str,
    de: &str,
    a: &str,
) -> Result<LawCompareResponse> {
    let from_date = parse_compare_bound(de, "de")?;
    let to_date = parse_compare_bound(a, "a")?;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_core::article_key::identity_key(num);

    let from_row = repo
        .law_article_at_date(text_uid, num_key, Some(from_date))
        .await?
        .ok_or(ApiError::NotFound)?;
    let to_row = repo
        .law_article_at_date(text_uid, num_key, Some(to_date))
        .await?
        .ok_or(ApiError::NotFound)?;

    let rows = lj_core::compare::align_texts(
        from_row.texte.as_deref().unwrap_or(""),
        to_row.texte.as_deref().unwrap_or(""),
    )
    .into_iter()
    .map(|r| LawCompareRow {
        kind: match r.kind {
            lj_core::compare::RowKind::Equal => LawCompareRowKind::Equal,
            lj_core::compare::RowKind::Changed => LawCompareRowKind::Changed,
            lj_core::compare::RowKind::Added => LawCompareRowKind::Added,
            lj_core::compare::RowKind::Removed => LawCompareRowKind::Removed,
        },
        left: r.left.into_iter().map(compare_segment_dto).collect(),
        right: r.right.into_iter().map(compare_segment_dto).collect(),
    })
    .collect();

    let versions = repo.law_article_versions(text_uid, num_key).await?;
    Ok(LawCompareResponse {
        code: text.slug.clone(),
        code_title: text.title.clone(),
        num: to_row.num.clone(),
        num_key: to_row.num_key.clone(),
        from: row_version_dto(&from_row),
        to: row_version_dto(&to_row),
        rows,
        versions: versions.into_iter().map(version_dto).collect(),
    })
}

fn compare_segment_dto(segment: lj_core::compare::CompareSegment) -> LawCompareSegment {
    LawCompareSegment {
        op: match segment.op {
            lj_core::compare::CompareOp::Equal => LawCompareOp::Equal,
            lj_core::compare::CompareOp::Insert => LawCompareOp::Insert,
            lj_core::compare::CompareOp::Delete => LawCompareOp::Delete,
        },
        text: segment.text,
    }
}

/// Convertit une ligne `law_decisions_citing` en DTO citant : le `title`
/// machine est dérivé des métadonnées (même source que le détail/MCP), le
/// libellé de repli du type venant du référentiel, la portée du groupe de
/// `publication_codes`.
pub(crate) fn citing_dto(
    row: CitingDecisionRow,
    refs: &crate::referential::Referential,
) -> CitingDecisionHit {
    let dockets = row.docket_numbers.filter(|d| !d.is_empty());
    let title = decision_title(
        refs.jurisdiction_type_label(&row.jurisdiction_type)
            .unwrap_or(&row.jurisdiction_type),
        row.jurisdiction_name.as_deref(),
        None,
        row.date_lecture.as_deref(),
        None,
        dockets.as_deref(),
    );
    let jurisdiction_type = parse_jur_type(&row.jurisdiction_type).unwrap_or(JurisdictionType::Ta);
    CitingDecisionHit {
        id: row.public_id,
        title,
        jurisdiction_type,
        date_lecture: row.date_lecture,
        publication_codes: row.publication_codes.unwrap_or_default(),
        summary: row.summary,
        cited_articles: row.cited_num_keys,
    }
}

/// Désérialise un code `jurisdiction_type` issu de la DB en [`JurisdictionType`]
/// (mapping serde des variantes). `None` si le code est inattendu.
fn parse_jur_type(raw: &str) -> Option<JurisdictionType> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

/// Décisions citant un article (`GET /api/texte/{code}/{num}/citing`), paginées.
/// `date` (ISO `YYYY-MM-DD`, `None` = en vigueur) sélectionne la version servie ;
/// les citantes sont bornées à sa fenêtre de validité (une décision cite la
/// version en vigueur à sa date, cf. renumérotations). 404 si code/article
/// inconnu ; 422 si `date` mal formée.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, date, limit, offset))]
pub async fn article_citing(
    state: &AppState,
    code: &str,
    num: &str,
    date: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<CitingDecisionHit>> {
    let date = date
        .map(|d| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| {
                ApiError::Unprocessable(crate::error::validation::date_parsing(
                    &["query", "date"],
                    d,
                    "invalid date",
                ))
            })
        })
        .transpose()?;

    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_core::article_key::identity_key(num);

    // Version servie (même résolution que l'en-tête d'article) → sa fenêtre de
    // validité borne les citantes. `date_debut` NOT NULL en base.
    let (num_key, article) = served_version(&repo, text_uid, num_key, date).await?;
    let num_key = num_key.as_str();
    let date_debut = article
        .date_debut
        .ok_or_else(|| ApiError::Internal("version d'article sans date_debut".to_string()))?;

    let rows = repo
        .law_decisions_citing(
            text_uid,
            num_key,
            date_debut,
            article.date_fin,
            limit,
            offset,
        )
        .await?;
    let refs = crate::referential::referential(state).await?;
    Ok(rows.into_iter().map(|r| citing_dto(r, &refs)).collect())
}

/// Articles interrogés pour les citantes d'une division. La liste des `num_key`
/// d'un Livre en compte des milliers, et le coût d'une union de citantes croît
/// avec le nombre de termes : on classe d'abord le sous-arbre par
/// `citing_decision_counts`, puis on ne lit les décisions que pour les articles
/// de tête. Même borne que le « voir tout » du front, qui compose une facette à
/// une valeur par article.
const SECTION_CITING_ARTICLES: usize = 20;

/// Articles du palmarès « les plus cités » d'une division ou d'un code (colonne
/// de rebond).
const SECTION_TOP_CITED: usize = 5;

/// Vivier du palmarès d'un texte entier : les termes les plus cités ne sont pas
/// tous des articles encore présents dans le texte (renumérotations, transferts).
/// On en tire quatre fois le plafond avant de joindre le catalogue.
const TOP_CITED_CANDIDATES: usize = SECTION_TOP_CITED * 4;

/// Textes citants d'une division : même plafond que les décisions du bloc.
const SECTION_CITING_TEXTS: usize = 5;

/// Seuil de débruitage des textes citants d'une division : un texte qui ne vise
/// qu'un seul de ses articles n'est pas un texte de la division. Deux articles
/// visés = une application de la division.
const SECTION_CITING_TEXT_MIN_ARTICLES: i64 = 2;

/// Décisions citant une DIVISION (`GET /api/texte/{code}/section/{cid}/citing`),
/// paginées : union des citantes de **tout le sous-arbre** de la division à la
/// date demandée, dédupliquée par décision et triée comme les citantes d'un
/// article. `date` (ISO `YYYY-MM-DD`, `None` = en vigueur) choisit la composition
/// du sous-arbre. 404 si le code ou le `cid` est inconnu ; 422 si `date` est mal
/// formée.
///
/// Coût borné par [`SECTION_CITING_ARTICLES`] : seuls les articles les plus
/// cités de la division sont interrogés, le classement venant de la table de
/// compteurs.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, cid = %cid, date, limit, offset))]
pub async fn section_citing(
    state: &AppState,
    code: &str,
    cid: &str,
    date: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<CitingDecisionHit>> {
    let date = parse_opt_date(date)?;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code).await?;
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let keys: Vec<String> = repo
        .toc_section_article_keys(&text.text_uid, cid, at)
        .await?
        .ok_or(ApiError::NotFound)?
        .into_iter()
        .map(|(num_key, _)| num_key)
        .collect();
    let top: Vec<String> = repo
        .citing_counts_for_articles(&text.text_uid, &keys)
        .await?
        .into_iter()
        .take(SECTION_CITING_ARTICLES)
        .map(|(num_key, _)| num_key)
        .collect();
    let rows = repo
        .law_decisions_citing_any(&text.text_uid, &top, limit, offset)
        .await?;
    let refs = crate::referential::referential(state).await?;
    Ok(rows.into_iter().map(|r| citing_dto(r, &refs)).collect())
}

/// Articles co-cités (`GET /api/texte/{code}/{num}/related`) : croisement
/// `legal_citation` (« souvent cité avec »), pondéré IDF côté repo. 404 si le
/// code est inconnu ; liste vide si l'article n'a pas de co-citations.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num))]
pub async fn article_co_cited(
    state: &AppState,
    code: &str,
    num: &str,
) -> Result<Vec<CoCitedArticle>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code).await?;
    let num_key = &lj_core::article_key::identity_key(num);
    let rows = repo
        .law_co_cited_articles(&text.text_uid, num_key, 8)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| CoCitedArticle {
            href: r
                .text_slug
                .as_deref()
                .map(|slug| format!("/texte/{slug}/{}", r.num_key)),
            num_key: r.num_key,
            text_title: r.text_title,
            count: r.count,
        })
        .collect())
}

/// Relaxed resolution of an URL `code`: slug, text name or acronym. The browser
/// only ever emits slugs and keeps the exact lookup of [`resolve_text`]; `mcp.rs`
/// resolves through the same repository call but keeps the candidates, of which
/// it makes a "did you mean".
pub async fn resolve_code(state: &AppState, raw: &str) -> Result<String> {
    use lj_store::repository::InstrumentResolution as R;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    match repo.resolve_instrument(raw).await? {
        R::Direct { slug, .. } | R::Rattrape { slug, .. } => Ok(slug),
        R::Inconnu { .. } => Err(ApiError::NotFound),
    }
}

/// What `/texte/{code}` answers with no article segment.
pub enum LawRoot {
    /// A text with no articles *is* its body (circulaire, publication decree,
    /// guidelines): 42,563 texts of the corpus, readable no other way.
    Body(LawCodeSummary),
    Toc(LawCodeSummary, CodeTocResponse),
}

pub async fn law_root(state: &AppState, code: &str) -> Result<LawRoot> {
    let summary = code_summary(state, code).await?;
    if summary.article_count == 0 && summary.body.is_some() {
        return Ok(LawRoot::Body(summary));
    }
    let toc = code_toc(state, code, None).await?;
    Ok(LawRoot::Toc(summary, toc))
}

/// `(code, num_key)` targets of a `LEGIARTI…`. Every version of one article
/// shares its id, so the id almost always designates a single target.
pub async fn article_refs_by_source_uid(
    state: &AppState,
    uid: &str,
) -> Result<Vec<(String, String)>> {
    let conn = state.pool.get().await?;
    Ok(DecisionRepository::new(&conn)
        .article_refs_by_source_uid(uid)
        .await?)
}

/// Sommaire d'un code (`GET /api/texte/{code}`) : métadonnées + nombre d'articles
/// en vigueur. 404 si le slug est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_summary(state: &AppState, code: &str) -> Result<LawCodeSummary> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    // Passe par `resolve_text` pour suivre un renommage de slug : la requête de
    // sommaire, elle, filtre sur le slug vivant.
    let slug = resolve_text(state, &repo, code).await?.slug.clone();
    let row = repo
        .law_code_summary(&slug)
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut upcoming_versions: Vec<String> = row
        .upcoming_versions
        .iter()
        .map(|d| d.to_string())
        .collect();
    upcoming_versions.sort();
    // Sur-facette portée : libellé exposé seulement pour la doctrine
    // administrative — les normes n'ont pas de badge.
    let scope = lj_core::referential_labels::nature_scope(&row.nature);
    let scope = (scope != "norme").then(|| {
        lj_core::referential_labels::scope_label(scope)
            .unwrap_or(scope)
            .to_string()
    });
    // Maillage retour vers le hub fond×année du catalogue des normes ; `None`
    // pour un acte individuel.
    let fond_link = repo.norm_fond_of_text(&row.text_uid).await?;
    let (fond, fond_label, fond_year) = match fond_link {
        Some((fond, year)) => {
            let label = lj_core::referential_labels::norm_fond_label(&fond)
                .unwrap_or(&fond)
                .to_string();
            (Some(fond), Some(label), year)
        }
        None => (None, None, None),
    };
    // Palmarès du texte entier : la colonne droite de la page code ouvre sur ses
    // articles les plus jugés. Lecture de la table de compteurs, constante en taille
    // de code.
    let top_cited = top_cited_articles(&repo, &row.text_uid, &slug).await?;
    Ok(LawCodeSummary {
        legitext: row.text_uid,
        code: slug,
        titre: row.title,
        nature: row.nature,
        derniere_modification: row.last_modified,
        article_count: row.article_count,
        upcoming_versions,
        body: row.body,
        status: row.status,
        nor: row.nor,
        date_texte: row.date_texte,
        scope,
        fond,
        fond_label,
        fond_year,
        top_cited,
    })
}

/// Longueur cible d'un extrait d'article dans les résultats `/recherche-textes`.
const ARTICLE_SNIPPET_MAX: usize = 280;

/// Recherche plein-texte d'articles (`GET /api/search-textes`),
/// **titre-primaire** : jambe titre (`search_title` formé, boostée + expansions
/// d'alias) > jambe corps. `code` optionnel borne la recherche à un texte (résolu
/// comme les pages `/texte`) ; absent ⇒ tout le référentiel navigable. Les facettes
/// `jurisdiction`/`nature`/`source` et le `total` sont comptés sous le même
/// prédicat BM25 + mêmes filtres que la page de hits, en une requête `GROUPING
/// SETS` lancée **en parallèle** des hits sur sa propre connexion — le prédicat
/// BM25 est le poste dominant, on ne le paie qu'une fois par jambe. Chaque hit
/// reçoit un extrait surligné via [`crate::snippets::highlight`] (fallback corps
/// tronqué).
///
/// Émet le signal d'usage du moteur `textes` sous le nom de span `search`, même
/// vocabulaire `librejustice.search.*` que les décisions ([`crate::usage`]) :
/// `{name="search"}` couvre donc les deux moteurs, ventilables par `.engine`.
#[instrument(
    name = "search",
    skip(state, caller),
    fields(
        db.system = "postgresql",
        limit,
        offset,
        librejustice.search.engine = tracing::field::Empty,
        librejustice.search.source = tracing::field::Empty,
        librejustice.search.query = tracing::field::Empty,
        librejustice.search.identity = tracing::field::Empty,
            librejustice.search.context = tracing::field::Empty,
        librejustice.search.origin = tracing::field::Empty,
        librejustice.search.caller = tracing::field::Empty,
        user_agent.original = tracing::field::Empty,
        librejustice.search.visit = tracing::field::Empty,
        librejustice.search.page = tracing::field::Empty,
        librejustice.search.filters = tracing::field::Empty,
        librejustice.search.results_count = tracing::field::Empty,
    )
)]
#[allow(clippy::too_many_arguments)]
pub async fn search_textes(
    state: &AppState,
    q: &str,
    code: Option<&str>,
    jurisdiction: Option<&str>,
    nature: Option<&str>,
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
    limit: i64,
    offset: i64,
    caller: crate::usage::Caller,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<ArticleSearchResponse> {
    const ENGINE: lj_dtos::SearchEngine = lj_dtos::SearchEngine::Textes;
    let filters = textes_filters(code, jurisdiction, nature, source, scope, at).to_string();
    let shape = crate::usage::Shape {
        page: crate::search_history::page_number(offset, limit).unwrap_or(1),
        filters: &filters,
    };
    caller.record_span(ENGINE, q, shape);
    let uncached = || {
        search_textes_uncached(
            state,
            q,
            code,
            jurisdiction,
            nature,
            source,
            scope,
            at,
            limit,
            offset,
            budget_key,
        )
    };
    let key = textes_cache_key(
        q,
        code,
        jurisdiction,
        nature,
        source,
        scope,
        at,
        limit,
        offset,
    );
    let mut own_err: Option<ApiError> = None;
    let mut paye = false;
    let hit = state
        .textes_cache
        .optionally_get_with(key, async {
            paye = true;
            match uncached().await {
                Ok(resp) => Some(std::sync::Arc::new(resp)),
                Err(err) => {
                    own_err = Some(err);
                    None
                }
            }
        })
        .await;
    crate::cache_metrics::record_access("textes", !paye);
    let resp = match (hit, own_err) {
        (Some(resp), _) => resp,
        (None, Some(err)) => return Err(err),
        (None, None) => std::sync::Arc::new(uncached().await?),
    };
    caller.record_outcome(ENGINE, q, shape, resp.total);
    Ok((*resp).clone())
}

/// Clé du cache textes, fenêtre comprise : le SQL pagine. Des arguments bruts et
/// jamais de [`textes_filters`], sérialisation d'affichage dont un champ retiré
/// ferait partager une entrée à deux requêtes distinctes.
#[allow(clippy::too_many_arguments)]
fn textes_cache_key(
    q: &str,
    code: Option<&str>,
    jurisdiction: Option<&str>,
    nature: Option<&str>,
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
    limit: i64,
    offset: i64,
) -> String {
    serde_json::json!([
        q,
        code,
        jurisdiction,
        nature,
        source,
        scope,
        at.map(|d| d.to_string()),
        limit,
        offset,
    ])
    .to_string()
}

/// Poids d'une réponse pour le plafond mémoire du cache.
pub(crate) fn response_weight(resp: &ArticleSearchResponse) -> usize {
    let hits: usize = resp
        .hits
        .iter()
        .map(|h| {
            std::mem::size_of::<lj_dtos::ArticleSearchHit>()
                + h.code.len()
                + h.code_title.len()
                + h.num.len()
                + h.num_key.len()
                + h.titre_path.as_ref().map_or(0, String::len)
                + h.snippet.len()
                + h.source.len()
                + h.status.as_ref().map_or(0, String::len)
                + h.date_debut.as_ref().map_or(0, String::len)
        })
        .sum();
    let f = &resp.facets;
    let facets: usize = [&f.code, &f.jurisdiction, &f.nature, &f.source, &f.scope]
        .into_iter()
        .flatten()
        .map(|c| {
            std::mem::size_of::<lj_dtos::FacetChoice>()
                + c.value.len()
                + c.label.len()
                + c.parent.as_ref().map_or(0, String::len)
        })
        .sum();
    hits + facets
}

/// Séparé de [`search_textes`] pour que single-flight, jeton et permit ne portent
/// que sur le MISS.
#[allow(clippy::too_many_arguments)]
async fn search_textes_uncached(
    state: &AppState,
    q: &str,
    code: Option<&str>,
    jurisdiction: Option<&str>,
    nature: Option<&str>,
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
    limit: i64,
    offset: i64,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<ArticleSearchResponse> {
    if state
        .budgets
        .take(crate::ratelimit::Cost::Search, budget_key)
        .await
        == crate::ratelimit::Verdict::Exhausted
        && state.settings.search_budget_enforce
    {
        return Err(ApiError::TooManyRequests);
    }
    // Le sémaphore des décisions, pas un second : deux bornes sur un pool unique
    // demanderaient 62 connexions sur 32.
    let _db_permit = state
        .search_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
    let checkout = || async { state.pool.get().await.map_err(ApiError::from) };
    let (conn_hits, conn_stats) = tokio::try_join!(checkout(), checkout())?;
    let repo_hits = DecisionRepository::new(&conn_hits);
    let repo_stats = DecisionRepository::new(&conn_stats);

    let text_uid = match code {
        Some(c) if !c.is_empty() => {
            Some(resolve_text(state, &repo_hits, c).await?.text_uid.clone())
        }
        _ => None,
    };
    // Expansion d'alias (acronymes/noms usuels → forme développée) OR-ée dans la
    // jambe titre.
    let expansions = lj_core::aliases::expand_query(q);
    // Sur-facette « portée » → ensemble de natures (mapping code) : doctrine
    // administrative = liste fermée, norme = son complément ouvert.
    let doctrine_natures: Vec<String> = lj_core::referential_labels::DOCTRINE_ADMIN_NATURES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let nature_set: Option<(&[String], bool)> = match scope {
        None => None,
        Some("doctrine_administrative") => Some((&doctrine_natures, true)),
        Some("norme") => Some((&doctrine_natures, false)),
        Some(other) => {
            return Err(ApiError::Unprocessable(
                crate::error::validation::enum_error(
                    &["query", "scope"],
                    other,
                    &["norme", "doctrine_administrative"],
                ),
            ))
        }
    };
    // Requête vide : parcours du corpus, listé par entrée en vigueur. Total et
    // facettes portent sur les articles seuls, comme la liste.
    if q.trim().is_empty() {
        let browse = lj_store::repository::ArticleBrowseFilters {
            text_uid: text_uid.as_deref(),
            jurisdiction,
            nature,
            source,
            nature_set,
            at,
        };
        let (rows, stats) = tokio::try_join!(
            repo_hits.browse_articles(browse, limit, offset),
            repo_stats.article_browse_stats(browse),
        )?;
        return Ok(ArticleSearchResponse {
            hits: hits_with_snippets(rows, q),
            total: stats.total,
            facets: facets_dto(stats),
        });
    }
    let (rows, stats) = tokio::try_join!(
        repo_hits.search_articles(
            q,
            &expansions,
            text_uid.as_deref(),
            jurisdiction,
            nature,
            source,
            nature_set,
            at,
            limit,
            offset,
        ),
        repo_stats.article_search_stats(
            q,
            &expansions,
            text_uid.as_deref(),
            jurisdiction,
            nature,
            source,
            nature_set,
            at,
        ),
    )?;
    Ok(ArticleSearchResponse {
        hits: hits_with_snippets(rows, q),
        total: stats.total,
        facets: facets_dto(stats),
    })
}

/// Filtres d'une recherche de textes, aux clés de l'URL de la page `/textes`
/// (`origine`, pas `source`) : sans la query ni la fenêtre. Sérialisation
/// partagée par l'historique en base et par l'attribut de span
/// `librejustice.search.filters` — la profondeur va dans `page`, que
/// l'historique ajoute ici et que le span porte à part.
pub(crate) fn textes_filters(
    code: Option<&str>,
    jurisdiction: Option<&str>,
    nature: Option<&str>,
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
) -> serde_json::Value {
    let date = at.map(|d| d.to_string());
    let mut filters = serde_json::Map::new();
    for (key, value) in [
        ("code", code),
        ("jurisdiction", jurisdiction),
        ("nature", nature),
        ("origine", source),
        ("scope", scope),
        ("date", date.as_deref()),
    ] {
        if let Some(v) = value {
            filters.insert(key.to_string(), serde_json::Value::String(v.to_string()));
        }
    }
    serde_json::Value::Object(filters)
}

/// Mappe les facettes du repo vers le DTO (chaque `FacetCount` devient un
/// `FacetChoice`, tri préservé : count décroissant puis valeur croissante),
/// libellés humanisés via `lj_core::referential_labels` (pays FR pour
/// `jurisdiction`, natures LEGI/curées pour `nature`, diffuseurs à jeton court
/// pour `source` — valeur brute en repli).
fn facets_dto(stats: ArticleSearchStats) -> ArticleSearchFacets {
    use lj_core::referential_labels::{jurisdiction_label, nature_label, source_label};
    let map = |rows: Vec<FacetCount>, label: fn(&str) -> Option<&'static str>| {
        rows.into_iter()
            .map(|row| FacetChoice {
                label: label(&row.value)
                    .map(str::to_string)
                    .unwrap_or_else(|| row.value.clone()),
                value: row.value,
                count: row.count,
                parent: None,
                hint: None,
            })
            .collect()
    };
    // Sur-facette portée : agrégat des buckets nature par mapping code.
    let mut scope_counts: std::collections::BTreeMap<&'static str, i64> =
        std::collections::BTreeMap::new();
    for row in &stats.nature {
        *scope_counts
            .entry(lj_core::referential_labels::nature_scope(&row.value))
            .or_default() += row.count;
    }
    let mut scope: Vec<FacetChoice> = scope_counts
        .into_iter()
        .map(|(value, count)| FacetChoice {
            label: lj_core::referential_labels::scope_label(value)
                .unwrap_or(value)
                .to_string(),
            value: value.to_string(),
            count,
            parent: None,
            hint: None,
        })
        .collect();
    scope.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    ArticleSearchFacets {
        // Slugs auto-descriptifs : le label est la valeur.
        code: map(stats.code, |_| None),
        jurisdiction: map(stats.jurisdiction, jurisdiction_label),
        nature: map(stats.nature, nature_label),
        source: map(stats.source, source_label),
        scope,
    }
}

/// Catalogue des codes navigables (`GET /api/codes`) : un texte de référentiel
/// par entrée, avec son nombre d'articles. `slug` du texte = `code` du DTO (lien
/// `/texte/{code}`).
/// Une entrée de cache par `head_only`, TTL 12 h : le comptage des articles en
/// vigueur, corrélé par texte, brasse 270 k lignes de `legal_article` et ne se
/// recalcule pas par requête (mesuré à 1,4 s).
pub async fn code_catalogue(
    state: &AppState,
    head_only: bool,
) -> Result<Arc<CodeCatalogueResponse>> {
    state
        .code_catalogue_cache
        .try_get_with(head_only, load_code_catalogue(state, head_only))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("catalogue codes: {e}")))
}

#[instrument(skip(state), fields(db.system = "postgresql"))]
async fn load_code_catalogue(
    state: &AppState,
    head_only: bool,
) -> Result<Arc<CodeCatalogueResponse>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let entries = repo
        .list_legal_texts(head_only)
        .await?
        .into_iter()
        .map(catalogue_entry)
        .collect();
    Ok(Arc::new(CodeCatalogueResponse { entries }))
}

fn catalogue_entry(row: LegalTextCatalogRow) -> CodeCatalogueEntry {
    CodeCatalogueEntry {
        code: row.slug,
        title: row.title,
        nature: row.nature,
        jurisdiction: row.jurisdiction,
        article_count: row.article_count,
    }
}

// Au-delà de ce nombre d'articles, le sommaire renvoie la table des matières
// (chips) ; en deçà, la vue-lecture intégrale — un texte court (BOFiP, décret,
// arrêté, loi ordinaire) se lit sur sa page. Partagé avec le front (éligibilité
// d'une division à l'accordéon).
use lj_dtos::{INLINE_READING_MAX, INLINE_READING_MAX_BYTES};

/// Table des matières d'un code (`GET /api/texte/{code}/sommaire`) : articles
/// ordonnés, chacun avec son fil d'Ariane et son état. Textes courts
/// (≤ [`INLINE_READING_MAX`] articles) : `reading` porte la vue-lecture
/// intégrale (corps joints) à la place des entrées. 404 si le slug est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_toc(
    state: &AppState,
    code: &str,
    date: Option<NaiveDate>,
) -> Result<CodeTocResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code).await?;
    let text_uid = &text.text_uid;
    // Arbre structurel réel daté à la date demandée (Chronolégi) sinon en vigueur
    // aujourd'hui ; repli sur le sommaire à plat (title_path) pour les textes sans
    // structure ingérée.
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let tree = repo.toc_tree(text_uid, at).await?;
    if !tree.is_empty() {
        let n_articles = tree.iter().filter(|r| r.child_kind == "article").count();
        let reading = if n_articles <= INLINE_READING_MAX {
            lecture_bornee(repo.toc_text_reading(text_uid, at).await?)
        } else {
            Vec::new()
        };
        return Ok(CodeTocResponse {
            entries: Vec::new(),
            tree: tree.into_iter().map(toc_node).collect(),
            reading,
        });
    }
    let rows = repo.code_table_of_contents(text_uid).await?;
    if rows.len() <= INLINE_READING_MAX {
        let reading = lecture_bornee(repo.flat_text_reading(text_uid).await?);
        if !reading.is_empty() {
            return Ok(CodeTocResponse {
                entries: Vec::new(),
                tree: Vec::new(),
                reading,
            });
        }
    }
    Ok(CodeTocResponse {
        entries: rows.into_iter().map(toc_entry).collect(),
        tree: Vec::new(),
        reading: Vec::new(),
    })
}

fn toc_node(row: TocTreeRow) -> TocNode {
    TocNode {
        kind: row.child_kind,
        depth: row.depth,
        label: row.label,
        cid: row.child_cid,
        num_key: row.child_num_key,
        etat: row.etat,
    }
}

/// Vue-lecture d'une section (`GET /api/texte/{code}/section/{cid}`) : le
/// sous-arbre de la section en vigueur aujourd'hui, corps des articles joints.
/// 404 si le slug ou le `cid` est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, cid = %cid))]
pub async fn law_section(
    state: &AppState,
    code: &str,
    cid: &str,
    date: Option<NaiveDate>,
) -> Result<LawSectionResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let slug = resolve_text(state, &repo, code).await?.slug.clone();
    let summary = repo
        .law_code_summary(&slug)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Sous-arbre et corps à la date demandée, sinon en vigueur aujourd'hui.
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let (title, items) = repo
        .toc_section_reading(&summary.text_uid, cid, at)
        .await?
        .ok_or(ApiError::NotFound)?;
    let tree = repo.toc_tree(&summary.text_uid, at).await?;
    let (ancestors, prev, next) = section_nav(&tree, cid);
    let mut items: Vec<LawSectionItem> = items.into_iter().map(section_item).collect();
    let omitted = borner_lecture(&mut items);
    let top_cited = section_top_cited(&repo, &summary.text_uid, &slug, &items).await?;
    let citing_texts = section_citing_texts(&repo, &summary.text_uid, &items).await?;
    Ok(LawSectionResponse {
        code: slug,
        code_title: Some(summary.title),
        cid: cid.to_string(),
        title,
        ancestors,
        prev,
        next,
        items,
        omitted,
        top_cited,
        citing_texts,
    })
}

/// La même règle au cas racine, où le sommaire est le repli : au-delà d'une
/// borne, la vue-lecture est vide et l'arbre reprend la main.
fn lecture_bornee(rows: Vec<TocReadingRow>) -> Vec<LawSectionItem> {
    let mut items: Vec<LawSectionItem> = rows.into_iter().map(section_item).collect();
    if borner_lecture(&mut items).is_some() {
        return Vec::new();
    }
    items
}

/// La règle de lecture en place, à ses deux bornes. Rend le nombre d'articles
/// réduits à leur index, les items gardant numéro et lien.
fn borner_lecture(items: &mut [LawSectionItem]) -> Option<usize> {
    let articles = items.iter().filter(|i| i.kind == "article").count();
    let octets: usize = items
        .iter()
        .map(|i| i.texte.as_deref().map_or(0, str::len) + i.nota.as_deref().map_or(0, str::len))
        .sum();
    if articles <= INLINE_READING_MAX && octets <= INLINE_READING_MAX_BYTES {
        return None;
    }
    for item in items.iter_mut() {
        item.texte = None;
        item.nota = None;
    }
    Some(articles)
}

/// Les [`SECTION_TOP_CITED`] articles les plus cités du sous-arbre **déjà lu** :
/// une lecture de `citing_decision_counts` sur ses `num_key`, sans jointure ni
/// relecture de l'arbre. `label` = le numéro porté par l'arête, celui que la
/// vue-lecture affiche.
async fn section_top_cited(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    code: &str,
    items: &[LawSectionItem],
) -> Result<Vec<TopCitedArticle>> {
    let labels: std::collections::HashMap<&str, &str> = items
        .iter()
        .filter(|i| i.kind == "article")
        .filter_map(|i| Some((i.num_key.as_deref()?, i.label.as_str())))
        .collect();
    let num_keys: Vec<String> = labels.keys().map(|k| (*k).to_string()).collect();
    Ok(repo
        .citing_counts_for_articles(text_uid, &num_keys)
        .await?
        .into_iter()
        .take(SECTION_TOP_CITED)
        .map(|(num_key, count)| TopCitedArticle {
            label: labels[num_key.as_str()].to_string(),
            href: Some(format!("/texte/{code}/{num_key}")),
            num_key,
            count,
        })
        .collect())
}

/// Le même palmarès au périmètre du **texte entier** : le classement vient du
/// repo, qui lit la table de compteurs sans énumérer les articles du code, et
/// rend déjà le numéro affichable.
async fn top_cited_articles(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    code: &str,
) -> Result<Vec<TopCitedArticle>> {
    Ok(repo
        .top_cited_articles(
            text_uid,
            SECTION_TOP_CITED as i64,
            TOP_CITED_CANDIDATES as i64,
        )
        .await?
        .into_iter()
        .map(|(num_key, label, count)| TopCitedArticle {
            label,
            href: Some(format!("/texte/{code}/{num_key}")),
            num_key,
            count,
        })
        .collect())
}

/// Textes citant la division : agrégés au grain du texte citant, filtrés à
/// [`SECTION_CITING_TEXT_MIN_ARTICLES`] articles visés et plafonnés comme les
/// décisions du même bloc. La liste sort de la vue-lecture déjà lue — ses
/// `num_key` sont exactement le sous-arbre daté.
async fn section_citing_texts(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    items: &[LawSectionItem],
) -> Result<Vec<SectionCitingText>> {
    let num_keys: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "article")
        .filter_map(|i| i.num_key.clone())
        .collect();
    Ok(repo
        .section_citing_texts(
            text_uid,
            &num_keys,
            SECTION_CITING_TEXT_MIN_ARTICLES,
            SECTION_CITING_TEXTS as i64,
        )
        .await?
        .into_iter()
        .map(|(label, slug, article_count)| SectionCitingText {
            label,
            href: slug.map(|s| format!("/texte/{s}")),
            article_count,
        })
        .collect())
}

/// Situe une section dans l'arbre aplati en ordre de lecture : divisions
/// englobantes (racine → parent), bloc précédent et suivant — les voisins en
/// ordre de lecture hors sous-arbre (le suivant d'un Livre est le Livre
/// d'après, pas son premier Titre).
fn section_nav(
    tree: &[TocTreeRow],
    cid: &str,
) -> (
    Vec<LawSectionRef>,
    Option<LawSectionRef>,
    Option<LawSectionRef>,
) {
    let sref = |row: &TocTreeRow| LawSectionRef {
        cid: row.child_cid.clone().unwrap_or_default(),
        label: row.label.clone(),
    };
    let Some(at) = tree
        .iter()
        .position(|r| r.child_kind == "section" && r.child_cid.as_deref() == Some(cid))
    else {
        return (Vec::new(), None, None);
    };
    let depth = tree[at].depth;

    // Pile des divisions ouvertes jusqu'à la section : ses ancêtres.
    let mut stack: Vec<&TocTreeRow> = Vec::new();
    for row in &tree[..at] {
        if row.child_kind == "section" {
            stack.truncate((row.depth - 1) as usize);
            stack.push(row);
        }
    }
    stack.truncate((depth - 1) as usize);
    let ancestor_cids: Vec<Option<&str>> = stack.iter().map(|r| r.child_cid.as_deref()).collect();
    let ancestors = stack.iter().map(|r| sref(r)).collect();

    let next = tree[at + 1..]
        .iter()
        .find(|r| r.child_kind == "section" && r.depth <= depth)
        .map(sref);
    // Inverse du suivant : le dernier bloc en ordre de lecture avant la
    // section, quelle que soit sa profondeur (hors ancêtres).
    let prev = tree[..at]
        .iter()
        .rev()
        .find(|r| r.child_kind == "section" && !ancestor_cids.contains(&r.child_cid.as_deref()))
        .map(sref);
    (ancestors, prev, next)
}

fn section_item(row: TocReadingRow) -> LawSectionItem {
    LawSectionItem {
        kind: row.child_kind,
        depth: row.depth,
        label: row.label,
        cid: row.child_cid,
        num_key: row.child_num_key,
        etat: row.etat,
        texte: row.texte,
        nota: row.nota,
    }
}

/// Mappe une ligne de sommaire du repo vers le DTO (`position` ignorée — l'ordre
/// est porté par la séquence de la requête).
fn toc_entry(row: TocArticleRow) -> TocEntry {
    TocEntry {
        num: row.num,
        num_key: row.num_key,
        title_path: row.title_path,
        status: row.status,
        expired: row.expired,
    }
}

/// Construit les hits DTO en surlignant le corps (`snippets::highlight`, mêmes
/// tokenizers que l'index). Les docs sont indexés par position (id synthétique) ;
/// un doc qui ne matche pas (absent du retour `highlight`) retombe sur un corps
/// tronqué. `slug` est non-NULL (la requête filtre `t.slug IS NOT NULL`).
fn hits_with_snippets(rows: Vec<ArticleSearchRow>, q: &str) -> Vec<ArticleSearchHit> {
    let docs: Vec<(i64, String)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i as i64, r.texte.clone().unwrap_or_default()))
        .collect();
    let snippets = crate::snippets::highlight(&docs, q, ARTICLE_SNIPPET_MAX);
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            let snippet = snippets.get(&(i as i64)).cloned().unwrap_or_else(|| {
                truncate_plain(r.texte.as_deref().unwrap_or(""), ARTICLE_SNIPPET_MAX)
            });
            ArticleSearchHit {
                code: r.slug.unwrap_or_default(),
                code_title: r.code_title,
                num: r.num,
                num_key: r.num_key,
                titre_path: r.title_path,
                snippet,
                source: r.source,
                status: (!r.status.is_empty()).then_some(r.status),
                date_debut: r.date_debut.map(|d| d.to_string()),
            }
        })
        .collect()
}

/// Tronque un texte brut à `max` caractères (frontière char), suffixe `…` si
/// coupé. Fallback quand le corps ne matche pas la requête de surlignage.
fn truncate_plain(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(max) {
        Some((byte_idx, _)) => format!("{}…", &trimmed[..byte_idx]),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arbre de test : L1 (T1 (C1, C2), T2), L2 — profondeurs 1/2/3.
    fn nav_tree() -> Vec<TocTreeRow> {
        let section = |cid: &str, depth: i32| TocTreeRow {
            depth,
            child_kind: "section".to_string(),
            child_uid: format!("uid-{cid}"),
            child_cid: Some(cid.to_string()),
            child_num_key: None,
            label: cid.to_uppercase(),
            etat: "VIGUEUR".to_string(),
        };
        let article = |num: &str, depth: i32| TocTreeRow {
            depth,
            child_kind: "article".to_string(),
            child_uid: format!("uid-a{num}"),
            child_cid: None,
            child_num_key: Some(num.to_string()),
            label: num.to_string(),
            etat: "VIGUEUR".to_string(),
        };
        vec![
            section("l1", 1),
            section("t1", 2),
            section("c1", 3),
            article("1", 4),
            section("c2", 3),
            article("2", 4),
            section("t2", 2),
            article("3", 3),
            section("l2", 1),
            article("4", 2),
        ]
    }

    #[test]
    fn section_nav_ancetres_et_voisins() {
        let tree = nav_tree();

        let (anc, prev, next) = section_nav(&tree, "c2");
        assert_eq!(
            anc.iter().map(|r| r.cid.as_str()).collect::<Vec<_>>(),
            ["l1", "t1"]
        );
        assert_eq!(prev.unwrap().cid, "c1");
        assert_eq!(next.unwrap().cid, "t2");

        let (anc, prev, next) = section_nav(&tree, "t2");
        assert_eq!(anc.len(), 1);
        assert_eq!(prev.unwrap().cid, "c2");
        assert_eq!(next.unwrap().cid, "l2");

        let (anc, prev, next) = section_nav(&tree, "l1");
        assert!(anc.is_empty() && prev.is_none());
        assert_eq!(next.unwrap().cid, "l2");
        let (_, _, next) = section_nav(&tree, "l2");
        assert!(next.is_none());

        let (anc, prev, next) = section_nav(&tree, "zzz");
        assert!(anc.is_empty() && prev.is_none() && next.is_none());
    }

    #[test]
    fn legifrance_source_url_native_is_versioned() {
        assert_eq!(
            legifrance_source_url("LEGIARTI000006419292", "1992-05-15").as_deref(),
            Some("https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006419292/1992-05-15")
        );
    }

    #[test]
    fn legifrance_source_url_none_for_curated() {
        assert_eq!(
            legifrance_source_url("JORFTEXT000000694290#6@1968-12-27", "2002-12-26"),
            None
        );
    }

    #[test]
    fn parse_jur_type_maps_known_codes() {
        assert_eq!(parse_jur_type("CC"), Some(JurisdictionType::Cc));
        assert_eq!(parse_jur_type("TA"), Some(JurisdictionType::Ta));
        assert_eq!(parse_jur_type("ZZZ"), None);
    }

    #[test]
    fn citing_dto_builds_title_and_falls_back_jur_type() {
        let row = CitingDecisionRow {
            id: 42,
            public_id: "CETATEXT000012345678".to_string(),
            jurisdiction_type: "CE".to_string(),
            jurisdiction_name: Some("Conseil d'État".to_string()),
            date_lecture: Some("2024-02-13".to_string()),
            docket_numbers: Some(vec!["123456".to_string()]),
            publication_codes: Some(vec!["A".to_string()]),
            summary: None,
            cited_num_keys: Vec::new(),
        };
        let refs = crate::referential::Referential::new(Vec::new(), Vec::new(), Vec::new());
        let hit = citing_dto(row, &refs);
        assert_eq!(hit.id, "CETATEXT000012345678");
        assert_eq!(hit.jurisdiction_type, JurisdictionType::Ce);
        assert_eq!(hit.title, "Conseil d'État, 13 février 2024, 123456");
        assert_eq!(hit.date_lecture.as_deref(), Some("2024-02-13"));
        assert_eq!(hit.publication_codes, vec!["A".to_string()]);
    }

    fn version(status: &str, debut: &str, fin: Option<&str>) -> LawVersionRow {
        LawVersionRow {
            source_uid: format!("LEGIARTI{status}"),
            status: status.to_string(),
            date_debut: Some(debut.to_string()),
            date_fin: fin.map(str::to_string),
        }
    }

    fn motif(versions: &[LawVersionRow], date: &str) -> String {
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        match no_version_error(versions, Some(d)) {
            ApiError::NotFound => "not_found".to_string(),
            ApiError::NoVersionAtDate(m) => m,
            other => panic!("motif inattendu : {other}"),
        }
    }

    /// Quelle date déclenche le repli sur la dernière version : après la fin de
    /// la dernière fenêtre seulement. Une version encore ouverte, ou une date
    /// antérieure à tout, laissent `derniere_fin` sans borne à dépasser.
    #[test]
    fn le_repli_ne_s_ouvre_qu_apres_la_derniere_fenetre() {
        let jour = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();

        let abroge = [
            version("MODIFIE", "2000-09-21", Some("2019-05-23")),
            version("ABROGE", "2019-05-24", Some("2021-05-01")),
        ];
        assert_eq!(derniere_fin(&abroge), Some(jour("2021-05-01")));

        let en_vigueur = [
            version("MODIFIE", "2000-09-21", Some("2019-05-23")),
            version("VIGUEUR", "2019-05-24", None),
        ];
        assert_eq!(derniere_fin(&en_vigueur), None);

        assert_eq!(derniere_fin(&[]), None);
    }

    /// Trois issues, et une seule prononce l'abrogation : dire « abrogé » d'un
    /// article dont une version est `VIGUEUR` rapporterait notre trou de
    /// couverture comme du droit.
    #[test]
    fn le_motif_n_affirme_l_abrogation_que_sans_version_en_vigueur() {
        assert_eq!(motif(&[], "2026-08-17"), "not_found");

        let abroge = [
            version("MODIFIE", "2000-09-21", Some("2019-05-23")),
            version("ABROGE", "2019-05-24", Some("2025-10-01")),
        ];
        assert!(motif(&abroge, "2026-08-17").starts_with("law article repealed"));
        assert!(motif(&abroge, "2026-08-17").contains("2025-10-01"));

        let trou = [
            version("VIGUEUR", "2006-03-24", Some("2007-12-31")),
            version("MODIFIE_MORT_NE", "2008-01-01", Some("2006-03-24")),
        ];
        let m = motif(&trou, "2026-08-01");
        assert!(m.contains("no version covers that date"), "{m}");
        assert!(!m.contains("repealed"), "{m}");
        assert!(m.contains("2006-03-24") && m.contains("2007-12-31"), "{m}");
    }

    /// `VIGUEUR_DIFF` est une entrée en vigueur différée, pas une abrogation :
    /// le préfixe suffit à la ranger du bon côté.
    #[test]
    fn la_vigueur_differee_ne_vaut_pas_abrogation() {
        let differe = [
            version("MODIFIE", "1950-04-30", Some("2026-01-01")),
            version("VIGUEUR_DIFF", "2027-01-01", Some("2027-08-31")),
        ];
        assert!(!motif(&differe, "2026-08-17").contains("repealed"));
    }

    /// Une clé par requête servie : deux appels identiques partagent l'entrée,
    /// et rien de ce qui change la réponse ne collisionne — ni un filtre déplacé
    /// d'un axe à l'autre, ni la fenêtre, que le SQL applique.
    #[test]
    fn textes_cache_key_separe_ce_qui_change_la_reponse() {
        let key = |code, jur, offset| {
            textes_cache_key("nosocomiale", code, jur, None, None, None, None, 10, offset)
        };
        assert_eq!(key(Some("csp"), None, 0), key(Some("csp"), None, 0));
        assert_ne!(key(Some("csp"), None, 0), key(None, Some("csp"), 0));
        assert_ne!(key(Some("csp"), None, 0), key(None, None, 0));
        assert_ne!(key(Some("csp"), None, 0), key(Some("csp"), None, 10));
    }

    fn item(kind: &str, texte: Option<&str>) -> LawSectionItem {
        LawSectionItem {
            kind: kind.into(),
            depth: 1,
            label: "Article 1".into(),
            cid: None,
            num_key: Some("1".into()),
            etat: "VIGUEUR".into(),
            texte: texte.map(str::to_string),
            nota: None,
        }
    }

    /// Les deux bornes de la lecture en place mordent chacune de leur cote,
    /// et laissent l'index quand elles mordent.
    #[test]
    fn la_lecture_en_place_a_deux_bornes() {
        let mut court = vec![item("article", Some("corps")); 3];
        assert_eq!(borner_lecture(&mut court), None);
        assert!(court.iter().all(|i| i.texte.is_some()));

        let mut trop_d_articles = vec![item("article", Some("x")); INLINE_READING_MAX + 1];
        assert_eq!(
            borner_lecture(&mut trop_d_articles),
            Some(INLINE_READING_MAX + 1)
        );
        assert!(trop_d_articles.iter().all(|i| i.texte.is_none()));
        assert!(trop_d_articles.iter().all(|i| i.num_key.is_some()));

        // Peu d'articles, mais du poids : le compte seul laissait passer 420 ko.
        let gros = "a".repeat(INLINE_READING_MAX_BYTES / 2 + 1);
        let mut trop_lourd = vec![item("article", Some(&gros)); 2];
        assert_eq!(borner_lecture(&mut trop_lourd), Some(2));
        assert!(trop_lourd.iter().all(|i| i.texte.is_none()));
    }
}
