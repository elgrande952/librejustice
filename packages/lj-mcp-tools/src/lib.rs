//! The MCP tool surface: names, descriptions, input and output schemas.
//!
//! Shared so the remote server and any standalone build expose one definition
//! of every tool, never two that drift.

use serde_json::{json, Value};

use lj_dtos::Domain;

pub const SERVER_INSTRUCTIONS: &str =
    "Search French and European case law: all French courts plus, among \
others, the CNDA, the Conseil constitutionnel, CNIL sanctions, the \
CEDH and the CJUE. The engine understands meaning, not just keywords: \
phrase the query as a natural question or a list of descriptive terms; \
synonyms and reformulations are handled. Typical workflow: \
search_decisions for a shortlist, then get_decision to read candidates \
in full. Previews only orient: never judge relevance, quote, or state \
what a decision holds from one, as the matched passage may be a \
party's argument, not the court's ruling. Iterate by refining the \
query or filters. Open any `url` with the matching tool: /decision/ \
links with get_decision, /norm/ links with get_norm. When \
mentioning a decision, hyperlink its `title` (or a conventional \
citation) to its `url`.";

/// Newest first: `negotiate_protocol_version` falls back to the head. Every
/// revision listed carries the tools surface unchanged, which is all we serve.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The spec has the server answer with the revision the client asked for when
/// it speaks it, and with its own newest otherwise.
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|asked| PROTOCOL_VERSIONS.iter().find(|known| **known == asked))
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

// ── Tools: input schemas ─────────────────────────────────────────────────────

/// Every schema is inlined, with no `title`, and `additionalProperties: false`
/// at the root.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "search_decisions",
            "title": "Search Decisions",
            "description": SEARCH_DECISIONS_DESC,
            "inputSchema": search_decisions_schema(),
            "outputSchema": search_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_decision",
            "title": "Get Decision",
            "description": GET_DECISION_DESC,
            "inputSchema": get_decision_schema(),
            "outputSchema": get_decision_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_norm",
            "title": "Get Norm",
            "description": GET_NORM_DESC,
            "inputSchema": get_norm_schema(),
            "outputSchema": get_norm_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "search_norms",
            "title": "Search Norms",
            "description": SEARCH_NORMS_DESC,
            "inputSchema": search_norms_schema(),
            "outputSchema": search_norms_output_schema(),
            "annotations": read_only_annotations(),
        }),
    ]
}

/// Names an MCP client may still call a tool by, with the tool each now names.
/// A client freezes the tool list when the connector is installed and the model
/// only emits a name from it, so a rename is recoverable by an alias and nothing
/// else; an alias leaves when no call arrives under it any more.
pub const RETIRED_NAMES: [(&str, &str); 6] = [
    ("get_legal_text", "get_norm"),
    ("get_legal_article", "get_norm"),
    ("get_law_article", "get_norm"),
    ("search_legal_texts", "search_norms"),
    ("search_legal_articles", "search_norms"),
    ("search_law_articles", "search_norms"),
];

/// Tools withdrawn with nothing to stand for them: a call gets an error naming
/// the removal rather than an unknown tool.
pub const REMOVED_NAMES: [&str; 1] = ["list_my_activity"];

/// The tool `name` calls: itself, or the one a retired name now stands for.
pub fn canonical_tool_name(name: &str) -> &str {
    RETIRED_NAMES
        .iter()
        .find(|(retired, _)| *retired == name)
        .map_or(name, |(_, current)| current)
}

/// No tool writes anything, so every tool is read-only.
pub fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

pub fn enum_values(values: &[&str]) -> Value {
    json!(values)
}

/// Single source: the `ALL` variants of `lj_dtos::schema`, never a copied list.
fn facet_enum_codes<T: serde::Serialize>(all: &[T]) -> Value {
    json!(all
        .iter()
        .filter_map(|v| serde_json::to_value(v).ok())
        .collect::<Vec<Value>>())
}

/// Optional field, serialised `{"anyOf": [inner, {"type": "null"}],
/// "default": null, <extra…>}`. `extra` carries the sibling keys of `anyOf`:
/// `description` on the inputSchema side, `format` on the outputSchema side.
pub fn nullable(inner: Value, extra: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("anyOf".into(), json!([inner, {"type": "null"}]));
    m.insert("default".into(), Value::Null);
    if let Value::Object(e) = extra {
        m.extend(e);
    }
    Value::Object(m)
}

fn search_decisions_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": lj_dtos::QUERY_MAX_CHARS,
                "description": SEARCH_QUERY_DESC,
            },
            "solution": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Solution::ALL)}}),
                json!({"description": "Filter by the ruling of the operative part (référentiel solution). REJET / IRRECEVABILITE / DESISTEMENT / NON_LIEU_A_STATUER are procedural or negative endings; CONFIRMATION / INFIRMATION* / REFORMATION are appeal outcomes; CASSATION* is cassation-specific; ANNULATION covers administrative annulment; SATISFACTION_TOTALE / SATISFACTION_PARTIELLE cover first-instance civil rulings granting the claim."}),
            ),
            "procedure": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Procedure::ALL)}}),
                json!({"description": "Filter by procedural track (référés, QPC, EU referral, révision, tierce opposition…). Absent value = ordinary contentious procedure."}),
            ),
            "office": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Office::ALL)}}),
                json!({"description": "Filter by specialised judge/office (JLD, JAF, JCP, JEX, juge des enfants, premier président, magistrat désigné). Absent value = ordinary bench."}),
            ),
            "legal_domain": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&Domain::ALL)}}),
                json!({"description": "Filter by legal domain (curated domain tree): 9 roots and their leaves (e.g. CIVIL_DROIT_LOCATIF). Selecting a root also matches all its leaves."}),
            ),
            "publication": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": enum_values(PUBLICATION_CODES)}}),
                json!({"description": "Filter by publication, at the grain the Cour de cassation itself uses (any-of, membership is multiple: a decision matches every value whose code it carries). Judicial order: PUBLIE_RAPPORT (annual report, the strongest signal), PUBLIE_BULLETIN, LETTRE_CHAMBRE, COMMUNIQUE, INEDIT_BULLETIN. Administrative order: PUBLIE_LEBON, MENTIONNE_LEBON, INEDIT_LEBON. Lower courts carry no publication statement and match no value."}),
            ),
            "seat": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 60}}),
                json!({"description": "Restrict to one or more seats, as a dotted path from the court type down to the room: \"CA\" (a whole type), \"CA.PARIS\" and \"TJ.BORDEAUX\" (one court), \"CC.C1_CIVILE\", \"CA.PARIS.P5.C3\", \"TJ.LYON.SOCIALE\", \"TJ.PARIS.REFERES\", \"TA.MARSEILLE.C5\" (one room). A path matches itself and everything below it, so start at the type and go deeper. Types: TJ (tribunal judiciaire), CA (cour d'appel), CC (Cour de cassation), TCOM (tribunal des activités économiques), TA (tribunal administratif), CAA (cour administrative d'appel), CE (Conseil d'État), CNDA (asylum), CONSTIT (Conseil constitutionnel), TC (Tribunal des conflits), CNIL (sanctions), CEDH and CJUE (European courts). Take the paths from facets.seat."}),
            ),
            "legal_instrument": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 100}}),
                json!({"description": "Restrict to decisions citing one or more given codes or statutes. Accepts a slug from facets.legal_instrument (e.g. \"code-civil\") or an exact text name resolved server-side (e.g. \"Code civil\")."}),
            ),
            "legal_article": nullable(
                json!({"type": "array", "maxItems": 20, "items": {"type": "string", "maxLength": 140}}),
                json!({"description": "Restrict to decisions citing a specific article of a specific code, as a composite key \"<instrument>|<article>\" where <instrument> is a slug or an exact text name, resolved like legal_instrument (e.g. \"code-civil|1240\", \"code-de-justice-administrative|L761-1\"). The instrument prefix is required: the same article number exists in several codes."}),
            ),
            "date_from": nullable(
                json!({"type": "string"}),
                json!({"description": "Earliest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "date_to": nullable(
                json!({"type": "string"}),
                json!({"description": "Latest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "mode": {
                "type": "string",
                "enum": ["auto", "lexical", "semantic", "reference"],
                "default": "auto",
                "description": SEARCH_MODE_DESC,
            },
            "sort": {
                "type": "string",
                "enum": ["relevance", "date_desc", "date_asc"],
                "default": "relevance",
                "description": "Result ordering. Use 'relevance' (default) unless the user wants chronological order.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (default 10).",
            },
            "ai_rerank": {
                "type": "boolean",
                "default": true,
                "description": "When enabled (default), reorders results by actual relevance to the query using an LLM reranker. Keep on for agentic use, shortlist quality is significantly higher. Cost: a few seconds of extra latency. Disable only for high-rate exploratory searches where latency matters more than ranking quality.",
            },
        },
    })
}

fn get_decision_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": {
                "type": "string",
                "minLength": 1,
                "description": "A librejustice.fr decision URL, from a search_decisions hit or an inline citation link; search_decisions first if you have neither.",
            },
        },
    })
}

/// Three doors to an article, plus `date`. No root combinator says which one is
/// required: that is checked in `law_target` instead.
fn get_norm_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "url": {
                "type": "string",
                "description": LAW_URL_DESC,
            },
            "code": {
                "type": "string",
                "description": "Code slug, when you hold the code and the article apart rather than as a url (\"code-civil\"). Alone, addresses the text itself.",
            },
            "article": {
                "type": "string",
                "description": "Lowercase article key, alongside `code` (\"l761-1\" for L. 761-1, \"1240\" for 1240).",
            },
            "textId": {
                "type": "string",
                "description": "Légifrance id: an article (LEGIARTI, JORFARTI, KALIARTI), a text (LEGITEXT, JORFTEXT) or a section (LEGISCTA).",
            },
            "date": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD): returns the version in force at that date. Omit for the version currently in force."}),
            ),
        },
    })
}

fn search_norms_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": lj_dtos::QUERY_MAX_CHARS,
                "description": "French query over legal articles. Matches article titles (boosted) and bodies; alias expansion handles acronyms and usual names.",
            },
            "code": nullable(
                json!({"type": "string"}),
                json!({"description": "Restrict to one code/text by its URL slug (\"code-civil\", as in facets.code) or exact name. Omit to search the whole navigable referential."}),
            ),
            "jurisdiction": nullable(
                json!({"type": "string"}),
                json!({"description": "Filter by country/legal order, as an ISO 3166 alpha-2 country code: \"FR\" (France, the bulk of the corpus) or a foreign code (\"SN\", \"DZ\", \"MA\", \"VN\", \"PE\", …); plus \"UE\" for EU law and \"INTL\" for treaties/international law."}),
            ),
            "date": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD): searches the versions valid at that date (point-in-time, same semantics as get_norm). Omit to search the versions currently in force."}),
            ),
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (default 10).",
            },
        },
    })
}

// ── Tools: output schemas ────────────────────────────────────────────────────

/// Mirrors `McpSearchResponse`.
pub fn search_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "total", "hits"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "chars"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "officialUrl": nullable(json!({"type": "string"}), json!({})),
                        "aiSummary": nullable(json!({"type": "string"}), json!({})),
                        "snippet": nullable(json!({"type": "string"}), json!({})),
                        "chars": {"type": "integer"},
                        "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
                        "docketNumbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
                        "seat": nullable(json!({"type": "string"}), json!({})),
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "procedure": nullable(json!({"type": "string"}), json!({})),
                        "office": nullable(json!({"type": "string"}), json!({})),
                        "legalDomain": nullable(json!({"type": "string"}), json!({})),
                        "publication": {"type": "array", "items": {"type": "string"}},
                        "citedArticles": {"type": "object", "additionalProperties": {"type": "array", "items": {"type": "string"}}},
                    },
                },
            },
            "pinned": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                    },
                },
            },
            "facets": {
                "type": "object",
                "additionalProperties": true,
            },
        },
    })
}

/// Mirrors `McpDecisionDetail`.
pub fn get_decision_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["title", "url", "jurisdictionType", "text"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "officialUrl": nullable(json!({"type": "string"}), json!({})),
            "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "docketNumbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
            "jurisdictionType": {"type": "string"},
            "jurisdictionCode": nullable(json!({"type": "string"}), json!({})),
            "seat": nullable(json!({"type": "string"}), json!({})),
            "solution": nullable(json!({"type": "string"}), json!({})),
            "procedure": nullable(json!({"type": "string"}), json!({})),
            "office": nullable(json!({"type": "string"}), json!({})),
            "legalDomain": nullable(json!({"type": "string"}), json!({})),
            "publication": {"type": "array", "items": {"type": "string"}},
            "appellateFate": nullable(json!({"type": "string"}), json!({})),
            "caseChronology": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "current"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": nullable(json!({"type": "string"}), json!({})),
                        "current": {"type": "boolean"},
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "linkToNext": nullable(json!({"type": "string"}), json!({})),
                    },
                },
            },
            "commentaires": {
                "type": "array",
                "items": commentaire_schema(),
            },
            "text": {"type": "string"},
        },
    })
}

/// Mirrors `lj_dtos::Commentaire`, shared by `get_decision` and
/// `get_norm`.
fn commentaire_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["kind"],
        "properties": {
            "kind": {"type": "string", "enum": ["conclusions", "note"]},
            "author": nullable(json!({"type": "string"}), json!({})),
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "title": nullable(json!({"type": "string"}), json!({})),
            "publisher": nullable(json!({"type": "string"}), json!({})),
            "volume": nullable(json!({"type": "string"}), json!({})),
            "issue": nullable(json!({"type": "string"}), json!({})),
            "pages": nullable(json!({"type": "string"}), json!({})),
            "access": nullable(json!({"type": "string", "enum": ["libre", "abonnes"]}), json!({})),
            "url": nullable(json!({"type": "string"}), json!({})),
        },
    })
}

/// Mirrors `LinkedTextRef`: a text named by an edge of the norm graph.
fn linked_text_ref_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["label"],
        "properties": {
            "label": {"type": "string"},
            "href": nullable(json!({"type": "string"}), json!({})),
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
        },
    })
}

/// Mirrors `McpLawArticle`.
pub fn get_norm_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["code", "title", "url", "versions"],
        "properties": {
            "code": {"type": "string"},
            "num": {"type": "string"},
            "articleCount": {"type": "integer"},
            "articles": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["kind", "num", "url"],
                    "properties": {
                        "kind": {"type": "string", "enum": ["article", "section"]},
                        "num": {"type": "string"},
                        "url": {"type": "string"},
                        "section": {"type": "string"},
                        "text": {"type": "string"},
                    },
                },
            },
            "omitted": {"type": "integer"},
            "title": {"type": "string"},
            "url": {"type": "string"},
            "etat": nullable(json!({"type": "string"}), json!({})),
            "dateDebut": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "dateFin": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "text": nullable(json!({"type": "string"}), json!({})),
            "sourceUrl": nullable(json!({"type": "string"}), json!({})),
            "nota": nullable(json!({"type": "string"}), json!({})),
            "section": {
                "additionalProperties": false,
                "type": "object",
                "required": ["label", "url"],
                "properties": {
                    "label": {"type": "string"},
                    "url": {"type": "string"},
                },
            },
            "commentaires": {
                "type": "array",
                "items": commentaire_schema(),
            },
            "modifiedBy": {
                "type": "array",
                "items": linked_text_ref_schema(),
            },
            "travauxParlementaires": {
                "type": "array",
                "items": linked_text_ref_schema(),
            },
            "versions": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["etat", "dateDebut"],
                    "properties": {
                        "etat": {"type": "string"},
                        "dateDebut": {"type": "string", "format": "date"},
                        "dateFin": nullable(json!({"type": "string"}), json!({"format": "date"})),
                    },
                },
            },
        },
    })
}

/// Mirrors `McpLawSearchResponse`.
pub fn search_norms_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "total", "hits", "facets"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "snippet", "source"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "snippet": {"type": "string"},
                        "titlePath": nullable(json!({"type": "string"}), json!({})),
                        "source": {"type": "string"},
                    },
                },
            },
            "pinned": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                    },
                },
            },
            "facets": {
                "type": "object",
                "additionalProperties": true,
            },
        },
    })
}

// ── Enum codes, aligned on the lj-core serde renames ─────────────────────────

/// Suffixes of the `publication:*` uids, in menu order.
const PUBLICATION_CODES: &[&str] = &[
    "PUBLIE_RAPPORT",
    "PUBLIE_BULLETIN",
    "LETTRE_CHAMBRE",
    "COMMUNIQUE",
    "INEDIT_BULLETIN",
    "PUBLIE_LEBON",
    "MENTIONNE_LEBON",
    "INEDIT_LEBON",
];

// ── Tool descriptions ────────────────────────────────────────────────────────

const SEARCH_DECISIONS_DESC: &str = "Search decisions by meaning and keywords combined. Returns a \
shortlist: `title`, `url`, an overview (`aiSummary`: AI-written \
summary of what the decision is about, a machine paraphrase never \
quotable as the court's words; or `snippet`: the verbatim passage \
where your keywords matched) and metadata; get_decision reads the \
full text. Put constraints in the structured filters (jurisdiction, \
dates, articles, codes), keep the query for the legal issue. Values \
within one filter are OR'd; different filters are AND'd. The response \
carries a `facets` block: per filter name, a map of filter value to \
decision count under the current query. Reuse those keys verbatim to \
refine. \
Hit metadata fields carry the same tokens under the same names: a \
hit's `seat`, `legalDomain` or `solution` passes back \
verbatim into the matching filter.";

const GET_DECISION_DESC: &str = "Fetch the full text and metadata of a decision by its `url`. The \
text carries inline markdown links to cited articles (/norm/, open with \
get_norm) and cited decisions (/decision/, open with \
get_decision); a citation spanning several articles (« articles 3 à 6 \
», « et suivants ») links its first article and appends the others as \
labelled links right after the span. `appellateFate` states in one line what became of THIS \
decision on review (INFIRMATION = reversed, it no longer stands; \
CONFIRMATION = upheld): read it before citing the decision as \
authority. `caseChronology` lists the prior AND subsequent \
decisions of the same case (appeal, pourvoi, renvoi). An absent fate \
or chronology never proves no recourse exists, \
only that none is linked in the corpus. `commentaires` links out to the \
rapporteur public's conclusions and the court's related documents: \
context, never the ruling, so quote the decision text and not a \
commentaire.";

const GET_NORM_DESC: &str = "Fetch a legal text, or one of its articles, as it stood on a given \
date. Returns the version in force at `date` (omit for today): full \
text, status, validity dates, and the timeline of all versions: say \
which version you quote. The text carries inline markdown links to \
cross-referenced articles (/norm/, open with get_norm; when \
served at a date, the links point to the same date). `commentaires` \
links out to commentary anchored on the article. `num` absent means \
you hold the whole text; \
the `url` of an article's `section`, or of a `section` entry of \
`articles`, returns all its articles at once.";

const SEARCH_NORMS_DESC: &str =
    "Find legal articles from their subject, their wording, or an article \
number with no code named; returns a shortlist with highlighted \
snippets and the exact `total`. Query in French, descriptive terms (« \
délai de recours contentieux refus implicite »); put the code in the \
`code` filter (slug or exact name), keep the query for the subject. \
The response carries a `facets` block (`code`, `jurisdiction`): per \
filter name, a map of filter value to article count, reuse those keys \
verbatim to refine. Chain a hit into get_norm with its \
`url`, plus `date` when the dispute is governed by an earlier version.";

const LAW_URL_DESC: &str = "A librejustice.fr article URL \
`/norm/{juridiction}/{code}/{article}`: take it from a search_norms hit \
or an inline citation link, or compose it: {juridiction} is the enacting state \
as ISO 3166-1 alpha-2 lowercase (\"fr\", \"be\"), \"eu\" for the Union, its \
treaties included, \"intl\" for a treaty signed outside it (the ECHR, a \
bilateral accord), {code} is the code slug (\"code-civil\", as in \
facets.legal_instrument), {article} the lowercase article key (\"l761-1\" for \
L. 761-1, \"1240\" for 1240). `/norm/{juridiction}/{code}` alone addresses the \
text itself, which is how a text with no articles (circulaire, publication \
decree) is read.";

const SEARCH_QUERY_DESC: &str = "French query, the primary input. Two regimes, pick the right \
tool for the job: \
(a) Natural language or descriptive keywords for legal-issue \
searches; the engine handles synonyms and reformulations. \
Examples: \
« responsabilité hôpital infection nosocomiale » ; \
« étranger malade soins inaccessibles dans son pays d'origine » ; \
« licenciement discrimination syndicale charge de la preuve ». \
(b) Quoted exact phrases and ET / OU / SAUF operators for a \
named entity (company, municipality, person), a precise legal \
formula, or the direction of a holding: semantic matching \
ignores negations (« n'est pas X » ranks like « est X »), and \
only an exact phrase targets which way the court ruled. Examples: \
« \"Société Générale\" » ; \
« \"commune de Saint-Denis\" SAUF Réunion » ; \
« \"force majeure\" ET épidémie ». \
Using operators or quotes switches the engine to keyword-only \
matching for the whole query (no synonyms), so do not mix the \
two regimes.";

const SEARCH_MODE_DESC: &str = "'auto' is the right choice in almost all cases, it picks \
the regime from the query shape. \
auto: combines meaning and keyword matching on plain queries; \
switches to keyword-only when the query contains ET / OU / \
SAUF or quoted phrases. \
lexical: force keyword-only matching. Every term becomes \
required and nothing recovers a miss, so it fits a string you \
expect word for word (a consecrated formula, the direction of a \
holding, a party named in the text) and an empty answer is then \
an answer. \
reference: force the lookup by identity, on the docket number, \
the case name, or the court and the date. \
semantic: force the meaning+keyword hybrid even on quoted or \
operator queries (where auto would fall back to keyword-only).";

#[cfg(test)]
mod tests {
    use super::*;

    /// A client reads the schema as sent: no `$ref` to resolve, no root `title`
    /// to strip, and an unknown argument refused rather than ignored.
    #[test]
    fn search_schema_is_strict_and_inlined() {
        let schema = search_decisions_schema();
        assert_eq!(schema["additionalProperties"], json!(false));
        assert!(schema.get("title").is_none());
        assert!(schema["properties"]["query"].get("title").is_none());
        assert_eq!(schema["required"], json!(["query"]));
        let dumped = serde_json::to_string(&schema).unwrap();
        assert!(!dumped.contains("$ref"));
        assert!(!dumped.contains("$defs"));
    }

    /// The standalone server reads `data/mcp-tools.json`; `LIBREJUSTICE_BLESS_MCP_TOOLS=1` rewrites it.
    #[test]
    fn the_committed_surface_matches_the_definitions() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/mcp-tools.json");
        let built = serde_json::to_string_pretty(&json!({
            "instructions": SERVER_INSTRUCTIONS,
            "protocolVersions": PROTOCOL_VERSIONS,
            "tools": tool_definitions(),
        }))
        .unwrap()
            + "\n";
        if std::env::var_os("LIBREJUSTICE_BLESS_MCP_TOOLS").is_some() {
            std::fs::write(path, &built).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(path).expect("data/mcp-tools.json");
        assert_eq!(
            committed, built,
            "data/mcp-tools.json is stale: LIBREJUSTICE_BLESS_MCP_TOOLS=1 cargo test -p lj-mcp-tools"
        );
    }
}
