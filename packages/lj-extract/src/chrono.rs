//! Liens de chronologie entre décisions : depuis une décision d'appel / de
//! cassation / de renvoi, la référence canonique de la décision ATTAQUÉE. Deux
//! sources : la métadonnée Judilibre `contested` ([`Decision::attacked`]) et le
//! texte (« Par un jugement n° X du D, le tribunal administratif de V… »,
//! « Décision déférée à la Cour : … »).
//!
//! La cible sort en clé pendante dans la grammaire `canonical_ref` exacte de
//! [`crate::identity`] (mêmes formes par juridiction, même normalisation) : elle
//! se résout côté base dès qu'une décision unique et active la porte.
//!
//! Une exception : `ca-parties|<code de cour>|<date>`, l'arrêt de CA attaqué par
//! un pourvoi, que le texte de cassation ne nomme jamais par son RG. La clé
//! n'égale aucun `canonical_ref` et se résout par les parties communes.

use std::collections::HashMap;
use std::sync::LazyLock;

use lj_core::decision::Decision;
use regex::Regex;

use crate::extract::common::parse_french_date;
use crate::identity::{
    looks_like_caa_rg, looks_like_pourvoi, normalize_component, normalize_pourvoi,
};

/// Nature du lien, du point de vue de la décision SOURCE (celle qui attaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    AppelDe,
    PourvoiContre,
    RenvoiApresCassation,
}

impl LinkType {
    /// Valeur `decision_links.link_type`.
    pub fn as_str(self) -> &'static str {
        match self {
            LinkType::AppelDe => "APPEL_DE",
            LinkType::PourvoiContre => "POURVOI_CONTRE",
            LinkType::RenvoiApresCassation => "RENVOI_APRES_CASSATION",
        }
    }
}

/// Lien extrait : type + `canonical_ref` de la décision attaquée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorRef {
    pub link_type: LinkType,
    pub target_ref: String,
}

/// Préfixe de la famille dont la clé ne porte pas le RG de sa cible.
const CA_PARTIES: &str = "ca-parties|";

impl PriorRef {
    /// Même cible que `other`, au grain que le texte peut atteindre : une clé
    /// `ca-parties|` n'a pas le RG, la comparaison tombe alors sur la cour et la
    /// date. Sert le banc, dont l'oracle `contested` porte, lui, le RG.
    pub fn same_target_as(&self, other: &PriorRef) -> bool {
        if self.target_ref == other.target_ref {
            return true;
        }
        if !self.target_ref.starts_with(CA_PARTIES) && !other.target_ref.starts_with(CA_PARTIES) {
            return false;
        }
        match (
            ca_court_and_date(&self.target_ref),
            ca_court_and_date(&other.target_ref),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

/// (cour pliée, date) d'une clé visant un arrêt de CA, dans l'une ou l'autre de
/// ses deux formes : `ca|<cour>|<rg>|<date>` et `ca-parties|<code>|<date>`.
fn ca_court_and_date(key: &str) -> Option<(String, &str)> {
    let mut parts = key.split('|');
    match parts.next()? {
        "ca" => {
            let court = parts.next()?.to_string();
            parts.next()?;
            Some((court, parts.next()?))
        }
        "ca-parties" => Some((normalize_component(parts.next()?), parts.next()?)),
        _ => None,
    }
}

/// Mapping (type de juridiction, ville pliée) → code de localisation
/// Judilibre, dérivé du référentiel `jurisdiction` en base au début du run. Les
/// clés `canonical_ref` CA/TJ/TCOM exigent ce code ; une ville ambiguë (deux
/// codes pour le même couple) ne mappe pas.
pub struct ChronoSnapshot {
    by_type_city: HashMap<(String, String), Option<String>>,
}

impl ChronoSnapshot {
    /// `rows` = lignes `(code, jurisdiction_type, city)` du référentiel
    /// `jurisdiction`. Le code TCOM référentiel est préfixé (`tcom0603`) alors
    /// que la localisation Judilibre est nue (`0603`) : on déprefixe. TA/CAA
    /// servent la chaîne du fond administratif ; CPH est vide aujourd'hui et se
    /// remplit à l'ingest des premières décisions prud'homales.
    pub fn new(rows: impl IntoIterator<Item = (String, String, String)>) -> Self {
        let mut by_type_city: HashMap<(String, String), Option<String>> = HashMap::new();
        for (code, jt, city) in rows {
            if !matches!(jt.as_str(), "CA" | "TJ" | "TCOM" | "TA" | "CAA" | "CPH") {
                continue;
            }
            let location = match jt.as_str() {
                "TCOM" => code.strip_prefix("tcom").unwrap_or(&code).to_string(),
                _ => code,
            };
            for city_key in city_keys(&normalize_component(&city)) {
                let key = (jt.clone(), city_key);
                match by_type_city.get(&key) {
                    Some(Some(prev)) if *prev != location => {
                        by_type_city.insert(key, None);
                    }
                    Some(_) => {}
                    None => {
                        by_type_city.insert(key, Some(location.clone()));
                    }
                }
            }
        }
        Self { by_type_city }
    }

    pub fn empty() -> Self {
        Self {
            by_type_city: HashMap::new(),
        }
    }

    /// Clé de chronologie d'une cible nommée en clair (« Aix-en-Provence »,
    /// « 21/00123 ») : le banc part d'une GT écrite à la main là où le moteur
    /// part du libellé plié du texte, et les deux doivent tomber sur la même clé.
    pub fn target_key(&self, jt: &str, city: &str, rg: &str, date: &str) -> Option<String> {
        self.target_key_folded(jt, &normalize_component(city), rg, date)
    }

    fn target_key_folded(
        &self,
        jt: &str,
        city_folded: &str,
        rg: &str,
        date: &str,
    ) -> Option<String> {
        let location = normalize_component(self.location(jt, city_folded)?);
        let rg = normalize_component(rg);
        if location.is_empty() || rg.is_empty() {
            return None;
        }
        Some(format!("{}|{location}|{rg}|{date}", jt.to_lowercase()))
    }

    /// Consommé aussi par le lexer de citations RG (`compiled::lex_case`) — même
    /// mapping ville → code que les liens de chronologie.
    pub(crate) fn location(&self, jt: &str, city_folded: &str) -> Option<&str> {
        self.by_type_city
            .get(&(jt.to_string(), city_folded.to_string()))?
            .as_deref()
    }
}

/// Liens de chronologie d'une décision — vide quand rien d'exploitable
/// (première instance, discriminants manquants, juridiction cible hors
/// nomenclature type CPH). `scan` est le scan unique du MÊME texte.
pub fn prior_decision_refs(
    d: &Decision,
    snap: &ChronoSnapshot,
    scan: &crate::scan::DocScan,
) -> Vec<PriorRef> {
    match d.jurisdiction_type.as_deref() {
        Some("CC") | Some("CA") => {
            let mut refs: Vec<PriorRef> = from_attacked_meta(d, snap)
                .or_else(|| from_deferee_text(d, snap))
                .or_else(|| from_pourvoi_text(d, snap, scan))
                .into_iter()
                .collect();
            // Un arrêt sur renvoi porte les deux liens : l'appel du jugement de
            // première instance qu'il rejuge, et la cassation qui le lui renvoie.
            if !refs
                .iter()
                .any(|r| r.link_type == LinkType::RenvoiApresCassation)
            {
                refs.extend(from_renvoi_text(d, scan));
            }
            refs
        }
        Some("CAA") | Some("CE") => from_admin_text(d).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Sous-mot que la regex de l'appelant exige contigu, cherché dans le texte
/// plié du scan unique. `fold_char` étant 1:1 par caractère, son absence prouve
/// l'absence de match : le garde est iso-résultat, pas une heuristique.
fn scan_lacks(scan: &crate::scan::DocScan, litteral: &str) -> bool {
    !scan.folded().contains(litteral)
}

/// Métadonnée Judilibre `contested` → lien. CC = pourvoi ; CA = appel, sauf
/// cible Cour de cassation = arrêt sur renvoi.
fn from_attacked_meta(d: &Decision, snap: &ChronoSnapshot) -> Option<PriorRef> {
    let a = d.attacked.as_ref()?;
    let label = attacked_label(a.jurisdiction.as_deref()?);
    let number = a.number.as_deref()?;
    let date = a.date.as_deref()?.trim();
    if date.len() != 10 {
        return None;
    }
    let source_jt = d.jurisdiction_type.as_deref()?;

    if label == "cour de cassation" || label.starts_with("cour de cassation ") {
        // Arrêt attaqué rendu par la Cassation : c'est un renvoi.
        let pourvoi = normalize_pourvoi(number);
        if !looks_like_pourvoi(&pourvoi) {
            return None;
        }
        return Some(PriorRef {
            link_type: LinkType::RenvoiApresCassation,
            target_ref: format!("cc|{pourvoi}|{date}"),
        });
    }

    let (target_jt, city) = jurisdiction_from_label(&label)?;
    let target_ref = snap.target_key_folded(target_jt, &city, number, date)?;
    let link_type = match source_jt {
        "CC" => LinkType::PourvoiContre,
        _ => LinkType::AppelDe,
    };
    Some(PriorRef {
        link_type,
        target_ref,
    })
}

/// Le greffe met la formation sur une seconde ligne (« cour d'appel de Paris\n
/// Pôle 6 - Chambre 10 »), qui n'entre dans aucune clé. Mesuré au fonds :
/// 7 024 libellés en deux lignes, dont pas un ne met la ville sur la seconde.
fn attacked_label(raw: &str) -> String {
    normalize_component(raw.split('\n').next().unwrap_or(raw))
}

/// Clés de ville d'une ligne référentielle : la forme pliée, plus la forme
/// sans article de tête (« le havre » → aussi « havre ») — les citations
/// contractent l'article (« du Havre », « des Sables-d'Olonne ») quand la
/// ville référentielle le porte en toutes lettres.
fn city_keys(folded: &str) -> Vec<String> {
    let mut keys = vec![folded.to_string()];
    for article in ["la ", "le ", "les "] {
        if let Some(rest) = folded.strip_prefix(article) {
            if !rest.is_empty() {
                keys.push(rest.to_string());
            }
            break;
        }
    }
    keys
}

/// Libellé plié (`cour d appel d aix en provence`) → (type, ville pliée).
/// Le juge de l'exécution et l'ex-TGI/TI sont des formations du TJ de la même
/// ville. CPH, tribunaux paritaires, cours d'assises : hors nomenclature.
fn jurisdiction_from_label(folded: &str) -> Option<(&'static str, String)> {
    const FORMS: &[(&str, &str)] = &[
        ("cour d appel ", "CA"),
        ("tribunal judiciaire ", "TJ"),
        ("tribunal de grande instance ", "TJ"),
        ("tribunal d instance ", "TJ"),
        ("tribunal de proximite ", "TJ"),
        ("juge de l execution ", "TJ"),
        ("juge de l expropriation ", "TJ"),
        ("juge aux affaires familiales ", "TJ"),
        ("juge des contentieux de la protection ", "TJ"),
        ("tj hors jaf jex jld j expro jcp ", "TJ"),
        ("tj ", "TJ"),
        ("tribunal de commerce ", "TCOM"),
        ("tribunal mixte de commerce ", "TCOM"),
        ("tc ", "TCOM"),
    ];
    for (prefix, jt) in FORMS {
        if let Some(rest) = folded.strip_prefix(prefix) {
            let city = rest
                .strip_prefix("de la ")
                .or_else(|| rest.strip_prefix("de "))
                .or_else(|| rest.strip_prefix("d "))
                .or_else(|| rest.strip_prefix("du "))
                .or_else(|| rest.strip_prefix("des "))
                .unwrap_or(rest)
                .trim();
            if city.is_empty() {
                return None;
            }
            return Some((jt, city.to_string()));
        }
    }
    None
}

/// Spans inline de la décision ATTAQUÉE (métadonnée Judilibre `contested`) :
/// les surfaces standard du corps qui la répètent — « l'arrêt attaqué (Paris,
/// 22 août 2024) », « l'arrêt rendu le 22 août 2024 par la cour d'appel de
/// Paris » — deviennent des citations de jurisprudence (`case_citation`)
/// portant le même `target_ref` que le lien de chronologie, résolues côté
/// base par le pont depuis `decision_links`. Ces mentions n'ont pas de docket
/// en texte : seule la métadonnée identifie la cible, et ville et date doivent
/// la matcher. Offsets en chars (grain `case_citation`).
pub fn attacked_text_spans(d: &Decision, snap: &ChronoSnapshot) -> Vec<(usize, usize, String)> {
    let Some(r) = from_attacked_meta(d, snap) else {
        return Vec::new();
    };
    let a = d.attacked.as_ref().unwrap();
    let label = attacked_label(a.jurisdiction.as_deref().unwrap_or(""));
    // Cible Cour de cassation (renvoi) : pas de ville, formes inline autres.
    let Some((_, city)) = jurisdiction_from_label(&label) else {
        return Vec::new();
    };
    let date = a.date.as_deref().unwrap_or("").trim();
    let text = &d.texte_integral_clean;
    let mut spans: Vec<(usize, usize)> = Vec::new();

    for cap in RE_ATTACKED_PAREN.captures_iter(text) {
        let inner = cap.get(1).unwrap();
        let Some(c) = RE_CITY_DATE.captures(inner.as_str()) else {
            continue;
        };
        if parse_french_date(&c[2], &c[3], &c[4]).as_deref() != Some(date)
            || normalize_component(&c[1]) != city
        {
            continue;
        }
        spans.push((inner.start(), inner.end()));
    }
    for cap in RE_ATTACKED_RENDU.captures_iter(text) {
        if parse_french_date(&cap[1], &cap[2], &cap[3]).as_deref() != Some(date) {
            continue;
        }
        // Le captage ville peut sur-courir de purs mots (« Douai a violé ») :
        // rogné mot à mot par la droite jusqu'à égaler la ville attendue.
        let cm = cap.get(4).unwrap();
        let raw = cm.as_str();
        let mut end = raw.len();
        loop {
            if normalize_component(&raw[..end]) == city {
                spans.push((
                    cap.get(0).unwrap().start(),
                    cm.start() + raw[..end].trim_end().len(),
                ));
                break;
            }
            match raw[..end].trim_end().rfind(char::is_whitespace) {
                Some(i) => end = i,
                None => break,
            }
        }
    }

    spans.sort_unstable();
    spans.dedup();
    spans
        .into_iter()
        .map(|(bs, be)| {
            (
                text[..bs].chars().count(),
                text[..be].chars().count(),
                r.target_ref.clone(),
            )
        })
        .collect()
}

/// « l'arrêt attaqué (Paris, 22 août 2024) » — la parenthèse standard des
/// moyens de cassation. Groupe 1 = contenu, validé par [`RE_CITY_DATE`].
static RE_ATTACKED_PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:arr[êe]ts?|jugements?|ordonnances?|d[ée]cisions?)\s+attaqu[ée]e?s?\s*\(\s*([^)]{2,80}?)\s*\)",
    )
    .unwrap()
});
/// Contenu de la parenthèse attaquée : « <Ville>, <date> ».
static RE_CITY_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^([\p{L}'’ \-]{2,40}?)\s*,\s*(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})$").unwrap()
});
/// « l'arrêt rendu le 22 août 2024 par la cour d'appel de Paris » — l'en-tête
/// de pourvoi. Le span court du nom de décision à la ville (rognée en aval).
static RE_ATTACKED_RENDU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:arr[êe]ts?|jugements?|ordonnances?)(?:\s+(?:n\s*[°o]\s*)?RG\s*(?:n\s*[°o]\s*)?:?\s*\d{2}/\d{3,5})?\s+rendue?s?\s+le\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})(?:\s*,[^,.;:0-9]{1,40},)?\s+par\s+l[ea]\s+(?:(?:premier\s+)?pr[ée]sident\s+(?:de\s+la\s+|du\s+|de\s+|d[’'])\s*)?(?:cour\s+d[’']appel|tribunal\s+judiciaire|tribunal\s+de\s+grande\s+instance|tribunal\s+d[’']instance|juge\s+de\s+l[’']ex[ée]cution|tribunal\s+(?:mixte\s+)?de\s+commerce)\s+(?:de\s+la\s+|de\s+|d[’']|du\s+)((?:[\p{L}'’\-]+)(?: [\p{L}'’\-]+){0,3})",
    )
    .unwrap()
});

/// « Par un jugement n° 2107366 du 13 septembre 2021, la magistrate désignée
/// du tribunal administratif de Marseille a rejeté sa demande. » — zone
/// « Procédure contentieuse antérieure » des CAA/CE. Groupes : n°, jour, mois,
/// année, libellé juridiction verbatim (article inclus, pour la forme nom de
/// la clé TA).
static RE_PRIOR_ADMIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bpar\s+(?:un|une|deux|trois|quatre)?\s*(?:jugements?|ordonnances?|arr[êe]ts?)\s+n[°ºo]?\s*s?\s*([0-9][0-9A-Za-z]{2,14})(?:\s*(?:,|et)\s*n[°ºo]?\s*s?\s*[0-9][0-9A-Za-z]{2,14})*\s+(?:du|en\s+date\s+du)\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})\s*,\s*l[ea]\s+(?:[\p{L}'’ \-]{0,60}?\b(?:du|de\s+la)\s+)?((?:tribunal\s+administratif|cour\s+administrative\s+d[’']appel)\s+(?:de\s+la\s+|de\s+|d[’'])[\p{L}][\p{L}'’ \-]{1,40}?)\s+(?:a\b|ont\b|n[’']a|s[’']est|statuant)",
    )
    .unwrap()
});

/// Réfs admin : toutes les décisions antérieures citées en en-tête ; la
/// décision ATTAQUÉE est la plus récente (un pourvoi CE cite le jugement TA
/// puis l'arrêt CAA — c'est l'arrêt qu'il attaque).
fn from_admin_text(d: &Decision) -> Option<PriorRef> {
    let text = &d.texte_integral_clean;
    let header_end = text.char_indices().nth(8000).map_or(text.len(), |(i, _)| i);
    let header = &text[..header_end];

    let mut best: Option<(String, PriorRef)> = None; // (date ISO, lien) — max date
    for cap in RE_PRIOR_ADMIN.captures_iter(header) {
        let date = match parse_french_date(&cap[2], &cap[3], &cap[4]) {
            Some(iso) => iso,
            None => continue,
        };
        let rg = normalize_component(&cap[1]);
        let jur = normalize_component(&cap[5]);
        if rg.is_empty() || jur.is_empty() {
            continue;
        }
        // Grammaire identity : CAA par RG auto-porteur, TA/repli par le nom.
        let target_ref = if jur.starts_with("cour administrative d appel") && looks_like_caa_rg(&rg)
        {
            format!("caa|{rg}|{date}")
        } else {
            format!("{jur}|{rg}|{date}")
        };
        let link_type = match d.jurisdiction_type.as_deref() {
            Some("CE") if header_has_pourvoi(header) => LinkType::PourvoiContre,
            _ => LinkType::AppelDe,
        };
        let candidate = (
            date.clone(),
            PriorRef {
                link_type,
                target_ref,
            },
        );
        if best.as_ref().is_none_or(|(b, _)| *b <= date) {
            best = Some(candidate);
        }
    }
    best.map(|(_, r)| r)
}

fn header_has_pourvoi(header: &str) -> bool {
    crate::compiled::fold_stable(header).contains("pourvoi")
}

/// Étiquettes qui ouvrent la zone où la cour nomme la décision dont elle est
/// saisie. Le deux-points n'est pas exigé : « Décision déférée du 02 Juin 2009 »
/// n'en a pas.
static RE_DEFEREE_ANCRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)d[ée]cisions?\s+(?:d[ée]f[ée]r[ée]e?s?|attaqu[ée]e?s?|entreprises?|de\s+premi[èe]re\s+instance)|appel\s+d[’']une?\s+(?:d[ée]cision|jugement)|sur\s+appel\s+(?:de|du)\b",
    )
    .unwrap()
});

/// Libellé de la juridiction de première instance et ce qui le suit, jusqu'au
/// premier caractère qu'une ville ne porte pas. La coupe fine revient à
/// [`city_prefix`], `regex` n'ayant pas de lookaround pour la faire ici.
static RE_DEFEREE_JUR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(\b(?:tribunal\s+judiciaire|tribunal\s+de\s+grande\s+instance|tribunal\s+d[’']instance|tribunal\s+de\s+proximit[ée]|tribunal\s+de\s+commerce|tribunal\s+mixte\s+de\s+commerce|juge\s+de\s+l[’']ex[ée]cution|juge\s+de\s+l[’']expropriation|juge\s+aux\s+affaires\s+familiales|juge\s+des\s+contentieux\s+de\s+la\s+protection|tj\s+hors\s+jaf,?\s*jex,?\s*jld,?\s*j\.?\s*expro,?\s*jcp|tj|tc)\s+(?:de\s+la\s+|de\s+|d[’']|du\s+)?[\p{L}][\p{L}'’ \-]{0,40})",
    )
    .unwrap()
});

/// Date de la décision déférée : la première de la zone, les suivantes datant
/// la déclaration d'appel ou l'arrêt lui-même.
static RE_DEFEREE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(\d{1,2}|1er)\s+(janvier|f[ée]vrier|mars|avril|mai|juin|juillet|ao[ûu]t|septembre|octobre|novembre|d[ée]cembre)\s+(\d{4})",
    )
    .unwrap()
});

/// Ce que l'en-tête pose autour des parties et qui n'est ni la date ni le
/// numéro de la décision déférée : la naissance d'une partie, que Douai met
/// juste après le libellé, et son timbre fiscal dématérialisé, que la cour
/// d'Orléans pose sur chacune. Ça s'ôte au lieu de se contourner : `regex` n'a
/// pas de lookaround.
static RE_BRUIT_ENTETE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)n[ée]e?\s+le\s+(?:\d{1,2}|1er)\s+[\p{L}]+\s+\d{4}|timbre\s+fiscal\s+d[ée]mat[ée]rialis[ée]e?\s*n[°ºo]\s*:?\s*\d+",
    )
    .unwrap()
});

/// Numéro de la décision déférée. Le marqueur est exigé, un nombre nu de
/// l'en-tête étant aussi bien un numéro de chambre ; le second groupe de
/// chiffres rattrape les anonymisations anciennes, qui l'espacent (« 09/ 01021 »).
static RE_DEFEREE_RG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:r[ée]pertoire\s+g[ée]n[ée]ral\s+sous\s+le\s+n[°ºo]|enregistr[ée]{1,2}s?\s+sous\s+le\s+n[°ºo]|sous\s+le\s+n[°ºo]|r\.?\s?g\.?|n[°ºo])\s*(?:n[°ºo]|num[ée]ro)?\s*:?\s*([0-9][0-9A-Za-z/.\-]{2,15}(?:\s[0-9]{1,8})?)",
    )
    .unwrap()
});

/// Ville d'un libellé : le plus long préfixe en mots que le référentiel
/// reconnaît. Coupe seul « PARIS 3ème Chambre » ou « BREST Pole Social », et
/// garde « SAINT-ETIENNE » qu'un séparateur énuméré tronquerait.
fn city_prefix(snap: &ChronoSnapshot, jt: &str, city_folded: &str) -> Option<String> {
    let mots: Vec<&str> = city_folded.split_whitespace().collect();
    (1..=mots.len())
        .rev()
        .map(|n| mots[..n].join(" "))
        .find(|c| snap.location(jt, c).is_some())
}

/// Tranche de `head` autour de `pos`, bornée aux frontières de caractères.
fn fenetre(head: &str, pos: usize, avant: usize, apres: usize) -> &str {
    let d = match avant {
        0 => pos,
        n => head[..pos]
            .char_indices()
            .rev()
            .nth(n - 1)
            .map_or(0, |(i, _)| i),
    };
    let f = head[pos..]
        .char_indices()
        .nth(apres)
        .map_or(head.len(), |(i, _)| pos + i);
    &head[d..f]
}

/// Première occurrence en aval, à défaut la plus proche en amont. L'en-tête
/// pose le numéro après le libellé (Lyon) ou avant (Douai), et le RG de la cour
/// elle-même traîne plus haut : la proximité le départage.
fn aval_sinon_amont<'a>(re: &Regex, aval: &'a str, amont: &'a str) -> Option<regex::Captures<'a>> {
    if let Some(c) = re.captures(aval) {
        return Some(c);
    }
    let debut = re.find_iter(amont).last()?.start();
    re.captures(&amont[debut..])
}

/// En-tête CA quand `contested` manque : la zone déférée nomme la juridiction,
/// la date et le numéro, dans un ordre qui varie d'une cour à l'autre. Sans
/// étiquette (« Jugement (N° X) rendu le D par le tribunal judiciaire de V »),
/// c'est le libellé de juridiction qui ouvre la zone.
fn from_deferee_text(d: &Decision, snap: &ChronoSnapshot) -> Option<PriorRef> {
    if d.jurisdiction_type.as_deref() != Some("CA") {
        return None;
    }
    let text = &d.texte_integral_clean;
    let header_end = text.char_indices().nth(4000).map_or(text.len(), |(i, _)| i);
    let head = RE_BRUIT_ENTETE.replace_all(&text[..header_end], " ");
    let head = head.as_ref();
    let (zone_jur, aval, amont) = match RE_DEFEREE_ANCRE.find(head) {
        Some(m) => (
            fenetre(head, m.start(), 200, 300),
            fenetre(head, m.start(), 0, 300),
            "",
        ),
        None => {
            let pos = RE_DEFEREE_JUR.find(fenetre(head, 0, 0, 1000))?.start();
            (
                fenetre(head, pos, 0, 300),
                fenetre(head, pos, 0, 300),
                fenetre(head, pos, 200, 0),
            )
        }
    };

    let libelle = normalize_component(&RE_DEFEREE_JUR.captures(zone_jur)?[1]);
    let (target_jt, ville) = jurisdiction_from_label(&libelle)?;
    let city = city_prefix(snap, target_jt, &ville)?;
    let date_cap = aval_sinon_amont(&RE_DEFEREE_DATE, aval, amont)?;
    let date = parse_french_date(&date_cap[1], &date_cap[2], &date_cap[3])?;
    if d.date_lecture
        .as_deref()
        .is_some_and(|lue| date.as_str() >= lue)
    {
        return None;
    }
    let rg_cap = aval_sinon_amont(&RE_DEFEREE_RG, aval, amont)?;
    Some(PriorRef {
        link_type: LinkType::AppelDe,
        target_ref: snap.target_key_folded(target_jt, &city, rg_cap[1].trim(), &date)?,
    })
}

/// « a formé le pourvoi n° G 22-16.931 contre l'arrêt rendu le 5 avril 2022 par
/// la cour d'appel de Nîmes (chambre sociale) », forme civile lue depuis l'ancre
/// du scan. Groupes : jour, mois, année, ville.
static RE_POURVOI_RENDU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)^(?:contre\s+(?:l[’']|un\s+|deux\s+)|en\s+cassation\s+d[’']un\s+)arr[êe]ts?\s+rendus?\s+le\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})\s+par\s+la\s+cour\s+d[’']appel\s+(?:de\s+la\s+|de\s+|d[’']|du\s+)((?:[\p{L}'’\-]+)(?: [\p{L}'’\-]+){0,3})",
    )
    .unwrap()
});

/// « contre l'arrêt de la cour d'appel de PARIS, chambre 5-13, en date du
/// 3 mars 2021 », forme criminelle : la ville précède la date.
/// Groupes : ville, jour, mois, année.
static RE_POURVOI_ARRET_DE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)^(?:contre\s+(?:l[’']|un\s+|deux\s+)|en\s+cassation\s+d[’']un\s+)arr[êe]ts?\s+de\s+la\s+cour\s+d[’']appel\s+(?:de\s+la\s+|de\s+|d[’']|du\s+)((?:[\p{L}'’\-]+)(?: [\p{L}'’\-]+){0,3}).{0,120}?(?:en\s+date\s+du|du|le)\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4})",
    )
    .unwrap()
});

/// Arrêt de CA attaqué par un pourvoi, lu dans l'en-tête faute d'un `contested`.
/// Le texte nomme la cour et la date, jamais le RG (101 textes sur 3 000, mesuré
/// le 25/08) : la clé sort dans la famille `ca-parties|`, que la résolution
/// départage par les parties, cour et date ne désignant qu'un arrêt sur 44.
fn from_pourvoi_text(
    d: &Decision,
    snap: &ChronoSnapshot,
    scan: &crate::scan::DocScan,
) -> Option<PriorRef> {
    if d.jurisdiction_type.as_deref() != Some("CC") || scan_lacks(scan, "pourvoi") {
        return None;
    }
    let target_ref = scan.contre_windows().into_iter().find_map(|w| {
        RE_POURVOI_RENDU
            .captures(&w)
            .and_then(|c| ca_party_key(snap, &c[4], &c[1], &c[2], &c[3]))
            .or_else(|| {
                RE_POURVOI_ARRET_DE
                    .captures(&w)
                    .and_then(|c| ca_party_key(snap, &c[1], &c[2], &c[3], &c[4]))
            })
    })?;
    Some(PriorRef {
        link_type: LinkType::PourvoiContre,
        target_ref,
    })
}

/// Le code de localisation entre BRUT dans la clé, là où les autres familles le
/// plient : la résolution jointe sur `decisions.jurisdiction_code` doit tenir
/// dans l'index `(jurisdiction_code, date_lecture)`.
fn ca_party_key(
    snap: &ChronoSnapshot,
    raw_city: &str,
    day: &str,
    month: &str,
    year: &str,
) -> Option<String> {
    let date = parse_french_date(day, month, year)?;
    // Le captage ville peut sur-courir sur les mots suivants (« Nîmes chambre
    // sociale ») : rogné mot à mot par la droite jusqu'à ce que le référentiel
    // le reconnaisse.
    let mut end = raw_city.len();
    loop {
        let trimmed = raw_city[..end].trim_end();
        if let Some(code) = snap.location("CA", &normalize_component(trimmed)) {
            return Some(format!("{CA_PARTIES}{code}|{date}"));
        }
        end = trimmed.rfind(char::is_whitespace)?;
    }
}

/// « Décision déférée à la Cour : sur renvoi après cassation - arrêt de la Cour
/// de cassation en date du 15 juin 2022 - pourvoi N° X 20-20.708 », ses variantes
/// de saisine, et la chaîne procédurale nue (« Cour d'Appel D'AIX du 20 septembre
/// 2012 RG 12/00432 - Cour de Cassation Civ.2 du 11 septembre 2014 Pourvoi
/// n°Q 12-35.412 »). Groupes : jour, mois, année, trois tranches du pourvoi.
static RE_RENVOI_CC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)cour\s+de\s+cassation.{0,120}?(?:en\s+date\s+du|en\s+date|prononc[ée]\s+le|rendue?\s+le|du|le)\s+(\d{1,2}|1er)\s+([a-zà-ÿ]+)\s+(\d{4}).{0,150}?(\d{2})\s?-\s?(\d{2})\s?[.]?\s?(\d{3})",
    )
    .unwrap()
});

/// Arrêt de cassation renvoyant l'affaire, lu dans l'en-tête de l'arrêt de CA
/// faute d'un `contested` qui le nomme. La date DOIT précéder le numéro, l'ordre
/// inverse désignant l'arrêt cassé. Fenêtre mesurée le 23/08 sur les 93 081
/// arrêts de CA portant « pourvoi » et « cassation » : 1 500 caractères rendent
/// 1 099 candidats, 1 019 résolus dont 1 018 visant une cassation ; tout le
/// texte ajoute 621 mentions à 35 % de rejets, soit des citations d'autorité.
fn from_renvoi_text(d: &Decision, scan: &crate::scan::DocScan) -> Option<PriorRef> {
    if d.jurisdiction_type.as_deref() != Some("CA") || scan_lacks(scan, "cassation") {
        return None;
    }
    let text = &d.texte_integral_clean;
    let header_end = text.char_indices().nth(1500).map_or(text.len(), |(i, _)| i);
    RE_RENVOI_CC
        .captures_iter(&text[..header_end])
        .find_map(|cap| {
            let date = parse_french_date(&cap[1], &cap[2], &cap[3])?;
            // Recomposé au format canonique : `12-20693` vise `12-20.693`.
            Some(PriorRef {
                link_type: LinkType::RenvoiApresCassation,
                target_ref: format!("cc|{}-{}.{}|{date}", &cap[4], &cap[5], &cap[6]),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> ChronoSnapshot {
        ChronoSnapshot::new(vec![
            ("ca_nancy".into(), "CA".into(), "Nancy".into()),
            (
                "ca_aix_provence".into(),
                "CA".into(),
                "Aix-en-Provence".into(),
            ),
            ("tj75056".into(), "TJ".into(), "Paris".into()),
            ("tj42218".into(), "TJ".into(), "Saint-Étienne".into()),
            ("tcom7501".into(), "TCOM".into(), "Paris".into()),
        ])
    }

    fn decision(jt: &str, text: &str) -> Decision {
        Decision {
            source_uid: "test".to_string(),
            member_name: String::new(),
            ecli: None,
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: None,
            jurisdiction_type: Some(jt.to_string()),
            jurisdiction_location: None,
            numero_dossier: None,
            numero_dossiers: None,
            numero_role: None,
            date_lecture: None,
            date_audience: None,
            date_mise_jour: None,
            formation: None,
            type_decision: None,
            type_recours: None,
            solution: None,
            publication_codes: vec![],
            avocat_requerant: None,
            texte_integral_raw: text.to_string(),
            texte_integral_clean: text.to_string(),
            sections: vec![],
            metadata_header: String::new(),
            visa_trim: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec![],
        }
    }

    /// Le scan unique du même texte, comme en prod : la chronologie s'y garde.
    fn liens(d: &Decision, snap: &ChronoSnapshot) -> Vec<PriorRef> {
        prior_decision_refs(d, snap, &crate::scan::scan(&d.texte_integral_clean))
    }

    #[test]
    fn cc_contested_ca_gives_pourvoi_contre() {
        let mut d = decision("CC", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Cour d'appel de Nancy".into()),
            number: Some("19/00207".into()),
            date: Some("2020-12-08".into()),
        });
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::PourvoiContre);
        assert_eq!(refs[0].target_ref, "ca|ca nancy|19 00207|2020-12-08");
    }

    #[test]
    fn article_city_resolves_contracted_citation() {
        let snap = ChronoSnapshot::new(vec![
            ("tj76351".into(), "TJ".into(), "Le Havre".into()),
            ("tj17300".into(), "TJ".into(), "La Rochelle".into()),
        ]);
        let mut d = decision("CC", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Tribunal judiciaire du Havre".into()),
            number: Some("21/00123".into()),
            date: Some("2023-01-10".into()),
        });
        let refs = liens(&d, &snap);
        assert_eq!(refs[0].target_ref, "tj|tj76351|21 00123|2023-01-10");

        let mut d = decision("CC", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Tribunal judiciaire de La Rochelle".into()),
            number: Some("22/00456".into()),
            date: Some("2023-06-15".into()),
        });
        let refs = liens(&d, &snap);
        assert_eq!(refs[0].target_ref, "tj|tj17300|22 00456|2023-06-15");
    }

    #[test]
    fn ca_contested_cc_is_renvoi() {
        let mut d = decision("CA", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Cour de cassation".into()),
            number: Some("20-22.085".into()),
            date: Some("2022-03-24".into()),
        });
        let refs = liens(&d, &snap());
        assert_eq!(refs[0].link_type, LinkType::RenvoiApresCassation);
        assert_eq!(refs[0].target_ref, "cc|20-22.085|2022-03-24");
    }

    #[test]
    fn caa_header_links_ta_judgment() {
        let d = decision(
            "CAA",
            "Vu la procédure suivante : Procédure contentieuse antérieure : M. B A a demandé \
             au tribunal administratif de Marseille d'annuler l'arrêté du 19 juillet 2021. \
             Par un jugement n° 2107366 du 13 septembre 2021, la magistrate désignée du \
             tribunal administratif de Marseille a rejeté sa demande. Procédure devant la \
             Cour : Par une requête enregistrée le 13 octobre 2021, M. A demande à la Cour \
             d'annuler ce jugement.",
        );
        let refs = liens(&d, &ChronoSnapshot::empty());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::AppelDe);
        assert_eq!(
            refs[0].target_ref,
            "tribunal administratif de marseille|2107366|2021-09-13"
        );
    }

    #[test]
    fn ce_pourvoi_targets_most_recent_caa_arret() {
        let d = decision(
            "CE",
            "Vu la procédure suivante : La société Fryan a demandé au tribunal administratif \
             de Lyon d'annuler l'arrêté du 9 janvier 2017. Par un jugement n° 1721444 du \
             14 novembre 2019, le tribunal administratif de Lyon a fait droit à cette \
             demande. Par un arrêt n° 20LY00096 du 14 décembre 2021, la cour administrative \
             d'appel de Lyon a, sur l'appel de la commune, annulé ce jugement. Par un \
             pourvoi, enregistré le 14 février 2022, la société Fryan demande au Conseil \
             d'Etat d'annuler cet arrêt.",
        );
        let refs = liens(&d, &ChronoSnapshot::empty());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::PourvoiContre);
        assert_eq!(refs[0].target_ref, "caa|20ly00096|2021-12-14");
    }

    #[test]
    fn caa_multi_judgments_takes_first_number() {
        let d = decision(
            "CAA",
            "Par deux jugements n° 2202850 et n° 2202851 du 16 mars 2023, le tribunal \
             administratif de Poitiers a rejeté leurs demandes. Par une requête, \
             enregistrée le 11 avril 2023, M. C demande à la cour d'annuler ce jugement.",
        );
        let refs = liens(&d, &ChronoSnapshot::empty());
        assert_eq!(
            refs[0].target_ref,
            "tribunal administratif de poitiers|2202850|2023-03-16"
        );
    }

    #[test]
    fn ca_deferee_jex_maps_to_tj() {
        let d = decision(
            "CA",
            "COUR D'APPEL DE PARIS Pôle 4 - Chambre 8 ARRÊT DU 27 NOVEMBRE 2014 Numéro \
             d'inscription au répertoire général : 13/24338 Décision déférée à la Cour : \
             Jugement du 19 Novembre 2013 - Juge de l'exécution de PARIS - RG n° 12/81319 \
             APPELANTE Madame X",
        );
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::AppelDe);
        assert_eq!(refs[0].target_ref, "tj|tj75056|12 81319|2013-11-19");
    }

    #[test]
    fn ca_deferee_du_date_rendu_par_le_tj() {
        let d = decision(
            "CA",
            "COUR D'APPEL DE PARIS Pôle 3 - Chambre 5 ARRET DU 23 JUIN 2026 Numéro \
             d'inscription au répertoire général : N° RG 25/00001 - N° Portalis X \
             Décision déférée à la Cour : Jugement du 23 janvier 2025 rendu par le \
             tribunal judiciaire de PARIS - RG n° 21/00002 APPELANT LE MINISTERE PUBLIC",
        );
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::AppelDe);
        assert_eq!(refs[0].target_ref, "tj|tj75056|21 00002|2025-01-23");
    }

    #[test]
    fn ca_renvoi_header_names_the_cassation_arret() {
        let d = decision(
            "CA",
            "COUR D'APPEL D'AIX-EN-PROVENCE Chambre 1-1 ARRÊT AU FOND DU 12 MARS 2024 \
             Numéro d'inscription au répertoire général : N° RG 22/16070 Décision déférée \
             à la Cour : Sur renvoi après cassation - arrêt de la Cour de cassation en date \
             du 15 juin 2022- pourvoi N° X 20-20.708 APPELANTE Madame [X]",
        );
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::RenvoiApresCassation);
        assert_eq!(refs[0].target_ref, "cc|20-20.708|2022-06-15");
    }

    /// La chaîne procédurale nue de l'en-tête, sans mot de renvoi : la dernière
    /// étape est l'arrêt de cassation, et une cour d'appel ne se trouve en aval
    /// d'une cassation que sur renvoi.
    #[test]
    fn ca_header_chain_ends_on_the_cassation_arret() {
        let d = decision(
            "CA",
            "COUR D'APPEL D'AIX EN PROVENCE 3ème Chambre B ARRÊT DU 27 NOVEMBRE 2018 \
             Tribunal de Grande Instance de NICE Au fond du 27 juin 2011 RG : 10/03730 \
             ch n°3 - Cour d'Appel D'AIX EN PROVENCE du 20 septembre 2012 RG n° : 12/00432 \
             3ème ch B - Cour de Cassation Civ.2 du 11 septembre 2014 Pourvoi n°Q 12-35.412 \
             DEMANDERESSE La société X",
        );
        let refs = liens(&d, &snap());
        assert_eq!(refs[0].link_type, LinkType::RenvoiApresCassation);
        assert_eq!(refs[0].target_ref, "cc|12-35.412|2014-09-11");
    }

    /// Pourvoi écrit sans le point, et l'appel qui subsiste sous le renvoi : un
    /// arrêt sur renvoi rejuge le jugement de première instance.
    #[test]
    fn ca_renvoi_joins_the_appeal_of_the_first_instance() {
        let mut d = decision(
            "CA",
            "COUR D'APPEL DE PARIS Pôle 5 RENVOI APRÈS CASSATION ARRÊT DU 4 AVRIL 2016 \
             Décision déférée à la Cour : Arrêt de la Cour de Cassation de PARIS en date du \
             02 Juillet 2013, enregistrée sous le no 12-20693 APPELANTE",
        );
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Tribunal judiciaire de Paris".into()),
            number: Some("11/00123".into()),
            date: Some("2012-01-10".into()),
        });
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].link_type, LinkType::AppelDe);
        assert_eq!(refs[0].target_ref, "tj|tj75056|11 00123|2012-01-10");
        assert_eq!(refs[1].link_type, LinkType::RenvoiApresCassation);
        assert_eq!(refs[1].target_ref, "cc|12-20.693|2013-07-02");
    }

    #[test]
    fn cc_pourvoi_header_names_the_ca_arret_without_its_rg() {
        let d = decision(
            "CC",
            "LA COUR DE CASSATION, CHAMBRE SOCIALE, a rendu l'arrêt suivant : Mme [X] a \
             formé le pourvoi n° G 22-16.931 contre l'arrêt rendu le 5 avril 2022 par la \
             cour d'appel de Nancy (chambre sociale), dans le litige l'opposant à la \
             société Y, défenderesse à la cassation.",
        );
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::PourvoiContre);
        assert_eq!(refs[0].target_ref, "ca-parties|ca_nancy|2022-04-05");
    }

    /// Forme d'avant 2019 : l'adresse des demandeurs sépare « pourvoi » de
    /// « contre ».
    #[test]
    fn cc_pourvoi_header_survives_the_party_addresses() {
        let d = decision(
            "CC",
            "LA COUR DE CASSATION, DEUXIÈME CHAMBRE CIVILE, a rendu l'arrêt suivant : \
             Statuant sur le pourvoi formé par Mme [L] [J], domiciliée [Adresse 1], 12 rue \
             des Longues Raies, résidence Les Hauts de Sainte-Marie, bâtiment C, escalier 3, \
             appartement 41, 54000 Nancy, agissant tant en son nom personnel qu'en qualité \
             d'ayant droit de [R] [J], et Mme [B] [J], domiciliée [Adresse 2], 8 avenue du \
             Général Leclerc, 54140 Jarville-la-Malgrange, contre l'arrêt rendu le \
             5 avril 2022 par la cour d'appel de Nancy (chambre sociale), dans le litige \
             les opposant à la société Y.",
        );
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].target_ref, "ca-parties|ca_nancy|2022-04-05");
    }

    /// Forme criminelle : ville avant date, et la chambre s'intercale.
    #[test]
    fn cc_pourvoi_header_criminal_shape_puts_the_city_first() {
        let d = decision(
            "CC",
            "N° X 21-84.123 F-D. Sur le pourvoi formé par M. [Y] contre l'arrêt de la cour \
             d'appel d'AIX-EN-PROVENCE, chambre 5-13, en date du 3 mars 2021, qui l'a \
             condamné pour recel à six mois d'emprisonnement.",
        );
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].link_type, LinkType::PourvoiContre);
        assert_eq!(refs[0].target_ref, "ca-parties|ca_aix_provence|2021-03-03");
    }

    /// La métadonnée du greffe l'emporte : elle porte le RG, donc une clé qui se
    /// résout par égalité, là où la voie texte doit passer par les parties.
    #[test]
    fn cc_contested_wins_over_the_pourvoi_header() {
        let mut d = decision(
            "CC",
            "Mme [X] a formé le pourvoi n° G 22-16.931 contre l'arrêt rendu le 5 avril 2022 \
             par la cour d'appel de Nancy (chambre sociale).",
        );
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Cour d'appel de Nancy".into()),
            number: Some("21/00987".into()),
            date: Some("2022-04-05".into()),
        });
        let refs = liens(&d, &snap());
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].target_ref, "ca|ca nancy|21 00987|2022-04-05");
    }

    /// Hors en-tête, une mention de la Cour de cassation est une citation
    /// d'autorité : 35 % des arrêts ainsi visés sont des rejets (mesuré le 23/08).
    #[test]
    fn cassation_cited_in_the_body_is_not_a_renvoi() {
        let filler = "Sur le premier moyen, la cour observe que les pieces produites \
                      etablissent la realite du prejudice invoque par la demanderesse. "
            .repeat(20);
        let d = decision(
            "CA",
            &format!(
                "COUR D'APPEL DE DOUAI ARRÊT DU 3 MAI 2024 {filler} Il est de jurisprudence \
                 constante, comme l'a jugé la Cour de cassation le 14 octobre 2020 \
                 (pourvoi n° 18-24.209), que le juge doit motiver sa décision."
            ),
        );
        assert!(liens(&d, &snap()).is_empty());
    }

    #[test]
    fn attacked_text_spans_require_city_and_date_match() {
        let text = "ont formé le pourvoi contre l'arrêt rendu le 8 décembre 2020 par la \
                    cour d'appel de Nancy (pôle 4, chambre 10), dans le litige. Sur le \
                    moyen : l'arrêt attaqué (Nancy, 8 décembre 2020) a violé la loi. \
                    L'arrêt attaqué (Paris, 3 mai 2019) relève d'un autre pourvoi.";
        let mut d = decision("CC", text);
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Cour d'appel de Nancy".into()),
            number: Some("19/00207".into()),
            date: Some("2020-12-08".into()),
        });
        let spans = attacked_text_spans(&d, &snap());
        let surfaces: Vec<String> = spans
            .iter()
            .map(|(s, e, r)| {
                assert_eq!(r, "ca|ca nancy|19 00207|2020-12-08");
                text.chars().skip(*s).take(e - s).collect()
            })
            .collect();
        assert_eq!(
            surfaces,
            vec![
                "arrêt rendu le 8 décembre 2020 par la cour d'appel de Nancy".to_string(),
                "Nancy, 8 décembre 2020".to_string(),
            ]
        );
    }

    #[test]
    fn attacked_text_spans_read_the_dispositif_and_what_it_inserts() {
        let text = "contre l'arrêt n° RG : 16/05913 rendu le 15 juin 2018 par la cour \
                    d'appel de Nancy. CASSE ET ANNULE l'arrêt rendu le 15 juin 2018, \
                    entre les parties, par la cour d'appel de Nancy ; remet l'affaire.";
        let mut d = decision("CC", text);
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("cour d'appel de Nancy\nPôle 6 - Chambre 10".into()),
            number: Some("16/05913".into()),
            date: Some("2018-06-15".into()),
        });
        let surfaces: Vec<String> = attacked_text_spans(&d, &snap())
            .iter()
            .map(|(s, e, r)| {
                assert_eq!(r, "ca|ca nancy|16 05913|2018-06-15");
                text.chars().skip(*s).take(e - s).collect()
            })
            .collect();
        assert_eq!(
            surfaces,
            vec![
                "arrêt n° RG : 16/05913 rendu le 15 juin 2018 par la cour d'appel de Nancy"
                    .to_string(),
                "arrêt rendu le 15 juin 2018, entre les parties, par la cour d'appel de Nancy"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn attacked_text_spans_cross_the_president_of_the_court() {
        let text = "contre l'ordonnance rendue le 5 avril 2018 par le président du \
                    tribunal de grande instance de Paris, statuant en référé.";
        let mut d = decision("CC", text);
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Tribunal de grande instance de Paris".into()),
            number: Some("17/59899".into()),
            date: Some("2018-04-05".into()),
        });
        let surfaces: Vec<String> = attacked_text_spans(&d, &snap())
            .iter()
            .map(|(s, e, r)| {
                assert_eq!(r, "tj|tj75056|17 59899|2018-04-05");
                text.chars().skip(*s).take(e - s).collect()
            })
            .collect();
        assert_eq!(
            surfaces,
            vec![
                "ordonnance rendue le 5 avril 2018 par le président du tribunal de grande \
                 instance de Paris"
                    .to_string()
            ]
        );
    }

    #[test]
    fn unsupported_target_yields_nothing() {
        let mut d = decision("CA", "");
        d.attacked = Some(lj_core::decision::AttackedRef {
            jurisdiction: Some("Conseil de Prud'hommes de PARIS".into()),
            number: Some("F15/02585".into()),
            date: Some("2017-09-07".into()),
        });
        assert!(liens(&d, &snap()).is_empty());
    }

    #[test]
    fn ambiguous_city_does_not_map() {
        let s = ChronoSnapshot::new(vec![
            ("tj00001".into(), "TJ".into(), "Doublon".into()),
            ("tj00002".into(), "TJ".into(), "Doublon".into()),
        ]);
        assert!(s.location("TJ", "doublon").is_none());
    }
}
