//! Turns the internal HTTP DTOs ([`lj_dtos`]) into the MCP outputs, whose
//! schemas are hand-mirrored in [`crate::mcp`].

use lj_dtos::{
    ArticleSearchResponse, CitationSpan, DecisionDetail, FacetChoice, FacetTag, LawArticleResponse,
    QueryMode, SearchFacets, SearchResponse,
};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::referential::Referential;
use crate::titles::decision_title;

/// The raw code, key of the `jurisdiction_type:*` rows of the referential and
/// of the titles.
fn jurisdiction_type_code(jt: lj_dtos::JurisdictionType) -> &'static str {
    use lj_dtos::JurisdictionType::*;
    match jt {
        Ta => "TA",
        Caa => "CAA",
        Ce => "CE",
        Constit => "CONSTIT",
        Tc => "TC",
        Cc => "CC",
        Ca => "CA",
        Tj => "TJ",
        Tcom => "TCOM",
        Cedh => "CEDH",
        Cjue => "CJUE",
        Cnda => "CNDA",
        Cnil => "CNIL",
    }
}

// ── MCP output models ────────────────────────────────────────────────────────

/// Exactly one of `aiSummary` (written by an AI, never the court's words) and
/// `snippet` (verbatim passage) is served per hit.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSearchHit {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub chars: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    /// Seat path, reusable verbatim as a filter value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub office: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<String>,
    /// Empty for a lower court, which carries no publication statement.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub publication: Vec<String>,
    /// Keyed by `legalInstrument` value, holding `legalArticle` values; the pair
    /// composes `/norm/{juris}/{slug}/{key}`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cited_articles: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSearchResponse {
    pub query: String,
    /// Decisions matched under the current filters, the whole set and not the
    /// served window.
    pub total: i64,
    pub hits: Vec<McpSearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<McpSearchFacets>,
    /// The decisions the query designates by number or by name, apart from the
    /// hits, which answer the question.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<McpPinned>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpPinned {
    pub title: String,
    pub url: String,
}

/// One `filter token -> count` map per `search_decisions` parameter, under the
/// same key names. No labels: every token is self-describing, the
/// `legal_instrument` slugs included.
#[derive(Debug, Clone, Serialize)]
pub struct McpSearchFacets {
    /// Seat paths and their counts, the whole axis in one map: court types,
    /// then the top courts, then every room a filtered court exposes.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub seat: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub office: BTreeMap<String, i64>,
    /// Top 10 by count, with no counter of the rest: the vocabulary is already
    /// in the input schema, only the counts are new.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub legal_domain: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub solution: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub procedure: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub publication: BTreeMap<String, i64>,
    /// Slugs of the cited texts, top 10 by count.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub legal_instrument: BTreeMap<String, i64>,
    /// Texts past the top 10 of `legal_instrument`, so that the truncation is
    /// announced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_instruments: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDecisionDetail {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_lecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docket_numbers: Option<Vec<String>>,
    pub jurisdiction_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    /// Chamber key, reusable verbatim as a filter value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub procedure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub office: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_domain: Option<String>,
    /// Empty for a lower court, which carries no publication statement.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub publication: Vec<String>,
    /// Fate of THIS decision before the court that reviewed it, precomputed
    /// from `case_chronology`. Absent means no recourse known to the corpus,
    /// which never proves that none exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appellate_fate: Option<String>,
    /// Prior and subsequent decisions of the same case, current one included,
    /// most recent first. A step the corpus does not hold carries no `url`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub case_chronology: Vec<McpChronologyStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<lj_dtos::Commentaire>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpChronologyStep {
    /// "Cour d'appel de Paris, 23 juin 2026".
    pub title: String,
    /// Absent when the corpus does not hold that step, which is then known only
    /// from the text of the decision attacking it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    /// Link to the step below, the attacked decision: `APPEL_DE` |
    /// `POURVOI_CONTRE` | `RENVOI_APRES_CASSATION`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_to_next: Option<String>,
}

/// Status and bounds, without the internal `legiarti` id.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawArticleVersion {
    pub etat: String,
    pub date_debut: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
}

/// The internals of the web DTO (`legiarti`/`legitext`, translation
/// provenance, publisher authority, reading context) are left out.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawArticle {
    pub code: String,
    /// Declared before anything long, so a client that cuts the answer short
    /// still leaves the caller a count to compare `articles` against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub article_count: Option<i64>,
    /// Absent when the url addresses a text with no articles (circulaire,
    /// publication decree): the whole text is then served, `text` carrying its
    /// body and `versions` being empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num: Option<String>,
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etat: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_debut: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_fin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
    /// The division directly enclosing the article, read whole from its `url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<McpLawSectionRef>,
    pub versions: Vec<McpLawArticleVersion>,
    /// Table of contents of that root, bodies joined below
    /// `INLINE_READING_MAX` articles.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub articles: Vec<McpLawTocEntry>,
    /// Articles the table serves without their body, past the threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commentaires: Vec<lj_dtos::Commentaire>,
    /// Texts that modified this article, each linking to its own page here:
    /// the hop from an article to the law or decree that wrote it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modified_by: Vec<lj_dtos::LinkedTextRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub travaux_parlementaires: Vec<lj_dtos::LinkedTextRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpLawSectionRef {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_path: Option<String>,
    pub source: String,
}

/// Same contract as [`McpSearchFacets`], each axis capped to its top 10, each
/// truncation announced by its counter of the rest.
#[derive(Debug, Clone, Serialize)]
pub struct McpLawSearchFacets {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub code: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_codes: Option<usize>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub jurisdiction: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_jurisdictions: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawSearchResponse {
    pub query: String,
    pub total: i64,
    pub hits: Vec<McpLawSearchHit>,
    pub facets: McpLawSearchFacets,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<McpPinned>,
}

// ── Presenters ───────────────────────────────────────────────────────────────

/// The preview follows the *resolved* mode (`query_mode`), not the one asked
/// for: lexical serves `snippet`, the verbatim passage with its `<mark>`
/// stripped; hybrid serves `aiSummary`, the precomputed `hit.summary`, falling
/// back on `snippet` when the summary is missing. Reference follows hybrid, its legs
/// matching an identity card rather than a passage.
pub fn present_search_response(
    response: &SearchResponse,
    web_base_url: &str,
    seats: &[String],
) -> McpSearchResponse {
    let use_summary = matches!(
        response.query_mode,
        QueryMode::Hybrid | QueryMode::Reference
    );
    let hits = response
        .hits
        .iter()
        .map(|hit| {
            let (ai_summary, snippet) = match (use_summary, hit.summary.as_deref()) {
                (true, Some(summary)) if !summary.is_empty() => (Some(summary.to_string()), None),
                _ => (None, Some(strip_marks(&hit.best_chunk.snippet))),
            };
            McpSearchHit {
                // The web hit title already carries the composed bench.
                title: strip_marks(&hit.title_html),
                url: format!("{web_base_url}/decision/{}", hit.id),
                official_url: hit.official_url.clone(),
                ai_summary,
                snippet,
                chars: hit.chars.unwrap_or(0),
                date_lecture: hit.date_lecture.clone(),
                docket_numbers: hit.docket_numbers.clone(),
                seat: tag_key(&hit.seat_path),
                solution: tag_key(&hit.solution),
                procedure: tag_key(&hit.procedure),
                office: tag_key(&hit.office),
                legal_domain: tag_key(&hit.legal_domain),
                publication: publication_keys(&hit.publication_codes),
                cited_articles: hit.cited_articles.clone(),
            }
        })
        .collect();
    // Facets only help refining when the total overflows the served page.
    let facets = response
        .facets
        .as_ref()
        .filter(|_| response.total.unwrap_or_default() > response.hits.len() as i64)
        .and_then(|f| present_search_facets(f, seats));
    let pinned = response
        .pinned
        .iter()
        .map(|d| McpPinned {
            title: d.title.clone(),
            url: format!("{web_base_url}/decision/{}", d.id),
        })
        .collect();
    McpSearchResponse {
        query: response.query.clone(),
        total: response.total.unwrap_or_default(),
        hits,
        facets,
        pinned,
    }
}

const FACET_INSTRUMENTS_LIMIT: usize = 10;
const FACET_OPEN_AXIS_LIMIT: usize = 10;
const FACET_SEAT_RANK_LIMIT: usize = 20;

/// `None` when every axis is empty.
fn present_search_facets(f: &SearchFacets, seats: &[String]) -> Option<McpSearchFacets> {
    fn counts(choices: &[FacetChoice]) -> BTreeMap<String, i64> {
        choices.iter().map(|c| (c.value.clone(), c.count)).collect()
    }
    fn top(choices: &[FacetChoice]) -> BTreeMap<String, i64> {
        let mut sorted: Vec<&FacetChoice> = choices.iter().collect();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.count));
        sorted
            .into_iter()
            .take(FACET_OPEN_AXIS_LIMIT)
            .map(|c| (c.value.clone(), c.count))
            .collect()
    }
    // The roots, then the rank right below each filtered seat. Several seats are
    // OR'd, so each opens its own. Serving every depth instead is the
    // referential: 1 516 keys, 40 kB.
    let depth = |c: &FacetChoice| c.value.split('.').count();
    let mut ranks: BTreeMap<&str, Vec<&FacetChoice>> = BTreeMap::new();
    for c in f.seat.iter().filter(|c| {
        depth(c) == 1
            || seats.iter().any(|s| {
                c.value == *s
                    || (c.value.starts_with(s.as_str())
                        && c.value.as_bytes().get(s.len()) == Some(&b'.')
                        && depth(c) == s.split('.').count() + 1)
            })
    }) {
        let parent = c.value.rsplit_once('.').map_or("", |(p, _)| p);
        ranks.entry(parent).or_default().push(c);
    }
    // A rank is capped against its siblings, never against another rank: a
    // room's count cannot exceed its court's, so one shared cap would serve
    // courts and never a room. At 20 the 13 types pass whole, 302 of the 307
    // room ranks too, and the five that spill lose a tail worth 0.2 % of their
    // mass; only the city lists are really cut (TJ keeps 65.7 % of 151,
    // measured 06/09/2026), and a path composes without the facet naming it.
    let seat: BTreeMap<String, i64> = ranks
        .into_values()
        .flat_map(|mut rank| {
            rank.sort_unstable_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            rank.truncate(FACET_SEAT_RANK_LIMIT);
            rank.into_iter().map(|c| (c.value.clone(), c.count))
        })
        .collect();
    // The token is the text slug, the only key the filters accept: an entry
    // without one is not filterable, so it is not shown and does not eat a slot.
    let mut instruments: Vec<(&String, i64)> = f
        .legal_instrument
        .iter()
        .filter_map(|l| l.slug.as_ref().map(|s| (s, l.count)))
        .collect();
    instruments.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let other_instruments = instruments
        .len()
        .checked_sub(FACET_INSTRUMENTS_LIMIT)
        .filter(|n| *n > 0);
    let legal_instrument: BTreeMap<String, i64> = instruments
        .into_iter()
        .take(FACET_INSTRUMENTS_LIMIT)
        .map(|(slug, count)| (slug.clone(), count))
        .collect();
    let facets = McpSearchFacets {
        seat,
        office: counts(&f.office),
        legal_domain: top(&f.legal_domain),
        solution: counts(&f.solution),
        procedure: counts(&f.procedure),
        publication: counts(&f.publication),
        legal_instrument,
        other_instruments,
    };
    let empty = facets.seat.is_empty()
        && facets.office.is_empty()
        && facets.legal_domain.is_empty()
        && facets.solution.is_empty()
        && facets.procedure.is_empty()
        && facets.publication.is_empty()
        && facets.legal_instrument.is_empty();
    (!empty).then_some(facets)
}

/// Falls back on the raw code when the referential has no entry.
fn type_label(jt: lj_dtos::JurisdictionType, refs: &Referential) -> &str {
    let code = jurisdiction_type_code(jt);
    refs.jurisdiction_type_label(code).unwrap_or(code)
}

/// `FacetTag.key` is a uid suffix, exactly what the `search_decisions` filters
/// accept.
fn tag_key(tag: &Option<FacetTag>) -> Option<String> {
    tag.as_ref().map(|t| t.key.clone())
}

/// Membership is multiple: a decision can carry several publication values.
fn publication_keys(codes: &[String]) -> Vec<String> {
    lj_core::publication::publication_keys(codes)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The resolved citations of the body (`paragraph_spans`, the ones the front
/// makes clickable) are rendered as inline markdown links in `text`.
pub fn present_decision_detail(
    detail: &DecisionDetail,
    web_base_url: &str,
    refs: &Referential,
) -> McpDecisionDetail {
    let text = detail
        .paragraphs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let spans = detail.paragraph_spans.get(i).map_or(&[][..], Vec::as_slice);
            paragraph_with_links(p, spans, web_base_url)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let case_chronology: Vec<McpChronologyStep> = detail
        .chronology
        .iter()
        .map(|e| McpChronologyStep {
            title: lj_core::titles::decision_title(
                &e.label,
                None,
                e.date.as_deref(),
                None,
                e.docket_numbers
                    .as_deref()
                    .and_then(lj_core::titles::docket_display)
                    .as_deref(),
            ),
            url: e
                .id
                .as_ref()
                .map(|id| format!("{web_base_url}/decision/{id}")),
            current: e.current,
            solution: e.solution.clone(),
            link_to_next: e.link.clone(),
        })
        .collect();
    let appellate_fate = appellate_fate(&case_chronology);
    // The known fate opens and closes the text: LLM consumers read the body and
    // skip the metadata, and model attention favours the end of a long output.
    let text = match &appellate_fate {
        Some(fate) => format!(
            "[SORT DE CETTE DÉCISION SUR RECOURS : {fate}]\n\n{text}\n\n[RAPPEL — SORT DE CETTE DÉCISION SUR RECOURS : {fate}]"
        ),
        None => text,
    };
    // The fate marks the title too: that is the field copied when writing a
    // citation.
    let title = {
        let base = decision_title(
            type_label(detail.jurisdiction_type, refs),
            detail.jurisdiction_name.as_deref(),
            detail.seat.as_deref(),
            detail.date_lecture.as_deref(),
            detail.case_name.as_deref(),
            detail.docket_numbers.as_deref(),
        );
        match &appellate_fate {
            Some(fate) => {
                let solution = fate.split(" — ").next().unwrap_or("RECOURS");
                format!("{base} [{solution} SUR RECOURS]")
            }
            None => base,
        }
    };
    McpDecisionDetail {
        title,
        url: format!("{web_base_url}/decision/{}", detail.id),
        official_url: detail.official_url.clone(),
        date_lecture: detail.date_lecture.clone(),
        docket_numbers: detail.docket_numbers.clone(),
        jurisdiction_type: jurisdiction_type_code(detail.jurisdiction_type).to_string(),
        jurisdiction_code: detail.jurisdiction_code.clone(),
        seat: tag_key(&detail.seat_path),
        solution: tag_key(&detail.solution),
        procedure: tag_key(&detail.procedure),
        office: tag_key(&detail.office),
        legal_domain: tag_key(&detail.legal_domain),
        publication: publication_keys(&detail.publication_codes),
        appellate_fate,
        case_chronology,
        commentaires: detail.commentaires.clone(),
        text,
    }
}

/// The step directly above the current decision in the chain, which is ordered
/// most recent first, summed up in one line.
fn appellate_fate(chronology: &[McpChronologyStep]) -> Option<String> {
    let idx = chronology.iter().position(|s| s.current)?;
    let above = chronology.get(idx.checked_sub(1)?)?;
    above.link_to_next.as_ref()?;
    Some(format!(
        "{} — {} ({})",
        above.solution.as_deref().unwrap_or("RECOURS"),
        above.title,
        above.url.as_deref()?
    ))
}

/// Wraps every resolved citation into a `[text](url)` markdown link, without
/// rewriting the span. On several targets, a range or a merge of overlaps, the
/// first resolved one wraps the span and the others are appended right after as
/// labelled links, so every article of the range stays openable. A span with no
/// resolved target stays plain.
///
/// Offsets are codepoints local to the paragraph, and merged spans are ordered
/// and non-overlapping: an invariant of the producer, not checked here.
fn paragraph_with_links(text: &str, spans: &[CitationSpan], web_base_url: &str) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + spans.len() * 64);
    let mut cursor = 0usize;
    for span in spans {
        let mut resolved = span
            .targets
            .iter()
            .filter_map(|t| t.href.as_deref().map(|href| (href, t.label.as_str())));
        let Some((href, _)) = resolved.next() else {
            continue;
        };
        out.extend(&chars[cursor..span.start]);
        out.push('[');
        out.extend(&chars[span.start..span.end]);
        out.push_str("](");
        out.push_str(web_base_url);
        out.push_str(href);
        out.push(')');
        let extras: Vec<String> = resolved
            .map(|(h, l)| format!("[{l}]({web_base_url}{h})"))
            .collect();
        if !extras.is_empty() {
            out.push_str(" (+ ");
            out.push_str(&extras.join(", "));
            out.push(')');
        }
        cursor = span.end;
    }
    out.extend(&chars[cursor..]);
    out
}

/// "Article {num} du/de la/de l'{codeTitle}", the contraction coming from
/// [`lj_dtos::instrument_with_de`], falling back on "Article {num}" when the
/// code title is missing.
fn law_article_title(num: &str, code_title: Option<&str>) -> String {
    match code_title {
        Some(t) if !t.is_empty() => {
            format!("Article {num} {}", lj_dtos::instrument_with_de(t))
        }
        _ => format!("Article {num}"),
    }
}

/// The resolved cross-references of the body (`texte_spans`, with dated hrefs
/// when the article is served at a date) are rendered as inline markdown links
/// in `text`, like the citations of a decision.
pub fn present_law_article(article: &LawArticleResponse, web_base_url: &str) -> McpLawArticle {
    McpLawArticle {
        title: law_article_title(&article.num, article.code_title.as_deref()),
        url: format!(
            "{web_base_url}{}/{}",
            lj_vocab::norm_path(&article.jurisdiction, &article.code),
            article.num_key
        ),
        code: article.code.clone(),
        num: Some(article.num.clone()),
        etat: Some(article.etat.clone()),
        date_debut: Some(article.date_debut.clone()),
        date_fin: article.date_fin.clone(),
        text: article
            .texte
            .as_ref()
            .map(|t| paragraph_with_links(t, &article.texte_spans, web_base_url)),
        source_url: article.source_url.clone(),
        nota: article.nota.clone(),
        section: article.breadcrumb.last().and_then(|division| {
            Some(McpLawSectionRef {
                label: division.label.clone(),
                url: format!("{web_base_url}{}", division.href.as_deref()?),
            })
        }),
        versions: article
            .versions
            .iter()
            .map(|v| McpLawArticleVersion {
                etat: v.etat.clone(),
                date_debut: v.date_debut.clone(),
                date_fin: v.date_fin.clone(),
            })
            .collect(),
        article_count: None,
        articles: Vec::new(),
        omitted: None,
        commentaires: article.commentaires.clone(),
        modified_by: article.modified_by.clone(),
        travaux_parlementaires: article.travaux_parlementaires.clone(),
    }
}

/// A text without articles, 42,563 of the corpus, served in the article shape:
/// a missing `num` says there is no article to ask for, and `text` carries the
/// monolithic body, the only way to read those.
pub fn present_law_text(summary: &lj_dtos::LawCodeSummary, web_base_url: &str) -> McpLawArticle {
    McpLawArticle {
        title: summary.titre.clone(),
        url: format!(
            "{web_base_url}{}",
            lj_vocab::norm_path(&summary.jurisdiction, &summary.code)
        ),
        code: summary.code.clone(),
        num: None,
        etat: summary.status.clone(),
        date_debut: summary.date_texte.clone(),
        date_fin: None,
        text: summary.body.clone(),
        source_url: None,
        nota: None,
        section: None,
        versions: Vec::new(),
        article_count: None,
        articles: Vec::new(),
        omitted: None,
        commentaires: summary.commentaires.clone(),
        modified_by: Vec::new(),
        travaux_parlementaires: Vec::new(),
    }
}

/// `url` is passed back to `get_norm`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLawTocEntry {
    /// `article` or `section`, both passed back to `get_norm`, a section
    /// serving all of its articles at once.
    pub kind: &'static str,
    pub num: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Attached only on a short text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The table of contents of a text with articles, bodies attached when
/// `code_toc` attached them, under the product's `INLINE_READING_MAX`.
pub fn present_law_text_root(
    summary: &lj_dtos::LawCodeSummary,
    toc: &lj_dtos::CodeTocResponse,
    web_base_url: &str,
) -> McpLawArticle {
    let code = &summary.code;
    let text = lj_vocab::norm_path(&summary.jurisdiction, code);
    let link = |num_key: &str| format!("{web_base_url}{text}/{num_key}");
    let section_link = |path: &str| format!("{web_base_url}{text}/{path}");
    let mut articles: Vec<McpLawTocEntry> = Vec::new();
    let mut section = None;
    for item in &toc.reading {
        match item.num_key.as_deref() {
            Some(num_key) => articles.push(McpLawTocEntry {
                kind: "article",
                num: item.label.clone(),
                url: link(num_key),
                section: section.clone(),
                text: item.texte.clone(),
            }),
            None => section = Some(item.label.clone()),
        }
    }
    if articles.is_empty() {
        // On a large text the section is what one call reads, so it enters the
        // toc with its address, not just as a heading.
        let mut path = None;
        for node in &toc.tree {
            match (node.num_key.as_deref(), node.path.as_deref()) {
                (Some(num_key), _) => articles.push(McpLawTocEntry {
                    kind: "article",
                    num: node.label.clone(),
                    url: link(num_key),
                    section: path.clone(),
                    text: None,
                }),
                (None, Some(section_path)) => {
                    articles.push(McpLawTocEntry {
                        kind: "section",
                        num: node.label.clone(),
                        url: section_link(section_path),
                        section: path.clone(),
                        text: None,
                    });
                    path = Some(node.label.clone());
                }
                (None, None) => path = Some(node.label.clone()),
            }
        }
    }
    if articles.is_empty() {
        articles.extend(toc.entries.iter().map(|e| McpLawTocEntry {
            kind: "article",
            num: e.num.clone(),
            url: link(&e.num_key),
            section: e.title_path.clone(),
            text: None,
        }));
    }
    let total = articles.len();
    articles.truncate(lj_dtos::INLINE_READING_MAX);
    let omitted = total - articles.len();
    McpLawArticle {
        code: code.clone(),
        article_count: Some(summary.article_count),
        num: None,
        title: summary.titre.clone(),
        url: format!("{web_base_url}{text}"),
        etat: summary.status.clone(),
        date_debut: summary.date_texte.clone(),
        date_fin: None,
        text: None,
        source_url: None,
        nota: None,
        section: None,
        versions: Vec::new(),
        articles,
        omitted: (omitted > 0).then_some(omitted),
        commentaires: summary.commentaires.clone(),
        modified_by: Vec::new(),
        travaux_parlementaires: Vec::new(),
    }
}

/// The articles of a section in reading order, bodies attached as long as the
/// section fits the reading bounds, otherwise reduced to their index and
/// `omitted` says so. Same shape as an article, without `num`.
pub fn present_law_section(
    section: &lj_dtos::LawSectionResponse,
    web_base_url: &str,
) -> McpLawArticle {
    let code = &section.code;
    let text = lj_vocab::norm_path(&section.jurisdiction, code);
    let mut path: Option<String> = section
        .ancestors
        .last()
        .map(|a| a.label.clone())
        .or_else(|| section.code_title.clone());
    let mut articles = Vec::new();
    for item in &section.items {
        match item.num_key.as_deref() {
            Some(num_key) => articles.push(McpLawTocEntry {
                kind: "article",
                num: item.label.clone(),
                url: format!("{web_base_url}{text}/{num_key}"),
                section: path.clone(),
                text: item.texte.clone(),
            }),
            None => path = Some(item.label.clone()),
        }
    }
    McpLawArticle {
        code: code.clone(),
        article_count: Some(articles.len() as i64),
        num: None,
        title: section.title.clone(),
        url: format!("{web_base_url}{text}/{}", section.path),
        etat: None,
        date_debut: None,
        date_fin: None,
        text: None,
        source_url: None,
        nota: None,
        section: None,
        versions: Vec::new(),
        articles,
        omitted: section.omitted,
        commentaires: Vec::new(),
        modified_by: Vec::new(),
        travaux_parlementaires: Vec::new(),
    }
}

const LAW_FACET_LIMIT: usize = 10;

/// Each facet is capped at ten, the slugs arriving sorted by decreasing count.
pub fn present_law_search(
    query: &str,
    response: &ArticleSearchResponse,
    web_base_url: &str,
) -> McpLawSearchResponse {
    // A zero on the jurisdiction axis is a position outside the searched corpora.
    let counts = |choices: &[lj_dtos::FacetChoice], cap: usize| {
        choices
            .iter()
            .filter(|c| c.count > 0)
            .take(cap)
            .map(|c| (c.value.clone(), c.count))
            .collect::<BTreeMap<_, _>>()
    };
    let rest = |choices: &[lj_dtos::FacetChoice], cap: usize| {
        choices
            .iter()
            .filter(|c| c.count > 0)
            .count()
            .checked_sub(cap)
            .filter(|n| *n > 0)
    };
    McpLawSearchResponse {
        query: query.to_string(),
        total: response.total,
        hits: response
            .hits
            .iter()
            .map(|h| McpLawSearchHit {
                title: law_article_title(&h.num, Some(&h.code_title)),
                url: format!(
                    "{web_base_url}{}/{}",
                    lj_vocab::norm_path(&h.jurisdiction, &h.code),
                    h.num_key
                ),
                snippet: h.snippet.clone(),
                title_path: h.titre_path.clone(),
                source: h.source.clone(),
            })
            .collect(),
        facets: McpLawSearchFacets {
            code: counts(&response.facets.code, LAW_FACET_LIMIT),
            other_codes: rest(&response.facets.code, LAW_FACET_LIMIT),
            jurisdiction: counts(&response.facets.jurisdiction, LAW_FACET_LIMIT),
            other_jurisdictions: rest(&response.facets.jurisdiction, LAW_FACET_LIMIT),
        },
        pinned: response
            .pinned
            .iter()
            .map(|a| McpPinned {
                title: law_article_title(&a.num, Some(&a.code_title)),
                url: format!(
                    "{web_base_url}{}/{}",
                    lj_vocab::norm_path(&a.jurisdiction, &a.code),
                    a.num_key
                ),
            })
            .collect(),
    }
}

// ── Annonces ────────────────────────────────────────────────────────────────

/// Enough to decide whether to go, and where to go: `url` is our page,
/// `platformUrl` the filing platform. The deep links, tender rules and tender
/// documents, stay on the page: an MCP client downloads no tender file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAnnonceHit {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The corpus the notice comes from (`marche`, `bodacc`).
    pub fund: String,
    /// Who the notice is about, and how its corpus names them.
    pub actor: String,
    pub actor_role: String,
    pub url: String,
    pub platform_url: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpv: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    pub published: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The channels publishing the notice (`boamp`, `atexo`…).
    pub channels: Vec<String>,
}

/// The facet tokens are the ones the input filters take.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAnnonceSearchResponse {
    pub total: i64,
    pub hits: Vec<McpAnnonceHit>,
    /// A missing axis is a filter the queried corpora cannot run, not an empty
    /// axis.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub facets: BTreeMap<String, BTreeMap<String, i64>>,
}

/// Facet axes are capped on the top counts, the order of the choices being the
/// referential's own.
pub fn present_annonce_search(
    response: &lj_dtos::AnnonceSearchResponse,
    web_base_url: &str,
) -> McpAnnonceSearchResponse {
    let counts = |choices: &[lj_dtos::FacetChoice], cap: usize| {
        let mut sorted: Vec<&lj_dtos::FacetChoice> = choices.iter().collect();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.count));
        sorted
            .into_iter()
            .take(cap)
            .map(|c| (c.value.clone(), c.count))
            .collect::<BTreeMap<_, _>>()
    };
    const CAP: usize = 15;
    McpAnnonceSearchResponse {
        total: response.total,
        hits: response
            .groupes
            .iter()
            .flat_map(|g| g.hits.iter())
            .map(|h| McpAnnonceHit {
                title: h.titre.clone(),
                description: h.description.clone(),
                fund: h.fond.clone(),
                actor: h.acteur.clone(),
                actor_role: h.acteur_role.clone(),
                url: annonce_url(web_base_url, h),
                platform_url: h.url_action.clone(),
                zones: h.zones.iter().map(|z| z.label.clone()).collect(),
                nature: h.famille.as_ref().map(|f| f.label.clone()),
                cpv: h.cpv.as_ref().map(|c| c.label.clone()),
                deadline: h.echeance.clone(),
                published: h.date_publication.clone(),
                reference: h.reference.clone(),
                channels: h.canaux.clone(),
            })
            .collect(),
        facets: response
            .axes
            .iter()
            .map(|axis| (axis.key.clone(), counts(&axis.choices, CAP)))
            .filter(|(_, m): &(String, BTreeMap<String, i64>)| !m.is_empty())
            .collect(),
    }
}

/// Our own page when we serve one, the official page of the corpus otherwise:
/// the URL is the only key, it has to open.
fn annonce_url(web_base_url: &str, hit: &lj_dtos::AnnonceHit) -> String {
    if hit.fond == lj_vocab::ANNONCE_FOND_MARCHE {
        format!("{web_base_url}{}", lj_vocab::notice_path(&hit.uid))
    } else {
        hit.url_action.clone()
    }
}

fn strip_marks(text: &str) -> String {
    text.replace("<mark>", "").replace("</mark>", "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::{
        ArticleSearchFacets, ArticleSearchHit, ArticleSearchResponse, BestChunk, CitationTarget,
        JurisdictionType, LawArticleResponse, LawArticleVersion, PinnedDecision, QueryMode,
        SearchHit, SearchResponse,
    };
    use lj_store::repository::FacetValueRow;

    /// The type labels and the tags the fixtures need.
    fn refs() -> Referential {
        let fv = |uid: &str, label: &str| FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: None,
            sort: 0,
            hint: None,
        };
        Referential::new(
            vec![
                fv("jurisdiction_type:CC", "Cour de cassation"),
                fv("jurisdiction_type:CE", "Conseil d'État"),
            ],
            Vec::new(),
            Vec::new(),
        )
    }

    fn tag(key: &str, libelle: &str) -> FacetTag {
        FacetTag {
            key: key.into(),
            label: libelle.into(),
        }
    }

    fn base_hit() -> SearchHit {
        SearchHit {
            id: "cc-123".into(),
            jurisdiction_type: JurisdictionType::Cc,
            jurisdiction_code: Some("cc".into()),
            jurisdiction_name: Some("Cour de cassation".into()),
            title_html: "Cour de <mark>cassation</mark>, 29 mai 2026, 24-17.384".into(),
            seat: None,
            score: 1.0,
            best_chunk: BestChunk {
                chunk_index: 0,
                snippet: "un <mark>bail</mark> commercial".into(),
            },
            date_lecture: Some("2026-05-29".into()),
            docket_numbers: Some(vec!["24-17.384".into()]),
            solution: Some(FacetTag {
                key: "REJET".into(),
                label: "Rejet".into(),
            }),
            procedure: None,
            office: None,
            legal_domain: None,
            seat_path: Some(FacetTag {
                key: "CC.SOCIALE".into(),
                label: "Chambre sociale".into(),
            }),
            publication_codes: vec!["b".into()],
            chars: Some(4200),
            summary: Some("Résumé de la décision.".into()),
            official_url: Some(
                "https://www.courdecassation.fr/decision/6194ba445a317cc1d116fbb9".into(),
            ),
            cited_articles: BTreeMap::from([("code-civil".into(), vec!["1240".into()])]),
            matched_docket: None,
        }
    }

    #[test]
    fn lexical_serves_stripped_snippet() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![base_hit()],
            query_mode: QueryMode::Lexical,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        let hit = &out.hits[0];
        assert_eq!(hit.snippet.as_deref(), Some("un bail commercial"));
        assert_eq!(hit.ai_summary, None);
        assert_eq!(hit.title, "Cour de cassation, 29 mai 2026, 24-17.384");
        assert_eq!(hit.url, "https://librejustice.fr/decision/cc-123");
        assert_eq!(hit.chars, 4200);
        assert_eq!(hit.procedure, None);
        assert_eq!(hit.seat.as_deref(), Some("CC.SOCIALE"));
        assert_eq!(hit.solution.as_deref(), Some("REJET"));
        assert_eq!(hit.publication, vec!["PUBLIE_BULLETIN".to_string()]);
    }

    #[test]
    fn hybrid_serves_ai_summary() {
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        assert_eq!(
            out.hits[0].ai_summary.as_deref(),
            Some("Résumé de la décision.")
        );
        assert_eq!(out.hits[0].snippet, None);
    }

    #[test]
    fn hybrid_without_summary_falls_back_to_snippet() {
        let mut hit = base_hit();
        hit.summary = None;
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![hit],
            query_mode: QueryMode::Hybrid,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        assert_eq!(out.hits[0].snippet.as_deref(), Some("un bail commercial"));
        assert_eq!(out.hits[0].ai_summary, None);
    }

    #[test]
    fn facets_compact_to_filter_tokens() {
        let choice = |value: &str, count: i64, parent: Option<&str>| FacetChoice {
            value: value.into(),
            label: format!("Label {value}"),
            count,
            parent: parent.map(str::to_string),
            hint: None,
        };
        let mut seat = vec![choice("TA", 74, None)];
        for i in 0..17 {
            seat.push(choice(&format!("TA.VILLE{i:02}"), 17 - i, Some("TA")));
        }
        seat.push(choice("TA.VILLE00.C1", 4, Some("TA.VILLE00")));
        seat.push(choice("TA.VILLE00.C1.S2", 2, Some("TA.VILLE00.C1")));
        seat.push(choice("TA.VILLE01.C7", 3, Some("TA.VILLE01")));
        let domaines: Vec<FacetChoice> = (0..13)
            .map(|i| choice(&format!("DOMAINE{i:02}"), 13 - i, None))
            .collect();
        let facets = SearchFacets {
            seat,
            legal_domain: domaines,
            solution: vec![choice("REJET", 63, None)],
            legal_instrument: vec![lj_dtos::LegalInstrumentFacet {
                value: "LEGITEXT000006070721".into(),
                label: "Code civil".into(),
                slug: Some("code-civil".into()),
                count: 657,
                articles: vec![],
            }],
            ..Default::default()
        };
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(92),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(facets.clone()),
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        let f = out.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("TA"), Some(&74));
        assert_eq!(f.seat.len(), 1);
        assert_eq!(f.solution.get("REJET"), Some(&63));
        assert_eq!(f.legal_instrument.get("code-civil"), Some(&657));
        assert_eq!(f.legal_domain.len(), 10);
        assert_eq!(f.legal_domain.get("DOMAINE00"), Some(&13));
        assert_eq!(f.legal_domain.get("DOMAINE12"), None);

        let asked = present_search_response(
            &resp,
            "https://librejustice.fr",
            &["TA.VILLE00".to_string()],
        );
        let f = asked.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("TA.VILLE00"), Some(&17));
        assert_eq!(f.seat.get("TA.VILLE00.C1"), Some(&4));
        assert_eq!(f.seat.get("TA.VILLE00.C1.S2"), None);
        assert_eq!(f.seat.get("TA.VILLE01"), None);
        assert_eq!(f.seat.len(), 3);

        let multi = present_search_response(
            &resp,
            "https://librejustice.fr",
            &["TA.VILLE00.C1".to_string(), "TA.VILLE01".to_string()],
        );
        let f = multi.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("TA.VILLE00.C1"), Some(&4));
        assert_eq!(f.seat.get("TA.VILLE00.C1.S2"), Some(&2));
        assert_eq!(f.seat.get("TA.VILLE01.C7"), Some(&3));
        assert_eq!(f.seat.len(), 5);

        let racine = present_search_response(&resp, "https://librejustice.fr", &["TA".to_string()]);
        let f = racine.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.len(), 18);
        assert_eq!(f.seat.get("TA.VILLE16"), Some(&1));

        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(1),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(facets),
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        assert!(out.facets.is_none());
    }

    #[test]
    fn a_deep_rank_is_not_capped_against_a_shallower_one() {
        let choice = |value: &str, count: i64| FacetChoice {
            value: value.into(),
            label: format!("Label {value}"),
            count,
            parent: None,
            hint: None,
        };
        let mut seat = vec![choice("TA", 74)];
        for i in 0..24 {
            seat.push(choice(&format!("TA.VILLE{i:02}"), 24 - i));
        }
        seat.push(choice("TA.VILLE00.C1", 4));
        seat.push(choice("TA.VILLE00.C9", 2));
        let resp = SearchResponse {
            query: "bail".into(),
            total: Some(92),
            hits: vec![base_hit()],
            query_mode: QueryMode::Hybrid,
            facets: Some(SearchFacets {
                seat,
                ..Default::default()
            }),
            all_hit_paths: vec![],
            reranked: false,
            pinned: Vec::new(),
            pinned_more: false,
        };
        let out = present_search_response(
            &resp,
            "https://librejustice.fr",
            &["TA".to_string(), "TA.VILLE00".to_string()],
        );
        let f = out.facets.expect("facets présentes (total > hits)");
        assert_eq!(f.seat.get("TA.VILLE19"), Some(&5));
        assert_eq!(f.seat.get("TA.VILLE20"), None);
        // A room thinner than a dropped city stays: its rank has its own cap.
        assert_eq!(f.seat.get("TA.VILLE00.C9"), Some(&2));
        assert_eq!(f.seat.len(), 23);
    }

    #[test]
    fn detail_concatenates_paragraphs_with_blank_line() {
        let detail = DecisionDetail {
            id: "ce-1".into(),
            jurisdiction_type: JurisdictionType::Ce,
            title: String::new(),
            case_name: None,
            paragraphs: vec!["Para 1.".into(), "Para 2.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            court_place: None,
            date_lecture: Some("2024-08-06".into()),
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            seat_path: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: None,
            chronology: Vec::new(),
            commentaires: vec![],
            renvoyants: vec![],
            citantes: vec![],
            citantes_total: None,
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        assert_eq!(out.text, "Para 1.\n\nPara 2.");
        assert_eq!(out.url, "https://librejustice.fr/decision/ce-1");
        assert_eq!(out.title, "Conseil d'État, 6 août 2024");
    }

    #[test]
    fn detail_renders_case_chronology_with_solution_and_urls() {
        let detail = DecisionDetail {
            id: "tj-1".into(),
            jurisdiction_type: JurisdictionType::Tj,
            title: String::new(),
            case_name: None,
            paragraphs: vec!["Motifs.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            court_place: None,
            date_lecture: Some("2025-01-23".into()),
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            seat_path: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: None,
            chronology: vec![
                lj_dtos::ChronologyEntry {
                    id: Some("ca-9".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Ca),
                    label: "Cour d'appel de Paris".into(),
                    date: Some("2026-06-23".into()),
                    current: false,
                    solution: Some("INFIRMATION".into()),
                    docket_numbers: Some(vec!["25/03815".into()]),
                    link: Some("APPEL_DE".into()),
                },
                lj_dtos::ChronologyEntry {
                    id: Some("tj-1".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Tj),
                    label: "Tribunal judiciaire de Paris".into(),
                    date: Some("2025-01-23".into()),
                    current: true,
                    solution: Some("SATISFACTION_TOTALE".into()),
                    docket_numbers: Some(vec!["21/16038".into()]),
                    link: None,
                },
            ],
            commentaires: vec![],
            renvoyants: vec![],
            citantes: vec![],
            citantes_total: None,
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        assert_eq!(out.case_chronology.len(), 2);
        let ca = &out.case_chronology[0];
        assert_eq!(ca.title, "Cour d'appel de Paris, 23 juin 2026, 25/03815");
        assert_eq!(
            ca.url.as_deref(),
            Some("https://librejustice.fr/decision/ca-9")
        );
        assert_eq!(ca.solution.as_deref(), Some("INFIRMATION"));
        assert_eq!(ca.link_to_next.as_deref(), Some("APPEL_DE"));
        assert!(!ca.current);
        assert!(out.case_chronology[1].current);
        assert_eq!(
            out.appellate_fate.as_deref(),
            Some(
                "INFIRMATION — Cour d'appel de Paris, 23 juin 2026, 25/03815 \
                 (https://librejustice.fr/decision/ca-9)"
            )
        );
        assert!(out.text.starts_with(
            "[SORT DE CETTE DÉCISION SUR RECOURS : INFIRMATION — \
             Cour d'appel de Paris, 23 juin 2026, 25/03815"
        ));
        assert!(out.text.ends_with(
            "[RAPPEL — SORT DE CETTE DÉCISION SUR RECOURS : INFIRMATION — \
             Cour d'appel de Paris, 23 juin 2026, 25/03815 \
             (https://librejustice.fr/decision/ca-9)]"
        ));
        assert!(out.title.ends_with("[INFIRMATION SUR RECOURS]"));
    }

    #[test]
    fn detail_renders_resolved_citations_as_markdown_links() {
        let para = "Vu l'énoncé de l'article 700 du CPC.";
        let span = |targets: Vec<CitationTarget>| CitationSpan {
            start: 17,
            end: 35,
            targets,
        };
        let target = |href: Option<&str>| CitationTarget {
            href: href.map(str::to_string),
            label: "Article 700 du Code de procédure civile".into(),
        };
        let detail = |spans: Vec<CitationSpan>| DecisionDetail {
            id: "ce-1".into(),
            jurisdiction_type: JurisdictionType::Ce,
            title: String::new(),
            case_name: None,
            paragraphs: vec![para.into(), "Rejette la requête.".into()],
            paragraph_spans: vec![spans, vec![]],
            sections: None,
            summary: None,
            jurisdiction_code: None,
            jurisdiction_name: None,
            court_place: None,
            date_lecture: None,
            solution: None,
            procedure: None,
            office: None,
            legal_domain: None,
            publication_codes: vec![],
            date_audience: None,
            docket_numbers: None,
            seat: None,
            seat_path: None,
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: None,
            chronology: Vec::new(),
            commentaires: vec![],
            renvoyants: vec![],
            citantes: vec![],
            citantes_total: None,
        };

        let resolved = detail(vec![span(vec![target(Some(
            "/norm/fr/code-de-procedure-civile/700",
        ))])]);
        let out = present_decision_detail(&resolved, "https://librejustice.fr", &refs());
        assert_eq!(
            out.text,
            "Vu l'énoncé de l'[article 700 du CPC]\
             (https://librejustice.fr/norm/fr/code-de-procedure-civile/700).\
             \n\nRejette la requête."
        );

        let multi = detail(vec![span(vec![
            target(None),
            target(Some("/norm/fr/code-de-procedure-civile/700")),
        ])]);
        let out = present_decision_detail(&multi, "https://librejustice.fr", &refs());
        assert!(out
            .text
            .contains("](https://librejustice.fr/norm/fr/code-de-procedure-civile/700)"));
        assert!(!out.text.contains("(+ "));

        let range = detail(vec![span(vec![
            target(Some("/norm/fr/code-de-procedure-civile/700")),
            CitationTarget {
                href: Some("/norm/fr/code-de-procedure-civile/701".into()),
                label: "701 — Code de procédure civile".into(),
            },
        ])]);
        let out = present_decision_detail(&range, "https://librejustice.fr", &refs());
        assert!(out.text.contains(
            "](https://librejustice.fr/norm/fr/code-de-procedure-civile/700) \
             (+ [701 — Code de procédure civile]\
             (https://librejustice.fr/norm/fr/code-de-procedure-civile/701))"
        ));

        let unresolved = detail(vec![span(vec![target(None)])]);
        let out = present_decision_detail(&unresolved, "https://librejustice.fr", &refs());
        assert_eq!(out.text, format!("{para}\n\nRejette la requête."));
    }

    #[test]
    fn law_article_is_slimmed_and_linked() {
        let article = LawArticleResponse {
            upcoming_version_date: None,
            breadcrumb: Vec::new(),
            legiarti: "LEGIARTI000006419292".into(),
            legitext: "LEGITEXT000006070721".into(),
            code: "code-civil".into(),
            jurisdiction: "FR".into(),
            code_title: Some("Code civil".into()),
            num: "1240".into(),
            num_key: "1240".into(),
            etat: "VIGUEUR".into(),
            date_debut: "2016-10-01".into(),
            source: "legifrance".into(),
            titre_text: None,
            date_fin: None,
            expired: false,
            texte: Some("Sous réserve de l'article 1241, tout fait…".into()),
            texte_spans: vec![CitationSpan {
                start: 18,
                end: 30,
                targets: vec![CitationTarget {
                    href: Some("/norm/fr/code-civil/1241?date=2018-01-01".into()),
                    label: "Article 1241 du Code civil".into(),
                }],
            }],
            texte_original: None,
            lang_original: None,
            translation: "officiel".into(),
            source_asof: Some("2026-06-01".into()),
            source_authority: "source gouvernementale".into(),
            source_official: true,
            source_url: Some(
                "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006419292/2018-01-01"
                    .into(),
            ),
            source_upstream_url: None,
            nota: None,
            versions: vec![LawArticleVersion {
                upcoming: false,
                legiarti: "LEGIARTI000006419292".into(),
                etat: "VIGUEUR".into(),
                date_debut: "2016-10-01".into(),
                date_fin: None,
            }],
            context: vec![],
            modifications: vec![],
            modified_by: vec![],
            cited_by: vec![],
            commentaires: vec![],
            travaux_parlementaires: vec![],
            applicability: None,
        };
        let out = present_law_article(&article, "https://librejustice.fr");
        assert_eq!(out.title, "Article 1240 du Code civil");
        assert_eq!(out.url, "https://librejustice.fr/norm/fr/code-civil/1240");
        assert_eq!(
            out.text.as_deref(),
            Some(
                "Sous réserve de l'[article 1241]\
                 (https://librejustice.fr/norm/fr/code-civil/1241?date=2018-01-01), \
                 tout fait…"
            )
        );
        assert_eq!(out.versions.len(), 1);
        assert_eq!(out.versions[0].etat, "VIGUEUR");
        conforms(
            &lj_mcp_tools::get_norm_output_schema(),
            &serde_json::to_value(&out).unwrap(),
            "get_norm",
        )
        .unwrap();
    }

    #[test]
    fn law_search_builds_citable_hits() {
        let response = ArticleSearchResponse {
            hits: vec![ArticleSearchHit {
                code: "code-de-procedure-civile".into(),
                jurisdiction: "FR".into(),
                code_title: "Code de procédure civile".into(),
                num: "700".into(),
                num_key: "700".into(),
                titre_path: Some("Livre Ier > Titre XVIII".into()),
                snippet: "les <mark>dépens</mark>".into(),
                source: "legifrance".into(),
                status: Some("VIGUEUR".into()),
                date_debut: None,
            }],
            total: 1,
            facets: ArticleSearchFacets {
                scope: Vec::new(),
                code: vec![lj_dtos::FacetChoice {
                    value: "code-de-procedure-civile".into(),
                    label: "code-de-procedure-civile".into(),
                    count: 1,
                    parent: None,
                    hint: None,
                }],
                jurisdiction: vec![],
                nature: vec![],
                source: vec![],
            },
            query_mode: lj_dtos::QueryMode::Lexical,
            pinned: Vec::new(),
            pinned_more: false,
            jurisdictions: Vec::new(),
            reader_place: None,
        };
        let out = present_law_search("dépens", &response, "https://librejustice.fr");
        assert_eq!(out.query, "dépens");
        assert_eq!(out.total, 1);
        let hit = &out.hits[0];
        assert_eq!(hit.title, "Article 700 du Code de procédure civile");
        assert_eq!(
            hit.url,
            "https://librejustice.fr/norm/fr/code-de-procedure-civile/700"
        );
        assert_eq!(out.facets.code.get("code-de-procedure-civile"), Some(&1));
        assert!(out.facets.jurisdiction.is_empty());
        conforms(
            &lj_mcp_tools::search_norms_output_schema(),
            &serde_json::to_value(&out).unwrap(),
            "search_norms",
        )
        .unwrap();
    }

    /// A strict MCP client, the official Python SDK among them, validates
    /// `structuredContent` against the declared `outputSchema`: an undeclared
    /// field or a wrong type gets the whole call rejected client-side, with no
    /// server-side trace.
    fn conforms(
        schema: &serde_json::Value,
        value: &serde_json::Value,
        path: &str,
    ) -> Result<(), String> {
        use serde_json::Value;
        if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
            return if branches.iter().any(|b| conforms(b, value, path).is_ok()) {
                Ok(())
            } else {
                Err(format!("{path}: no anyOf branch accepts {value}"))
            };
        }
        let Some(declared) = schema.get("type").and_then(Value::as_str) else {
            return Ok(());
        };
        let got = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_f64() && !n.is_i64() && !n.is_u64() => "number",
            Value::Number(_) => "integer",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if declared != got && !(declared == "number" && got == "integer") {
            return Err(format!("{path}: declared {declared}, served {got}"));
        }
        match value {
            Value::Object(map) => {
                let open = schema.get("additionalProperties") != Some(&serde_json::json!(false));
                for (key, v) in map {
                    match schema.get("properties").and_then(|p| p.get(key)) {
                        Some(s) => conforms(s, v, &format!("{path}.{key}"))?,
                        None if open => {}
                        None => return Err(format!("{path}.{key}: undeclared field")),
                    }
                }
            }
            Value::Array(items) => {
                if let Some(s) = schema.get("items") {
                    for (i, v) in items.iter().enumerate() {
                        conforms(s, v, &format!("{path}[{i}]"))?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The other direction of `conforms`: every declared property is served by
    /// the fixture. Without it a field added to the DTO and missing from the
    /// schema passes the test, never being serialised, and the strict client is
    /// the one who finds out.
    fn covers(
        schema: &serde_json::Value,
        value: &serde_json::Value,
        except_: &[&str],
        path: &str,
    ) -> Result<(), String> {
        use serde_json::Value;
        // `nullable()` wraps the real schema in an `anyOf` with `null`.
        let real = |s: &Value| -> Value {
            s.get("anyOf")
                .and_then(Value::as_array)
                .and_then(|b| {
                    b.iter()
                        .find(|x| x.get("type").and_then(Value::as_str) != Some("null"))
                })
                .cloned()
                .unwrap_or_else(|| s.clone())
        };
        let (Some(props), Some(map)) = (
            schema.get("properties").and_then(Value::as_object),
            value.as_object(),
        ) else {
            return Ok(());
        };
        for (key, sub) in props {
            let here = format!("{path}.{key}");
            let Some(v) = map.get(key) else {
                if except_.contains(&here.as_str()) {
                    continue;
                }
                return Err(format!("{here}: declared, never served by the fixture"));
            };
            let sub = real(sub);
            match v {
                Value::Object(_) => covers(&sub, v, except_, &here)?,
                Value::Array(items) => {
                    if let (Some(first), Some(items_schema)) = (items.first(), sub.get("items")) {
                        covers(items_schema, first, except_, &format!("{here}[0]"))?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    #[test]
    fn search_response_conforms_to_its_output_schema() {
        let mut hit = base_hit();
        hit.seat_path = Some(tag("CA.PARIS.C3", "Troisième chambre"));
        hit.procedure = Some(tag("REFERE", "Référé"));
        hit.office = Some(tag("PLEIN_CONTENTIEUX", "Plein contentieux"));
        hit.legal_domain = Some(tag("BAIL", "Baux"));
        let resp = SearchResponse {
            query: "19-20.312".into(),
            total: Some(1),
            hits: vec![hit],
            query_mode: QueryMode::Lexical,
            facets: None,
            all_hit_paths: vec![],
            reranked: false,
            pinned: vec![PinnedDecision {
                id: "cc-1".into(),
                jurisdiction_type: lj_dtos::JurisdictionType::Cc,
                title: "Cour de cassation, 12 mars 2020, 19-20.312".into(),
                seat: None,
                solution: None,
                procedure: None,
            }],
            pinned_more: false,
        };
        let out = present_search_response(&resp, "https://librejustice.fr", &[]);
        let served = serde_json::to_value(&out).unwrap();
        assert!(!out.hits[0].publication.is_empty());
        conforms(&lj_mcp_tools::search_output_schema(), &served, "search").unwrap();
        // aiSummary and snippet exclude each other, a lexical hit carrying the
        // snippet; facets declares no property, so there is nothing to cover.
        covers(
            &lj_mcp_tools::search_output_schema(),
            &served,
            &["search.hits[0].aiSummary", "search.facets"],
            "search",
        )
        .unwrap();
    }

    #[test]
    fn decision_detail_conforms_to_its_output_schema() {
        let detail = DecisionDetail {
            id: "cc-1".into(),
            jurisdiction_type: JurisdictionType::Cc,
            title: String::new(),
            case_name: None,
            paragraphs: vec!["Motifs.".into()],
            paragraph_spans: Vec::new(),
            sections: None,
            summary: None,
            jurisdiction_code: Some("cc".into()),
            jurisdiction_name: Some("Cour de cassation".into()),
            court_place: None,
            date_lecture: Some("2026-05-29".into()),
            solution: Some(tag("REJET", "Rejet")),
            procedure: Some(tag("REFERE", "Référé")),
            office: Some(tag("PLEIN_CONTENTIEUX", "Plein contentieux")),
            legal_domain: Some(tag("BAIL", "Baux")),
            publication_codes: vec!["b".into()],
            date_audience: None,
            docket_numbers: Some(vec!["24-17.384".into()]),
            seat: None,
            seat_path: Some(tag("CA.PARIS.C3", "Troisième chambre")),
            legal_references: None,
            source_xml: None,
            themes: Vec::new(),
            nac: None,
            ecli: None,
            source: None,
            official_url: Some("https://www.courdecassation.fr/decision/abc".into()),
            chronology: vec![
                lj_dtos::ChronologyEntry {
                    id: Some("cc-2".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Cc),
                    label: "Cour de cassation".into(),
                    date: Some("2026-05-29".into()),
                    current: false,
                    solution: Some("CASSATION".into()),
                    docket_numbers: Some(vec!["24-17.384".into()]),
                    link: Some("POURVOI_CONTRE".into()),
                },
                lj_dtos::ChronologyEntry {
                    id: Some("cc-1".into()),
                    jurisdiction_type: Some(lj_dtos::JurisdictionType::Ca),
                    label: "Cour d'appel de Paris".into(),
                    date: Some("2024-03-14".into()),
                    current: true,
                    solution: Some("CONFIRMATION".into()),
                    docket_numbers: Some(vec!["22-04.118".into()]),
                    link: None,
                },
            ],
            commentaires: vec![lj_dtos::Commentaire {
                kind: "note".into(),
                author: Some("Cour de cassation".into()),
                date: Some("2026-05-29".into()),
                title: Some("Note explicative".into()),
                publisher: Some("Cour de cassation".into()),
                volume: Some("134".into()),
                issue: Some("2".into()),
                pages: Some("407-422".into()),
                access: Some("libre".into()),
                url: Some("https://www.courdecassation.fr/note/abc".into()),
            }],
            renvoyants: Vec::new(),
            citantes: Vec::new(),
            citantes_total: None,
        };
        let out = present_decision_detail(&detail, "https://librejustice.fr", &refs());
        let served = serde_json::to_value(&out).unwrap();
        assert!(!out.publication.is_empty());
        assert!(out.appellate_fate.is_some());
        conforms(
            &lj_mcp_tools::get_decision_output_schema(),
            &served,
            "decision",
        )
        .unwrap();
        covers(
            &lj_mcp_tools::get_decision_output_schema(),
            &served,
            &[],
            "decision",
        )
        .unwrap();
    }

    fn code_summary(article_count: i64) -> lj_dtos::LawCodeSummary {
        lj_dtos::LawCodeSummary {
            legitext: "LEGITEXT000006070721".into(),
            code: "code-civil".into(),
            jurisdiction: "FR".into(),
            titre: "Code civil".into(),
            nature: "CODE".into(),
            derniere_modification: None,
            article_count,
            upcoming_versions: Vec::new(),
            body: None,
            status: Some("VIGUEUR".into()),
            nor: None,
            date_texte: Some("1804-03-21".into()),
            scope: None,
            fond: None,
            fond_label: None,
            fond_year: None,
            top_cited: Vec::new(),
            commentaires: Vec::new(),
        }
    }

    fn lecture(label: &str, num_key: Option<&str>) -> lj_dtos::LawSectionItem {
        lj_dtos::LawSectionItem {
            kind: if num_key.is_some() {
                "article"
            } else {
                "section"
            }
            .into(),
            depth: 1,
            label: label.into(),
            path: None,
            num_key: num_key.map(str::to_string),
            etat: "VIGUEUR".into(),
            texte: num_key.map(|k| format!("Corps de {k}.")),
            nota: None,
        }
    }

    #[test]
    fn text_root_reads_the_reading_view_then_the_tree_then_the_flat_list() {
        let base = "https://librejustice.fr";
        let vide = || lj_dtos::CodeTocResponse {
            entries: Vec::new(),
            tree: Vec::new(),
            reading: Vec::new(),
        };

        let mut toc = vide();
        toc.reading = vec![
            lecture("Titre Ier", None),
            lecture("Article 1", Some("1")),
            lecture("Article 2", Some("2")),
        ];
        let out = present_law_text_root(&code_summary(2), &toc, base);
        assert_eq!(out.num, None);
        assert_eq!(out.article_count, Some(2));
        assert_eq!(out.omitted, None);
        assert_eq!(out.articles.len(), 2);
        assert_eq!(out.articles[0].url, format!("{base}/norm/fr/code-civil/1"));
        assert_eq!(out.articles[0].section.as_deref(), Some("Titre Ier"));
        assert_eq!(out.articles[0].text.as_deref(), Some("Corps de 1."));

        let mut toc = vide();
        toc.tree = vec![
            lj_dtos::TocNode {
                kind: "section".into(),
                depth: 1,
                label: "Livre Ier".into(),
                path: Some("livre-i".into()),
                num_key: None,
                etat: "VIGUEUR".into(),
            },
            lj_dtos::TocNode {
                kind: "article".into(),
                depth: 2,
                label: "Article 7".into(),
                path: None,
                num_key: Some("7".into()),
                etat: "VIGUEUR".into(),
            },
        ];
        // The section enters the toc with its address: it is what one call
        // reads, article by article being the fallback.
        let out = present_law_text_root(&code_summary(1), &toc, base);
        assert_eq!(out.articles.len(), 2);
        assert_eq!(out.articles[0].kind, "section");
        assert_eq!(
            out.articles[0].url,
            format!("{base}/norm/fr/code-civil/livre-i")
        );
        assert_eq!(out.articles[1].kind, "article");
        assert_eq!(out.articles[1].section.as_deref(), Some("Livre Ier"));
        assert_eq!(out.articles[1].text, None);

        let mut toc = vide();
        toc.entries = vec![lj_dtos::TocEntry {
            num: "L761-1".into(),
            num_key: "l761-1".into(),
            title_path: Some("Livre VII > Titre VI".into()),
            status: "VIGUEUR".into(),
            expired: false,
        }];
        let out = present_law_text_root(&code_summary(1), &toc, base);
        assert_eq!(
            out.articles[0].url,
            format!("{base}/norm/fr/code-civil/l761-1")
        );
        assert_eq!(
            out.articles[0].section.as_deref(),
            Some("Livre VII > Titre VI")
        );
    }

    #[test]
    fn a_toc_too_long_is_cut_and_says_so() {
        let toc = lj_dtos::CodeTocResponse {
            entries: (0..lj_dtos::INLINE_READING_MAX + 12)
                .map(|i| lj_dtos::TocEntry {
                    num: format!("{i}"),
                    num_key: format!("{i}"),
                    title_path: None,
                    status: "VIGUEUR".into(),
                    expired: false,
                })
                .collect(),
            tree: Vec::new(),
            reading: Vec::new(),
        };
        let out = present_law_text_root(&code_summary(162), &toc, "https://librejustice.fr");
        assert_eq!(out.articles.len(), lj_dtos::INLINE_READING_MAX);
        assert_eq!(out.omitted, Some(12));
    }

    /// A section serves its articles in one call, and says when it had to
    /// reduce them to their index.
    #[test]
    fn a_section_serves_its_articles_and_declares_what_it_bounded() {
        let base = "https://librejustice.fr";
        let section = |omitted: Option<usize>, texte: Option<&str>| lj_dtos::LawSectionResponse {
            code: "code-du-travail".into(),
            jurisdiction: "FR".into(),
            code_title: Some("Code du travail".into()),
            path: "partie-legislative/livre-i".into(),
            title: "Sous-section 2 : Dérogations au repos dominical".into(),
            ancestors: vec![lj_dtos::LawSectionRef {
                path: "partie-legislative".into(),
                label: "Section 1 : Repos hebdomadaire".into(),
            }],
            prev: None,
            next: None,
            items: vec![lecture("Article L3132-20", Some("l3132-20"))]
                .into_iter()
                .map(|mut i| {
                    i.texte = texte.map(str::to_string);
                    i
                })
                .collect(),
            omitted,
            top_cited: Vec::new(),
            citing_texts: Vec::new(),
        };

        let out = present_law_section(&section(None, Some("Le repos peut être suspendu.")), base);
        assert_eq!(out.num, None);
        assert_eq!(out.omitted, None);
        assert_eq!(
            out.url,
            format!("{base}/norm/fr/code-du-travail/partie-legislative/livre-i")
        );
        assert_eq!(out.articles.len(), 1);
        assert_eq!(out.articles[0].kind, "article");
        assert_eq!(
            out.articles[0].url,
            format!("{base}/norm/fr/code-du-travail/l3132-20")
        );
        assert_eq!(
            out.articles[0].text.as_deref(),
            Some("Le repos peut être suspendu.")
        );
        assert_eq!(
            out.articles[0].section.as_deref(),
            Some("Section 1 : Repos hebdomadaire")
        );

        // Bounded: the bodies are gone, the link stays, and `omitted` says so.
        let bounded = present_law_section(&section(Some(82), None), base);
        assert_eq!(bounded.omitted, Some(82));
        assert_eq!(bounded.articles[0].text, None);
        assert!(bounded.articles[0].url.ends_with("/l3132-20"));

        let served = serde_json::to_value(&bounded).unwrap();
        conforms(&lj_mcp_tools::get_norm_output_schema(), &served, "section").unwrap();
    }

    /// A root serves no article field: `covers` only holds on the toc entry,
    /// where the four fields are declared.
    #[test]
    fn a_text_root_fits_the_get_norm_schema() {
        let toc = lj_dtos::CodeTocResponse {
            entries: Vec::new(),
            tree: Vec::new(),
            reading: vec![lecture("Titre Ier", None), lecture("Article 1", Some("1"))],
        };
        let out = present_law_text_root(&code_summary(1), &toc, "https://librejustice.fr");
        let served = serde_json::to_value(&out).unwrap();
        let schema = lj_mcp_tools::get_norm_output_schema();
        conforms(&schema, &served, "texte").unwrap();
        covers(
            &schema["properties"]["articles"]["items"],
            &served["articles"][0],
            &[],
            "texte.articles[0]",
        )
        .unwrap();
    }
}
