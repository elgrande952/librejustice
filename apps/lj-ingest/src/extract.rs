//! Pont ingest → store : exécute la pile d'extraction lourde (`lj-extract`) et
//! produit les bundles que `lj-store` persiste. Toute l'extraction (champs,
//! `canonical_ref`, normalisation des citations) vit ici, côté ingest.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_core::decision::Decision;
use lj_extract::link::{LinkSnapshot, LinkTarget};
use lj_store::repository::{
    CitationOccurrenceRow, ExtractedFields, TextCaseCitationRow, TextLegalCitationRow,
};

/// `canonical_ref` (citation légale) d'une décision déjà parsée, via son
/// extracteur routé. `None` si la juridiction n'est pas routée ou si l'identité
/// est incomplète : on ne pose pas de clé bancale.
pub fn canonical_ref(decision: &Decision) -> Option<String> {
    lj_extract::extract::routed(decision).ok()?;
    lj_extract::identity::decision_canonical_ref(decision)
}

/// What the extraction reads of the corpus AT THE MOMENT OF THE PASS, and of
/// nothing else: hydrated once per process, immutable afterwards.
pub struct Snapshots<'a> {
    pub link: &'a LinkSnapshot,
    pub vocab: &'a lj_extract::compiled::CompiledVocab,
    pub chrono: &'a lj_extract::chrono::ChronoSnapshot,
    /// Referential labels healed with their city, keyed by SOURCE code.
    pub jur_labels: &'a HashMap<String, String>,
    /// Source code to canonical code.
    pub jur_codes: &'a HashMap<String, String>,
    pub case_names: &'a lj_extract::case_link::CaseNameSnapshot,
    pub seats: &'a lj_extract::seat_link::SeatVocabulary,
}

/// Champs structurés extraits d'une décision, prêts à persister.
///
/// Un [`lj_extract::compiled::doc_extract`] par décision : une seule passe
/// automate partagée entre les composeurs de champs (`DocScan`) et les citations
/// compilées.
pub fn extracted_fields(decision: &Decision, snap: &Snapshots<'_>) -> Result<ExtractedFields> {
    let Snapshots {
        link,
        vocab,
        chrono,
        jur_labels,
        jur_codes,
        case_names,
        seats,
    } = *snap;
    use lj_extract::extract as ex;
    // La garde `routed` couvre la nomenclature FR/EU ; le fond international
    // (JuriCAF, types propres `COUR_SUPREME`/`CCJA`…) passe par le traitement
    // générique des extracteurs (champs métadonnées + citations texte).
    if !decision.source_uid.starts_with("juricaf/") {
        ex::routed(decision).map_err(|e| anyhow!("extract: {e}"))?;
    }

    fn parse_date(s: Option<String>) -> Option<NaiveDate> {
        let s = s?;
        if s.is_empty() {
            return None;
        }
        NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
    }

    // Forum (parent implicite par juridiction) : CJUE/CEDH citent leurs textes
    // propres sans les nommer — dérivé du type + ECLI (EU:C/T/F).
    let forum = lj_extract::link::Forum::of(
        decision.jurisdiction_type.as_deref(),
        decision.ecli.as_deref(),
    );
    // Pas de texte porteur : une décision cite, elle ne revendique pas ses
    // propres numéraux nus.
    let docx = lj_extract::compiled::doc_extract(
        &decision.texte_integral_clean,
        vocab,
        link,
        chrono,
        forum,
        None,
    );
    let scan = Some(&docx.scan);

    let axes = ex::formation_axes_scanned(decision, scan);

    let citation_occurrences = Some(
        docx.citations
            .iter()
            .map(|c| CitationOccurrenceRow {
                char_start: c.char_start as i32,
                char_end: c.char_end as i32,
                ref_text_uid: c.target.ref_text_uid.clone(),
                ref_num_key: c.target.ref_num_key.clone(),
                suivants: c.suivants,
            })
            .collect(),
    );

    let docket_numbers = ex::docket_numbers_scanned(decision, scan).unwrap_or_default();
    let own_keys = lj_extract::identity::pending_target_keys(
        decision.jurisdiction_type.as_deref().unwrap_or_default(),
        decision.jurisdiction_location.as_deref(),
        &docket_numbers,
    );
    let numerotees = merge_bridged_spans(
        case_citation_rows(&docx.cases, &docket_numbers),
        lj_extract::chrono::attacked_text_spans(decision, chrono),
    );
    let nommees = drop_named_next_to_ref(
        lj_extract::case_link::cited_case_name_spans(
            &decision.texte_integral_clean,
            case_names,
            &own_keys,
            lj_extract::case_link::Citing {
                european: matches!(
                    decision.jurisdiction_type.as_deref(),
                    Some("CJUE") | Some("CEDH")
                ),
                date: decision.date_lecture.as_deref(),
            },
        ),
        &numerotees,
        &decision.texte_integral_clean,
    );
    let case_citations = Some(merge_bridged_spans(numerotees, nommees));

    let mut procedure = ex::procedure_scanned(decision, scan);
    // Vote domaine par le profil des codes cités (`lj_extract::domain`),
    // seulement quand le chemin signaux n'a rien rendu.
    if procedure.legal_domain_uid.is_none() {
        let ctx = lj_extract::domain::DomainContext {
            admin: !lj_extract::extract::is_judiciaire(decision),
            hint: procedure.domain_hint,
        };
        let cites = docx.citations.iter().filter_map(|c| {
            Some((
                c.target.ref_text_uid.as_deref()?,
                c.target.ref_num_key.as_deref(),
            ))
        });
        procedure.legal_domain_uid = lj_extract::domain::legal_domain_uid(cites, ctx);
    }
    // Raffinement par votes de termes du scan : ne touche qu'un domaine nul ou
    // parent nu — les sous-domaines posés par les codes cités priment.
    procedure.legal_domain_uid = lj_extract::domain::refine_with_terms(
        procedure.legal_domain_uid,
        &docx
            .scan
            .domain_term_votes(!lj_extract::extract::is_judiciaire(decision)),
    );
    // Le code NAC du greffe comble les trous et tranche les désaccords de
    // parent — jamais un sous-domaine du même parent posé par les codes.
    procedure.legal_domain_uid =
        lj_extract::domain::refine_with_nac(procedure.legal_domain_uid, decision.nac.as_deref());
    // Les thèmes Judilibre (titrage CC, nomenclature CA/TJ) comblent et
    // tranchent de même — le CRIMINEL de la chambre criminelle reste
    // intouchable.
    procedure.legal_domain_uid =
        lj_extract::domain::refine_with_themes(procedure.legal_domain_uid, &decision.themes);
    // Deux sociétés au litige : le défaut « obligations » est commercial.
    let applicant_companies = ex::applicant_companies_scanned(scan).unwrap_or_default();
    let defendant_companies = ex::defendant_companies_scanned(scan).unwrap_or_default();
    procedure.legal_domain_uid = lj_extract::domain::recolor_commercial_parties(
        procedure.legal_domain_uid,
        !applicant_companies.is_empty() && !defendant_companies.is_empty(),
    );
    // Juridiction catégorielle : la ligne référentielle directe.
    let jurisdiction = ex::jurisdiction_scanned(decision, scan, &docket_numbers).map(|j| {
        lj_store::repository::JurisdictionRow {
            source_code: j.code.clone(),
            code: j.code,
            jurisdiction_type: j.jurisdiction_type,
            city: j.city,
            label: j.label,
        }
    });
    // Nomenclatures TA/CAA fermées : un nom de greffe hors liste ne fabrique pas
    // de code fantôme — la décision s'ingère sans facette juridiction, et on le
    // signale.
    if jurisdiction.is_none()
        && matches!(
            decision.jurisdiction_type.as_deref(),
            Some("TA") | Some("CAA")
        )
    {
        tracing::warn!(
            source_uid = %decision.source_uid,
            nom = decision.jurisdiction_name.as_deref().unwrap_or(""),
            "juridiction admin hors nomenclature : décision sans facette juridiction"
        );
    }
    let seat = seat_of(&axes, decision, jurisdiction.as_ref(), jur_codes);
    let mut fields = ExtractedFields {
        jurisdiction,
        date_lecture: parse_date(ex::extract_date_lecture(decision)),
        date_audience: parse_date(ex::date_audience_scanned(decision, scan)),
        docket_numbers,
        chamber_key: axes.chamber_key(),
        seat_paths: seat
            .as_ref()
            .map(|s| {
                seats
                    .resolve(&lj_extract::seat_link::SeatRow {
                        seat: s.clone(),
                        chamber: decision.chamber.clone(),
                        formation: decision.formation.clone(),
                        span: None,
                    })
                    .paths()
            })
            .unwrap_or_default(),
        seat,
        formation_uid: axes.formation_uid.map(str::to_string),
        publication_codes: decision.publication_codes.clone(),
        solution_uid: ex::solution_scanned(decision, scan),
        // Le rôle lu dans la formation greffe (JLD, JCP, juge des référés…)
        // complète les scanners texte, qui priment quand ils ont parlé.
        procedure_uid: procedure
            .procedure_uid
            .or(axes.procedure_uid.map(str::to_string)),
        office_uid: procedure.office_uid.or(axes.office_uid.map(str::to_string)),
        legal_domain_uid: procedure.legal_domain_uid,
        applicant_counsel_names: ex::applicant_counsel_names_scanned(decision, scan)
            .unwrap_or_default(),
        applicant_law_firms: ex::applicant_law_firms_scanned(decision, scan).unwrap_or_default(),
        applicant_companies,
        defendant_counsel_names: ex::defendant_counsel_names_scanned(scan).unwrap_or_default(),
        defendant_law_firms: ex::defendant_law_firms_scanned(scan).unwrap_or_default(),
        defendant_companies,
        intervenors: ex::intervenors_scanned(scan).unwrap_or_default(),
        themes: decision.themes.clone(),
        citation_occurrences,
        decision_links: Some(
            lj_extract::chrono::prior_decision_refs(decision, chrono, &docx.scan)
                .into_iter()
                .map(|r| lj_store::repository::DecisionLinkRow {
                    link_type: r.link_type.as_str().to_string(),
                    target_ref: r.target_ref,
                })
                .collect(),
        ),
        case_citations,
        // Le titre EUR-Lex, en tête du texte, nomme les parties ; HUDOC le porte
        // hors du texte, et passe par `prebuilt_from_source_fields`.
        case_name: decision
            .source_uid
            .starts_with("cjue/")
            .then(|| lj_core::parsing::cjue_case_name(&decision.texte_integral_clean))
            .flatten(),
        ..Default::default()
    };
    fields.search_title = compose_search_title(
        &fields,
        decision.jurisdiction_type.as_deref(),
        jur_labels,
        fields.case_name.as_deref(),
    );
    // Relation canonique `decision_party` : grain acteur dérivé des cellules
    // plates finales (spans-évidences par matching replié, nature, resolve_key). La
    // qualité `intervenor` est gatée en prod — la colonne plate reste la seule
    // émission.
    use lj_extract::parties::Cell;
    // Les parties personnes physiques ne sont pas des colonnes de `decisions` :
    // elles n'existent que dans la relation, où la nature les distingue.
    let app_indiv = ex::applicant_individuals_scanned(scan).unwrap_or_default();
    let def_indiv = ex::defendant_individuals_scanned(scan).unwrap_or_default();
    let cells: [Cell<'_>; 8] = [
        Cell::new("party", Some("applicant"), &fields.applicant_companies),
        Cell::new("party", Some("defendant"), &fields.defendant_companies),
        Cell::physique("party", Some("applicant"), &app_indiv),
        Cell::physique("party", Some("defendant"), &def_indiv),
        Cell::new("law_firm", Some("applicant"), &fields.applicant_law_firms),
        Cell::new("law_firm", Some("defendant"), &fields.defendant_law_firms),
        Cell::new(
            "counsel_name",
            Some("applicant"),
            &fields.applicant_counsel_names,
        ),
        Cell::new(
            "counsel_name",
            Some("defendant"),
            &fields.defendant_counsel_names,
        ),
    ];
    let parties = lj_extract::parties::actor_rows_folded(docx.scan.folded(), &cells)
        .into_iter()
        .map(|r| lj_store::repository::DecisionPartyRow {
            quality: r.quality.to_string(),
            side: r.side.map(str::to_string),
            value: r.value,
            resolve_key: r.resolve_key,
            nature: r.nature.map(|n| n.as_str().to_string()),
            barreau: r.barreau,
            role: r.role.map(str::to_string),
            firm_ord: r.firm_ord,
            party_ord: r.party_ord,
            sub_ord: r.sub_ord,
            char_starts: r.char_starts,
            char_ends: r.char_ends,
        })
        .collect();
    fields.parties = Some(parties);
    Ok(fields)
}

/// The seat the decision sat in, positioned on its court. The court code is the
/// CANONICAL one, the resolution `ensure_jurisdictions` runs at write time: the
/// paths are derived here, before it.
fn seat_of(
    axes: &lj_extract::formation::FormationAxes,
    decision: &Decision,
    jurisdiction: Option<&lj_store::repository::JurisdictionRow>,
    jur_codes: &HashMap<String, String>,
) -> Option<lj_core::chamber::Seat> {
    let mut seat = axes.chamber.clone();
    seat.jurisdiction_type = decision.jurisdiction_type.clone()?;
    seat.jurisdiction_code = jurisdiction.map(|j| {
        jur_codes.get(&j.source_code).cloned().unwrap_or_else(|| {
            lj_store::repository::canonical_jurisdiction_code(
                &j.jurisdiction_type,
                j.city.as_deref(),
                &j.source_code,
            )
        })
    });
    Some(seat)
}

/// Persistent canonical title (`search_title`), composed through `lj_core::titles`,
/// the same recomposition the display runs. Jurisdiction is the categorial label,
/// healed by the referential one (`jur_labels`, code to label with city) when bare,
/// otherwise the type label. The seat is NOT in it: it reads from the stored seat,
/// and the indexed title equals the displayed one.
///
/// `case_name` is taken rather than read off `e`: the extraction only knows the name
/// its source carries, while the corpus also holds names elected downstream, and
/// composing without them silently drops the name from the indexed title.
pub fn compose_search_title(
    e: &ExtractedFields,
    jurisdiction_type: Option<&str>,
    jur_labels: &std::collections::HashMap<String, String>,
    case_name: Option<&str>,
) -> Option<String> {
    use lj_core::titles as t;
    let jurisdiction = e
        .jurisdiction
        .as_ref()
        .map(|j| match &j.city {
            Some(_) => j.label.clone(),
            None => jur_labels.get(&j.code).cloned().unwrap_or(j.label.clone()),
        })
        .or_else(|| {
            jurisdiction_type
                .and_then(t::jurisdiction_type_label)
                .map(str::to_string)
        })?;
    let date = e.date_lecture.map(|d| d.to_string());
    Some(t::decision_title(
        &jurisdiction,
        None,
        date.as_deref(),
        case_name,
        e.docket_numbers.first().map(String::as_str),
    ))
}

/// Variante sans `Decision` (fonds scrapés à `ExtractedFields` préconstruits,
/// ex. CNDA) : ligne `jurisdiction` par override du type et titre, pas de code
/// de localisation ni de nom scanné.
pub fn with_facet_uids(mut e: ExtractedFields, jurisdiction_type: Option<&str>) -> ExtractedFields {
    e.jurisdiction = jurisdiction_type
        .and_then(|jt| lj_extract::facets::jurisdiction_ref(jt, None, None))
        .map(|j| lj_store::repository::JurisdictionRow {
            source_code: j.code.clone(),
            code: j.code,
            jurisdiction_type: j.jurisdiction_type,
            city: j.city,
            label: j.label,
        });
    // Label toujours porté par l'override du type (CNDA…) : rien à guérir.
    let nom = e.case_name.clone();
    e.search_title = compose_search_title(
        &e,
        jurisdiction_type,
        &std::collections::HashMap::new(),
        nom.as_deref(),
    );
    e
}

/// Couche préconstruite reconstruite depuis `(source_uid, source_fields)` — le
/// pendant, en ré-extraction, de ce que le parser source pose à l'ingest via
/// `prebuilt_extracted`.
///
/// La règle : un champ préconstruit survit à la décomposition `source_payload
/// ⟷ (full_text, source_fields)` s'il tient dans `Decision` ou dans
/// `source_fields`. ArianeWeb passe par le second (il écrit un payload
/// Judilibre minimal, relu par `from_source_fields_json`). CNDA a un champ qui
/// ne tenait dans ni l'un ni l'autre — `solution_uid`, lu du dispositif par une
/// connaissance que l'extracteur générique n'a pas ; il vit en `source_fields`,
/// et se relit ici. La CEDH est dans le même cas : le nom de l'affaire est dans
/// le `docname` HUDOC, que le texte ne porte pas.
pub fn prebuilt_from_source_fields(
    source_uid: &str,
    source_fields: &serde_json::Value,
) -> Option<ExtractedFields> {
    if source_uid.starts_with("cedh/") {
        let docname = source_fields.get("docname").and_then(|v| v.as_str())?;
        return Some(ExtractedFields {
            case_name: Some(lj_core::parsing::cedh_case_name(docname)?),
            ..Default::default()
        });
    }
    let solution_uid = source_uid
        .starts_with("cnda/")
        .then(|| source_fields.get("solution_uid").and_then(|v| v.as_str()))
        .flatten()?;
    Some(ExtractedFields {
        solution_uid: Some(solution_uid.to_string()),
        ..Default::default()
    })
}

/// Fusionne les champs préconstruits par un parser source (CNDA, ArianeWeb :
/// connaissance de grain source — solution lue du dispositif, dates ISO de la
/// fiche) sur une extraction complète : le préconstruit prime là où il a parlé
/// (`Option` posée, `Vec` non vide), l'extraction fournit tout le reste
/// (citations, domaine, instruments, parties…).
pub fn merge_prebuilt(pre: ExtractedFields, full: ExtractedFields) -> ExtractedFields {
    fn vec_or(pre: Vec<String>, full: Vec<String>) -> Vec<String> {
        if pre.is_empty() {
            full
        } else {
            pre
        }
    }
    ExtractedFields {
        date_lecture: pre.date_lecture.or(full.date_lecture),
        date_audience: pre.date_audience.or(full.date_audience),
        docket_numbers: vec_or(pre.docket_numbers, full.docket_numbers),
        chamber_key: pre.chamber_key.or(full.chamber_key),
        seat: pre.seat.or(full.seat),
        seat_paths: vec_or(pre.seat_paths, full.seat_paths),
        formation_uid: pre.formation_uid.or(full.formation_uid),
        case_name: pre.case_name.or(full.case_name),
        search_title: pre.search_title.or(full.search_title),
        publication_codes: vec_or(pre.publication_codes, full.publication_codes),
        solution_uid: pre.solution_uid.or(full.solution_uid),
        procedure_uid: pre.procedure_uid.or(full.procedure_uid),
        office_uid: pre.office_uid.or(full.office_uid),
        legal_domain_uid: pre.legal_domain_uid.or(full.legal_domain_uid),
        applicant_counsel_names: vec_or(pre.applicant_counsel_names, full.applicant_counsel_names),
        applicant_law_firms: vec_or(pre.applicant_law_firms, full.applicant_law_firms),
        applicant_companies: vec_or(pre.applicant_companies, full.applicant_companies),
        defendant_counsel_names: vec_or(pre.defendant_counsel_names, full.defendant_counsel_names),
        defendant_law_firms: vec_or(pre.defendant_law_firms, full.defendant_law_firms),
        defendant_companies: vec_or(pre.defendant_companies, full.defendant_companies),
        intervenors: vec_or(pre.intervenors, full.intervenors),
        themes: vec_or(pre.themes, full.themes),
        jurisdiction: pre.jurisdiction.or(full.jurisdiction),
        citation_occurrences: pre.citation_occurrences.or(full.citation_occurrences),
        decision_links: pre.decision_links.or(full.decision_links),
        case_citations: pre.case_citations.or(full.case_citations),
        parties: pre.parties.or(full.parties),
    }
}

/// Une occurrence de citation enrichie de ses clés canoniques et de la cible
/// posée par le linker in-pass — l'artefact partagé ingest ↔ banc : l'ingest le
/// projette en [`CitationOccurrenceRow`] (span + cible), le banc le score tel
/// quel. Même normalisation, même linker.
pub struct LinkedOccurrence {
    pub char_start: usize,
    pub char_end: usize,
    /// Forme de surface de l'instrument (capture brute).
    pub instrument: String,
    /// Forme de surface de l'article, si la ligne en porte un.
    pub article: Option<String>,
    /// `normalize_instrument(instrument)` — même vocabulaire que
    /// `legal_text.title_key`.
    pub text_key: String,
    pub article_key: Option<String>,
    pub target: LinkTarget,
}

/// Projette les citations de jurisprudence en lignes à persister, propre
/// en-tête exclu : un span dont le numéro (chiffres seuls) est un docket de la
/// décision est sa propre référence (« RG n° 21/04532 » du bandeau CA,
/// « Pourvoi n° … » Cassation), pas une citation. Partagé par le bridge ingest
/// et le banc.
pub fn case_citation_rows(
    cases: &[lj_extract::compiled::CompiledCase],
    docket_numbers: &[String],
) -> Vec<lj_store::repository::CaseCitationRow> {
    let own_digits: std::collections::HashSet<String> = docket_numbers
        .iter()
        .map(|d| d.chars().filter(char::is_ascii_digit).collect())
        .collect();
    cases
        .iter()
        .filter(|c| {
            let digits: String = c
                .target_ref
                .rsplit('|')
                .next()
                .unwrap_or("")
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            !own_digits.contains(&digits)
        })
        .map(|c| lj_store::repository::CaseCitationRow {
            char_start: c.char_start as i32,
            char_end: c.char_end as i32,
            target_ref: c.target_ref.clone(),
        })
        .collect()
}

/// Fusionne les spans qu'un pont apporte là où le texte ne porte pas de numéro :
/// la décision attaquée citée inline (« l'arrêt attaqué (Paris, 22 août 2024) »)
/// et le nom d'usage (« la jurisprudence Danthony »). Les spans déjà présents
/// priment en cas de chevauchement.
/// Codepoints between the end of a name and the number that follows it: « arrêt
/// Pellegrin c. France [GC], n° 28541/95 » is one citation, not two. The nine
/// cases reread sit between 5 and 48 characters, the longest being « décision
/// Czabaj (Assemblée du contentieux, 13 juillet 2016, no 387763) » (31/08/2026).
const NAMED_REF_WINDOW: i32 = 60;

/// A usage name followed by its reference yields a single line, carried by the
/// number, the only identifying token. Sentence-ending punctuation in between
/// separates two citations.
pub fn drop_named_next_to_ref(
    named: Vec<(usize, usize, String)>,
    case_rows: &[lj_store::repository::CaseCitationRow],
    text: &str,
) -> Vec<(usize, usize, String)> {
    named
        .into_iter()
        .filter(|(_, fin, _)| {
            let fin = *fin as i32;
            !case_rows.iter().any(|c| {
                c.char_start >= fin
                    && c.char_start - fin <= NAMED_REF_WINDOW
                    && !text
                        .chars()
                        .skip(fin as usize)
                        .take((c.char_start - fin) as usize)
                        .any(|ch| ch == '.' || ch == ';')
            })
        })
        .collect()
}

pub fn merge_bridged_spans(
    mut case_rows: Vec<lj_store::repository::CaseCitationRow>,
    attacked: Vec<(usize, usize, String)>,
) -> Vec<lj_store::repository::CaseCitationRow> {
    for (s, e, target_ref) in attacked {
        let (s, e) = (s as i32, e as i32);
        if case_rows.iter().any(|c| c.char_start < e && s < c.char_end) {
            continue;
        }
        case_rows.push(lj_store::repository::CaseCitationRow {
            char_start: s,
            char_end: e,
            target_ref,
        });
    }
    case_rows.sort_by_key(|c| c.char_start);
    case_rows
}

/// Occurrences liées du moteur compilé : scan automate unique + composition
/// par-document, lien par le snapshot catalogue du run.
pub fn linked_occurrences_compiled(
    text: &str,
    vocab: &lj_extract::compiled::CompiledVocab,
    link: &LinkSnapshot,
    forum: Option<lj_extract::link::Forum>,
) -> Vec<LinkedOccurrence> {
    lj_extract::compiled::extract_citations(text, vocab, link, forum)
        .into_iter()
        .map(|c| LinkedOccurrence {
            char_start: c.char_start,
            char_end: c.char_end,
            instrument: c.instrument,
            article: c.article,
            text_key: c.text_key,
            article_key: c.article_key,
            target: c.target,
        })
        .collect()
}

/// Spans extraits d'UN corps émetteur, prêts à écrire.
pub struct OwnerRows {
    pub cases: Vec<TextCaseCitationRow>,
    pub cites: Vec<TextLegalCitationRow>,
}

/// Oracle DILA : `(propriétaire, article, numéro cité en clé publique)` →
/// texte déclaré cible. `None` = le même numéro est déclaré vers plusieurs textes
/// depuis cet article, l'oracle s'abstient.
pub type Declared = HashMap<(String, String, String), Option<String>>;

/// Plie les arêtes déclarées en oracle. Les propriétaires au grain texte
/// (corps monolithique) portent `owner_num_key = ''`, comme `legal_link`.
pub fn build_declared(rows: Vec<(String, String, String, String)>) -> Declared {
    let mut oracle: Declared = HashMap::with_capacity(rows.len());
    for (owner_text_uid, owner_num_key, target_num_key, target_text_uid) in rows {
        let key = (
            owner_text_uid,
            owner_num_key,
            lj_core::article_key::article_key(&target_num_key),
        );
        match oracle.entry(key) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Some(target_text_uid));
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if e.get().as_deref() != Some(target_text_uid.as_str()) {
                    e.insert(None);
                }
            }
        }
    }
    oracle
}

/// Texte que la source DÉCLARE cible pour ce numéro cité depuis cet article.
/// `None` si elle ne déclare rien, ou plusieurs textes pour le même numéro.
fn declared_target<'a>(
    oracle: &'a Declared,
    owner_text_uid: &str,
    owner_num_key: Option<&str>,
    article_key: &str,
) -> Option<&'a str> {
    oracle
        .get(&(
            owner_text_uid.to_string(),
            owner_num_key.unwrap_or_default().to_string(),
            article_key.to_string(),
        ))?
        .as_deref()
}

/// Les deux flux d'un corps émetteur du référentiel (offsets codepoints sur le
/// texte émetteur). Renvois : seulement les cibles résolues, jamais l'auto-renvoi
/// (cible = l'émetteur lui-même).
#[allow(clippy::too_many_arguments)]
pub fn owner_rows(
    texte: &str,
    owner_text_uid: &str,
    owner_num_key: Option<String>,
    owner_date_debut: Option<NaiveDate>,
    link: &LinkSnapshot,
    vocab: &lj_extract::compiled::CompiledVocab,
    chrono: &lj_extract::chrono::ChronoSnapshot,
    declared: &Declared,
) -> OwnerRows {
    let docx =
        lj_extract::compiled::doc_extract(texte, vocab, link, chrono, None, Some(owner_text_uid));
    let cases = docx
        .cases
        .into_iter()
        .map(|c| TextCaseCitationRow {
            owner_num_key: owner_num_key.clone(),
            owner_date_debut,
            char_start: c.char_start as i32,
            char_end: c.char_end as i32,
            target_ref: c.target_ref,
        })
        .collect();
    let occurrences = arbitrate(
        docx.citations,
        owner_text_uid,
        owner_num_key.as_deref(),
        link,
        declared,
    );
    let cites = occurrences
        .into_iter()
        .filter_map(|o| {
            Some(TextLegalCitationRow {
                owner_num_key: owner_num_key.clone(),
                owner_date_debut,
                char_start: o.char_start as i32,
                char_end: o.char_end as i32,
                ref_text_uid: o.target.ref_text_uid?,
                ref_num_key: o.target.ref_num_key,
            })
        })
        .collect();
    OwnerRows { cases, cites }
}

/// Occurrences d'un corps émetteur du référentiel, cible arbitrée mais rien de
/// jeté : le banc a besoin de voir aussi ce qui n'a pas été lié, pour scorer le
/// rappel et l'abstention. [`owner_rows`] n'en garde ensuite que les cibles
/// résolues.
#[allow(clippy::too_many_arguments)]
pub fn owner_occurrences(
    texte: &str,
    owner_text_uid: &str,
    owner_num_key: Option<&str>,
    link: &LinkSnapshot,
    vocab: &lj_extract::compiled::CompiledVocab,
    chrono: &lj_extract::chrono::ChronoSnapshot,
    declared: &Declared,
) -> Vec<LinkedOccurrence> {
    let docx =
        lj_extract::compiled::doc_extract(texte, vocab, link, chrono, None, Some(owner_text_uid));
    arbitrate(
        docx.citations,
        owner_text_uid,
        owner_num_key,
        link,
        declared,
    )
}

/// Tranche la cible de chaque occurrence d'un corps du référentiel : oracle de
/// la source, puis cible de l'extracteur, puis repli d'auto-renvoi. Une cible
/// écartée devient une occurrence sans cible, jamais une occurrence perdue.
fn arbitrate(
    citations: Vec<lj_extract::compiled::CompiledCitation>,
    owner_text_uid: &str,
    owner_num_key: Option<&str>,
    link: &LinkSnapshot,
    declared: &Declared,
) -> Vec<LinkedOccurrence> {
    citations
        .into_iter()
        .map(|c| {
            // Oracle DILA : pour un numéro qu'elle déclare citer depuis cet article, la
            // source tranche le texte — elle sait de quel décret du même jour elle parle,
            // et si le numéro nu est un renvoi interne.
            let oracle = c
                .article_key
                .as_deref()
                .and_then(|ak| declared_target(declared, owner_text_uid, owner_num_key, ak))
                .map(|uid| {
                    let num = link.num_key_for(uid, c.article_key.as_deref());
                    (uid.to_string(), num)
                });
            let target = match (oracle, c.target.ref_text_uid.clone()) {
                (Some((uid, num)), _) => Some((uid, num)),
                (None, Some(uid)) => Some((uid, c.target.ref_num_key.clone())),
                // Article nu dans un corps du référentiel (« en application de
                // l'article 21-2 » dans le code civil) : la cible est le texte émetteur
                // lui-même quand il possède l'article — renvoi interne, la convention
                // rédactionnelle des codes. L'orphelin génitif (« du présent code », « de la
                // loi susvisée ») n'est pas nu : il reste écarté.
                (None, None) => c
                    .article_key
                    .as_deref()
                    .filter(|_| c.bare)
                    .filter(|ak| link.has_article(owner_text_uid, ak))
                    .map(|ak| {
                        (
                            owner_text_uid.to_string(),
                            link.num_key_for(owner_text_uid, Some(ak)),
                        )
                    }),
            };
            // L'auto-renvoi à soi-même (même texte, même article) n'est pas un lien : le
            // lecteur y est déjà.
            let target = target
                .filter(|(uid, num)| uid != owner_text_uid || num.as_deref() != owner_num_key);
            LinkedOccurrence {
                char_start: c.char_start,
                char_end: c.char_end,
                instrument: c.instrument,
                article: c.article,
                text_key: c.text_key,
                article_key: c.article_key,
                target: LinkTarget {
                    ref_text_uid: target.as_ref().map(|(uid, _)| uid.clone()),
                    ref_num_key: target.and_then(|(_, num)| num),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(owner: &str, art: &str, num: &str, target: &str) -> (String, String, String, String) {
        (
            owner.to_string(),
            art.to_string(),
            num.to_string(),
            target.to_string(),
        )
    }

    #[test]
    fn declared_oracle_arbitrates_then_abstains() {
        let oracle = build_declared(vec![
            row("LEGI_CPC", "1056-1", "1049", "LEGI_CPC"),
            row("LEGI_CPC", "1056-1", "57", "LEGI_CIV"),
            row("LEGI_CSS", "L161-1", "5", "LEGI_RUR"),
            row("LEGI_CSS", "L161-1", "5", "LEGI_CIV"),
            row("cir_60001", "", "12", "LEGI_TRAV"),
        ]);

        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1056-1"), "1049"),
            Some("LEGI_CPC")
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1056-1"), "57"),
            Some("LEGI_CIV")
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CSS", Some("L161-1"), "5"),
            None
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1056-1"), "1055"),
            None
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1057"), "1049"),
            None
        );
        assert_eq!(
            declared_target(&oracle, "cir_60001", None, "12"),
            Some("LEGI_TRAV")
        );
    }

    #[test]
    fn declared_oracle_folds_the_cited_number() {
        let oracle = build_declared(vec![row("LEGI_CSS", "L162-1", "L. 161-1", "LEGI_RUR")]);
        let ak = lj_core::article_key::article_key("L. 161-1");
        assert_eq!(
            declared_target(&oracle, "LEGI_CSS", Some("L162-1"), &ak),
            Some("LEGI_RUR")
        );
    }

    #[test]
    fn prebuilt_from_source_fields_restores_the_cnda_solution() {
        let sf = serde_json::json!({ "solution_uid": "solution:REJET" });
        let pre = prebuilt_from_source_fields("cnda/24001234", &sf).expect("couche CNDA");
        assert_eq!(pre.solution_uid.as_deref(), Some("solution:REJET"));

        assert!(prebuilt_from_source_fields("cnda/24001234", &serde_json::json!({})).is_none());
        assert!(prebuilt_from_source_fields("judilibre/abc", &sf).is_none());

        let full = ExtractedFields {
            solution_uid: Some("solution:AUTRE".to_string()),
            legal_domain_uid: Some("legal_domain:PUBLIC".to_string()),
            ..Default::default()
        };
        let merged = merge_prebuilt(pre, full);
        assert_eq!(merged.solution_uid.as_deref(), Some("solution:REJET"));
        assert_eq!(
            merged.legal_domain_uid.as_deref(),
            Some("legal_domain:PUBLIC")
        );
    }

    #[test]
    fn prebuilt_from_source_fields_names_the_cedh_case() {
        let sf = serde_json::json!({ "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE" });
        let pre = prebuilt_from_source_fields("cedh/001-250438", &sf).expect("couche CEDH");
        assert_eq!(
            pre.case_name.as_deref(),
            Some("TAMAKULOVA ET AUTRES c. UKRAINE")
        );

        let anonyme = serde_json::json!({ "docname": "AFFAIRE K c. FRANCE" });
        assert!(prebuilt_from_source_fields("cedh/001-250439", &anonyme).is_none());
        assert!(prebuilt_from_source_fields("cedh/001-250440", &serde_json::json!({})).is_none());
    }

    #[test]
    fn compose_search_title_places_the_case_name_between_date_and_docket() {
        let nomme = ExtractedFields {
            date_lecture: NaiveDate::from_ymd_opt(1992, 12, 16),
            docket_numbers: vec!["C-144/91".to_string()],
            case_name: Some("Gilbert Demoor en Zonen NV contre État belge".to_string()),
            ..Default::default()
        };
        assert_eq!(
            compose_search_title(
                &nomme,
                Some("CJUE"),
                &HashMap::new(),
                nomme.case_name.as_deref()
            )
            .as_deref(),
            Some(
                "Cour de justice de l'Union européenne, 16 décembre 1992, \
                 Gilbert Demoor en Zonen NV contre État belge, C-144/91"
            )
        );

        let anonyme = ExtractedFields {
            case_name: None,
            ..nomme
        };
        assert_eq!(
            compose_search_title(&anonyme, Some("CJUE"), &HashMap::new(), None).as_deref(),
            Some("Cour de justice de l'Union européenne, 16 décembre 1992, C-144/91")
        );

        // A French decision is named by the ones citing it, long after extraction, so
        // its name reaches the title only through the argument.
        assert_eq!(
            compose_search_title(&anonyme, Some("CJUE"), &HashMap::new(), Some("Danthony"))
                .as_deref(),
            Some("Cour de justice de l'Union européenne, 16 décembre 1992, Danthony, C-144/91")
        );
    }

    #[test]
    fn merge_prebuilt_overlays_parser_fields_and_keeps_extraction() {
        let pre = ExtractedFields {
            solution_uid: Some("solution:SATISFACTION_TOTALE".to_string()),
            docket_numbers: vec!["26002382".to_string()],
            ..Default::default()
        };
        let full = ExtractedFields {
            solution_uid: Some("solution:REJET".to_string()),
            docket_numbers: vec!["ignored".to_string()],
            legal_domain_uid: Some("legal_domain:ETRANGERS".to_string()),
            citation_occurrences: Some(vec![CitationOccurrenceRow {
                char_start: 0,
                char_end: 10,
                ref_text_uid: Some("legal_text:CESEDA".to_string()),
                ref_num_key: Some("l532-2".to_string()),
                suivants: false,
            }]),
            ..Default::default()
        };
        let merged = merge_prebuilt(pre, full);
        assert_eq!(
            merged.solution_uid.as_deref(),
            Some("solution:SATISFACTION_TOTALE")
        );
        assert_eq!(merged.docket_numbers, vec!["26002382".to_string()]);
        assert_eq!(
            merged.legal_domain_uid.as_deref(),
            Some("legal_domain:ETRANGERS")
        );
        assert_eq!(merged.citation_occurrences.as_ref().map(Vec::len), Some(1));
    }
}
