//! Hydratation DB (metadata + facettes), denylist procédurale, construction des
//! snippets et assemblage des `SearchHit` d'une page.
//!
//! Les facettes et les tags des hits sont résolus depuis le référentiel DB
//! ([`crate::referential::Referential`]) : les compteurs se groupent par colonnes
//! `_uid`/`jurisdiction_code`, les libellés viennent du cache.

use std::collections::HashMap;

use deadpool_postgres::Client;
use lj_core::truecase;
use lj_store::error::StoreError;

use crate::error::{ApiError, Result};
use crate::referential::{uid_suffix, Referential};
use crate::snippets;
use crate::snippets::highlight;

use lj_dtos::{
    BestChunk, FacetChoice, FacetTag, JurisdictionType, LegalInstrumentFacet, SearchFacets,
    SearchHit,
};

use super::legs::LegHit;

const SNIPPET_MAX_CHARS: usize = 280;
const TITLE_SNIPPET_CHARS: usize = 500;
const LEGAL_INSTRUMENT_FACET_LIMIT: usize = 30;
const LEGAL_ARTICLE_FACET_LIMIT: usize = 20;

// ── Facettes ─────────────────────────────────────────────────────────────

/// Compteurs bruts d'une réponse de facettes, avant résolution des libellés.
/// Deux chemins les remplissent, le parcours d'un jeu de hits et les agrégats
/// du fonds filtré ; passer par la même structure est ce qui garantit une forme
/// de réponse identique.
#[derive(Default)]
pub(crate) struct FacetCounters {
    /// Unfolded seat paths (`CA`, `CA.PARIS`, `CA.PARIS.P5.C3`), the whole
    /// tree: a path carries its own depth, so one map is enough.
    pub(crate) seats: HashMap<String, i64>,
    pub(crate) office: HashMap<String, i64>,
    pub(crate) domain: HashMap<String, i64>,
    pub(crate) solution: HashMap<String, i64>,
    pub(crate) procedure: HashMap<String, i64>,
    /// Clés déjà préfixées `publication:`.
    pub(crate) publication: HashMap<String, i64>,
    /// Années en texte, telles qu'elles sortent en `value` de facette.
    pub(crate) year: HashMap<String, i64>,
    pub(crate) instrument: InstrumentCounters,
}

/// Compteurs de « Textes cités ». Vides, ils rendent une facette vide : c'est
/// ainsi que le mode parcours s'en dispense.
#[derive(Default)]
pub(crate) struct InstrumentCounters {
    pub(crate) counts: HashMap<String, i64>,
    pub(crate) labels: HashMap<String, String>,
    pub(crate) slugs: HashMap<String, Option<String>>,
    pub(crate) articles: HashMap<String, HashMap<String, i64>>,
}

/// Résout les libellés et l'ordre : compteurs bruts vers réponse de facettes.
pub(crate) fn facets_from_counters(c: &FacetCounters, refs: &Referential) -> SearchFacets {
    let li_facets: Vec<LegalInstrumentFacet> =
        most_common(&c.instrument.counts, LEGAL_INSTRUMENT_FACET_LIMIT)
            .into_iter()
            .map(|(uid, count)| {
                let articles = c
                    .instrument
                    .articles
                    .get(&uid)
                    .map(|a| {
                        most_common(a, LEGAL_ARTICLE_FACET_LIMIT)
                            .into_iter()
                            .map(|(value, count)| FacetChoice {
                                label: value.clone(),
                                value,
                                count,
                                parent: None,
                                hint: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                LegalInstrumentFacet {
                    label: c
                        .instrument
                        .labels
                        .get(&uid)
                        .cloned()
                        .unwrap_or_else(|| uid.clone()),
                    slug: c.instrument.slugs.get(&uid).cloned().flatten(),
                    value: uid,
                    count,
                    articles,
                }
            })
            .collect();

    SearchFacets {
        seat: seat_choices(&c.seats, refs),
        office: uid_choices(&c.office, refs),
        legal_domain: group_choices(&c.domain, refs),
        solution: uid_choices(&c.solution, refs),
        procedure: uid_choices(&c.procedure, refs),
        publication: scale_choices(&c.publication, refs),
        date_lecture_year: ranked(&c.year)
            .into_iter()
            .map(|(y, count)| FacetChoice {
                label: y.clone(),
                value: y,
                count,
                parent: None,
                hint: None,
            })
            .collect(),
        legal_instrument: li_facets,
    }
}

/// Clé de tri naturelle d'un libellé, chiffres comparés en nombres : « 2e
/// chambre » avant « 10e chambre », et les formations numérotées avant les
/// nommées, leur première suite de texte étant vide.
fn label_sort_key(label: &str) -> Vec<(String, u64)> {
    let folded = lj_core::normalizer::strip_accents(label).to_lowercase();
    let mut out: Vec<(String, u64)> = Vec::new();
    let mut rest = folded.as_str();
    while !rest.is_empty() {
        let digit = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (text, tail) = rest.split_at(digit);
        let end = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        let (digits, tail) = tail.split_at(end);
        out.push((text.to_string(), digits.parse().unwrap_or(0)));
        rest = tail;
    }
    out
}

/// Clé de tri d'une cour : sa ville, obtenue en retirant le libellé du type puis
/// l'article. Sur le libellé entier, l'apostrophe précède toute lettre et range
/// « Cour d'appel d'Orléans » entre Angers et Basse-Terre.
fn court_sort_key(label: &str, type_label: Option<&str>) -> Vec<(String, u64)> {
    let reste = type_label
        .and_then(|t| label.strip_prefix(t))
        .map(str::trim_start)
        .unwrap_or(label);
    let ville = ["de la ", "des ", "du ", "de ", "d'", "l'"]
        .iter()
        .find_map(|a| reste.strip_prefix(a))
        .unwrap_or(reste);
    label_sort_key(ville)
}

/// Tri d'un compteur en `(clé, count)` : count décroissant, tie-break par clé.
fn ranked(counter: &HashMap<String, i64>) -> Vec<(String, i64)> {
    let mut items: Vec<(String, i64)> = counter.iter().map(|(k, v)| (k.clone(), *v)).collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    items
}

/// Choix d'une facette plate keyée par **uid complet** : `value` = suffixe
/// d'uid, `label` résolu depuis le référentiel.
fn uid_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    ranked(counter)
        .into_iter()
        .map(|(uid, count)| choice(uid, count, refs))
        .collect()
}

/// Les mêmes choix, mais dans l'ordre du référentiel : une facette dont les
/// valeurs forment une échelle se lit du plus fort au plus faible, là où le
/// compteur mettrait l'inédit devant le rapport annuel.
fn scale_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    let mut uids: Vec<&String> = counter.keys().collect();
    uids.sort_by_key(|uid| (refs.rank(uid), uid.as_str()));
    uids.into_iter()
        .map(|uid| choice(uid.clone(), counter[uid], refs))
        .collect()
}

fn choice(uid: String, count: i64, refs: &Referential) -> FacetChoice {
    FacetChoice {
        value: uid_suffix(&uid).to_string(),
        label: refs.label(&uid).to_string(),
        count,
        parent: None,
        hint: refs.hint(&uid),
    }
}

// ── DB hydration / facets ────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub(crate) struct DecisionMeta {
    pub(crate) public_id: String,
    pub(crate) jurisdiction_type: String,
    pub(crate) jurisdiction_code: Option<String>,
    pub(crate) date_lecture: Option<String>,
    pub(crate) solution_uid: Option<String>,
    pub(crate) procedure_uid: Option<String>,
    pub(crate) office_uid: Option<String>,
    pub(crate) legal_domain_uid: Option<String>,
    pub(crate) publication_codes: Vec<String>,
    pub(crate) docket_numbers: Option<Vec<String>>,
    pub(crate) summary: Option<String>,
    pub(crate) chars: Option<i64>,
    /// Axes formation : position recomposée + type de formation, pour le siège du
    /// titre des hits.
    pub(crate) seat: Option<lj_core::chamber::Seat>,
    pub(crate) seat_paths: Vec<String>,
    pub(crate) formation_uid: Option<String>,
    pub(crate) official_url: Option<String>,
    pub(crate) case_name: Option<String>,
}

/// Identifiants publics de la page.
pub(crate) async fn fetch_pub_ids(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, String>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT d.id, d.public_id FROM decisions d WHERE d.id = ANY($1)",
            &[&decision_ids],
        )
        .await?;
    let mut pub_ids: HashMap<i64, String> = HashMap::new();
    for row in &rows {
        pub_ids.insert(row.get(0), row.get(1));
    }
    Ok(pub_ids)
}

/// Seat facet: one choice per path, `parent` being the path minus its last
/// segment. Roots and courts take their label from the referential, which names
/// them better than their segment does; rooms take theirs from the grammar of
/// the path.
fn seat_choices(seats: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    let mut rows: Vec<(&String, &i64)> = seats.iter().collect();
    rows.sort_by_cached_key(|(path, _)| {
        let depth = path.split('.').count();
        let racine = path.split('.').next().unwrap_or_default().to_string();
        let libelle = seat_label(path, refs);
        let ordre = match depth {
            1 => Vec::new(),
            2 => court_sort_key(&libelle, None),
            _ => label_sort_key(&libelle),
        };
        (
            refs.rank(&format!("jurisdiction_type:{racine}")),
            depth.min(3),
            ordre,
            path.to_string(),
        )
    });
    rows.into_iter()
        .map(|(path, count)| FacetChoice {
            label: seat_label(path, refs),
            value: path.clone(),
            count: *count,
            parent: path.rsplit_once('.').map(|(parent, _)| parent.to_string()),
            hint: None,
        })
        .collect()
}

/// Label of a seat node, folded back onto its segment when nothing names it.
fn seat_label(path: &str, refs: &Referential) -> String {
    if path.find('.').is_none() {
        return refs
            .jurisdiction_type_label(path)
            .unwrap_or(path)
            .to_string();
    }
    if let Some(label) = refs.court_label_of_path(path) {
        return label.to_string();
    }
    lj_core::chamber::seat_path_label(path)
        .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(path).to_string())
}

/// Facette hiérarchisée (arbre 2 niveaux, domaine et matière) : le count d'un
/// groupe agrège ses annotations directes + celles de ses membres ; `value` =
/// suffixe d'uid, `parent` (membres) = suffixe du groupe.
fn group_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    let mut root_counts: HashMap<String, i64> = HashMap::new();
    let mut leaf_counts: HashMap<String, i64> = HashMap::new();
    for (uid, count) in counter {
        match refs.value(uid).and_then(|e| e.parent_uid.clone()) {
            Some(root_uid) => {
                *root_counts.entry(root_uid).or_insert(0) += count;
                *leaf_counts.entry(uid.clone()).or_insert(0) += count;
            }
            None => *root_counts.entry(uid.clone()).or_insert(0) += count,
        }
    }
    let mut out: Vec<FacetChoice> = ranked(&root_counts)
        .into_iter()
        .map(|(uid, count)| FacetChoice {
            value: uid_suffix(&uid).to_string(),
            label: refs.label(&uid).to_string(),
            count,
            parent: None,
            hint: refs.hint(&uid),
        })
        .collect();
    out.extend(ranked(&leaf_counts).into_iter().map(|(uid, count)| {
        let parent = refs
            .value(&uid)
            .and_then(|e| e.parent_uid.as_deref())
            .map(|p| uid_suffix(p).to_string());
        FacetChoice {
            value: uid_suffix(&uid).to_string(),
            label: refs.label(&uid).to_string(),
            count,
            parent,
            hint: refs.hint(&uid),
        }
    }));
    out
}

/// `Counter.most_common(n)` : tri par count décroissant puis insertion (ici
/// déterminisé par clé pour la reproductibilité, faute d'ordre d'insertion).
fn most_common(counter: &HashMap<String, i64>, n: usize) -> Vec<(String, i64)> {
    let mut items = ranked(counter);
    items.truncate(n);
    items
}

pub(crate) async fn hydrate_decisions(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, DecisionMeta>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code, \
             d.date_lecture, d.solution_uid, d.procedure_uid, d.office_uid, \
             d.legal_domain_uid, d.publication_codes, \
             d.docket_numbers, d.summary, c.chars, \
             d.seat, d.seat_paths, d.formation_uid, \
             src.source, src.source_uid, \
             src.source_fields->'Dossier'->>'Code_Juridiction', \
             COALESCE(src.source_fields->'META_JURI_CONSTIT'->>'URL_CC', \
                      src.source_fields->>'fiche_url'), \
             src.source_fields->>'decision_date', \
             src.source_fields->'numbers'->>0, \
             d.case_name \
             FROM decisions d \
             LEFT JOIN LATERAL (SELECT max(char_end) AS chars FROM decision_chunks \
               WHERE decision_id = d.id) c ON true \
             LEFT JOIN LATERAL (SELECT source, source_uid, source_fields \
               FROM decision_sources WHERE decision_id = d.id AND deleted_at IS NULL \
               ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, source_uid ASC \
               LIMIT 1) src ON true \
             WHERE d.id = ANY($1)",
            &[&decision_ids],
        )
        .await?;
    let mut out = HashMap::new();
    for r in &rows {
        let public_id: String = r.get(1);
        let date_raw: Option<chrono::NaiveDate> = r.get(4);
        let chars: Option<i32> = r.get(12);
        out.insert(
            r.get::<_, i64>(0),
            DecisionMeta {
                public_id,
                jurisdiction_type: r.get(2),
                jurisdiction_code: r.get(3),
                date_lecture: date_raw.map(|d| d.to_string()),
                solution_uid: r.get(5),
                procedure_uid: r.get(6),
                office_uid: r.get(7),
                legal_domain_uid: r.get(8),
                publication_codes: r.get(9),
                docket_numbers: r.get(10),
                summary: r.get(11),
                chars: chars.map(|c| c as i64),
                seat: r
                    .get::<_, Option<tokio_postgres::types::Json<lj_core::chamber::Seat>>>(13)
                    .map(|j| j.0),
                seat_paths: r.get::<_, Option<Vec<String>>>(14).unwrap_or_default(),
                formation_uid: r.get(15),
                official_url: r
                    .get::<_, Option<&str>>(16)
                    .zip(r.get::<_, Option<&str>>(17))
                    .and_then(|(source, source_uid)| {
                        lj_core::official_url::official_url(&lj_core::official_url::SourceRef {
                            source,
                            source_uid,
                            court_code: r.get(18),
                            published_url: r.get(19),
                            ariane_ref: r
                                .get::<_, Option<&str>>(20)
                                .zip(r.get::<_, Option<&str>>(21)),
                        })
                    }),
                case_name: r.get(22),
            },
        );
    }
    Ok(out)
}

/// `full_text` (TOAST pglz) des décisions d'**une page** (≤ `limit`), par
/// `decision_id` : source unique du snippet. Le moteur de highlight re-tokenise
/// le texte fourni — aucun offset BM25 requis. Lu seulement pour les décisions
/// réellement affichées, sur un miss `page_memo`.
async fn load_full_texts(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, String>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT id, full_text FROM decisions \
             WHERE id = ANY($1) AND full_text IS NOT NULL",
            &[&decision_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
        .collect())
}

/// Spans char `(char_start, char_end)` des chunks gagnants ANN d'une page, par
/// `chunk_id` : fenêtre de fallback du snippet pour un hit purement sémantique
/// (aucun terme requête dans `full_text`) — on montre alors la région du chunk
/// gagnant plutôt que la tête de la décision.
async fn load_chunk_spans(
    conn: &Client,
    chunk_ids: &[i64],
) -> std::result::Result<HashMap<i64, (i32, i32)>, StoreError> {
    if chunk_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT id, char_start, char_end FROM decision_chunks WHERE id = ANY($1)",
            &[&chunk_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), (r.get::<_, i32>(1), r.get::<_, i32>(2))))
        .collect())
}

// ── Construction des hits de page ─────────────────────────────────────────────

fn make_snippet(text: &str) -> String {
    let count = text.chars().count();
    if count <= SNIPPET_MAX_CHARS {
        return text.to_string();
    }
    let mut s: String = text.chars().take(SNIPPET_MAX_CHARS - 1).collect();
    s.push('…');
    s
}

fn jt_from_str(code: &str) -> Option<JurisdictionType> {
    match code {
        "TA" => Some(JurisdictionType::Ta),
        "CAA" => Some(JurisdictionType::Caa),
        "CE" => Some(JurisdictionType::Ce),
        "CONSTIT" => Some(JurisdictionType::Constit),
        "TC" => Some(JurisdictionType::Tc),
        "CC" => Some(JurisdictionType::Cc),
        "CA" => Some(JurisdictionType::Ca),
        "TJ" => Some(JurisdictionType::Tj),
        "TCOM" => Some(JurisdictionType::Tcom),
        "CEDH" => Some(JurisdictionType::Cedh),
        "CJUE" => Some(JurisdictionType::Cjue),
        "CNDA" => Some(JurisdictionType::Cnda),
        "CNIL" => Some(JurisdictionType::Cnil),
        _ => None,
    }
}

/// Tag référentiel d'un uid optionnel (`solution:REJET` → `{REJET, Rejet}`).
fn opt_tag(uid: &Option<String>, refs: &Referential) -> Option<FacetTag> {
    uid.as_deref().map(|u| refs.tag(u))
}

/// Juridiction résolue d'une décision (nom réel assaini sinon libellé du type).
fn display_jurisdiction(m: &DecisionMeta, refs: &Referential) -> String {
    let jurisdiction_name = m
        .jurisdiction_code
        .as_deref()
        .and_then(|c| refs.jurisdiction(c))
        .map(|j| j.label.as_str());
    crate::titles::decision_jurisdiction(
        refs.jurisdiction_type_label(&m.jurisdiction_type)
            .unwrap_or(&m.jurisdiction_type),
        jurisdiction_name,
    )
}

/// Titre d'affichage des cartes/topbar : « <juridiction résolue>,
/// <date FR>, <n°> » — **sans** le siège (rendu à part, [`display_seat`], en 2ᵉ
/// ligne). Une seule ligne, ne wrappe pas. Le titre BM25 (`search_title`), lui,
/// garde le siège pour le matching.
pub(crate) fn display_title(m: &DecisionMeta, refs: &Referential) -> String {
    let jur_display = display_jurisdiction(m, refs);
    lj_core::titles::decision_title(
        &jur_display,
        None,
        m.date_lecture.as_deref(),
        m.case_name.as_deref(),
        m.docket_numbers
            .as_deref()
            .and_then(|d| d.first())
            .map(String::as_str),
    )
}

/// Siège recomposé depuis les axes structurés (chambre · formation/office),
/// rendu en 2ᵉ ligne sous le titre. `None` si aucun axe.
pub(crate) fn display_seat(m: &DecisionMeta, refs: &Referential) -> Option<String> {
    let jur_display = display_jurisdiction(m, refs);
    crate::titles::decision_seat(
        &jur_display,
        m.seat
            .as_ref()
            .and_then(lj_core::chamber::Seat::label)
            .as_deref(),
        m.formation_uid.as_deref(),
        m.office_uid.as_deref(),
    )
}

fn make_hit(
    m: &DecisionMeta,
    score: f64,
    chunk: &LegHit,
    title_html: String,
    summary: Option<String>,
    refs: &Referential,
) -> SearchHit {
    SearchHit {
        id: m.public_id.clone(),
        jurisdiction_type: jt_from_str(&m.jurisdiction_type).unwrap_or(JurisdictionType::Ta),
        jurisdiction_code: m.jurisdiction_code.clone(),
        jurisdiction_name: m
            .jurisdiction_code
            .as_deref()
            .and_then(|c| refs.jurisdiction(c))
            .map(|j| j.label.clone()),
        title_html,
        seat: display_seat(m, refs),
        score,
        date_lecture: m.date_lecture.clone(),
        docket_numbers: m.docket_numbers.clone(),
        solution: opt_tag(&m.solution_uid, refs),
        procedure: opt_tag(&m.procedure_uid, refs),
        office: opt_tag(&m.office_uid, refs),
        legal_domain: opt_tag(&m.legal_domain_uid, refs),
        seat_path: crate::titles::seat_path_tag(&m.seat_paths),
        publication_codes: m.publication_codes.clone(),
        best_chunk: BestChunk {
            chunk_index: chunk.chunk_index,
            snippet: chunk.snippet.clone().unwrap_or_default(),
        },
        chars: m.chars,
        summary,
        official_url: m.official_url.clone(),
    }
}

/// Assemble les `SearchHit` d'**une seule page** (≤ `limit`) à partir de la
/// metadata déjà hydratée ([`hydrate_decisions`]). Accès DB : le `full_text` des
/// ≤`limit` décisions affichées ([`load_full_texts`]) + les spans char des
/// chunks gagnants ANN ([`load_chunk_spans`], fenêtre de fallback). Suit le
/// highlight tantivy de `full_text` + titres de la page (Tier 2, mémoïsé par
/// page). Le snippet vient du texte décision.
pub(crate) async fn assemble_page(
    conn: &Client,
    ranked_page: &[(i64, f64, LegHit)],
    meta: &HashMap<i64, DecisionMeta>,
    query_text: &str,
    refs: &Referential,
) -> Result<Vec<SearchHit>> {
    // Hits affichés (présents dans la metadata hydratée) + le chunk gagnant
    // éventuel (jambe ANN, `chunk_id != -1`) qui ancre la fenêtre de fallback.
    let display: Vec<(i64, f64, LegHit)> = ranked_page
        .iter()
        .filter(|(d, _, _)| meta.contains_key(d))
        .cloned()
        .collect();

    let decision_ids: Vec<i64> = display.iter().map(|(d, _, _)| *d).collect();
    let ann_chunk_ids: Vec<i64> = display
        .iter()
        .map(|(_, _, c)| c.chunk_id)
        .filter(|id| *id != -1)
        .collect();
    let full_texts = load_full_texts(conn, &decision_ids)
        .await
        .map_err(ApiError::Store)?;
    // Vieilles décisions opendata intégralement en MAJUSCULES (surtout Cassation) :
    // recasse déterministe pour l'affichage du snippet, en miroir de la vue détail
    // (`decisions.rs`), gardée par `is_caps_lock`. Le highlight re-tokenise insensible
    // casse/accents → les `<mark>` se placent toujours correctement. CPU pur
    // (tokenizer + dicos) : offload `spawn_blocking`, le runtime étant
    // `current_thread` (SSR Leptos). La map recasée alimente les deux chemins de
    // snippet (highlight + fallback brut).
    let full_texts = tokio::task::spawn_blocking(move || {
        full_texts
            .into_iter()
            .map(|(id, t)| {
                let t = if truecase::is_caps_lock(&t) {
                    truecase::truecase(&t)
                } else {
                    t
                };
                (id, t)
            })
            .collect::<HashMap<i64, String>>()
    })
    .await
    .map_err(|e| ApiError::Internal(format!("recase task: {e}")))?;
    let chunk_spans = load_chunk_spans(conn, &ann_chunk_ids)
        .await
        .map_err(ApiError::Store)?;

    let body_docs: Vec<(i64, String)> = display
        .iter()
        .filter_map(|(d, _, _)| full_texts.get(d).map(|t| (*d, t.clone())))
        .collect();
    // `highlight` tokenise les textes + sélectionne les fragments : CPU pur (le
    // tokenizer regex domine), offloadé sur `spawn_blocking` — le runtime est
    // `current_thread` (contrainte SSR Leptos).
    let query_body = query_text.to_string();
    let body_task = tokio::task::spawn_blocking(move || {
        highlight(&body_docs, &query_body, snippets::DEFAULT_MAX_CHARS)
    });

    // Titres composés depuis les référentiels (jamais la colonne `search_title`,
    // qui porte la formation source brute) : highlight puis fallback verbatim.
    let titles: HashMap<i64, String> = display
        .iter()
        .filter_map(|(d, _, _)| meta.get(d).map(|m| (*d, display_title(m, refs))))
        .collect();
    let title_docs: Vec<(i64, String)> = titles.iter().map(|(d, t)| (*d, t.clone())).collect();
    let query_title = query_text.to_string();
    let title_task = tokio::task::spawn_blocking(move || {
        highlight(&title_docs, &query_title, TITLE_SNIPPET_CHARS)
    });

    let body_snippets = body_task
        .await
        .map_err(|e| ApiError::Internal(format!("body snippet task: {e}")))?;
    let title_snippets = title_task
        .await
        .map_err(|e| ApiError::Internal(format!("title snippet task: {e}")))?;

    let mut hits = Vec::with_capacity(display.len());
    for (decision_id, score, mut display_chunk) in display {
        let Some(decision_meta) = meta.get(&decision_id) else {
            continue;
        };
        // Snippet : fragment highlighté de `full_text` si un terme matche, sinon
        // fenêtre brute. Fallback ancré sur le chunk gagnant ANN (`[char_start..
        // char_end]`) — région du match sémantique — ou la tête du texte pour un
        // hit lexical/title-only. `make_hit` rend `""` faute de texte.
        display_chunk.snippet = body_snippets.get(&decision_id).cloned().or_else(|| {
            let full_text = full_texts.get(&decision_id)?;
            let raw = match chunk_spans.get(&display_chunk.chunk_id) {
                Some(&(cs, ce)) => full_text
                    .chars()
                    .skip(cs.max(0) as usize)
                    .take((ce - cs).max(0) as usize)
                    .collect::<String>(),
                None => full_text.clone(),
            };
            Some(make_snippet(&raw))
        });
        let title_html = title_snippets
            .get(&decision_id)
            .cloned()
            .or_else(|| titles.get(&decision_id).cloned())
            .unwrap_or_default();
        // Résumé garanti en base et déjà joint au hot path : embarqué
        // dans chaque hit quel que soit le mode → vue « Résumé » instantanée côté
        // front, sans aller-retour `/summary`.
        let summary = decision_meta.summary.clone();
        hits.push(make_hit(
            decision_meta,
            score,
            &display_chunk,
            title_html,
            summary,
            refs,
        ));
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_store::repository::FacetValueRow;

    /// Le tri lexical mettait « 10e chambre » avant « 2e chambre », et le tri par
    /// compteur intercalait les nommées entre les numérotées.
    #[test]
    fn label_sort_key_compte_les_chiffres_en_nombres() {
        let mut labels = vec![
            "Étrangers et rétention",
            "10e chambre",
            "Pôle 6, 8e chambre",
            "2e chambre",
            "Chambre sociale",
            "Pôle 1, 11e chambre",
            "1re chambre",
        ];
        labels.sort_by_key(|l| label_sort_key(l));
        assert_eq!(
            labels,
            [
                "1re chambre",
                "2e chambre",
                "10e chambre",
                "Chambre sociale",
                "Étrangers et rétention",
                "Pôle 1, 11e chambre",
                "Pôle 6, 8e chambre",
            ]
        );
    }

    #[test]
    fn court_sort_key_range_les_cours_par_leur_ville() {
        let mut labels = vec![
            "Cour d'appel de Nîmes",
            "Cour d'appel d'Agen",
            "Cour d'appel de Paris",
            "Cour d'appel d'Orléans",
            "Cour d'appel de Basse-Terre",
        ];
        labels.sort_by_key(|l| court_sort_key(l, Some("Cour d'appel")));
        assert_eq!(
            labels,
            [
                "Cour d'appel d'Agen",
                "Cour d'appel de Basse-Terre",
                "Cour d'appel de Nîmes",
                "Cour d'appel d'Orléans",
                "Cour d'appel de Paris",
            ]
        );
    }

    #[test]
    fn snippet_truncation_adds_ellipsis() {
        let long = "a".repeat(400);
        let s = make_snippet(&long);
        assert_eq!(s.chars().count(), SNIPPET_MAX_CHARS);
        assert!(s.ends_with('…'));
    }

    fn fv(uid: &str, label: &str, parent: Option<&str>) -> FacetValueRow {
        FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: parent.map(str::to_string),
            sort: 0,
            hint: None,
        }
    }

    fn jr(code: &str, jt: &str, label: &str) -> lj_store::repository::JurisdictionRow {
        lj_store::repository::JurisdictionRow {
            code: code.to_string(),
            source_code: code.to_uppercase(),
            jurisdiction_type: jt.to_string(),
            city: None,
            label: label.to_string(),
        }
    }

    /// Une cour seule sous son type et homonyme de lui n'est pas un étage : elle
    /// se replie et rend ses formations à la racine. Une cour parmi d'autres, ou
    /// nommée autrement, tient sa place.
    #[test]
    fn la_cour_unique_homonyme_de_son_type_se_replie() {
        let refs = Referential::new(
            vec![
                FacetValueRow {
                    sort: 1,
                    ..fv("jurisdiction_type:CC", "Cour de cassation", None)
                },
                FacetValueRow {
                    sort: 2,
                    ..fv("jurisdiction_type:CA", "Cour d'appel", None)
                },
                FacetValueRow {
                    sort: 12,
                    ..fv(
                        "jurisdiction_type:CNDA",
                        "Cour nationale du droit d'asile",
                        None,
                    )
                },
            ],
            vec![
                jr("cc", "CC", "Cour de cassation"),
                jr("ca_paris", "CA", "Cour d'appel de Paris"),
                jr("cnda", "CNDA", "CNDA"),
            ],
            Vec::new(),
        );
        let choices = seat_choices(
            &HashMap::from([
                ("CA".to_string(), 178_074i64),
                ("CC".to_string(), 119_525),
                ("CNDA".to_string(), 71),
                ("CC.SOCIALE".to_string(), 30_907),
                ("CA.PARIS".to_string(), 40_000),
                ("CA.PARIS.C3".to_string(), 900),
            ]),
            &refs,
        );
        let roots: Vec<&str> = choices
            .iter()
            .filter(|c| c.parent.is_none())
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(roots, ["CC", "CA", "CNDA"], "l'ordre vient du référentiel");
        let at = |value: &str| {
            choices
                .iter()
                .find(|c| c.value == value)
                .unwrap_or_else(|| panic!("{value} absent"))
        };
        assert!(
            !choices.iter().any(|c| c.value == "CC.CC"),
            "une cour homonyme de son type n'ouvre pas de niveau"
        );
        assert_eq!(at("CC.SOCIALE").parent.as_deref(), Some("CC"));
        assert_eq!(at("CA.PARIS").parent.as_deref(), Some("CA"));
        assert_eq!(at("CA.PARIS").label, "Cour d'appel de Paris");
        assert_eq!(at("CA.PARIS.C3").parent.as_deref(), Some("CA.PARIS"));
        assert_eq!(at("CA.PARIS.C3").label, "3e chambre");
    }

    /// La publication est une échelle : le rang du référentiel gagne sur le
    /// compteur, sinon l'inédit ouvre le menu parce qu'il est le plus nombreux.
    #[test]
    fn publication_facet_reads_in_referential_order() {
        let refs = Referential::new(
            vec![
                FacetValueRow {
                    sort: 1,
                    ..fv("publication:PUBLIE_RAPPORT", "Publié au rapport", None)
                },
                FacetValueRow {
                    sort: 2,
                    ..fv("publication:PUBLIE_BULLETIN", "Publié au bulletin", None)
                },
                FacetValueRow {
                    sort: 5,
                    ..fv("publication:INEDIT_BULLETIN", "Inédit au bulletin", None)
                },
            ],
            vec![],
            Vec::new(),
        );
        let counter = HashMap::from([
            ("publication:INEDIT_BULLETIN".to_string(), 4000i64),
            ("publication:PUBLIE_BULLETIN".to_string(), 300),
            ("publication:PUBLIE_RAPPORT".to_string(), 2),
        ]);
        let scaled = scale_choices(&counter, &refs);
        let values: Vec<&str> = scaled.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(
            values,
            ["PUBLIE_RAPPORT", "PUBLIE_BULLETIN", "INEDIT_BULLETIN"]
        );
        assert_eq!(
            uid_choices(&counter, &refs)[0].value,
            "INEDIT_BULLETIN",
            "les facettes plates gardent l'ordre du compteur"
        );
    }

    #[test]
    fn group_facet_aggregates_members_under_group() {
        let refs = Referential::new(
            vec![
                fv("legal_domain:CIVIL", "Civil", None),
                fv(
                    "legal_domain:CIVIL_DROIT_LOCATIF",
                    "Droit locatif",
                    Some("legal_domain:CIVIL"),
                ),
                fv("legal_domain:FISCAL", "Fiscal", None),
            ],
            Vec::new(),
            Vec::new(),
        );
        let counter = HashMap::from([
            ("legal_domain:CIVIL_DROIT_LOCATIF".to_string(), 3i64),
            ("legal_domain:CIVIL".to_string(), 1),
            ("legal_domain:FISCAL".to_string(), 2),
        ]);
        let choices = group_choices(&counter, &refs);
        let civil = choices.iter().find(|c| c.value == "CIVIL").unwrap();
        assert_eq!((civil.count, civil.parent.clone()), (4, None));
        let fiscal = choices.iter().find(|c| c.value == "FISCAL").unwrap();
        assert_eq!(fiscal.count, 2);
        let leaf = choices
            .iter()
            .find(|c| c.value == "CIVIL_DROIT_LOCATIF")
            .unwrap();
        assert_eq!(
            (leaf.count, leaf.parent.as_deref(), leaf.label.as_str()),
            (3, Some("CIVIL"), "Droit locatif")
        );
    }

    #[test]
    fn juridiction_facet_nests_types_codes_et_formations() {
        let refs = Referential::new(
            vec![fv("jurisdiction_type:TJ", "Tribunal judiciaire", None)],
            vec![lj_store::repository::JurisdictionRow {
                code: "tj_le_havre".to_string(),
                source_code: "tj76351".to_string(),
                jurisdiction_type: "TJ".to_string(),
                city: Some("Le Havre".to_string()),
                label: "Tribunal judiciaire du Havre".to_string(),
            }],
            Vec::new(),
        );
        let choices = seat_choices(
            &HashMap::from([
                ("TJ".to_string(), 5i64),
                ("TJ.LE_HAVRE".to_string(), 4),
                ("TJ.LE_HAVRE.C3".to_string(), 2),
            ]),
            &refs,
        );
        assert_eq!(choices[0].value, "TJ");
        assert_eq!(choices[0].label, "Tribunal judiciaire");
        let child = choices.iter().find(|c| c.value == "TJ.LE_HAVRE").unwrap();
        assert_eq!(child.parent.as_deref(), Some("TJ"));
        assert_eq!(child.label, "Tribunal judiciaire du Havre");
        let chambre = choices
            .iter()
            .find(|c| c.value == "TJ.LE_HAVRE.C3")
            .unwrap();
        assert_eq!(chambre.parent.as_deref(), Some("TJ.LE_HAVRE"));
        assert_eq!((chambre.label.as_str(), chambre.count), ("3e chambre", 2));
    }

    #[test]
    fn le_referentiel_qualifie_le_libelle_de_chambre() {
        let refs = Referential::new(
            vec![fv("jurisdiction_type:CC", "Cour de cassation", None)],
            vec![lj_store::repository::JurisdictionRow {
                code: "cc".to_string(),
                source_code: "cc".to_string(),
                jurisdiction_type: "CC".to_string(),
                city: None,
                label: "Cour de cassation".to_string(),
            }],
            Vec::new(),
        );
        let choices = seat_choices(
            &HashMap::from([
                ("CC".to_string(), 9i64),
                ("CC.C2_CIVILE".to_string(), 5),
                ("CC.SOCIALE".to_string(), 4),
            ]),
            &refs,
        );
        let label = |v: &str| {
            choices
                .iter()
                .find(|c| c.value == v)
                .map(|c| c.label.as_str())
        };
        assert_eq!(label("CC.C2_CIVILE"), Some("2e chambre civile"));
        assert_eq!(label("CC.SOCIALE"), Some("Chambre sociale"));
    }
}
