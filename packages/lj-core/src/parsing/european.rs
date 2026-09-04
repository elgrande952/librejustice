use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Sources européennes : CEDH (HUDOC) et CJUE (EUR-Lex).
//
// Parsers PURS : reçoivent le corps déjà strippé (HTML→texte au bord lj-sources)
// + les métadonnées désérialisées (`columns` HUDOC / `predicates` CDM). Le texte
// vit dans `texte_integral_*` ; les métadonnées partent verbatim en
// `source_fields` (construites par le pipeline depuis le même `Value`, hors de
// `Decision`).
// ─────────────────────────────────────────────────────────────────────────────

/// Lit une chaîne non-vide d'une clé d'un objet JSON (`columns`/`predicates`).
fn json_str_nonempty<'a>(obj: &'a Value, key: &str) -> Option<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// `kpdate` HUDOC (`1960-11-14T00:00:00`, ISO horodaté) → `YYYY-MM-DD`. On retient
/// les 10 premiers caractères ssi ce sont une date ISO valide (`%Y-%m-%d`) — pas
/// de troncature aveugle. `None` sinon.
fn cedh_kpdate_to_iso(kpdate: &str) -> Option<String> {
    let head = kpdate.get(..10)?;
    jiff::civil::Date::strptime("%Y-%m-%d", head)
        .ok()
        .map(|_| head.to_string())
}

/// `judgementdate` HUDOC (`14/11/1960 00:00:00`, `DD/MM/YYYY` + heure) → `YYYY-MM-DD`.
/// Repli quand `kpdate` manque. `None` si le préfixe n'est pas une date FR valide.
fn cedh_judgementdate_to_iso(judgementdate: &str) -> Option<String> {
    let head = judgementdate.split_whitespace().next()?;
    let d = jiff::civil::Date::strptime("%d/%m/%Y", head).ok()?;
    Some(d.strftime("%Y-%m-%d").to_string())
}

/// Parse un arrêt CEDH (HUDOC) en [`Decision`]. `body_text` est le corps déjà
/// converti HTML→texte au bord (`lj-sources`) ; `columns` est le bloc
/// `results[].columns` désérialisé ; `itemid` est la PK HUDOC (`001-…`).
///
/// `jurisdiction_type` = `"CEDH"` (posé explicitement). `ecli` lu verbatim de
/// `columns["ecli"]` (arrêts seulement, `None` sinon) — jamais dérivé.
/// `numero_dossiers` = `columns["appno"]` (`;`-séparé → liste). `date_lecture` =
/// `columns["kpdate"]` normalisé ISO `YYYY-MM-DD` (repli `judgementdate`
/// `DD/MM/YYYY`), car la frontière store ne parse que `%Y-%m-%d`. Le titre
/// (`docname`) et toutes les autres colonnes restent en `source_fields`.
///
/// Erreur franche [`CoreError::Xml`] si `itemid` ou `body_text` est vide.
pub fn parse_cedh(
    body_text: &str,
    columns: &Value,
    itemid: &str,
) -> crate::error::Result<Decision> {
    use crate::error::CoreError;

    if itemid.is_empty() {
        return Err(CoreError::Xml("CEDH: itemid vide".to_string()));
    }
    if body_text.trim().is_empty() {
        return Err(CoreError::Xml(format!("CEDH {itemid}: corps vide")));
    }

    let sections = extract_sections_xml(body_text);

    let numero_dossiers = json_str_nonempty(columns, "appno").map(|appno| {
        appno
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let numero_dossier = numero_dossiers.as_ref().and_then(|v| v.first().cloned());

    // `date_lecture` sort en ISO `YYYY-MM-DD` : la frontière store ne parse QUE
    // `%Y-%m-%d`. HUDOC livre `kpdate` en ISO horodaté (`1960-11-14T00:00:00`) et
    // `judgementdate` en `DD/MM/YYYY HH:MM:SS` — kpdate prioritaire.
    let date_lecture = json_str_nonempty(columns, "kpdate")
        .and_then(cedh_kpdate_to_iso)
        .or_else(|| {
            json_str_nonempty(columns, "judgementdate").and_then(cedh_judgementdate_to_iso)
        });

    Ok(Decision {
        source_uid: format!("cedh/{itemid}"),
        member_name: itemid.to_string(),
        ecli: json_str_nonempty(columns, "ecli").map(str::to_string),
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some("Cour européenne des droits de l'homme".to_string()),
        jurisdiction_type: Some("CEDH".to_string()),
        jurisdiction_location: None,
        numero_dossier,
        numero_dossiers,
        numero_role: None,
        date_lecture,
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        texte_integral_raw: body_text.to_string(),
        texte_integral_clean: body_text.to_string(),
        sections,
        metadata_header: String::new(),
        visa_trim: String::new(),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    })
}

/// Numéro d'affaire usuel dérivé du CELEX (`62020CJ0560` → `C-560/20`). Le CELEX
/// secteur 6 encode : secteur `6`, année d'affaire (4 chiffres), type
/// (`CJ`/`TJ`/`CO`/`TO`/`CC`…), numéro (chiffres, zéros de tête à retirer). La
/// lettre de la juridiction est l'initiale du type (`C`/`T`/`F`). Repli sur le
/// CELEX brut si la forme n'est pas reconnue.
fn celex_to_case_number(celex: &str) -> String {
    let bytes = celex.as_bytes();
    // `6` + 4 chiffres année + ≥1 lettre type + ≥1 chiffre numéro.
    if bytes.len() < 7 || bytes[0] != b'6' || !bytes[1..5].iter().all(u8::is_ascii_digit) {
        return celex.to_string();
    }
    let year = &celex[1..5];
    let rest = &celex[5..];
    let type_end = rest.find(|c: char| c.is_ascii_digit()).filter(|&i| i > 0);
    let Some(type_end) = type_end else {
        return celex.to_string();
    };
    let type_code = &rest[..type_end];
    let number = rest[type_end..].trim_start_matches('0');
    let number = if number.is_empty() { "0" } else { number };
    let Some(court) = type_code.chars().next() else {
        return celex.to_string();
    };
    let yy = &year[2..];
    format!("{court}-{number}/{yy}")
}

/// Parse un arrêt CJUE (EUR-Lex) en [`Decision`]. `body_text` est le texte FR
/// déjà converti (xhtml/html→texte) au bord (`lj-sources`) ; `predicates` est le
/// bloc de prédicats CDM désérialisé ; `celex` est la PK.
///
/// `jurisdiction_type` = `"CJUE"` (posé explicitement). `ecli` lu verbatim de
/// `predicates["case-law_ecli"]` (100 % présent, `None` si vide) — **jamais
/// dérivé du CELEX** (l'ECLI dérivé serait faux). `numero_dossiers` = numéro
/// d'affaire dérivé du CELEX (`C-560/20`), repli sur le CELEX brut.
/// `date_lecture` = `predicates["work_date_document"]`. La bannière « objet » de
/// l'en-tête (titre) et les autres prédicats restent en `source_fields`.
///
/// Erreur franche [`CoreError::Xml`] si `celex` ou `body_text` est vide.
pub fn parse_cjue(
    body_text: &str,
    predicates: &Value,
    celex: &str,
) -> crate::error::Result<Decision> {
    use crate::error::CoreError;

    if celex.is_empty() {
        return Err(CoreError::Xml("CJUE: celex vide".to_string()));
    }
    if body_text.trim().is_empty() {
        return Err(CoreError::Xml(format!("CJUE {celex}: corps vide")));
    }

    let sections = extract_sections_xml(body_text);

    let case_number = celex_to_case_number(celex);
    let numero_dossiers = Some(vec![case_number.clone()]);

    Ok(Decision {
        source_uid: format!("cjue/{celex}"),
        member_name: celex.to_string(),
        ecli: json_str_nonempty(predicates, "case-law_ecli").map(str::to_string),
        jurisdiction_source_code: None,
        chamber: None,
        nac: None,
        jurisdiction_name: Some("Cour de justice de l'Union européenne".to_string()),
        jurisdiction_type: Some("CJUE".to_string()),
        jurisdiction_location: None,
        numero_dossier: Some(case_number),
        numero_dossiers,
        numero_role: None,
        date_lecture: json_str_nonempty(predicates, "work_date_document").map(str::to_string),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        texte_integral_raw: body_text.to_string(),
        texte_integral_clean: body_text.to_string(),
        sections,
        metadata_header: String::new(),
        visa_trim: String::new(),
        themes: Vec::new(),
        attacked: None,
        parse_warnings: Vec::new(),
    })
}

/// Mots qui lient les parties sans en nommer aucune.
const LINK_WORDS: [&str; 4] = ["et", "autres", "and", "others"];

/// Séparateurs des deux parties d'un nom d'affaire : « c. » à Strasbourg,
/// « contre » à Luxembourg, « v. » quand la source ne livre que l'anglais.
const PARTY_SEPARATORS: [&str; 4] = ["c.", "v.", "v", "contre"];

/// `true` si le mot n'est qu'une suite d'initiales pointées (« N.T.P. »,
/// « M.T. »), ce que la pseudonymisation laisse d'un patronyme.
fn is_initials(word: &str) -> bool {
    word.ends_with('.')
        && word
            .split_terminator('.')
            .all(|part| part.chars().count() == 1)
}

/// Le nom désigne-t-il quelqu'un ? Il s'écrit « <requérant> <séparateur>
/// <défendeur> », et ne vaut que si la partie qui précède le séparateur porte un
/// mot : « AFFAIRE K c. FRANCE » nomme l'anonymat que la Cour a accordé.
fn names_a_party(case_name: &str) -> bool {
    let words: Vec<&str> = case_name.split_whitespace().collect();
    let separator = words
        .iter()
        .position(|w| PARTY_SEPARATORS.contains(&w.to_lowercase().as_str()));
    let Some(separator) = separator else {
        return false;
    };
    words[..separator].iter().any(|w| {
        !LINK_WORDS.contains(&w.to_lowercase().as_str())
            && !is_initials(w)
            && w.chars().filter(|c| c.is_alphabetic()).count() >= 2
    })
}

/// Étiquettes que HUDOC pose devant le nom de l'affaire.
const HUDOC_LABELS: [&str; 4] = ["AFFAIRES ", "AFFAIRE ", "CASE OF ", "ARRET "];

/// Le nom sous lequel la CEDH désigne sa propre affaire, lu du `docname`
/// HUDOC : « AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE » → « TAMAKULOVA ET AUTRES
/// c. UKRAINE ». La casse de la source est gardée telle quelle : l'abaisser
/// détruirait le `İ` turc, qui vaut deux caractères en minuscule. `None` quand
/// le docname ne nomme personne.
pub fn cedh_case_name(docname: &str) -> Option<String> {
    let docname = crate::text::normalize_spaces(docname);
    // « [Extraits] » dit ce que le recueil publie, pas qui est en cause.
    let without_mention = match (docname.ends_with(']'), docname.rfind('[')) {
        (true, Some(open)) => docname[..open].trim_end(),
        _ => docname.as_str(),
    };
    let name = HUDOC_LABELS
        .iter()
        .find_map(|label| without_mention.strip_prefix(label))
        .unwrap_or(without_mention);
    names_a_party(name).then(|| name.to_string())
}

/// Étiquettes ouvrant la ligne de titre EUR-Lex.
const EURLEX_LABELS: [&str; 4] = ["Arrêt ", "Ordonnance ", "Conclusions ", "Judgment "];

/// Séparateur des champs de la ligne de titre EUR-Lex.
const EURLEX_FIELD: &str = ". - ";

/// Lignes de tête où chercher la ligne de titre (le bandeau EUR-Lex en occupe
/// quatre).
const EURLEX_HEAD_LINES: usize = 12;

/// Le nom de l'affaire tel qu'EUR-Lex l'écrit en tête du texte. La ligne de
/// titre aligne « <acte> du <date>. - <parties>. - <objet>. - <affaire> » : le
/// nom est le champ des parties. Les entités HTML que la source laisse passer y
/// sont dépliées (`M&uuml;nchen`). `None` hors de cette forme : les textes
/// modernes n'ont pas de ligne de titre, et ne nomment leurs parties que dans le
/// corps, où rien ne les délimite.
pub fn cjue_case_name(full_text: &str) -> Option<String> {
    let title = full_text
        .lines()
        .take(EURLEX_HEAD_LINES)
        .map(str::trim)
        .find(|line| {
            line.contains(EURLEX_FIELD) && EURLEX_LABELS.iter().any(|l| line.starts_with(l))
        })?;
    let parties = title.split(EURLEX_FIELD).nth(1)?;
    let name = html_escape::decode_html_entities(parties)
        .trim()
        .to_string();
    names_a_party(&name).then_some(name)
}

/// `true` si `source_uid` provient d'une source HTML européenne : préfixe
/// `cedh/` (HUDOC) ou `cjue/` (EUR-Lex), posé par [`parse_cedh`] /
/// [`parse_cjue`]. Discriminant de famille du dispatch
/// [`Decision::from_source_fields`] — ces préfixes sont stables en DB et jamais
/// portés par les autres fonds (XML opendata, JSON Judilibre, DILA, CNDA).
pub(crate) fn source_uid_is_html_europe(source_uid: &str) -> bool {
    source_uid.starts_with("cedh/") || source_uid.starts_with("cjue/")
}

impl Decision {
    /// Branche HTML européenne (CEDH/CJUE) de [`Decision::from_source_fields`] :
    /// reconstruit une `Decision` **identique** à [`parse_cedh`]/[`parse_cjue`]
    /// depuis `(full_text, source_fields)`. Pendant exact de [`build_source_fields`]
    /// côté HTML : `source_fields` = les métadonnées (colonnes HUDOC / prédicats CDM)
    /// verbatim + la clé `sections` (que les parsers ignorent — ils relisent les
    /// sections de `full_text`).
    ///
    /// La parité est structurelle : ces parsers posent `texte_integral_clean =
    /// texte_integral_raw = body_text` (aucun nettoyage) et `metadata_header =
    /// visa_trim = ""`, le chunk ne dépend donc que de `full_text` (stocké verbatim).
    /// On réinvoque le parser dédié sur `(full_text, source_fields, pk)` — `pk`
    /// (`itemid`/`celex`) est dérivé du `source_uid` (`cedh/<itemid>` /
    /// `cjue/<celex>`).
    ///
    /// Panique si le parser échoue : à l'aval, `(full_text, source_fields)` provient
    /// d'une décision déjà ingérée, donc valide.
    pub(crate) fn from_source_fields_html_europe(
        full_text: &str,
        source_fields: &Value,
        source_uid: &str,
    ) -> Decision {
        if let Some(itemid) = source_uid.strip_prefix("cedh/") {
            parse_cedh(full_text, source_fields, itemid)
                .unwrap_or_else(|e| panic!("from_source_fields CEDH {source_uid}: {e}"))
        } else if let Some(celex) = source_uid.strip_prefix("cjue/") {
            parse_cjue(full_text, source_fields, celex)
                .unwrap_or_else(|e| panic!("from_source_fields CJUE {source_uid}: {e}"))
        } else {
            panic!("from_source_fields_html_europe: source_uid non HTML européen {source_uid}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── CEDH / CJUE ────────────────────────────────────────────────────────

    #[test]
    fn cedh_maps_fields_ecli_appno_list_and_type() {
        let columns = json!({
            "itemid": "001-250438",
            "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE",
            "appno": "20890/16;19185/25;26238/10",
            "ecli": "ECLI:CE:ECHR:2026:0611JUD002089016",
            "judgementdate": "2026-06-11T00:00:00",
            "kpdate": "2026-06-11T00:00:00",
            "doctype": "HFJUD",
        });
        let body = "Vu : la procédure suivie devant la Cour. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour décide.";
        let d = parse_cedh(body, &columns, "001-250438").expect("parse CEDH");

        assert_eq!(d.source_uid, "cedh/001-250438");
        assert_eq!(d.member_name, "001-250438");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CEDH"));
        assert_eq!(
            d.jurisdiction_name.as_deref(),
            Some("Cour européenne des droits de l'homme")
        );
        assert_eq!(
            d.ecli.as_deref(),
            Some("ECLI:CE:ECHR:2026:0611JUD002089016")
        );
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(
                &[
                    "20890/16".to_string(),
                    "19185/25".to_string(),
                    "26238/10".to_string()
                ][..]
            )
        );
        assert_eq!(d.numero_dossier.as_deref(), Some("20890/16"));
        assert_eq!(d.date_lecture.as_deref(), Some("2026-06-11"));
        assert_eq!(d.texte_integral_clean, body);
        assert_eq!(d.texte_integral_raw, body);
        let kinds: Vec<&str> = d.sections.iter().map(|s| s.kind.as_str()).collect();
        assert!(kinds.contains(&"visa"));
        assert!(kinds.contains(&"motivations"));
    }

    #[test]
    fn cedh_ecli_none_when_absent_and_kpdate_fallback() {
        let columns = json!({
            "appno": "10000/20",
            "ecli": "",
            "kpdate": "2026-05-01T00:00:00",
        });
        let d =
            parse_cedh("Résumé juridique du litige.", &columns, "002-14552").expect("parse CEDH");
        assert_eq!(d.ecli, None);
        assert_eq!(d.date_lecture.as_deref(), Some("2026-05-01"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["10000/20".to_string()][..])
        );
    }

    #[test]
    fn cedh_date_falls_back_to_judgementdate_ddmmyyyy_normalized_iso() {
        let columns = json!({
            "appno": "332/57",
            "ecli": "ECLI:CE:ECHR:1960:1114JUD000033257",
            "judgementdate": "14/11/1960 00:00:00",
        });
        let d = parse_cedh("Arrêt Lawless.", &columns, "001-57516").expect("parse CEDH");
        assert_eq!(d.date_lecture.as_deref(), Some("1960-11-14"));
    }

    #[test]
    fn cedh_errors_on_empty_itemid_or_body() {
        let columns = json!({ "appno": "1/20" });
        assert!(parse_cedh("texte", &columns, "").is_err());
        assert!(parse_cedh("   ", &columns, "001-1").is_err());
    }

    #[test]
    fn cjue_maps_fields_ecli_verbatim_and_case_number_from_celex() {
        let predicates = json!({
            "case-law_ecli": "ECLI:EU:C:2024:96",
            "work_date_document": "2024-01-30",
            "resource_legal_type": "CJ",
            "subject-matter": "IMMI",
        });
        let body = "ARRÊT DE LA COUR (grande chambre) 30 janvier 2024. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour dit pour droit.";
        let d = parse_cjue(body, &predicates, "62020CJ0560").expect("parse CJUE");

        assert_eq!(d.source_uid, "cjue/62020CJ0560");
        assert_eq!(d.member_name, "62020CJ0560");
        assert_eq!(d.jurisdiction_type.as_deref(), Some("CJUE"));
        assert_eq!(d.ecli.as_deref(), Some("ECLI:EU:C:2024:96"));
        assert_ne!(d.ecli.as_deref(), Some("ECLI:EU:C:2020:560"));
        assert_eq!(d.numero_dossier.as_deref(), Some("C-560/20"));
        assert_eq!(
            d.numero_dossiers.as_deref(),
            Some(&["C-560/20".to_string()][..])
        );
        assert_eq!(d.date_lecture.as_deref(), Some("2024-01-30"));
        assert_eq!(d.texte_integral_clean, body);
    }

    #[test]
    fn cjue_celex_to_case_number_variants() {
        assert_eq!(celex_to_case_number("62020CJ0560"), "C-560/20");
        assert_eq!(celex_to_case_number("62023CO0614"), "C-614/23");
        assert_eq!(celex_to_case_number("62023TJ0188"), "T-188/23");
        assert_eq!(celex_to_case_number("62016CJ0550"), "C-550/16");
        assert_eq!(celex_to_case_number("NOTACELEX"), "NOTACELEX");
        assert_eq!(celex_to_case_number("32020R0560"), "32020R0560");
    }

    #[test]
    fn cjue_ecli_none_when_absent_and_errors_on_empty() {
        let predicates = json!({ "case-law_ecli": "", "work_date_document": "2024-01-30" });
        let d = parse_cjue("texte de l'arrêt", &predicates, "62020CJ0560").expect("parse CJUE");
        assert_eq!(d.ecli, None);

        assert!(parse_cjue("texte", &predicates, "").is_err());
        assert!(parse_cjue("  ", &predicates, "62020CJ0560").is_err());
    }

    #[test]
    fn vocab_cedh_cjue_labels_and_types() {
        assert_eq!(
            vocab::jurisdiction_type(Some("cedh")).as_deref(),
            Some("CEDH")
        );
        assert_eq!(
            vocab::jurisdiction_type(Some("cjue")).as_deref(),
            Some("CJUE")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("cedh")).as_deref(),
            Some("Cour européenne des droits de l'homme")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("cjue")).as_deref(),
            Some("Cour de justice de l'Union européenne")
        );
    }

    #[test]
    fn vocab_constit_tc_labels_and_types() {
        assert_eq!(
            vocab::jurisdiction_label(Some("constit")).as_deref(),
            Some("Conseil constitutionnel")
        );
        assert_eq!(
            vocab::jurisdiction_label(Some("tc")).as_deref(),
            Some("Tribunal des conflits")
        );
        assert_eq!(
            vocab::jurisdiction_type(Some("constit")).as_deref(),
            Some("CONSTIT")
        );
        assert_eq!(vocab::jurisdiction_type(Some("tc")).as_deref(), Some("TC"));
    }

    // ── Nom d'usage porté par la source ──────────────────────────────────────

    #[test]
    fn cedh_case_name_drops_the_label_and_the_publication_mention() {
        assert_eq!(
            cedh_case_name("AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE").as_deref(),
            Some("TAMAKULOVA ET AUTRES c. UKRAINE")
        );
        assert_eq!(
            cedh_case_name("CASE OF LUKOŠIN v. LITHUANIA").as_deref(),
            Some("LUKOŠIN v. LITHUANIA")
        );
        assert_eq!(
            cedh_case_name("AFFAIRES GADZHIYEV ET GOSTEV c. RUSSIE").as_deref(),
            Some("GADZHIYEV ET GOSTEV c. RUSSIE")
        );
        assert_eq!(
            cedh_case_name("ARRET LUTZ c. FRANCE").as_deref(),
            Some("LUTZ c. FRANCE")
        );
        assert_eq!(
            cedh_case_name("AFFAIRE DEDOVSKI ET AUTRES c. RUSSIE [Extraits]").as_deref(),
            Some("DEDOVSKI ET AUTRES c. RUSSIE")
        );
        // Le numéro d'ordre distingue deux affaires des mêmes parties : il reste.
        assert_eq!(
            cedh_case_name("CASE OF MASLÁK v. SLOVAKIA (No. 3)").as_deref(),
            Some("MASLÁK v. SLOVAKIA (No. 3)")
        );
    }

    #[test]
    fn cedh_case_name_refuses_what_names_nobody() {
        for docname in [
            "AFFAIRE K c. FRANCE",
            "AFFAIRE N.T.P. ET AUTRES c. FRANCE",
            "CASE OF S.S. AND J.H. v. NORWAY",
            "AFFAIRE T.Ç. ET H.Ç.c. TURQUIE",
            "AFFAIRE « RELATIVE À CERTAINS ASPECTS DU RÉGIME LINGUISTIQUE DE \
             L'ENSEIGNEMENT EN BELGIQUE » (FOND)",
        ] {
            assert_eq!(cedh_case_name(docname), None, "docname {docname:?}");
        }
        // Un nom court reste un nom.
        assert_eq!(
            cedh_case_name("AFFAIRE GREENS ET M.T. c. ROYAUME-UNI [Extraits]").as_deref(),
            Some("GREENS ET M.T. c. ROYAUME-UNI")
        );
        assert_eq!(
            cedh_case_name("AFFAIRE APBP c. FRANCE").as_deref(),
            Some("APBP c. FRANCE")
        );
    }

    /// Tête de texte EUR-Lex telle qu'elle est stockée : bandeau de quatre
    /// lignes, puis la ligne de titre.
    fn eurlex(celex: &str, title: &str) -> String {
        format!("EUR-Lex - {celex} - FR\nAvis juridique important\n|\n{celex}\n{title}\nRecueil de jurisprudence 1992 page I-06613\nSommaire\n")
    }

    #[test]
    fn cjue_case_name_reads_the_parties_field_of_the_title_line() {
        let arret = eurlex(
            "61991J0144",
            "Arrêt de la Cour (sixième chambre) du 16 décembre 1992. - Gilbert Demoor en \
             Zonen NV et autres contre &Eacute;tat belge. - Demandes de décision \
             préjudicielle: Rechtbank van eerste aanleg Brussel - Belgique. - Taxes \
             parafiscales. - Affaires jointes C-144/91 et C-145/91.",
        );
        assert_eq!(
            cjue_case_name(&arret).as_deref(),
            Some("Gilbert Demoor en Zonen NV et autres contre État belge")
        );

        let conclusions = eurlex(
            "61990C0087",
            "Conclusions de l'avocat général Darmon présentées le 29 mai 1991. - A. Verholen \
             et autres contre Sociale Verzekeringsbank Amsterdam. - Demandes de décision \
             préjudicielle: Raad van Beroep 's-Hertogenbosch - Pays-Bas. - Egalité entre \
             hommes et femmes. - Affaires jointes C-87/90, C-88/90 et C-89/90.",
        );
        assert_eq!(
            cjue_case_name(&conclusions).as_deref(),
            Some("A. Verholen et autres contre Sociale Verzekeringsbank Amsterdam")
        );

        let penal = eurlex(
            "62002J0012",
            "Arrêt de la Cour (première chambre) du 2 octobre 2003. - Procédure pénale contre \
             Marco Grilli. - Libre circulation. - Affaire C-12/02.",
        );
        assert_eq!(
            cjue_case_name(&penal).as_deref(),
            Some("Procédure pénale contre Marco Grilli")
        );
    }

    #[test]
    fn cjue_case_name_refuses_outside_the_title_line() {
        // Texte moderne : ni bandeau ni ligne de titre, les parties ne se lisent
        // que dans le corps.
        let moderne = "DOCUMENT DE TRAVAIL\nARRÊT DU TRIBUNAL (huitième chambre)\n\
                       27 septembre 2023 (*)\n« Aides d'État. Régime d'aides. »\n\
                       Dans l'affaire T-252/15,\nFerrovial, SA, établie à Madrid (Espagne),\n";
        assert_eq!(cjue_case_name(moderne), None);

        // Titre sans champ de parties : l'objet occupe la place.
        let sans_parties = eurlex(
            "61979O0105",
            "Ordonnance de la Cour du 27 juin 1979. - Demande de décision préjudicielle du \
             Juge chargé du service du tribunal d'instance. - Affaire 105/79.",
        );
        assert_eq!(cjue_case_name(&sans_parties), None);

        // Fonctionnaire anonymisé.
        let anonyme = eurlex(
            "61998A0084",
            "Judgment of the Court of First Instance (First Chamber) of 16 June 2000. - C v \
             Council of the European Union. - Officials. - Case T-84/98.",
        );
        assert_eq!(cjue_case_name(&anonyme), None);
    }

    // ── Round-trip `from_source_fields` ⟷ parse direct ───────────────────────

    #[test]
    fn cedh_round_trips_via_source_fields() {
        let columns = json!({
            "itemid": "001-250438",
            "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE",
            "appno": "20890/16;19185/25",
            "ecli": "ECLI:CE:ECHR:2026:0611JUD002089016",
            "judgementdate": "2026-06-11T00:00:00",
            "doctype": "HFJUD",
        });
        let body = "Vu : la procédure suivie devant la Cour. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour décide.";
        let orig = parse_cedh(body, &columns, "001-250438").expect("parse CEDH");
        let sf = build_source_fields(&columns, &orig.sections);
        let rebuilt =
            Decision::from_source_fields(&orig.texte_integral_clean, &sf, &orig.source_uid);
        assert_eq!(orig, rebuilt);
    }

    #[test]
    fn cjue_round_trips_via_source_fields() {
        let predicates = json!({
            "case-law_ecli": "ECLI:EU:C:2024:96",
            "work_date_document": "2024-01-30",
            "resource_legal_type": "CJ",
        });
        let body = "ARRÊT DE LA COUR (grande chambre) 30 janvier 2024. \
                    Considérant ce qui suit : motifs. PAR CES MOTIFS, la Cour dit pour droit.";
        let orig = parse_cjue(body, &predicates, "62020CJ0560").expect("parse CJUE");
        let sf = build_source_fields(&predicates, &orig.sections);
        let rebuilt =
            Decision::from_source_fields(&orig.texte_integral_clean, &sf, &orig.source_uid);
        assert_eq!(orig, rebuilt);
    }
}
