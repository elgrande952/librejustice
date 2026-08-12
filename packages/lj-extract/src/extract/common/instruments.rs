//! Normalisation et canonicalisation des instruments (codes, lois, décrets,
//! conventions, directives/règlements UE…).

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::data::{instrument_aliases, legifrance_codes};

use super::text::{capitalize_only, fold, uppercase_first};

const CGI_LPF: &str = "Code général des impôts et le livre des procédures fiscales";

/// Supprime une séquence de 1 à 4 mots immédiatement répétée à l'identique, au
/// pli près : « Code de code de procédure civile ».
pub(crate) fn collapse_adjacent_repeats(raw: &str) -> String {
    let mut tokens: Vec<&str> = raw.split_whitespace().collect();
    let folded: Vec<String> = tokens.iter().map(|t| fold(t)).collect();
    let mut folded = folded;
    let mut changed = true;
    while changed {
        changed = false;
        'scan: for k in 1..=4usize {
            if tokens.len() < 2 * k {
                continue;
            }
            for i in 0..=(tokens.len() - 2 * k) {
                if folded[i..i + k] == folded[i + k..i + 2 * k] {
                    tokens.drain(i + k..i + 2 * k);
                    folded.drain(i + k..i + 2 * k);
                    changed = true;
                    break 'scan;
                }
            }
        }
    }
    tokens.join(" ")
}

/// Snapshot officiel des titres de codes Légifrance, plié + trié par longueur
/// de clé décroissante.
static CANON_CODE_TITLES: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
    let mut pairs: Vec<(String, String)> = legifrance_codes()
        .codes
        .into_iter()
        .map(|c| (fold(&c.titre), c.titre))
        .collect();
    pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
    pairs
});

/// Variantes orthographiques figées → titre Légifrance officiel. Clés en
/// lowercase accentué.
static LEGIFRANCE_CODE_ALIASES: &[(&str, &str)] = &[
    (
        "nouveau code de procédure civile",
        "Code de procédure civile",
    ),
    (
        "code justice administrative",
        "Code de justice administrative",
    ),
    (
        "code de la justice administrative",
        "Code de justice administrative",
    ),
    (
        "code de justice administratif",
        "Code de justice administrative",
    ),
    (
        "code de l'entrée et du séjour et du droit d'asile",
        "Code de l'entrée et du séjour des étrangers et du droit d'asile",
    ),
    (
        "code des relations du public avec l'administration",
        "Code des relations entre le public et l'administration",
    ),
    (
        "code des relations entre l'administration et le public",
        "Code des relations entre le public et l'administration",
    ),
    (
        "code de la fonction publique",
        "Code général de la fonction publique",
    ),
    ("code du commerce", "Code de commerce"),
    ("code commerce", "Code de commerce"),
    ("code procédure civile", "Code de procédure civile"),
    ("code de sécurité sociale", "Code de la sécurité sociale"),
    (
        "code de sécurité intérieure",
        "Code de la sécurité intérieure",
    ),
    (
        "code des postes et communications électroniques",
        "Code des postes et des communications électroniques",
    ),
];

/// Racines (pliées) de gentilés étrangers. Un titre de code français n'en porte
/// jamais : sa présence signale du droit étranger, qu'on ne replie pas sur le code
/// FR homonyme. Comparaison par préfixe de token plié.
pub(crate) const FOREIGN_NATIONALITY_STEMS: &[&str] = &[
    "iranien",
    "irakien",
    "syrien",
    "libanai",
    "afghan",
    "pakistanai",
    "bangladai",
    "indien",
    "ivoirien",
    "guineen",
    "malien",
    "senegalai",
    "congolai",
    "camerounai",
    "tchadien",
    "nigerien",
    "nigeria",
    "soudanai",
    "erythreen",
    "ethiopien",
    "somalien",
    "centrafricain",
    "rwandai",
    "burundai",
    "gabonai",
    "togolai",
    "beninoi",
    "gambien",
    "liberien",
    "ghaneen",
    "angolai",
    "mauritanien",
    "marocain",
    "tunisien",
    "algerien",
    "libyen",
    "egyptien",
    "palestinien",
    "jordanien",
    "yemenite",
    "saoudien",
    "georgien",
    "armenien",
    "azerbaidjanai",
    "ukrainien",
    "russe",
    "bielorusse",
    "tchetchene",
    "albanai",
    "serbe",
    "bosnien",
    "kosovar",
    "macedonien",
    "chinoi",
    "vietnamien",
    "cambodgien",
    "birman",
    "tibetain",
    "mongol",
    "srilankai",
    "nepalai",
    "bhoutanai",
    "ouzbek",
    "tadjik",
    "kirghiz",
    "kazakh",
    "turkmene",
    "haitien",
    "colombien",
    "venezuelien",
    "salvadorien",
    "hondurien",
    "kurde",
    "turc",
    "turqu",
    "allemand",
    "autrichien",
    "suisse",
    "belge",
    "luxembourgeoi",
    "neerlandai",
    "hollandai",
    "espagnol",
    "portugai",
    "italien",
    "anglai",
    "britannique",
    "ecossai",
    "irlandai",
    "americain",
    "canadien",
    "quebecoi",
    "bresilien",
    "argentin",
    "mexicain",
    "chilien",
    "danoi",
    "suedoi",
    "norvegien",
    "finlandai",
    "islandai",
    "polonai",
    "tcheque",
    "slovaque",
    "slovene",
    "hongroi",
    "roumain",
    "bulgare",
    "grec",
    "croate",
    "estonien",
    "letton",
    "lituanien",
    "maltai",
    "chypriote",
    "monegasque",
    "japonai",
    "coreen",
    "indonesien",
    "thailandai",
    "philippin",
    "australien",
    "neozelandai",
    // « français » est volontairement exclu : « code civil français » se replie
    // sur « Code civil ».
    // — Europe (compléments)
    "moldave",
    "montenegrin",
    "andorran",
    "liechtensteinoi",
    "saint-marinai",
    // — Moyen-Orient / Golfe
    "israelien",
    "koweitien",
    "qatari",
    "bahreini",
    "omanai",
    "emirati",
    // — Afrique (compléments)
    "africain",
    "sud-africain",
    "burkinabe",
    "capverdien",
    "malgache",
    "mauricien",
    "comorien",
    "seychelloi",
    "djiboutien",
    "kenyan",
    "tanzanien",
    "ougandai",
    "zambien",
    "zimbabween",
    "mozambicain",
    "namibien",
    "botswanai",
    "swazi",
    "nigerian",
    "sierra-leonai",
    "bissau-guineen",
    "equato-guineen",
    // — Amériques (compléments)
    "cubain",
    "dominicain",
    "jamaiquain",
    "trinidadien",
    "guatemalteque",
    "costaricien",
    "panameen",
    "nicaraguayen",
    "equatorien",
    "peruvien",
    "bolivien",
    "paraguayen",
    "uruguayen",
    "portoricain",
    "guyanien",
    "surinamai",
    // — Asie (compléments)
    "laotien",
    "malaisien",
    "singapourien",
    "bruneien",
    "taiwanai",
    "maldivien",
    "timorai",
    "qatarien",
    // — Océanie
    "fidjien",
    "papouasien",
    "samoan",
    "tongien",
];

/// Titre officiel dont `raw` (plié) est un préfixe à la frontière de mot, et
/// seulement si un seul convient. Borné aux noms commençant par « code ».
pub(crate) fn snap_code_name(raw: &str) -> Option<String> {
    let folded = fold(raw);
    if !folded.starts_with("code") && !folded.starts_with("livre") {
        return None;
    }
    // Un « code … <gentilé étranger> » ne se replie pas sur le code FR homonyme ;
    // on tronque seulement la prose captée après le gentilé.
    if folded
        .split_whitespace()
        .any(|w| FOREIGN_NATIONALITY_STEMS.iter().any(|s| w.starts_with(s)))
    {
        return cut_after_foreign_code_gentile(raw);
    }
    for (canon_folded, title) in CANON_CODE_TITLES.iter() {
        if &folded == canon_folded {
            return Some(title.clone());
        }
        if folded.starts_with(canon_folded.as_str()) {
            if let Some(next) = folded[canon_folded.len()..].chars().next() {
                if !next.is_alphanumeric() {
                    return Some(title.clone());
                }
                // Prose collée SANS espace à un titre complet (« Code de procédure
                // civileles dépens ») : les extensions réelles passent par une espace.
                if next.is_lowercase() {
                    return Some(title.clone());
                }
            }
        }
    }
    // Snap-up : un préfixe sans ambiguïté d'EXACTEMENT un titre officiel.
    let mut expansions: HashSet<&str> = HashSet::new();
    for (canon_folded, title) in CANON_CODE_TITLES.iter() {
        if canon_folded.len() > folded.len() && canon_folded.starts_with(&folded) {
            if let Some(next) = canon_folded[folded.len()..].chars().next() {
                if !next.is_alphanumeric() {
                    expansions.insert(title.as_str());
                }
            }
        }
    }
    if expansions.len() == 1 {
        return expansions.into_iter().next().map(str::to_string);
    }
    // Squelette : liaison fausse ou manquante (« Code la sécurité sociale »),
    // pluriel divergent. Jamais de snap sur ambiguïté.
    let skel = code_skeleton(&folded);
    if !skel.is_empty() {
        if let Some(title) = CODE_TITLE_SKELETONS.get(&skel) {
            return Some(title.clone());
        }
    }
    None
}

/// Tronque ce qui suit le gentilé : « Code <tête> <gentilé> » EST l'identité, le
/// reste est de la prose. Gaté sur tête « code/livre ».
fn cut_after_foreign_code_gentile(raw: &str) -> Option<String> {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let head = fold(tokens.first()?);
    if !head.starts_with("code") && !head.starts_with("livre") {
        return None;
    }
    for (i, tok) in tokens.iter().enumerate() {
        let folded = fold(tok);
        let word = folded.trim_matches(|c: char| !c.is_alphanumeric());
        if !word.is_empty()
            && FOREIGN_NATIONALITY_STEMS
                .iter()
                .any(|s| word.starts_with(s))
        {
            // Nettoie la ponctuation collée au gentilé en position finale.
            let clean = tok.trim_end_matches(|c: char| !c.is_alphanumeric());
            let has_trailing_tokens = i + 1 < tokens.len();
            let gentile_has_junk = clean.len() != tok.len();
            if !has_trailing_tokens && !gentile_has_junk {
                return None;
            }
            let mut kept: Vec<&str> = tokens[..i].to_vec();
            kept.push(clean);
            return Some(kept.join(" "));
        }
    }
    None
}

// Mots de liaison nus, sans valeur discriminante dans un titre de code.
const SKELETON_STOPWORDS: &[&str] = &[
    "de", "du", "des", "d", "l", "le", "la", "les", "et", "a", "au", "aux", "en",
];

/// Squelette d'un titre plié : apostrophes éclatées, liaisons retirées, pluriels
/// normalisés. `code`/`livre` conservés comme ancre.
fn code_skeleton(folded: &str) -> String {
    folded
        .replace('\'', " ")
        .split_whitespace()
        .filter(|w| !SKELETON_STOPWORDS.contains(w))
        .map(|w| match w.strip_suffix('s') {
            Some(stem) if stem.chars().count() > 3 => stem.to_string(),
            _ => w.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `squelette → titre officiel`, restreint aux squelettes **non ambigus** : une
/// collision est écartée plutôt que tranchée.
static CODE_TITLE_SKELETONS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    let mut by_skel: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (canon_folded, title) in CANON_CODE_TITLES.iter() {
        by_skel
            .entry(code_skeleton(canon_folded))
            .or_default()
            .push(title.clone());
    }
    by_skel
        .into_iter()
        .filter_map(|(skel, titles)| {
            let uniq: HashSet<&String> = titles.iter().collect();
            (uniq.len() == 1).then(|| (skel, titles.into_iter().next().unwrap()))
        })
        .collect()
});

static RE_INSTRUMENT_PROSE_CUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)\s+(?:que|qu['\x{2019}]|qui|dont|lequel|laquelle|lesquels|lesquelles|duquel|desquels|desquelles|auquel|auxquels|auxquelles|vise|visent|résulte|resulte|ainsi|selon|lorsque|lorsqu['\x{2019}]|pendant|compose|composent|ne|n['\x{2019}]|permet|permettent|permettant|comporte|comportant|énonçant|énonce|signé|signée|signés|signées|par|les\s+dispositions|a\s+remplacé|ont\s+remplacé|remplace|remplaçant|est|sont|était|étaient|exclut|inclut|prévoit|prévoient|stipule|stipulent|conforme|imposant|organisant|tendant\s+à|et\s+(?:à\s+)?l['\x{2019}]articles?|a\s+été\s+respect[éè]e?s?|se\s+substituant|quant\s+à)\b.*$",
    )
    .unwrap()
});

static RE_INSTR_INTRA_NEWLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([a-z\x{e0}-\x{ff}])[ \t]*\n[ \t]*([a-z\x{e0}-\x{ff}])").unwrap()
});
static RE_INSTR_ET_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+et\s+\d.*$").unwrap());
// Prose collée sans séparateur à un millésime (« …1989ordonner l'expulsion ») :
// on coupe à partir de la lettre. Les numéros UE sont suivis d'un `/`, pas d'une
// lettre.
static RE_INSTR_GLUED_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})[a-z\x{e0}-\x{ff}].*$").unwrap());
// Connecteur pendant en fin de capture (« … et », « … relatif »). Itéré.
static RE_INSTR_TRAILING_CONNECTOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+(?:et|relatif|relative|applicable)$").unwrap());
// Queue verbale capturée depuis le corps : clause, pas titre.
static RE_INSTR_VERB_TAIL2: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:et\s+)?(?:ont|avait|avaient)\s+(?:été\s+)?\w+.*$").unwrap()
});
// Queue de prose verbale collée au nom (« Code de l'expropriation dispose »).
// On NE coupe PAS sur « et le/la <nom> », « comme », « sur l'aide » : ces formes
// appartiennent à de vrais titres.
static RE_INSTR_GLUED_VERB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s+(?:(?:définit|definit|dispose|disposant|prohibent|prohibant|prohibe|impose|imposant|exige|exigent|autorise|autorisent|figurant|prises|prise|créé|cree|institue|prévoyant|prevoyant|consistant|étant|mettent|mettant|met|incombe|incombent|reposait|repose|reposent|resteront|restent|prescrivent|prescrit)\b|(?:il\s+)?incombe\s+à\b|et\s+(?:a|ont|avait|avaient)\s+\w|et\s+les\s+articles?\b|et\s+aux\s+(?:entiers\s+)?d[ée]pens\b|et\s+(?:aux|les)\s+d[ée]pens\b|composition\s+de\s+la\s+cour\b).*$",
    )
    .unwrap()
});
static RE_INSTR_SUBDIVISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:livre|titre|chapitre|section)\s+\S+(?:\s+(?:bis|ter|préliminaire))?\s+du\s+(code\b.*)$",
    )
    .unwrap()
});
static RE_INSTR_VERB_CUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s+(?:stipule\s+que|dispose\s+(?:que|notamment)|prévoit\s+(?:que|qu['\x{2019}])|garantit\s+que|rappelle\s+qu[e\x{2019}']|permet\s+(?:de|que|qu['\x{2019}]|notamment)|prohibe\b|suppose\b|dès\s+lors\s+que).*$",
    )
    .unwrap()
});
static RE_INSTR_PROSE_CUT2: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s+(?:doit\s+être\s+\w+|doivent\s+être\s+\w+|et\s+fait\s+partie\b|ayant\s+la\s+même\s+valeur\s+que\b|ne\s+peut\s+qu['\x{2019}]être\b|qui\s+(?:dispose|prévoit|précise|stipule|garantit|comporte|interdit|impose|permet|autorise|réserve)\b|qu['\x{2019}]elle\s+a\b|que\s+lui\s+a\s+opposé\b|n['\x{2019}](?:impose|interdit|autorise|comporte|exige|prévoit)\b|permet-il\b|sont\s+assujetti(?:s|es?)\b|sont\s+entach[ée]e?s?\b|et\s+(?:est|sont)\s+entach[ée]e?s?\b|et\s+porte\s+atteinte\b|sous\s+réserve\s+que\b|alors\s+que\b|alors\s+qu['\x{2019}]|et\s+le\s+maire\s+n['\x{2019}]a\b|de\s+suspendre\s+l['\x{2019}]exécution\b|qu[\x{2018}\x{2019}]il\s+(?:s[\x{2018}\x{2019}]agit|comporte|ne\s+comporte)\b|lorsqu[\x{2018}\x{2019}]il\b|est\s+de\s+nature\b|ne\s+devant\b|à\s+l[\x{2018}\x{2019}]égard\s+de\b|et\s+contiennent\b|et\s+figurent?\b|figurent?\s+(?:à|dans|au)\b|rappel[eé]es?\s+ci-dessus\b|cit[eé]es?\b).*$",
    )
    .unwrap()
});
// Marqueurs de prose d'espèce, absents des titres officiels. « en cas de » est
// exclu, il figure dans de vrais titres ; « lors de » n'est coupé que devant un
// possessif ou un démonstratif.
static RE_INSTR_PROSE_CUT3: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*,?\s+(?:ser(?:a|ait|ont|aient)|doi(?:t|vent)|peu(?:t|vent)|aur(?:a|ait|ont|aient)|dev(?:ra|rait|ront|raient)|pourr(?:a|ait|ont|aient)|(?:a|ont)\s+été|dès\s+lors|à\s+peine\s+d|celui-ci|celle-ci|ceux-ci|celles-ci|c'est-à-dire|il\s+s'agit|s'agissant|puisque|puisqu'|car|afin\s+(?:de|d'|que|qu')|aux?\s+motifs?|faute\s+de|lors\s+de\s+(?:son|sa|ses|leur|leurs|ce|cette)\b)\b.*$",
    )
    .unwrap()
});
static RE_INSTR_TEL_QUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*,?\s+(?:tel(?:le)?s?\s+qu['\x{2019}]|dans\s+(?:sa|leur|cette)\s+(?:rédaction|version)\b|aux\s+termes\s+(?:du|des|de\s+l['\x{2019}]|duquel|desquels)\b|applicable\s+(?:au\s+litige|aux?\s+faits|[àa]\s+l['\x{2019}]espèce|en\s+l['\x{2019}]espèce|au\s+cas\s+d['\x{2019}]espèce|au\s+présent\s+litige)\b).*$",
    )
    .unwrap()
});
static RE_INSTR_NOTAMMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*,?\s+(?:et\s+)?notamment\s+(?:son|ses|le|la|les|l['\x{2019}]|du|des|au|aux)\b.*$",
    )
    .unwrap()
});
// Appareil de citation traîné après l'identité de l'instrument :
// - « modifié du <date> » : le mot est SUPPRIMÉ, la date qui suit est l'identité ;
// - « modifié par … », « devenu … », « alors … », « visé » nu : coupe ;
// - « ensemble <déterminant|chiffre> … » : coupe. Le déterminant est OBLIGATOIRE,
//   « ensemble » étant aussi un nom commun et un adverbe ;
// - « visé à l'article … » est INTACT, les titres officiels le contiennent.
static RE_INSTR_MODIFIE_DU: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+modifi[ée]e?s?\s+(du\s+\d)").unwrap());
static RE_INSTR_MODIFIE_PAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*,?\s+modifi[ée]e?s?\s+(?:par|notamment)\b.*$").unwrap());
static RE_INSTR_DEVENU: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*,?\s+devenue?s?\b.*$").unwrap());
static RE_INSTR_ENSEMBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*,?\s+ensemble\s+(?:le|la|les|l['\x{2019}]|du|des|de\s+la|de\s+l['\x{2019}]|au|aux|son|ses|celles?|ceux|\d)\b.*$",
    )
    .unwrap()
});
static RE_INSTR_ALORS_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*,?\s+alors(?:\s+(?:applicable|en\s+vigueur)\b.*)?$").unwrap()
});
static RE_INSTR_VISE_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+visée?s?(?:\s+(?:ci-dessus|plus\s+haut|précédemment)\b.*)?$").unwrap()
});
static RE_INSTR_SUSVISE_ET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\s+susvis[éè]e?s?)\s+et\s+.+$").unwrap());
// L'identité d'un instrument FR daté est « Famille [organique] du <date> » :
// après une date complète, tout ce qui suit est jeté. Hors familles UE (le numéro
// est l'identité) et conventionnelles (l'objet fait partie du nom). L'adjectif
// intercalé (« Arrêté préfectoral du… ») et « Loi du pays » restent dans la tête.
static RE_INSTR_DATED_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^((?:Loi(?:\s+du\s+pays)?|Décret|Decret|Ordonnance|Arrêté|Arrete|Décision|Decision|Délibération|Deliberation|Circulaire)(?:\s+organique)?(?:\s+(?:municipal|préfectoral|prefectoral|ministériel|ministeriel|interministériel|interministeriel|communal))?(?:\s+(?:n\s*[°o]?\.?\s*)?\d[\w./\-]*)?\s+(?:du\s+\d{1,2}(?:er)?|des\s+\d+(?:[\s,\-]+\d+)*(?:\s+et\s+\d+)?)\s+\p{L}+\s+\d{4})[\s,].*$",
    )
    .unwrap()
});
static RE_INSTR_SUSVISE_DU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^((?:Loi|Décret|Ordonnance|Convention|Charte|Règlement|Arrêté)(?:\s+organique)?)\s+susvis[éè]e?s?\s+du\b",
    )
    .unwrap()
});
static RE_INSTR_NUMERO_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bn°\s*(\d)").unwrap());
static RE_INSTR_UE_NUM_DASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bn°\s*(\d{1,4})-(\d{4})\b").unwrap());
static RE_INSTR_UE_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{4}/\d+)/\s+(UE|CE|CEE)\b").unwrap());
static RE_INSTR_DIRECTIVE_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(Directive|Règlement)\s+n°\s+(\d{4}/)").unwrap());
// Formats UE dégradés (gated tête Directive/Règlement) : « 2003-88 CE »,
// « 2003-88/CE », « 2003/88 CE » → « 2003/88/CE ».
static RE_INSTR_UE_YEAR_DASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b((?:19|20)\d{2})\s*-\s*(\d+)[/\s]+(CE|CEE|UE)\b").unwrap());
static RE_INSTR_UE_NOSLASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{4}/\d+)\s+(CE|CEE|UE)\b").unwrap());
// Identité UE uniformisée vers les formes dominantes du JO : « Règlement
// (SIGLE) n° NUM » et « Directive NUM/SIGLE ».
static RE_INSTR_EU_NUM_ZEROS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b0+(\d+/\d+)").unwrap());
static RE_INSTR_RGT_CANON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:Règlement|Reglement)\s+\(?(CE|CEE|UE)\)?\s*(?:n\s*[°o]?\s*)?(\d+(?:/\d+)+)",
    )
    .unwrap()
});
static RE_INSTR_DIR_CANON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^Directive\s+\(?(CE|CEE|UE)\)?\s*(?:n\s*[°o]?\s*)?(\d{4}/\d+)(?:\s*/\s*(?:CE|CEE|UE))?\b",
    )
    .unwrap()
});
// « Directive dite retour n° 2008/115/CE » : l'interjection est dépliée pour
// que la standardisation numérotée s'applique.
static RE_INSTR_DITE_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^((?:Directive|Règlement|Reglement)\s+)dite?s?\s+\S+\s+").unwrap()
});
// Alias d'usage après une identité chiffrée (« … dit Rome III ») : coupé.
// « Loi dite Badinter » n'a QUE son alias pour identité, elle reste intacte.
static RE_INSTR_DIT_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*,?\s+dite?s?\s+\S.*$").unwrap());
// Numéro d'un protocole CESDH (« n° 7 », « protocole 12 »). Le « n » exige un
// début de mot, sinon il accroche la finale de « convention ».
static RE_PROTO_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\s)n\s*[°o]?\s*(\d{1,2})\b|^(?:premier\s+)?protocole\s+(?:additionnel\s+)?(\d{1,2})\b")
        .unwrap()
});
static RE_INSTR_MODIFIEE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:\s*,\s*|\s+)modifi[ée]e?s?$").unwrap());
static RE_INSTR_SUSVISE_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+(?:susvis[éè]|préci(?:t[éè]|tes?))e?s?$").unwrap());
// « Nouveau code de procédure civile » et sa variante de capture dégradée
// « Nouveau du code … » → le code en vigueur.
static RE_INSTR_NOUVEAU_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^nouveau\s+(?:du\s+)?code\b").unwrap());
static RE_INSTR_RGT_DIR_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:règlement|reglement|directive)\b").unwrap());
// Boilerplate institutionnel UE : le NUMÉRO est l'identité, on jette
// attribution, date et alias parenthétique. $1 = type + identité numérotée. Une
// date seule sans numéro ne matche pas.
static RE_INSTR_EU_BOILERPLATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^((?:Directive|Règlement|Reglement)\s+(?:\([A-Z]{2,3}\)\s*)?(?:n°?\s*)?\d{2,4}[/\-]\d+(?:[/\-][A-Za-z]{1,3})?)\s+(?:du|de\s+la|des?|,|\().*$",
    )
    .unwrap()
});
static RE_PLUIHM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bplu[ihm]*\b").unwrap());
// Instruments FR datés : on retire le token-numéro entre le type et
// « du <date> » → forme datée unique. JAMAIS les Directive/Règlement UE, dont le
// numéro EST l'identité.
static RE_INSTR_DROP_DATED_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^((?:Loi(?:\s+du\s+pays)?|Décret|Decret|Ordonnance|Arrêté|Arrete|Décision|Decision|Délibération|Deliberation)(?:\s+organique)?(?:\s+(?:municipal|préfectoral|prefectoral|ministériel|ministeriel|interministériel|interministeriel|communal))?)\s+(?:n\s*[°o]?\.?\s*)?\d[\w./\-]*\s+(du\s+\d)",
    )
    .unwrap()
});

const TRAILING_PUNCT: &[char] = &[',', ';', ':', '.', ' ', '\t'];

const MONTHS_FR: &[&str] = &[
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

// Jour zéro-paddé d'une date (« du 06 juillet 1989 ») : dé-paddé.
static RE_INSTR_DAY_ZERO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(du|des)\s+0(\d)\b").unwrap());
// Date tout-numérique « du 16/01/2018 » (mois 01-12), dépliée en littéral.
static RE_INSTR_NUM_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(du|des)\s+(\d{1,2})\s*[/.]\s*(\d{1,2})\s*[/.]\s*(\d{4})\b").unwrap()
});
// Tiret de numéro détaché par une espace (« n° 2018 -1021 »).
static RE_INSTR_NUM_SPACED_DASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(n°\s*\d{2,4})\s+-\s*(\d)").unwrap());
// Token NOR, graphie collée comprise ; parenthèses appariées absorbées.
static RE_INSTR_NOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(?\s*\bNOR\s*:?\s*[A-Z]{4}\d{7}[A-Z]\b\s*\)?").unwrap());

/// Strip article défini, title-case 1er mot, applique
/// l'arbre de canonicalisation figé (alias, snap, formes spéciales).
pub fn normalize_instrument(raw: &str) -> String {
    let mut raw = raw
        .trim()
        .trim_end_matches(TRAILING_PUNCT)
        .replace('\u{2019}', "'")
        .replace('\u{2013}', "-")
        // Indicateur ordinal º (U+00BA, « nº ») tapé pour le degré °.
        .replace('\u{ba}', "\u{b0}");
    raw = RE_INSTR_INTRA_NEWLINE
        .replace_all(&raw, "$1 $2")
        .into_owned();
    raw = raw.split('\n').next().unwrap_or(&raw).to_string();
    raw = collapse_adjacent_repeats(&raw);
    // Parenthèse fermante ORPHELINE en fin de token (« … du règlement) ») : on
    // retire les ')' de fin tant qu'ils sont NON appariés, une paire présente est
    // préservée.
    while raw.ends_with(')') && raw.matches(')').count() > raw.matches('(').count() {
        raw.pop();
        raw = raw.trim_end_matches(TRAILING_PUNCT).to_string();
    }
    raw = RE_INSTR_DAY_ZERO.replace_all(&raw, "$1 $2").into_owned();
    raw = RE_INSTR_NUM_DATE
        .replace_all(&raw, |c: &regex::Captures| {
            let month: usize = c[3].parse().unwrap_or(0);
            match MONTHS_FR.get(month.wrapping_sub(1)) {
                Some(m) => format!(
                    "{} {} {} {}",
                    &c[1],
                    &c[2].trim_start_matches('0'),
                    m,
                    &c[4]
                ),
                None => c[0].to_string(),
            }
        })
        .into_owned();
    raw = RE_INSTR_NUM_SPACED_DASH
        .replace_all(&raw, "$1-$2")
        .into_owned();
    // NOR embarqué : retiré quand une identité chiffrée survit. Mention à NOR
    // seul : gardé, c'est l'identité.
    if raw.contains("NOR") {
        let stripped: String = RE_INSTR_NOR
            .replace_all(&raw, " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if stripped.contains(|c: char| c.is_ascii_digit()) {
            raw = stripped;
        }
    }
    raw = RE_INSTR_ET_NUM.replace(&raw, "").into_owned();
    raw = RE_INSTR_GLUED_YEAR.replace(&raw, "$1").into_owned();
    raw = RE_INSTR_VERB_TAIL2.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_GLUED_VERB.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_SUBDIVISION.replace(&raw, "$1").into_owned();
    raw = RE_INSTRUMENT_PROSE_CUT.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_VERB_CUT.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_PROSE_CUT2.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_PROSE_CUT3.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_TEL_QUE.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_MODIFIE_DU.replace(&raw, " $1").into_owned();
    raw = RE_INSTR_MODIFIE_PAR.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_ENSEMBLE.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_DEVENU.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_ALORS_TAIL.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_VISE_TAIL.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_NOTAMMENT.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_SUSVISE_ET.replace(&raw, "$1").trim().to_string();
    raw = RE_INSTR_SUSVISE_DU.replace(&raw, "$1 du").into_owned();
    raw = RE_INSTR_DATED_TAIL.replace(&raw, "$1").trim().to_string();
    raw = RE_INSTR_NUMERO_SPACE
        .replace_all(&raw, "n° $1")
        .into_owned();
    if RE_INSTR_RGT_DIR_HEAD.is_match(&raw) {
        raw = RE_INSTR_UE_NUM_DASH
            .replace_all(&raw, "n° $1/$2")
            .into_owned();
        raw = RE_INSTR_UE_YEAR_DASH
            .replace_all(&raw, "$1/$2/$3")
            .into_owned();
        raw = RE_INSTR_UE_NOSLASH.replace_all(&raw, "$1/$2").into_owned();
        raw = RE_INSTR_DITE_HEAD.replace(&raw, "$1").into_owned();
        raw = RE_INSTR_EU_NUM_ZEROS.replace_all(&raw, "$1").into_owned();
        raw = RE_INSTR_RGT_CANON
            .replace(&raw, |c: &regex::Captures| {
                format!("Règlement ({}) n° {}", c[1].to_uppercase(), &c[2])
            })
            .into_owned();
        raw = RE_INSTR_DIR_CANON
            .replace(&raw, |c: &regex::Captures| {
                format!("Directive {}/{}", &c[2], c[1].to_uppercase())
            })
            .into_owned();
    }
    raw = RE_INSTR_UE_SPACE.replace_all(&raw, "$1/$2").into_owned();
    raw = RE_INSTR_DIRECTIVE_NUM.replace(&raw, "$1 $2").into_owned();
    if let Some(m) = RE_INSTR_DIT_TAIL.find(&raw) {
        if raw[..m.start()].contains(|c: char| c.is_ascii_digit()) {
            raw = raw[..m.start()].trim_end().to_string();
        }
    }
    raw = RE_INSTR_MODIFIEE.replace(&raw, "").trim().to_string();
    raw = RE_INSTR_TRAILING_CONNECTOR
        .replace(&raw, "")
        .trim()
        .to_string();
    raw = RE_INSTR_EU_BOILERPLATE
        .replace(&raw, "$1")
        .trim()
        .to_string();
    if raw.contains(|c: char| c.is_ascii_digit()) {
        raw = RE_INSTR_SUSVISE_TAIL.replace(&raw, "").trim().to_string();
    }
    raw = raw.trim_end_matches(TRAILING_PUNCT).to_string();
    let lower = raw.to_lowercase();
    for prefix in ["l'", "les ", "la ", "le "] {
        if lower.starts_with(prefix) {
            raw = raw[prefix.len()..].to_string();
            break;
        }
    }
    // Après le strip d'article, « la loi n° 65-557 du … » doit prendre la même
    // forme datée que « Loi n° 65-557 du … » — la regex est ancrée ^, sinon la
    // normalisation n'est pas idempotente.
    raw = RE_INSTR_DROP_DATED_NUM.replace(&raw, "$1 $2").into_owned();
    raw = RE_INSTR_NOUVEAU_CODE.replace(&raw, "code").into_owned();

    let lowered = raw.to_lowercase();
    raw = canonicalize_instrument(&raw, &lowered);

    // Un titre canonique reconnu est déjà bien cassé : la recasse interne
    // re-minusculerait ses majuscules internes (« Nouvelle-Calédonie »).
    if CANON_TITLE_SET.contains(&raw) {
        return raw;
    }

    // Source tout en capitales : chaque mot ressemble à un acronyme et traverse la
    // recasse intact. On repasse en minuscule ; les vrais acronymes sont restaurés
    // par `instrument_internal_case`. Un instrument réduit à un acronyme connu reste
    // tel quel.
    if !raw.is_empty()
        && !raw.contains(char::is_lowercase)
        && !INSTRUMENT_ACRONYMS.contains(raw.as_str())
    {
        raw = raw.to_lowercase();
    }

    if !raw.is_empty() {
        raw = uppercase_first(&raw);
        raw = instrument_internal_case(&raw);
    }
    raw
}

// Anaphore : mot de famille nu + marqueur de renvoi (« précité », « susvisé »,
// « du même … ») dont l'antécédent n'a pas été résolu. Le marqueur est capté
// n'importe où après la famille, le reste est de la prose.
static RE_ANAPHORA_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:préci(?:t[ée]|tes?)e?s?|susvis[ée]e?s?|du\s+même\b|de\s+ce(?:t|tte)?\s+même\b|de\s+la\s+même\b|de\s+ce(?:t|tte)?\b|dudit\b|de\s+ladite\b)",
    )
    .unwrap()
});

// Subdivision numérotée nue (« Livre VIII », « Titre III ») : subdivision
// orpheline, jamais un instrument citable. La tête numérotée est EXIGÉE pour ne
// pas jeter le « Livre des procédures fiscales ».
static RE_UNRESOLVABLE_SUBDIVISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:livre|titre|chapitre|section|annexe)\s+(?:\d|[ivxlcdm]+(?:er|ère|e|ème)?\b|premi(?:er|ère)\b)")
        .unwrap()
});
// Famille de tête + queue de prose sans identité — ni date/numéro, ni intitulé
// officiel, ni qualificatif de type. Cible la sur-capture où « de l'/de la/à sa »
// introduisait un complément de prose : « Instruction de la demande ».
static RE_UNRESOLVABLE_FAMILY_PROSE: LazyLock<Regex> = LazyLock::new(|| {
    // L'exclusion des intitulés officiels se fait en Rust : le crate `regex` n'a
    // pas de lookahead.
    Regex::new(r"(?i)^(?:instruction|circulaire|note)\s+(?:de\s+(?:la|sa|son|ses|leur|leurs)\s+|de\s+l'|du\s+|des\s+|[àa]\s+|et\s+|sur\s+)").unwrap()
});
/// Marqueurs d'intitulé officiel : leur présence signale une identité résoluble.
const OFFICIAL_TITLE_MARKERS: &[&str] = &[
    "relati",
    "portant",
    "fixant",
    "pris pour",
    "générale",
    "generale",
    "interministériel",
    "interministeriel",
    "ministériel",
    "ministeriel",
    "européen",
    "europeen",
    "international",
];

/// Libellé qui ne dénote aucun texte identifiable : mot de famille nu
/// (« Décret », « Loi »…), anaphore « <famille> précité/susvisé/du même… » dont
/// l'antécédent n'a pas été résolu, ou « convention collective » sans domaine
/// distinctif (cf. [`is_generic_convention_collective`]).
pub fn is_unresolvable_instrument(instrument: &str) -> bool {
    let low = instrument.trim().to_lowercase();
    // Subdivision orpheline (« Livre VIII », « Titre III du même code »…).
    if RE_UNRESOLVABLE_SUBDIVISION.is_match(&low) {
        return true;
    }
    // « Convention collective » nue, anaphorique ou générique. Une CCN qualifiée
    // ou datée est gardée.
    if is_generic_convention_collective(&low) {
        return true;
    }
    // Résidu « Nouveau … » non réduit en code : mangle local type « Nouveau
    // règlement du PLU ».
    if low.starts_with("nouveau ") || low == "nouveau" {
        return true;
    }
    // Famille + prose sans identité, hors textes datés ou numérotés (garde digit)
    // et hors intitulé officiel.
    if !low.contains(|c: char| c.is_ascii_digit())
        && RE_UNRESOLVABLE_FAMILY_PROSE.is_match(&low)
        && !OFFICIAL_TITLE_MARKERS.iter().any(|m| low.contains(m))
    {
        return true;
    }
    // Anaphore : famille nue + marqueur de renvoi. Un chiffre quelque part =
    // identité résoluble, on ne jette pas.
    if !low.contains(|c: char| c.is_ascii_digit()) {
        if let Some(m) = RE_ANAPHORA_MARKER.find(&low) {
            let head = low[..m.start()].trim();
            if is_bare_family(head) {
                return true;
            }
        }
    }
    let core = low
        .strip_suffix(" précité")
        .or_else(|| low.strip_suffix(" précitée"))
        .or_else(|| low.strip_suffix(" précités"))
        .or_else(|| low.strip_suffix(" précitées"))
        .or_else(|| low.strip_suffix(" susvisé"))
        .or_else(|| low.strip_suffix(" susvisée"))
        .or_else(|| low.strip_suffix(" susvisés"))
        .or_else(|| low.strip_suffix(" susvisées"))
        .unwrap_or(low.as_str());
    is_bare_family(core)
}

/// « Convention collective » sans domaine distinctif. Couvre la forme nue, les
/// habillages non distinctifs (« nationale », « de travail »), l'anaphore et la
/// sur-capture de prose verbale. Une CCN qualifiée garde un reste distinctif : on
/// l'émet.
fn is_generic_convention_collective(low: &str) -> bool {
    let Some(rest) = low.strip_prefix("convention collective") else {
        return false;
    };
    // « nationale » / « de travail » qualifient le *type* de convention, pas
    // *laquelle* : on les pèle pour atteindre l'éventuel domaine distinctif.
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("nationale").unwrap_or(rest).trim_start();
    let rest = rest.strip_prefix("de travail").unwrap_or(rest).trim_start();
    if rest.is_empty() {
        return true;
    }
    // Reste non vide : générique seulement s'il débute par un marqueur anaphorique
    // ou un verbe/adjectif de prose. Tout autre reste = domaine.
    const GENERIC_HEADS: &[&str] = &[
        "précité",
        "precite",
        "susvisé",
        "susvise",
        "applicable",
        "précise",
        "precise",
        "prévoit",
        "prevoit",
        "dispose",
        "stipule",
        "prévoyait",
    ];
    GENERIC_HEADS.iter().any(|h| rest.starts_with(h))
}

/// Mot(s) de famille nu(s), sans identité (numéro/date/titre).
fn is_bare_family(core: &str) -> bool {
    matches!(
        core,
        "loi"
            | "décret"
            | "decret"
            | "arrêté"
            | "arrete"
            | "règlement"
            | "reglement"
            | "ordonnance"
            | "accord"
            | "convention"
            | "charte"
            | "directive"
            | "protocole"
            | "statut"
            | "circulaire"
            | "instruction"
            | "livre"
            | "code"
    )
}

/// Arbre `if/elif` de canonicalisation des formes spéciales d'instrument.
fn canonicalize_instrument(raw: &str, lowered: &str) -> String {
    const CESDH: &str =
        "Convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales";
    // Abréviations d'usage nues de la CESDH : aucune autre identité que le sigle,
    // on les rabat sur le titre long.
    if lowered == "cedh" || lowered == "cesdh" {
        return CESDH.to_string();
    }
    if lowered.starts_with("convention européenne de sauvegarde")
        || (lowered.starts_with("convention européenne")
            && (lowered.contains("droits de l'homme")
                || lowered.contains("droits de l\u{2019}homme")
                || lowered.contains("libertés fondamentales")
                || lowered.contains("libertés fondamentale")
                || lowered.contains("libertés individuelles")
                || lowered.contains("liberté fondamentales")
                || lowered.contains("droits et libertés")
                || lowered.contains("droits et des libertés")))
    {
        return CESDH.to_string();
    }
    if lowered.starts_with("loi") && lowered.contains("10 juillet 1991") {
        return "Loi du 10 juillet 1991".to_string();
    }
    // CIDE : « des/relative aux/sur les » droits de l'enfant, ou lieu de signature
    // (« Convention de New-York »). Toutes ces formes convergent vers le titre long.
    if lowered.starts_with("convention internationale des droits de l'enfant")
        || lowered.starts_with("convention internationale relative aux droits de l'enfant")
        || lowered.starts_with("convention internationale sur les droits de l'enfant")
        || lowered.starts_with("convention de new-york")
        || lowered.starts_with("convention de new york")
    {
        return "Convention internationale relative aux droits de l'enfant".to_string();
    }
    if lowered.starts_with("convention de genève") {
        return "Convention de Genève".to_string();
    }
    if lowered
        .starts_with("convention de sauvegarde des droits de l'homme et des libertés fondamentales")
        || lowered.starts_with(
            "convention de sauvegarde des droits de l\u{2019}homme et des libertés fondamentales",
        )
        || lowered.starts_with(
            "convention européenne de sauvegarde des droits humains et des libertés fondamentales",
        )
        || lowered.starts_with(
            "convention de sauvegarde des libertés fondamentales et des droits de l'homme",
        )
        || lowered.starts_with(
            "convention de sauvegarde des libertés fondamentales et des droits de l\u{2019}homme",
        )
    {
        return CESDH.to_string();
    }
    // Protocoles CESDH : des centaines de variantes pour une poignée de
    // protocoles, ramenées à « Protocole n° N à la convention … ». Le protocole
    // « additionnel » sans numéro EST le n° 1.
    if (lowered.starts_with("protocole") || lowered.starts_with("premier protocole"))
        && ((lowered.contains("sauvegarde") && lowered.contains("droits de l'homme"))
            || lowered.contains("convention européenne des droits de l'homme"))
    {
        let n = RE_PROTO_NUM
            .captures(lowered)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map_or_else(|| "1".to_string(), |m| m.as_str().to_string());
        return format!(
            "Protocole n° {n} à la convention européenne de sauvegarde des droits de l'homme et des libertés fondamentales"
        );
    }
    if lowered.starts_with("accord franco-algérien") {
        return "Accord franco-algérien du 27 décembre 1968".to_string();
    }
    if lowered.starts_with("accord franco-marocain") {
        return "Accord franco-marocain du 9 octobre 1987".to_string();
    }
    if lowered.starts_with("accord franco-tunisien") {
        return "Accord franco-tunisien du 17 mars 1988".to_string();
    }
    // CESEDA : corps parfois non accentué, suffixe variable. Le préfixe plié
    // jusqu'à « etrangers » identifie le code sans ambiguïté.
    {
        let folded = fold(lowered);
        if folded.starts_with("code de l'entree et du sejour des etrangers")
            || folded.starts_with("code de l'entree et de sejour des etrangers")
            || folded.starts_with("code l'entree et du sejour des etrangers")
        {
            return "Code de l'entrée et du séjour des étrangers et du droit d'asile".to_string();
        }
    }
    if lowered.starts_with("code général des impôts et le livre des procédures fiscales") {
        return CGI_LPF.to_string();
    }
    if lowered.starts_with("règlement")
        && (lowered.contains("604/2013")
            || lowered.contains("604-2013")
            || lowered.contains("du 26 juin 2013")
            || lowered.contains("européen du 26 juin 2013")
            || lowered.contains("604/2003"))
    {
        return "Règlement (UE) n° 604/2013".to_string();
    }
    if lowered.starts_with("règlement")
        && (lowered.contains("603/2013") || lowered.contains("603-2013"))
    {
        return "Règlement (UE) n° 603/2013".to_string();
    }
    // Règlements UE « civils » cités tantôt par leur numéro JO sous graphie
    // variable, tantôt par leur surnom doctrinal (« Bruxelles I bis ») : le numéro EST
    // l'identité. Ordre impératif — la variante spécifique AVANT la générique dont
    // elle contient le surnom comme préfixe, la première qui matche `return`.
    if lowered.starts_with("règlement") || lowered.starts_with("reglement") {
        if lowered.contains("1215/2012") || lowered.contains("1215-2012") {
            return "Règlement (UE) n° 1215/2012".to_string();
        }
        if lowered.contains("2201/2003") || lowered.contains("2201-2003") {
            return "Règlement (CE) n° 2201/2003".to_string();
        }
        if lowered.contains("44/2001") || lowered.contains("44-2001") {
            return "Règlement (CE) n° 44/2001".to_string();
        }
        if lowered.contains("864/2007") || lowered.contains("864-2007") {
            return "Règlement (CE) n° 864/2007".to_string();
        }
        if lowered.contains("593/2008") || lowered.contains("593-2008") {
            return "Règlement (CE) n° 593/2008".to_string();
        }
        if lowered.contains("1259/2010") || lowered.contains("1259-2010") {
            return "Règlement (UE) n° 1259/2010".to_string();
        }
        if lowered.contains("650/2012") || lowered.contains("650-2012") {
            return "Règlement (UE) n° 650/2012".to_string();
        }
        // Surnoms nus (pas de numéro). « bis » / « ii bis » avant « i » : préfixe.
        if lowered.contains("bruxelles ii bis") || lowered.contains("bruxelles 2 bis") {
            return "Règlement (CE) n° 2201/2003".to_string();
        }
        if lowered.contains("bruxelles i bis") || lowered.contains("bruxelles 1 bis") {
            return "Règlement (UE) n° 1215/2012".to_string();
        }
        if lowered.contains("bruxelles i") || lowered.contains("bruxelles 1") {
            return "Règlement (CE) n° 44/2001".to_string();
        }
        if lowered.contains("rome iii") || lowered.contains("rome 3") {
            return "Règlement (UE) n° 1259/2010".to_string();
        }
        if lowered.contains("rome ii") || lowered.contains("rome 2") {
            return "Règlement (CE) n° 864/2007".to_string();
        }
        if lowered.contains("rome i") || lowered.contains("rome 1") {
            return "Règlement (CE) n° 593/2008".to_string();
        }
        // Identité par DATE D'ADOPTION, faute de numéro reconnu : la date d'adoption
        // identifie le règlement à elle seule.
        if lowered.contains("22 décembre 2000") {
            return "Règlement (CE) n° 44/2001".to_string();
        }
        if lowered.contains("12 décembre 2012") {
            return "Règlement (UE) n° 1215/2012".to_string();
        }
        if lowered.contains("17 juin 2008") {
            return "Règlement (CE) n° 593/2008".to_string();
        }
        if lowered.contains("11 juillet 2007") {
            return "Règlement (CE) n° 864/2007".to_string();
        }
        if lowered.contains("27 novembre 2003") {
            return "Règlement (CE) n° 2201/2003".to_string();
        }
        if lowered.contains("4 juillet 2012") {
            return "Règlement (UE) n° 650/2012".to_string();
        }
    }
    // Codes allemands cités par leur nom natif (BGB, HGB, StGB) ou leur descriptif
    // français, rabattus sur la forme française d'usage. EGBGB, acte introductif du
    // BGB, est un instrument DISTINCT : sa graphie déclinée ne contient pas
    // « bürgerliches gesetzbuch », et le test sigle est borné au PREMIER mot.
    let first_tok = lowered
        .split_whitespace()
        .next()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()));
    if lowered.contains("bürgerliches gesetzbuch")
        || lowered.contains("burgerliches gesetzbuch")
        || first_tok == Some("bgb")
    {
        return "Code civil allemand".to_string();
    }
    if lowered.contains("handelsgesetzbuch") || first_tok == Some("hgb") {
        return "Code de commerce allemand".to_string();
    }
    if lowered.contains("strafgesetzbuch") || first_tok == Some("stgb") {
        return "Code pénal allemand".to_string();
    }
    // Code des obligations suisse : ordre des mots variable et titre officiel
    // verbeux ramenés à une forme d'usage unique. Gaté sur « obligations » pour ne pas
    // happer le code civil suisse seul. « Code des obligations » nu le désigne, la
    // France n'en a aucun. « …et des contrats » est EXCLU, c'est le COC tunisien ou
    // marocain.
    if lowered.contains("obligations")
        && !lowered.contains("et des contrats")
        && (lowered.contains("suisse")
            || lowered.starts_with("code des obligations")
            || (lowered.contains("loi fédérale") && lowered.contains("code des obligations")))
    {
        return "Code des obligations suisse".to_string();
    }
    // CVIM / Convention de Vienne du 11 avril 1980 sur la vente internationale de
    // marchandises : titres très variables. Ancré sur « vente internationale » +
    // « marchandises » ou le sigle, pour ne pas happer une autre convention de Vienne.
    if (lowered.contains("vente internationale") && lowered.contains("marchandises"))
        || lowered.contains("(cvim)")
        || lowered.contains(" cvim")
        || lowered.contains("cvim)")
        // La date « 11 avril 1980 » identifie à elle seule la CVIM ; les autres
        // conventions de Vienne portent d'autres dates.
        || (lowered.contains("vienne") && lowered.contains("11 avril 1980"))
    {
        return "Convention de Vienne du 11 avril 1980 sur les contrats de vente internationale de marchandises".to_string();
    }
    // Convention de Rome sur la loi applicable aux obligations contractuelles
    // (19 juin 1980). Gaté pour ne pas happer le Statut de Rome (CPI).
    if lowered.contains("rome")
        && !lowered.contains("statut")
        && (lowered.contains("obligations contractuelles")
            || lowered.contains("relations contractuelles")
            || lowered.contains("19 juin 1980")
            || lowered.contains("18 juin 1980")
            // Forme nue (anaphore). Le Statut de Rome est déjà exclu.
            || lowered.contains("convention de rome"))
    {
        return "Convention de Rome du 19 juin 1980 sur la loi applicable aux obligations contractuelles".to_string();
    }
    // Conventions de La Haye et de Bruxelles : la date identifie l'instrument.
    // Uniquement les dates SANS ambiguïté — 2 octobre 1973, 14 mars 1978 et
    // 5 octobre 1961 portent chacune deux conventions distinctes, donc exclues.
    if lowered.contains("haye") {
        if lowered.contains("15 juin 1955") {
            return "Convention de la haye du 15 juin 1955 sur la loi applicable aux ventes à caractère international d'objets mobiliers corporels".to_string();
        }
        if lowered.contains("15 avril 1958") {
            return "Convention de la haye du 15 avril 1958 concernant la reconnaissance et l'exécution des décisions en matière d'obligations alimentaires envers les enfants".to_string();
        }
        if lowered.contains("4 mai 1971") {
            return "Convention de la haye du 4 mai 1971 sur la loi applicable en matière d'accidents de la circulation routière".to_string();
        }
        if lowered.contains("25 octobre 1980") {
            return "Convention de la haye du 25 octobre 1980 sur les aspects civils de l'enlèvement international d'enfants".to_string();
        }
        if lowered.contains("29 mai 1993") {
            return "Convention de la haye du 29 mai 1993 sur la protection des enfants et la coopération en matière d'adoption internationale".to_string();
        }
    }
    if lowered.contains("bruxelles") && lowered.contains("27 septembre 1968") {
        return "Convention de Bruxelles du 27 septembre 1968 concernant la compétence judiciaire et l'exécution des décisions en matière civile et commerciale".to_string();
    }
    // Convention de Lugano (compétence, reconnaissance, exécution civile et
    // commerciale) ; version en vigueur du 30 octobre 2007.
    if lowered.contains("lugano") {
        return "Convention de Lugano du 30 octobre 2007".to_string();
    }
    if lowered == "convention européenne des droits de l'homme"
        || lowered == "convention européenne des droits de l\u{2019}homme"
    {
        return CESDH.to_string();
    }
    if lowered == "code de la construction" {
        return "Code de la construction et de l'habitation".to_string();
    }
    if lowered.starts_with("règlement")
        && (lowered.contains("plan local d'urbanisme")
            || lowered.contains("plan local d\u{2019}urbanisme")
            || RE_PLUIHM.is_match(lowered))
    {
        return "Règlement du plan local d'urbanisme".to_string();
    }
    if lowered == "code des relations" {
        return "Code des relations entre le public et l'administration".to_string();
    }
    if let Some((_, alias)) = LEGIFRANCE_CODE_ALIASES.iter().find(|(k, _)| *k == lowered) {
        return (*alias).to_string();
    }
    // Table d'alias auditée : variantes qu'aucune règle générique ne recolle
    // sûrement. Pliée pour tolérer casse et accents ; consultée AVANT le snap, elle
    // fait autorité sur un snap-squelette ambigu (« Code des procédures civile » →
    // CPCE et non CPC).
    if let Some(code) = INSTRUMENT_ALIASES.get(&fold(raw)) {
        return code.clone();
    }
    if let Some(snapped) = snap_code_name(raw) {
        return snapped;
    }
    raw.to_string()
}

/// Ensemble des titres de codes Légifrance canoniques (casse officielle).
static CANON_TITLE_SET: LazyLock<HashSet<String>> = LazyLock::new(|| {
    legifrance_codes()
        .codes
        .into_iter()
        .map(|c| c.titre)
        .collect()
});

/// Table d'alias auditée `fold(variante) → titre canonique`.
static INSTRUMENT_ALIASES: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    instrument_aliases()
        .aliases
        .into_iter()
        .map(|(variant, code)| (fold(&variant), code))
        .collect()
});

static INSTRUMENT_PROPER_NOUNS_LOWER: LazyLock<HashSet<String>> = LazyLock::new(|| {
    [
        "Union",
        "Communauté",
        "Commission",
        "Conseil",
        "Parlement",
        "Assemblée",
        "République",
        "État",
        "Etat",
        "États",
        "Etats",
        "France",
        "Genève",
        "Strasbourg",
        "Lyon",
        "Paris",
        "Bruxelles",
        "Luxembourg",
        "Nuremberg",
        "Marrakech",
        "Vienne",
        "Londres",
        "Rome",
        "Madrid",
        "Berlin",
        "Versailles",
        "York",
        "New",
        "Polynésie",
        "Calédonie",
        "Algérie",
        "Schengen",
        "Dublin",
        "Oviedo",
        "Lugano",
    ]
    .iter()
    .map(|n| n.to_lowercase())
    .collect()
});

static INSTRUMENT_ACRONYMS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "PLU", "PLUI", "PLUH", "PLUIH", "PLUM", "POS", "ZAC", "ZAD", "ZUS", "PPR", "PPRI", "PPRN",
        "PPRIF", "PPRT", "UA", "UB", "UC", "UD", "UE", "UF", "UG", "UH", "UI", "UJ", "UAB", "UAC",
        "UAD", "UAH", "ELAN", "ALUR", "SRU", "OQTF", "IRTF", "ITF", "CESEDA", "AME", "CMU", "ANAH",
        "UNEDIC", "ASSEDIC", "CNOM", "CIMADE", "CROUS", "CADA", "URSSAF", "MSA", "CPAM", "CAF",
        "EPCI", "EPA", "OPHLM", "SIVU", "SIVOM", "SDIS", "CGT", "CFDT", "CFTC", "CGC", "FO",
        "UNSA", "FSU", "EURATOM", "BCE", "ONU", "OMC", "OCDE", "OTAN", "COVID", "RGPD", "GAEC",
        "ICPE", "TVA", "CSG", "CRDS",
    ]
    .into_iter()
    .collect()
});

// Parens opaques | suite de lettres | suite de non-lettres.
static RE_INSTR_TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\([^)]*\)|[A-Za-z\x{c0}-\x{d6}\x{d8}-\x{f6}\x{f8}-\x{ff}]+|[^A-Za-z\x{c0}-\x{d6}\x{d8}-\x{f6}\x{f8}-\x{ff}]+")
        .unwrap()
});
static RE_ROMAN_NUMERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^M{0,3}(?:CM|CD|D?C{0,3})(?:XC|XL|L?X{0,3})(?:IX|IV|V?I{0,3})(?:er|re|e|s)?$")
        .unwrap()
});

/// 1er mot intact, parens/acronymes préservés,
/// noms propres recasés, reste en minuscule.
pub(crate) fn instrument_internal_case(text: &str) -> String {
    let mut out = String::new();
    let mut seen_first_word = false;
    for tok in RE_INSTR_TOKENS.find_iter(text).map(|m| m.as_str()) {
        if tok.starts_with('(') && tok.ends_with(')') {
            out.push_str(tok);
            continue;
        }
        if !tok.chars().any(|c| c.is_alphabetic()) {
            out.push_str(tok);
            continue;
        }
        if !seen_first_word {
            out.push_str(tok);
            seen_first_word = true;
            continue;
        }
        let upper = tok.to_uppercase();
        if INSTRUMENT_ACRONYMS.contains(upper.as_str()) {
            out.push_str(&upper);
            continue;
        }
        if tok.chars().count() >= 2 && tok == upper && tok != tok.to_lowercase() {
            out.push_str(tok);
            continue;
        }
        if tok.chars().next().is_some_and(|c| "IVXLCDM".contains(c))
            && RE_ROMAN_NUMERAL.is_match(tok)
        {
            out.push_str(&uppercase_first(tok));
            continue;
        }
        if INSTRUMENT_PROPER_NOUNS_LOWER.contains(&tok.to_lowercase()) {
            out.push_str(&capitalize_only(tok));
            continue;
        }
        out.push_str(&tok.to_lowercase());
    }
    out
}
#[cfg(test)]
#[path = "instruments_tests.rs"]
mod tests;
