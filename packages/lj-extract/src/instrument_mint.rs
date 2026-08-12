//! Frappe d'identités d'instruments internationaux : dérive du titre d'un
//! décret de publication l'identité de l'instrument publié — titre, nature
//! honnête, date de signature, cœur d'uid `INTL/…`.
//!
//! Traduction table en main (« Décret … portant publication de X » → X), jamais
//! d'inférence d'identité depuis des motifs libres : un titre hors grammaire
//! sort en [`Derivation::Detag`] (acte domestique tagué à tort) ou
//! [`Derivation::Review`] (à trancher à la main) — jamais en pari.

use std::sync::LazyLock;

use regex::Regex;

use lj_core::text::{fold, slug};
use lj_core::truecase::{is_caps_lock, truecase};

/// Verdict de dérivation pour une fiche INTL héritée.
#[derive(Debug, Clone, PartialEq)]
pub enum Derivation {
    /// Décret/loi de publication : l'instrument est la queue du titre.
    Mint(MintedInstrument),
    /// La fiche porte déjà un titre d'instrument (DUDH…) : identité = titre entier.
    MintWhole(MintedInstrument),
    /// Acte domestique tagué à tort : dé-tag vers son identité native, sans frappe.
    Detag {
        /// Nature native lue en tête de titre (`CIRCULAIRE`, `AVIS`…).
        native_nature: String,
    },
    /// Hors grammaire : à trancher à la main avant tout batch.
    Review,
}

/// Identité d'instrument dérivée d'un titre de décret de publication.
#[derive(Debug, Clone, PartialEq)]
pub struct MintedInstrument {
    /// Titre de l'instrument (espaces normalisés, recasé si strate capitale,
    /// initiale majuscule, marqueur de renvoi « (1) » final retiré).
    pub title: String,
    /// Nature honnête (vocabulaire fermé de [`NATURE_HEADS`]).
    pub nature: String,
    /// Date de signature (ISO) : date ancrée (« fait à X le … », « signé
    /// le … ») en priorité, sinon la date unique du segment ; ambigu → None.
    pub signed: Option<String>,
    /// Cœur d'uid `INTL/<SLUG>`, sans suffixe de collision : le suffixage global
    /// appartient à l'outil de frappe.
    pub uid_core: String,
}

/// Têtes de titre → nature d'instrument (vocabulaire fermé ; pluriels par
/// préfixe). L'ordre n'importe pas : la première tête reconnue du segment
/// gagne, après saut des articles et qualifieurs (« projet de », « texte
/// révisé de »…).
const NATURE_HEADS: &[(&str, &str)] = &[
    ("convention", "CONVENTION"),
    ("accord", "ACCORD"),
    ("avenant", "AVENANT"),
    ("protocole", "PROTOCOLE"),
    ("traite", "TRAITE"),
    ("amendement", "AMENDEMENT"),
    ("declaration", "DECLARATION"),
    ("charte", "CHARTE"),
    ("pacte", "PACTE"),
    ("statut", "STATUT"),
    ("mesure", "MESURE"),
    ("resolution", "RESOLUTION"),
    ("reglement", "REGLEMENT"),
    ("arrangement", "ARRANGEMENT"),
    ("memorandum", "MEMORANDUM"),
    ("modification", "MODIFICATION"),
    ("constitution", "CONSTITUTION"),
    ("code", "CODE"),
    ("recommandation", "RECOMMANDATION"),
    ("annexe", "ANNEXE"),
    ("instrument", "INSTRUMENT"),
    ("acte", "ACTE"),
];

/// Têtes de titre → nature native domestique (fiches taguées à tort : le
/// dé-tag restaure la classe que le titre énonce lui-même).
const DETAG_HEADS: &[(&str, &str)] = &[
    ("circulaire", "CIRCULAIRE"),
    ("avis", "AVIS"),
    ("arrete", "ARRETE"),
    ("instruction", "INSTRUCTION"),
    ("note", "NOTE"),
    ("deliberation", "DELIBERATION"),
];

/// Têtes de véhicule : seules ces classes portent la grammaire « portant
/// publication de … ».
const VEHICLE_HEADS: &[&str] = &["decret", "loi", "ordonnance"];

/// Mots sautés entre la coupe et la tête de nature (articles, qualifieurs,
/// ordinaux — « du quatrième avenant », « des deux échanges de lettres »).
const SKIP_WORDS: &[&str] = &[
    "la",
    "le",
    "l",
    "les",
    "un",
    "une",
    "des",
    "du",
    "de",
    "d",
    "projet",
    "texte",
    "textes",
    "revise",
    "revisee",
    "nouveau",
    "nouvelle",
    "ensemble",
    "premier",
    "premiere",
    "deuxieme",
    "troisieme",
    "quatrieme",
    "cinquieme",
    "sixieme",
    "septieme",
    "huitieme",
    "neuvieme",
    "dixieme",
    "deux",
    "trois",
    "quatre",
    "cinq",
    "six",
    "sept",
    "huit",
    "neuf",
    "dix",
];

/// Boilerplate exclu du cœur d'uid : les gouvernements contractants et la
/// glose relationnelle ne discriminent rien — le pays, la matière et la date
/// vivent dans la queue du titre. Les mots ≤ 2 chars sont exclus d'office.
const UID_STOP_WORDS: &[&str] = &[
    "gouvernement",
    "gouvernements",
    "republique",
    "francaise",
    "france",
    "royaume",
    "etat",
    "etats",
    "entre",
    "pour",
    "sur",
    "dans",
    "par",
    "aux",
    "avec",
    "relatif",
    "relative",
    "relatifs",
    "relatives",
    "concernant",
    "portant",
    "ainsi",
    "que",
    "qui",
    "dont",
    "ensemble",
    "autre",
    "part",
    "une",
    "les",
    "des",
    "signe",
    "signee",
    "signes",
    "signees",
    "fait",
    "faite",
    "faits",
    "faites",
    "adopte",
    "adoptee",
    "conclu",
    "conclue",
    "ouvert",
    "ouverte",
    "signature",
    "matiere",
    "domaine",
    "cadre",
    "titre",
    "sous",
    "forme",
    "ont",
    "ete",
    "son",
    "ses",
    "leur",
    "leurs",
    "cette",
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

/// Têtes admises seulement en contexte de coupe (« portant publication
/// de … ») : hors coupe, ces mots ouvrent des actes domestiques homonymes
/// (« Décision n° 93-318 DC », lettres ministérielles…).
const CUT_ONLY_HEADS: &[(&str, &str)] = &[
    ("lettre", "LETTRE"),
    ("decision", "DECISION"),
    ("proces", "PROCES_VERBAL"),
    ("recueil", "RECUEIL"),
];

static RE_CUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:portant\s+)?(?:publication|promulgation)\s*[;:,]?\s+(?:et\s+(?:de\s+la\s+)?(?:mise\s+en\s+)?(?:vigueur|application)\s*(?:,?\s*a\s+titre\s+provisoire\s*,?\s*|provisoire\s+)?(?:de\s+)?)?(?:de\s+la\s+|de\s+l['’]\s*|du\s+|des\s+|de\s+|d['’]\s*)(.+)$",
    )
    .unwrap()
});

/// Dates du segment instrument (folded) : « le/du 25 fevrier 1965 » et la
/// graphie numérique historique « 04-10-1956 ».
static RE_SEG_DATE: LazyLock<Regex> = LazyLock::new(|| {
    // `les` : les signatures plurielles (« signées les 28 novembre 1996 et
    // 21 janvier 1997 ») comptent comme dates.
    Regex::new(r"\b(?:du|les?)\s+(\d{1,2})(?:er)?\s+(janvier|fevrier|mars|avril|mai|juin|juillet|aout|septembre|octobre|novembre|decembre)\s+(\d{4})\b").unwrap()
});
static RE_SEG_DATE_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,2})-(\d{2})-(\d{4})\b").unwrap());
/// Ancres de signature : la date qui suit désigne la signature de CET acte
/// (et non celle d'un acte hôte cité dans le segment, ex. « avenant à
/// l'accord du 04-10-1956 »).
static RE_SIGN_ANCHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:faits?|faites?|signes?|signees?|adoptes?|adoptees?|conclus?|conclues?|ouverts? a la signature|ouvertes? a la signature)\b[^,;]*?\b(?:le|du)\s").unwrap()
});

/// Dérive l'identité d'instrument d'une fiche INTL héritée depuis son titre.
pub fn derive_instrument(title: &str) -> Derivation {
    let clean = clean_title(title);
    let folded = fold(&clean);
    let head = folded.split_whitespace().next().unwrap_or("");

    if let Some((_, native)) = DETAG_HEADS.iter().find(|(h, _)| head.starts_with(h)) {
        return Derivation::Detag {
            native_nature: (*native).to_string(),
        };
    }

    if VEHICLE_HEADS.iter().any(|h| head.starts_with(h)) {
        let Some(m) = RE_CUT.captures(&clean) else {
            return Derivation::Review;
        };
        let seg = m.get(1).expect("groupe 1 de RE_CUT").as_str();
        return match minted(seg, true) {
            Some(minted) => Derivation::Mint(minted),
            None => Derivation::Review,
        };
    }

    // La fiche porte directement un titre d'instrument (DUDH…) : l'identité
    // est le titre entier, sans coupe.
    if classify_nature(&folded, false).is_some() {
        return match minted(&clean, false) {
            Some(minted) => Derivation::MintWhole(minted),
            None => Derivation::Review,
        };
    }

    Derivation::Review
}

/// Construit l'identité depuis le segment instrument ; `None` si la tête de
/// nature n'est pas au vocabulaire.
fn minted(seg: &str, cut: bool) -> Option<MintedInstrument> {
    let seg = seg.trim();
    let folded = fold(seg);
    let nature = classify_nature(&folded, cut)?;
    let mut title = if is_caps_lock(seg) {
        truecase(seg)
    } else {
        seg.to_string()
    };
    if let Some(first) = title.chars().next() {
        let upper: String = first.to_uppercase().collect();
        title = upper + &title[first.len_utf8()..];
    }
    let uid_core = uid_core(&folded, &nature);
    Some(MintedInstrument {
        signed: signature_date(&folded),
        title,
        nature,
        uid_core,
    })
}

/// Première tête de nature du segment folded, après saut des articles et
/// qualifieurs. `None` = hors vocabulaire. `cut` = segment issu d'une coupe
/// « portant publication de … » : le contexte lève l'ambiguïté de têtes
/// interdites en titre entier (une « Décision n° 93-318 DC » n'est pas un
/// instrument ; « la décision du Conseil réuni au niveau des chefs d'État »
/// publiée par décret en est un).
fn classify_nature(folded_seg: &str, cut: bool) -> Option<String> {
    for word in folded_seg.split_whitespace().take(8) {
        let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if SKIP_WORDS.contains(&word) {
            continue;
        }
        if word == "echange" || word == "echanges" {
            // « échange de lettres/notes » : la matière est le complément.
            let target = if folded_seg.contains("de notes") {
                "ECHANGE_NOTES"
            } else {
                "ECHANGE_LETTRES"
            };
            return Some(target.to_string());
        }
        if cut {
            if let Some((_, n)) = CUT_ONLY_HEADS.iter().find(|(h, _)| word.starts_with(h)) {
                return Some((*n).to_string());
            }
        }
        return NATURE_HEADS
            .iter()
            .find(|(h, _)| word.starts_with(h))
            .map(|(_, n)| (*n).to_string());
    }
    None
}

/// Cœur d'uid `INTL/<NATURE>-<tokens distinctifs>` : le boilerplate
/// contractuel est exclu ([`UID_STOP_WORDS`]), les 4 premiers tokens restants
/// portent le pays et la matière (l'outil de frappe ajoute l'année et le
/// suffixe de collision).
fn uid_core(folded_seg: &str, nature: &str) -> String {
    let nature_parts: Vec<String> = nature.split('_').map(str::to_lowercase).collect();
    let tokens: Vec<String> = folded_seg
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3)
        .filter(|w| !UID_STOP_WORDS.contains(w) && !SKIP_WORDS.contains(w))
        .filter(|w| !nature_parts.iter().any(|p| w.starts_with(p.as_str())))
        .filter(|w| {
            // Les années rejoignent l'uid par l'outil (année de signature ou
            // du véhicule), pas par le titre.
            !(w.len() == 4 && w.starts_with(['1', '2']) && w.chars().all(|c| c.is_ascii_digit()))
        })
        .take(4)
        .map(str::to_string)
        .collect();
    let stem = if tokens.is_empty() {
        slug(folded_seg)
    } else {
        tokens.join("-")
    };
    format!(
        "INTL/{}-{}",
        nature.replace('_', "-"),
        truncate_slug(&stem, 48)
    )
}

/// Date de signature du segment folded : date ancrée en priorité, sinon la
/// date unique du segment, sinon None (ambigu — un avenant citant son acte
/// hôte daté porte deux dates).
fn signature_date(folded_seg: &str) -> Option<String> {
    // Fenêtres depuis le DÉBUT de chaque ancre : le « le/du » consommé par
    // l'ancre est le préfixe que le motif de date exige. Plusieurs ancres à dates
    // distinctes → abstention.
    let mut anchored: Vec<String> = RE_SIGN_ANCHOR
        .find_iter(folded_seg)
        .filter_map(|m| first_date(&folded_seg[m.start()..]))
        .collect();
    anchored.dedup();
    match anchored.as_slice() {
        [one] => return Some(one.clone()),
        [] => {}
        _ => return None,
    }
    let all = all_dates(folded_seg);
    match all.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

fn first_date(folded: &str) -> Option<String> {
    all_dates(folded).into_iter().next()
}

fn all_dates(folded: &str) -> Vec<String> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for m in RE_SEG_DATE.captures_iter(folded) {
        let day: u32 = m[1]
            .parse()
            .ok()
            .filter(|d| (1..=31).contains(d))
            .unwrap_or(0);
        if day == 0 {
            continue;
        }
        let month = match &m[2] {
            "janvier" => 1,
            "fevrier" => 2,
            "mars" => 3,
            "avril" => 4,
            "mai" => 5,
            "juin" => 6,
            "juillet" => 7,
            "aout" => 8,
            "septembre" => 9,
            "octobre" => 10,
            "novembre" => 11,
            _ => 12,
        };
        out.push((
            m.get(0).expect("match RE_SEG_DATE").start(),
            format!("{}-{month:02}-{day:02}", &m[3]),
        ));
    }
    for m in RE_SEG_DATE_NUM.captures_iter(folded) {
        let (day, month): (u32, u32) = (m[1].parse().unwrap_or(0), m[2].parse().unwrap_or(0));
        if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
            continue;
        }
        out.push((
            m.get(0).expect("match RE_SEG_DATE_NUM").start(),
            format!("{}-{month:02}-{day:02}", &m[3]),
        ));
    }
    out.sort();
    out.into_iter().map(|(_, d)| d).collect()
}

/// Espaces normalisés, chars de format (U+200E…) retirés, marqueur de renvoi
/// « (1) » final coupé.
fn clean_title(title: &str) -> String {
    static RE_FOOTNOTE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\(\d+\)\s*\.?\s*$").unwrap());
    let no_format: String = title
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{feff}'))
        .collect();
    let squeezed = no_format.split_whitespace().collect::<Vec<_>>().join(" ");
    RE_FOOTNOTE.replace(&squeezed, "").trim().to_string()
}

/// Tronque un slug à `max` chars sur une frontière de mot, en MAJUSCULES
/// (style des uids instrument existants : `CONV-GENEVE-1951`).
fn truncate_slug(s: &str, max: usize) -> String {
    let upper = s.to_ascii_uppercase();
    if upper.len() <= max {
        return upper;
    }
    match upper[..max].rfind('-') {
        Some(cut) => upper[..cut].to_string(),
        None => upper[..max].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint(title: &str) -> MintedInstrument {
        match derive_instrument(title) {
            Derivation::Mint(m) => m,
            other => panic!("attendu Mint, obtenu {other:?}"),
        }
    }

    #[test]
    fn publication_moderne() {
        let m = mint(
            "Décret n° 2015-1542 du 26 novembre 2015 portant publication de l'accord entre le \
             Gouvernement de la République française et le Gouvernement de la République \
             d'Azerbaïdjan relatif à la coopération en matière de sécurité intérieure, signé à \
             Bakou le 27 mars 2014",
        );
        assert_eq!(m.nature, "ACCORD");
        assert!(m.title.starts_with("Accord entre le Gouvernement"));
        assert_eq!(m.signed.as_deref(), Some("2014-03-27"));
        assert_eq!(
            m.uid_core,
            "INTL/ACCORD-AZERBAIDJAN-COOPERATION-SECURITE-INTERIEURE"
        );
    }

    #[test]
    fn strate_capitale_recasee_et_date_numerique() {
        let m = mint(
            "Décret n°49-379 du 17 mars 1949 PUBLICATION DE LA CONVENTION NO 44 DE GENEVE DU \
             23-06-1934 RELATIVE AUX INDEMNITES OU ALLOCATIONS AUX CHOMEURS INVOLONTAIRES",
        );
        assert_eq!(m.nature, "CONVENTION");
        assert_eq!(m.signed.as_deref(), Some("1934-06-23"));
        assert!(!is_caps_lock(&m.title), "titre recasé : {}", m.title);
    }

    #[test]
    fn mise_en_vigueur_provisoire() {
        let m = mint(
            "Décret n°55-1677 du 29 décembre 1955 PUBLICATION ET MISE EN VIGUEUR PROVISOIRE DE \
             L'ACCORD DE TUNIS DU 28 décembre 1955, RELATIF A LA NAVIGATION MARITIME",
        );
        assert_eq!(m.nature, "ACCORD");
        assert_eq!(m.signed.as_deref(), Some("1955-12-28"));
    }

    #[test]
    fn avenant_deux_dates_sans_ancre_reste_sans_signature() {
        let m = mint(
            "Décret n°87-735 du 1 septembre 1987 PORTANT PUBLICATION DE L'AVENANT DU 20-12-1985 \
             A L'ACCORD DU 04-10-1956 ENTRE LE GOUVERNEMENT DE LA REPUBLIQUE FRANCAISE ET LE \
             GOUVERNEMENT DU CANADA",
        );
        assert_eq!(m.nature, "AVENANT");
        assert_eq!(m.signed, None);
    }

    #[test]
    fn dates_plurielles_et_ancres_multiples_abstiennent() {
        let m = mint(
            "Décret n° 97-1332 du 30 décembre 1997 portant publication de l'accord sous forme \
             d'échange de lettres entre le Gouvernement de la République française et le \
             Gouvernement du Royaume d'Espagne amendant l'accord cinématographique du \
             25 mars 1988, signées à Madrid les 28 novembre 1996 et 21 janvier 1997",
        );
        assert_eq!(m.signed, None);
        let m = mint(
            "Décret portant publication des lettres échangées entre le gouvernement français \
             et le gouvernement suédois en vue de modifier la convention tendant à éviter les \
             doubles impositions, signée à Paris le 24 décembre 1936, et l'avenant à la même \
             convention, signé à Paris le 29 mars 1956",
        );
        assert_eq!(m.signed, None);
    }

    #[test]
    fn date_ancree_gagne_sur_la_date_de_l_acte_hote() {
        let m = mint(
            "Décret n°71-96 du 2 février 1971 PORTANT PUBLICATION DE L'AVENANT A LA CONVENTION \
             DU 8 AVRIL 1966 ENTRE LA FRANCE ET L'ALGERIE RELATIVE A LA COOPERATION TECHNIQUE \
             ET CULTURELLE ET DE L'ECHANGE DE LETTRES SIGNES A ALGER LE 22 août 1970",
        );
        assert_eq!(m.nature, "AVENANT");
        assert_eq!(m.signed.as_deref(), Some("1970-08-22"));
    }

    #[test]
    fn promulgation_projet_de_convention() {
        let m = mint(
            "Décret du 27 juillet 1939 portant promulgation du projet de convention \
             internationale fixant l'âge minimum d'admission des enfants aux travaux non \
             industriels, adoptée à Genève le 24 juin 1932",
        );
        assert_eq!(m.nature, "CONVENTION");
        assert_eq!(m.signed.as_deref(), Some("1932-06-24"));
    }

    #[test]
    fn amendements_et_echange_de_lettres() {
        let m = mint(
            "Décret no 97-340 du 10 avril 1997 portant publication des amendements à la \
             Convention internationale de 1974 pour la sauvegarde de la vie humaine en mer (1)",
        );
        assert_eq!(m.nature, "AMENDEMENT");
        assert!(!m.title.ends_with("(1)"));

        let m = mint(
            "Décret n°65-253 du 31 mars 1965 PORTANT PUBLICATION DE L'ECHANGE DE LETTRES ENTRE \
             LA FRANCE ET LA GRANDE-BRETAGNE DU 25 février 1965, RELATIF A L'ATTRIBUTION D'UN \
             TERRAIN",
        );
        assert_eq!(m.nature, "ECHANGE_LETTRES");
        assert_eq!(m.signed.as_deref(), Some("1965-02-25"));
    }

    #[test]
    fn circulaire_taguee_a_tort_se_detague() {
        assert_eq!(
            derive_instrument(
                "Circulaire du 12 mai 1987 RELATIVE A L'ACTION EXTERIEURE DES COLLECTIVITES \
                 TERRITORIALES"
            ),
            Derivation::Detag {
                native_nature: "CIRCULAIRE".to_string()
            }
        );
    }

    #[test]
    fn titre_d_instrument_direct_frappe_entier() {
        match derive_instrument("Déclaration universelle des droits de l'homme du 10 décembre 1948")
        {
            Derivation::MintWhole(m) => {
                assert_eq!(m.nature, "DECLARATION");
                assert_eq!(m.signed.as_deref(), Some("1948-12-10"));
            }
            other => panic!("attendu MintWhole, obtenu {other:?}"),
        }
    }

    #[test]
    fn hors_grammaire_part_en_review() {
        assert_eq!(
            derive_instrument("du 26 juillet 1979 RELATIF AUX BIENS ET AVOIRS FRANCAIS EN GUINEE"),
            Derivation::Review
        );
        assert_eq!(
            derive_instrument("Décret n° 2004-21 du 7 janvier 2004 relatif aux archives"),
            Derivation::Review
        );
    }
}
