//! Parse des « compteurs » d'applicabilité outre-mer : les articles qui
//! étendent (ou écartent) des articles du même code aux collectivités régies
//! par la spécialité législative (NC, PF, WF, TAAF).
//!
//! Trois morphologies :
//! - phrase simple : « L'article R. 425-2 est applicable en Polynésie
//!   française dans sa rédaction résultant du décret n° 2017-733 du
//!   4 mai 2017. » ;
//! - prose multi-phrases avec plages : « Sont applicables à la Polynésie
//!   française les articles L. 218-1 à L. 218-72, à l'exception du II de
//!   l'article L. 218-44, … » ;
//! - énumération sous en-tête : « …sont applicables de plein droit en
//!   Polynésie française : / 1° Les articles R. 561-1 à R. 561-38-9 ; … ».
//!
//! La morphologie « tableau ci-après » a son parser dédié
//! ([`parse_compteur_table`]) : le rendu LEGI aplatit le tableau en une
//! cellule par ligne — ligne de références (colonne de gauche), puis ligne
//! de rédaction optionnelle (colonne de droite), divisions structurelles
//! intercalées (« Au titre II »), ancre globale d'en-tête « sauf mention
//! contraire ». Le tableau se ferme sur une section romaine (« II. - »), un
//! item d'adaptation (« 1° … ») ou un NOTA. [`compteur_has_table`] signale
//! la morphologie à l'appelant, qui lance les deux parsers.
//!
//! Le parser rend les cibles en clé [`crate::article_key::article_key`],
//! les plages non dépliées (le dépliage exige la liste des articles du code,
//! côté I/O), et l'ancre de rédaction en date ISO. Les exceptions
//! infra-article (« du II de l'article X », « de son troisième alinéa »)
//! restent de la prose — le modèle pose la portée au grain de l'article,
//! l'exception se lit dans `source`. Seules les exclusions d'article ENTIER
//! (« à l'exception des articles X et Y ») sont soustraites.

use std::sync::LazyLock;

use regex::Regex;

use crate::article_key::article_key;

/// Cible d'un événement : référence simple ou plage bornée (clés publiques).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetRef {
    Single(String),
    Range(String, String),
}

/// Exception attachée à un événement : la prose verbatim, plus les articles
/// ENTIERS qu'elle exclut (vide pour une exception infra-article).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterException {
    pub prose: String,
    pub full_articles: Vec<String>,
}

/// Un événement d'applicabilité énoncé par le compteur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterEvent {
    pub targets: Vec<TargetRef>,
    /// Portées nommées dans la phrase (`geo:PF`…) ; vide = hériter de la
    /// portée dominante du compteur (phrase « L'article X est applicable
    /// dans sa rédaction… » sans nommer la collectivité).
    pub scopes: Vec<String>,
    /// `true` = « ne sont pas applicables » (mode `ecarte`).
    pub negated: bool,
    /// Date ISO du texte modificateur de l'ancre « dans sa rédaction
    /// résultant de la loi n° X du D » ; `None` = rédaction en vigueur à la
    /// date du compteur (gel de la spécialité).
    pub redaction_date: Option<String>,
    pub exceptions: Vec<CounterException>,
    /// Clauses « sous réserve … », verbatim (prose de `source`).
    pub reserves: Vec<String>,
    /// `true` si les cibles appartiennent à un AUTRE texte (« les articles
    /// L. 121-1 à L. 121-3 du code de la consommation sont applicables… ») :
    /// la résolution contre le texte porteur du compteur serait fausse —
    /// l'appelant saute et compte.
    pub foreign_owner: bool,
}

/// Collectivités de la spécialité législative reconnues, graphies → uid.
const SCOPE_NAMES: &[(&str, &str)] = &[
    ("polynésie française", "geo:PF"),
    ("nouvelle-calédonie", "geo:NC"),
    ("nouvelle calédonie", "geo:NC"),
    ("wallis-et-futuna", "geo:WF"),
    ("wallis et futuna", "geo:WF"),
    ("terres australes et antarctiques françaises", "geo:TF"),
];

/// Référence d'article préfixée (`L. 218-1`, `R561-39`, `LO 6213-1`,
/// `L. 218-14 bis`) — le compteur cite toujours en préfixé dans les codes à
/// parties. Groupes : 1 = préfixe, 2 = numéro, 3 = suffixe latin optionnel.
static RE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(LO|LP|L|R|D|A)\.?\s*([1-9][0-9]*(?:-[0-9]+)*)(?:\s+(bis|ter|quater|quinquies))?\b",
    )
    .unwrap()
});

/// Pivot d'applicabilité : « sont/est applicable(s) », négation en option.
static RE_PIVOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:ne\s+|n')?(?:sont|est)\s+(?:pas\s+)?applicables?\b").unwrap()
});

/// Pivot négatif — « ne sont pas applicables », « n'est pas applicable ».
fn is_negated(line: &str) -> bool {
    line.to_lowercase().contains("pas applicable")
}

/// Ancre de rédaction : la PREMIÈRE date après « rédaction résultant de/du »
/// (les intitulés de lois qui suivent portent d'autres dates — non-greedy).
static RE_REDACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)rédactions?\s+(?:résultant|issue)s?\s+(?:de|du)[^;]*?n°\s*[0-9]{2,4}-[0-9]+\s+du\s+(1er|[0-9]{1,2})\s+(janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre)\s+([0-9]{4})",
    )
    .unwrap()
});

/// Clause d'exception : « à l'exception de … » (groupe 1) jusqu'au prochain
/// `;`, la reprise d'énumération (`, [et] les/l'article(s)`), une réserve,
/// ou la fin de phrase (groupe 2 = terminateur, restitué au remplacement —
/// la crate `regex` n'a pas de lookahead).
static RE_EXCEPTION: LazyLock<Regex> = LazyLock::new(|| {
    // Fin de phrase = point + espace + MAJUSCULE : le « L. » d'une référence
    // (point + espace + chiffre) n'est pas une fin de phrase.
    Regex::new(r"(?i)(à l'exception\s+(?:de la|de l'|des|du|de)\s+[^;]*?)(,\s+(?:et\s+)?les\s+articles|,\s+(?:et\s+)?l'article|,\s+sous\s+réserve|;|\.\s+(?-i:[A-ZÀÂÉÈÊÎÔÙÛÇ«])|$)").unwrap()
});

/// Clause de réserve : « sous réserve … » (groupe 1) jusqu'au `;` ou fin de
/// phrase (groupe 2, même règle de majuscule que l'exception).
static RE_RESERVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(sous réserve\s+[^;]*?)(;|\.\s+(?-i:[A-ZÀÂÉÈÊÎÔÙÛÇ«])|$)").unwrap()
});

/// En-tête d'énumération : pivot + `:` en fin de ligne.
static RE_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)applicables?\b[^:;]*:\s*$").unwrap());

/// Item d'énumération (`1° …`, `2° bis …`, `- …`).
static RE_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[0-9]+°(?:\s+(?:bis|ter|quater))?|-)\s+").unwrap());

/// Référence immédiatement suivie d'un génitif de propriétaire (« L. 121-3
/// du code de la consommation ») : les cibles vivent dans un autre texte.
/// « de la présente loi » ne matche pas (le mot suivant n'est pas
/// code/loi/ordonnance/décret).
static RE_FOREIGN_OWNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:LO|LP|L|R|D|A)\.?\s*[0-9][0-9-]*(?:\s+(?:bis|ter|quater|quinquies))?\s+(?:du|de la|de l')\s*(?:code|loi|ordonnance|décret)\b",
    )
    .unwrap()
});

fn month_num(m: &str) -> u32 {
    match m {
        "janvier" => 1,
        "février" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "août" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        _ => 12,
    }
}

/// `true` si le compteur renvoie à un tableau (morphologie non parsée ici).
pub fn compteur_has_table(texte: &str) -> bool {
    let t = texte.to_lowercase();
    t.contains("tableau ci-après") || t.contains("tableau ci-dessous") || t.contains("<table")
}

/// Portées nommées dans un fragment, dans l'ordre du texte. Les trois
/// départements du droit local (Bas-Rhin, Haut-Rhin, Moselle) valent
/// `geo:FR-ALS` dès qu'un département rhénan est nommé — la Moselle seule
/// est ambiguë (le département tout court) et ne suffit pas.
fn scopes_in(fragment: &str) -> Vec<String> {
    let lower = fragment.to_lowercase();
    let mut out = Vec::new();
    for (name, uid) in SCOPE_NAMES {
        if lower.contains(name) && !out.iter().any(|u| u == uid) {
            out.push((*uid).to_string());
        }
    }
    if (lower.contains("bas-rhin") || lower.contains("haut-rhin"))
        && !out.iter().any(|u| u == "geo:FR-ALS")
    {
        out.push("geo:FR-ALS".to_string());
    }
    out
}

/// Grain d'une clause auto-référente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfGrain {
    /// « La présente loi / le présent arrêté / décret / code … »
    Text,
    /// « Le présent article … »
    Article,
    /// « Les dispositions du présent chapitre / de la présente section … »
    Division,
}

/// Clause auto-référente de portée : « Le présent arrêté n'est pas
/// applicable dans les départements du Bas-Rhin… » →
/// `(grain, portées, négation)`. Première ligne porteuse gagnante.
pub fn self_scope(texte: &str) -> Option<(SelfGrain, Vec<String>, bool)> {
    static RE_SELF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(?:les dispositions?\s+(?:du|de la|de l')\s+)?(présente?s?)\s+(loi|arrêté|décret|ordonnance|code|article|chapitre|titre|livre|section|sous-section)\b",
        )
        .unwrap()
    });
    for line in texte.lines() {
        if !RE_PIVOT.is_match(line) {
            continue;
        }
        let Some(c) = RE_SELF.captures(line) else {
            continue;
        };
        let scopes = scopes_in(line);
        if scopes.is_empty() {
            continue;
        }
        let grain = match c[2].to_lowercase().as_str() {
            "article" => SelfGrain::Article,
            "loi" | "arrêté" | "décret" | "ordonnance" | "code" => SelfGrain::Text,
            _ => SelfGrain::Division,
        };
        return Some((grain, scopes, is_negated(line)));
    }
    None
}

/// Clé publique d'une capture [`RE_REF`] (suffixe latin inclus).
fn key_of(c: &regex::Captures) -> String {
    match c.get(3) {
        Some(sfx) => article_key(&format!("{}{} {}", &c[1], &c[2], sfx.as_str())),
        None => article_key(&format!("{}{}", &c[1], &c[2])),
    }
}

/// Suffixe lettre d'un article (« L. 1121-16-1 A », « 1609 G ») : une
/// majuscule isolée juste après la référence. La crate `regex` n'a pas de
/// lookahead — peek procédural dans [`targets_in`].
static RE_LETTER_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+([A-Z])(?:[\s,;.):]|$)").unwrap());

/// Cibles d'un fragment : références simples et plages « X à Y » (le « à »
/// ne borne une plage que s'il est immédiatement suivi d'une référence).
/// Les spans d'exception ont déjà été neutralisés par l'appelant — les
/// références qu'ils portaient ne sont pas des cibles, et l'énumération
/// reprend après eux.
fn targets_in(frag: &str) -> Vec<TargetRef> {
    let caps: Vec<regex::Captures> = RE_REF.captures_iter(frag).collect();
    let mut out: Vec<TargetRef> = Vec::new();
    let mut last_end = 0usize;
    let mut pending: Option<String> = None;
    for (i, c) in caps.iter().enumerate() {
        let m = c.get(0).unwrap();
        let mut key = key_of(c);
        let mut end = m.end();
        // Suffixe lettre : article distinct (l1121-16-1-a ≠ l1121-16-1) —
        // sauf si la majuscule ouvre la référence suivante (« à R. 3123-9 »).
        if c.get(3).is_none() {
            if let Some(sc) = RE_LETTER_SUFFIX.captures(&frag[m.end()..]) {
                let letter = sc.get(1).unwrap();
                let abs = m.end() + letter.start();
                let next_start = caps.get(i + 1).map(|n| n.get(0).unwrap().start());
                if next_start != Some(abs) {
                    key.push('-');
                    key.push(letter.as_str().to_ascii_lowercase().chars().next().unwrap());
                    end = abs + letter.as_str().len();
                }
            }
        }
        let gap_trim = frag[last_end..m.start()].trim();
        if pending.is_some() && (gap_trim == "à" || gap_trim == "a") {
            out.push(TargetRef::Range(pending.take().unwrap(), key));
        } else {
            if let Some(p) = pending.take() {
                out.push(TargetRef::Single(p));
            }
            pending = Some(key);
        }
        last_end = end;
    }
    if let Some(p) = pending {
        out.push(TargetRef::Single(p));
    }
    out
}

/// Exceptions d'un fragment. Une exclusion d'article ENTIER commence par
/// « des articles » / « de l'article » ; tout le reste (« du II de l'article
/// X », « de son troisième alinéa », « des 2°, 3°… ») est infra-article et
/// reste de la prose.
fn exceptions_in(fragment: &str) -> Vec<CounterException> {
    RE_EXCEPTION
        .captures_iter(fragment)
        .map(|cap| {
            let prose = cap[1].trim().trim_end_matches(',').to_string();
            let lower = prose.to_lowercase();
            let full = lower.starts_with("à l'exception des articles")
                || lower.starts_with("à l'exception de l'article");
            let full_articles = if full {
                RE_REF.captures_iter(&prose).map(|c| key_of(&c)).collect()
            } else {
                Vec::new()
            };
            CounterException {
                prose,
                full_articles,
            }
        })
        .collect()
}

fn redaction_date_in(fragment: &str) -> Option<String> {
    RE_REDACTION.captures(fragment).map(|c| {
        let day: u32 = if &c[1] == "1er" {
            1
        } else {
            c[1].parse().unwrap()
        };
        format!("{}-{:02}-{:02}", &c[3], month_num(&c[2]), day)
    })
}

/// Événement d'une ligne porteuse (phrase autonome ou item d'énumération).
/// `None` si la ligne ne cible aucun article.
fn event_of(line: &str, inherited_scopes: &[String], negated: bool) -> Option<CounterEvent> {
    let exceptions = exceptions_in(line);
    // Neutralise les spans d'exception avant le scan des cibles : leurs
    // références ne sont pas des cibles, l'énumération reprend après (le
    // terminateur — reprise, `;`, fin — est restitué).
    let cleaned = RE_EXCEPTION.replace_all(line, " ; ${2}");
    let targets = targets_in(&cleaned);
    if targets.is_empty() {
        return None;
    }
    let mut scopes = scopes_in(line);
    if scopes.is_empty() {
        scopes = inherited_scopes.to_vec();
    }
    Some(CounterEvent {
        targets,
        scopes,
        negated,
        redaction_date: redaction_date_in(line),
        exceptions,
        reserves: RE_RESERVE
            .captures_iter(line)
            .map(|cap| cap[1].trim().to_string())
            .collect(),
        foreign_owner: RE_FOREIGN_OWNER.is_match(&cleaned),
    })
}

/// Parse le texte d'un article compteur en événements d'applicabilité.
/// Segmentation à l'alinéa (les compteurs LEGI mettent une phrase par
/// ligne) ; un en-tête « … sont applicables … : » ouvre un contexte hérité
/// par les items `1°` qui suivent. Les références citées dans un en-tête
/// (« en application du 8° de l'article 7 de la loi organique… ») ne sont
/// jamais des cibles.
pub fn parse_compteur(texte: &str) -> Vec<CounterEvent> {
    let mut out = Vec::new();
    let mut header_ctx: Option<(Vec<String>, bool)> = None;
    for line in texte.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if RE_HEADER.is_match(line) {
            header_ctx = Some((scopes_in(line), is_negated(line)));
            continue;
        }
        if RE_ITEM.is_match(line) {
            if let Some((scopes, negated)) = &header_ctx {
                if let Some(ev) = event_of(line, scopes, *negated) {
                    out.push(ev);
                }
            }
            continue;
        }
        // Phrase autonome : le pivot d'applicabilité est requis.
        header_ctx = None;
        if !RE_PIVOT.is_match(line) {
            continue;
        }
        if let Some(ev) = event_of(line, &[], is_negated(line)) {
            out.push(ev);
        }
    }
    out
}

/// Résultat du parse d'un compteur à tableau.
#[derive(Debug, Default)]
pub struct TableParse {
    pub events: Vec<CounterEvent>,
    /// Lignes de division structurelles (« Au titre II ») — le grain
    /// division attend le dépliage par TOC, comme en prose.
    pub divisions: usize,
    /// Lignes du tableau non reconnues (échantillon borné à 20).
    pub unrecognized: Vec<String>,
}

/// En-tête de colonne (« Articles applicables », « DANS LEUR REDACTION »).
static RE_TABLE_COL_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:articles?\s+applicables?|dispositions?\s+applicables?|dans\s+(?:sa|leur)\s+r[ée]daction)\b",
    )
    .unwrap()
});

/// Ligne de division structurelle du tableau (« Au livre Ier », « Titre II »).
static RE_TABLE_DIVISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:au|aux|du|de\s+la|le|la)?\s*(?:livre|titre|chapitre|section|sous-section|partie)\b")
        .unwrap()
});

/// Fin de la zone tableau : section romaine (« II. - »), NOTA.
static RE_TABLE_SECTION_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[IVXLC]+\s*\.\s*-|NOTA)").unwrap());

/// Amorce tolérée d'une cellule de références (« 1° Les articles … »).
static RE_ROW_LEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:[0-9]+°(?:\s+(?:bis|ter|quater))?\s+)?(?:les\s+articles\s+|l'article\s+|articles?\s+)?",
    )
    .unwrap()
});

/// Amorce d'une cellule de rédaction (« Résultant du décret… »,
/// « l'ordonnance n° … », « Loi n° … », « n° … », « la présente loi »).
static RE_REDACTION_CELL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:r[ée]sultant|dans\s+(?:sa|leur)\s+r[ée]daction|(?:de\s+la\s+|la\s+|de\s+l'|l'|le\s+|du\s+)?(?:loi|ordonnance|d[ée]cret|arr[êe]t[ée])\b|n°|(?:de\s+la\s+|la\s+|le\s+|du\s+)pr[ée]sente?\b)",
    )
    .unwrap()
});

/// Date d'une cellule de rédaction : celle qui suit le numéro d'acte
/// (« n° 2022-1683 du 28 décembre 2022 » — pas celle d'un intitulé).
static RE_CELL_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)n°\s*[0-9]{2,4}-[0-9]+\s+du\s+(1er|[0-9]{1,2})\s+(janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre)\s+([0-9]{4})",
    )
    .unwrap()
});

/// Date nue (actes anciens sans numéro : « Loi du 9 décembre 1905 »).
static RE_BARE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(1er|[0-9]{1,2})\s+(janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre)\s+([0-9]{4})",
    )
    .unwrap()
});

/// Exception dans une cellule de références : supprimée en préservant la
/// plage (« L. 533-4-2 à l'exception de son IV, à L. 533-4-9 » doit rester
/// une plage). La virgule de reprise part avec la clause.
static RE_CELL_EXCEPTION_STRIP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i),?\s*à\s+l'exception\s+(?:de\s+la|de\s+l'|des|du|de|d')\s*[^,]*,?").unwrap()
});

fn iso_of(day: &str, month: &str, year: &str) -> String {
    let d: u32 = if day == "1er" {
        1
    } else {
        day.parse().unwrap()
    };
    format!("{year}-{:02}-{d:02}", month_num(&month.to_lowercase()))
}

/// Parse la zone tableau d'un compteur. `None` si aucune ligne d'en-tête
/// (pivot + « tableau ») n'ouvre de tableau. Les phrases hors tableau sont
/// ignorées — [`parse_compteur`] les couvre en parallèle.
pub fn parse_compteur_table(texte: &str) -> Option<TableParse> {
    struct Ctx {
        scopes: Vec<String>,
        negated: bool,
        global_anchor: Option<String>,
        reserves: Vec<String>,
        foreign: bool,
    }
    let mut out = TableParse::default();
    let mut ctx: Option<Ctx> = None;
    let mut opened = false;
    let mut pending: Option<(Vec<TargetRef>, Vec<CounterException>, bool)> = None;

    let emit = |ctx: &Ctx,
                pending: &mut Option<(Vec<TargetRef>, Vec<CounterException>, bool)>,
                anchor: Option<String>,
                out: &mut TableParse| {
        if let Some((targets, exceptions, cell_foreign)) = pending.take() {
            out.events.push(CounterEvent {
                targets,
                scopes: ctx.scopes.clone(),
                negated: ctx.negated,
                redaction_date: anchor,
                exceptions,
                reserves: ctx.reserves.clone(),
                foreign_owner: ctx.foreign || cell_foreign,
            });
        }
    };

    for line in texte.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Ouverture (ou ré-ouverture : plusieurs tableaux par compteur).
        if RE_PIVOT.is_match(line) && line.to_lowercase().contains("tableau") {
            if let Some(c) = &ctx {
                let anchor = c.global_anchor.clone();
                emit(c, &mut pending, anchor, &mut out);
            }
            ctx = Some(Ctx {
                scopes: scopes_in(line),
                negated: is_negated(line),
                global_anchor: redaction_date_in(line),
                reserves: RE_RESERVE
                    .captures_iter(line)
                    .map(|cap| cap[1].trim().to_string())
                    .collect(),
                foreign: RE_FOREIGN_OWNER.is_match(line),
            });
            opened = true;
            continue;
        }
        let Some(c) = &ctx else { continue };
        // Fermeture : section d'adaptation (« II. - … », items « 1° … »)
        // ou NOTA — leurs références ne sont pas des lignes de tableau.
        if RE_TABLE_SECTION_END.is_match(line) || RE_ITEM.is_match(line) {
            let anchor = c.global_anchor.clone();
            emit(c, &mut pending, anchor, &mut out);
            ctx = None;
            continue;
        }
        let has_date = RE_BARE_DATE.is_match(line);
        if RE_TABLE_COL_HEADER.is_match(line) && !has_date {
            continue;
        }
        if RE_TABLE_DIVISION.is_match(line) && !RE_REF.is_match(line) {
            let anchor = c.global_anchor.clone();
            emit(c, &mut pending, anchor, &mut out);
            out.divisions += 1;
            continue;
        }
        // Cellule de références : la ligne commence par une référence
        // (après l'amorce tolérée).
        let lead = RE_ROW_LEAD.find(line).map_or(0, |m| m.end());
        if RE_REF.find(&line[lead..]).is_some_and(|m| m.start() == 0) {
            let anchor = c.global_anchor.clone();
            emit(c, &mut pending, anchor, &mut out);
            let exceptions = exceptions_in(line);
            let cleaned = RE_CELL_EXCEPTION_STRIP.replace_all(line, " ");
            let targets = targets_in(&cleaned);
            if targets.is_empty() {
                if out.unrecognized.len() < 20 {
                    out.unrecognized.push(line.to_string());
                }
                continue;
            }
            let cell_foreign = RE_FOREIGN_OWNER.is_match(&cleaned);
            pending = Some((targets, exceptions, cell_foreign));
            continue;
        }
        // Cellule de rédaction : ancre de la ligne en attente. Sans date
        // (« la présente ordonnance ») = gel à la date du compteur.
        if RE_REDACTION_CELL.is_match(line) {
            if pending.is_some() {
                let anchor = RE_CELL_DATE
                    .captures(line)
                    .or_else(|| RE_BARE_DATE.captures(line))
                    .map(|d| iso_of(&d[1], &d[2], &d[3]));
                emit(c, &mut pending, anchor, &mut out);
            } else if out.unrecognized.len() < 20 {
                out.unrecognized.push(line.to_string());
            }
            continue;
        }
        if out.unrecognized.len() < 20 {
            out.unrecognized.push(line.to_string());
        }
    }
    if let Some(c) = &ctx {
        let anchor = c.global_anchor.clone();
        emit(c, &mut pending, anchor, &mut out);
    }
    opened.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_simple_ancre_decret() {
        let evs = parse_compteur(
            "L'article R. 425-2 est applicable en Polynésie française dans sa rédaction résultant du décret n° 2017-733 du 4 mai 2017.",
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].targets, vec![TargetRef::Single("r425-2".into())]);
        assert_eq!(evs[0].scopes, vec!["geo:PF"]);
        assert_eq!(evs[0].redaction_date.as_deref(), Some("2017-05-04"));
        assert!(!evs[0].negated);
    }

    #[test]
    fn prose_plages_exception_infra_et_heritage() {
        let texte = "Sont applicables à la Polynésie française les articles L. 218-1 à L. 218-72, à l'exception du II de l'article L. 218-44, et les articles L. 218-83 à L. 218-86, sous réserve des compétences dévolues au territoire dans les eaux territoriales.\nLes articles L. 218-1 et L. 218-2 sont applicables en Polynésie française dans leur rédaction résultant de la loi n° 2021-1308 du 8 octobre 2021 portant diverses dispositions d'adaptation au droit de l'Union européenne dans le domaine des transports, de l'environnement, de l'économie et des finances, sous réserve des compétences dévolues à cette collectivité dans les eaux territoriales.\nL'article L. 218-42 est applicable dans sa rédaction résultant de l'ordonnance n° 2015-1534 du 26 novembre 2015 prise en application de l'article 30 de la loi n° 2015-917 du 28 juillet 2015 et portant diverses dispositions concernant la défense, les anciens combattants et l'action de l'Etat en mer.";
        let evs = parse_compteur(texte);
        assert_eq!(evs.len(), 3);

        assert_eq!(
            evs[0].targets,
            vec![
                TargetRef::Range("l218-1".into(), "l218-72".into()),
                TargetRef::Range("l218-83".into(), "l218-86".into()),
            ]
        );
        assert_eq!(evs[0].exceptions.len(), 1);
        assert!(evs[0].exceptions[0].full_articles.is_empty());
        assert!(evs[0].exceptions[0].prose.contains("du II de l'article"));
        assert_eq!(evs[0].reserves.len(), 1);
        assert!(evs[0].redaction_date.is_none());

        assert_eq!(evs[1].redaction_date.as_deref(), Some("2021-10-08"));
        assert_eq!(
            evs[1].targets,
            vec![
                TargetRef::Single("l218-1".into()),
                TargetRef::Single("l218-2".into())
            ]
        );

        assert!(evs[2].scopes.is_empty());
        assert_eq!(evs[2].redaction_date.as_deref(), Some("2015-11-26"));
        assert_eq!(evs[2].targets, vec![TargetRef::Single("l218-42".into())]);
    }

    #[test]
    fn enumeration_sous_en_tete() {
        let texte = "I. - En application du 8° de l'article 7 de la loi organique n° 2004-192 du 27 février 2004, sous réserve des adaptations prévues au II, au III et au IV, sont applicables de plein droit en Polynésie française :\n1° Les articles R. 561-1 à R. 561-38-9 ;\n2° L'article R. 561-39 à l'exception de son troisième alinéa ;\n5° Les articles R. 561-42-2 à R. 561-56-1, l'article R. 561-57 à l'exception des 23°, 24°, 26°, 27° et 30°, les articles R. 561-58 à R. 561-58-6 et R. 561-60 à R. 561-65.\nII. - Pour l'application du I :\n1° Les références à la fiducie au sens du code civil sont supprimées ;";
        let evs = parse_compteur(texte);
        assert_eq!(evs.len(), 3);
        assert_eq!(
            evs[0].targets,
            vec![TargetRef::Range("r561-1".into(), "r561-38-9".into())]
        );
        assert_eq!(evs[0].scopes, vec!["geo:PF"]);

        assert_eq!(evs[1].targets, vec![TargetRef::Single("r561-39".into())]);
        assert_eq!(evs[1].exceptions.len(), 1);
        assert!(evs[1].exceptions[0].full_articles.is_empty());

        assert_eq!(
            evs[2].targets,
            vec![
                TargetRef::Range("r561-42-2".into(), "r561-56-1".into()),
                TargetRef::Single("r561-57".into()),
                TargetRef::Range("r561-58".into(), "r561-58-6".into()),
                TargetRef::Range("r561-60".into(), "r561-65".into()),
            ]
        );
    }

    #[test]
    fn negation_ecarte() {
        let evs = parse_compteur(
            "Les articles L. 218-3 et L. 218-4 ne sont pas applicables en Nouvelle-Calédonie.",
        );
        assert_eq!(evs.len(), 1);
        assert!(evs[0].negated);
        assert_eq!(evs[0].scopes, vec!["geo:NC"]);
    }

    #[test]
    fn exception_article_entier() {
        let evs = parse_compteur(
            "Sont applicables en Polynésie française les articles L. 100-1 à L. 100-9, à l'exception des articles L. 100-3 et L. 100-5.",
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(
            evs[0].exceptions[0].full_articles,
            vec!["l100-3".to_string(), "l100-5".to_string()]
        );
    }

    #[test]
    fn clause_auto_referente_alsace() {
        assert_eq!(
            self_scope(
                "La présente loi est applicable aux départements du Haut-Rhin, du Bas-Rhin et de la Moselle."
            ),
            Some((SelfGrain::Text, vec!["geo:FR-ALS".to_string()], false))
        );
        assert_eq!(
            self_scope(
                "Le présent arrêté n'est pas applicable dans les départements de la Moselle, du Bas-Rhin et du Haut-Rhin."
            ),
            Some((SelfGrain::Text, vec!["geo:FR-ALS".to_string()], true))
        );
        assert_eq!(
            self_scope(
                "Les dispositions du présent chapitre sont applicables dans les départements du Bas-Rhin, du Haut-Rhin et de la Moselle."
            ),
            Some((SelfGrain::Division, vec!["geo:FR-ALS".to_string()], false))
        );
        assert_eq!(
            self_scope("Le présent article est applicable dans le département de la Moselle."),
            None
        );
    }

    #[test]
    fn proprietaire_etranger_signale() {
        let evs = parse_compteur(
            "Les articles L. 121-1 à L. 121-3 du code de la consommation sont applicables en Polynésie française.",
        );
        assert_eq!(evs.len(), 1);
        assert!(evs[0].foreign_owner);

        let evs = parse_compteur(
            "Les articles L. 218-1 et L. 218-2 de la présente loi sont applicables en Polynésie française.",
        );
        assert!(!evs[0].foreign_owner);
        assert_eq!(evs.len(), 1);
    }

    #[test]
    fn tableau_signale() {
        assert!(compteur_has_table(
            "Les dispositions mentionnées dans le tableau ci-après sont applicables en Polynésie française :"
        ));
    }

    #[test]
    fn suffixe_lettre_article_distinct() {
        let evs = parse_compteur(
            "Les articles L. 1121-16-1 A et L. 80 A sont applicables en Polynésie française.",
        );
        assert_eq!(
            evs[0].targets,
            vec![
                TargetRef::Single("l1121-16-1-a".into()),
                TargetRef::Single("l80-a".into()),
            ]
        );
        let evs = parse_compteur(
            "Sont applicables en Polynésie française les articles R. 3122-6 à R. 3123-9.",
        );
        assert_eq!(
            evs[0].targets,
            vec![TargetRef::Range("r3122-6".into(), "r3123-9".into())]
        );
    }

    #[test]
    fn tableau_ancre_globale_sauf_mention() {
        let texte = "Sous la réserve des adaptations prévues par le présent titre, les dispositions suivantes sont applicables dans les Terres australes et antarctiques françaises aux contrats de concession, dans leur rédaction résultant du décret n° 2018-1075 du 3 décembre 2018, sauf mention contraire dans le tableau ci-après.\n\nDISPOSITIONS APPLICABLES\nDANS LEUR REDACTION\n\nAu livre Ier\n\nAu titre Ier\n\nR. 3111-1 à R. 3111-3\n\nR. 3113-1\nRésultant du décret n° 2022-1683 du 28 décembre 2022\n\nR. 3114-1 et R. 3114-2\n";
        let t = parse_compteur_table(texte).unwrap();
        assert_eq!(t.divisions, 2);
        assert!(t.unrecognized.is_empty());
        assert_eq!(t.events.len(), 3);
        assert_eq!(
            t.events[0].targets,
            vec![TargetRef::Range("r3111-1".into(), "r3111-3".into())]
        );
        assert_eq!(t.events[0].redaction_date.as_deref(), Some("2018-12-03"));
        assert_eq!(t.events[0].scopes, vec!["geo:TF"]);
        assert_eq!(
            t.events[1].targets,
            vec![TargetRef::Single("r3113-1".into())]
        );
        assert_eq!(t.events[1].redaction_date.as_deref(), Some("2022-12-28"));
        assert_eq!(
            t.events[2].targets,
            vec![
                TargetRef::Single("r3114-1".into()),
                TargetRef::Single("r3114-2".into()),
            ]
        );
        assert_eq!(t.events[2].redaction_date.as_deref(), Some("2018-12-03"));
    }

    #[test]
    fn tableau_rangees_appariees_et_fermeture() {
        let texte = "I. - Sous réserve des dispositions prévues au II et au III, sont applicables en Nouvelle-Calédonie les articles mentionnés dans la colonne de gauche du tableau ci-après, dans leur rédaction indiquée dans la colonne de droite du même tableau :\n\nArticles applicables\nDans leur rédaction résultant de\n\nL. 533-1\n\nl'ordonnance n° 2007-544 du 12 avril 2007\n\nL. 533-2-1 à L. 533-2-3\nl'ordonnance n° 2021-796 du 23 juin 2021\n\nL. 533-4 à l'exception de ses deuxième et cinquième alinéas\nl'ordonnance n° 2021-796 du 23 juin 2021\n\nL. 533-4-2 à l'exception de son IV, à L. 533-4-9\nl'ordonnance n° 2021-796 du 23 juin 2021\n\nII. - Pour l'application du I, les références aux unités mentionnées à l'article L. 229-7 du code de l'environnement ne sont pas applicables.\n1° Au troisième alinéa de l'article L. 533-2-2, les mots : \" x \" sont remplacés par les mots : \" y \" ;\n";
        let t = parse_compteur_table(texte).unwrap();
        assert_eq!(t.events.len(), 4);
        assert_eq!(
            t.events[0].targets,
            vec![TargetRef::Single("l533-1".into())]
        );
        assert_eq!(t.events[0].redaction_date.as_deref(), Some("2007-04-12"));
        assert_eq!(t.events[0].scopes, vec!["geo:NC"]);
        assert_eq!(
            t.events[1].targets,
            vec![TargetRef::Range("l533-2-1".into(), "l533-2-3".into())]
        );
        assert_eq!(
            t.events[2].targets,
            vec![TargetRef::Single("l533-4".into())]
        );
        assert_eq!(t.events[2].exceptions.len(), 1);
        assert!(t.events[2].exceptions[0].full_articles.is_empty());
        assert_eq!(
            t.events[3].targets,
            vec![TargetRef::Range("l533-4-2".into(), "l533-4-9".into())]
        );
        assert!(t
            .events
            .iter()
            .all(|e| !e.targets.contains(&TargetRef::Single("l229-7".into()))));
    }
}
