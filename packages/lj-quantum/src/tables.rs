//! Recapitulative tables: a poste label followed by its amounts in columns, read wherever the decision lays them out.
use std::collections::HashSet;

use super::dispositif::{Extraction, Mode};
use super::leftovers::{self, Amount, Zone, Zoning};
use super::vocabulary::{self, Cue};
use super::{Poste, Victime};
use lj_extract::scan::DocScan;

/// The separators a flattened table leaves between two cells. A single space is
/// not one: it holds the thousands of « 40 000,00 € » together.
const SPACES: [char; 5] = [' ', '\u{a0}', '\u{202f}', '\u{2009}', '\u{2007}'];
const CURRENCIES: [&str; 8] = [
    "euros",
    "euro",
    "eur",
    "f cfp",
    "fcfp",
    "xpf",
    "francs cfp",
    "franc cfp",
];

/// Abbreviations the tables use as labels, matched on a whole cell only: out of
/// a cell « se » or « pa » is a word of the prose.
#[rustfmt::skip]
// « ATP » alone does not say whether the row is the permanent poste or the
// temporary one, which the nomenclature files under frais divers: only the
// doubled letter settles it.
const SIGLES: [(&str, &str); 18] = [
    ("dsa", "DSA"), ("fd", "FD"), ("pgpa", "PGPA"), ("dsf", "DSF"), ("atpp", "ATP"),
    ("ip", "IP"), ("pgpf", "PGPF"), ("dft", "DFT"), ("se", "SE"), ("pet", "PET"),
    ("dfp", "DFP"), ("pa", "PA"), ("pep", "PEP"), ("ps", "PS"), ("pe", "PE"),
    ("fla", "FLA"), ("fva", "FVA"), ("psu", "PSU"),
];

/// Infinitives and verbs of a party's conclusions: a table they introduce is
/// what someone asks for, not what the court retains.
const CLAIMED: [&str; 21] = [
    "condamner",
    "infirmer",
    "reformer",
    "evaluer",
    "liquider",
    "debouter",
    "fixer les",
    "fixer le prejudice",
    "sollicit",
    "dernieres conclusions",
    "a titre subsidiaire",
    "a titre principal",
    "demande a la cour",
    "offre de",
    "evalue le prejudice",
    "evalue les prejudices",
    "a droit a l'indemnisation",
    "par jugement du",
    "le tribunal a",
    "premiers juges",
    "jugement dont appel",
];

/// What a sentence carries and a cell of a table does not: a row whose label
/// says who ruled, or asked, is a stretch of prose the amounts have cut out.
const PROSE: [&str; 16] = [
    "le tribunal",
    "la cour",
    "il convient",
    "les premiers juges",
    "sollicit",
    "condamn",
    "jugement",
    "reclame",
    "s'eleve",
    "estime",
    "retient",
    "fixe a",
    "evalue a",
    "alloue une",
    " que ",
    " sera ",
];

/// The relatives get their own table, and the record holds nobody by name.
const RICOCHET: [&str; 5] = [
    "ricochet",
    "victimes indirectes",
    "des proches",
    "ayants droit",
    "prejudices reflechis",
];

#[derive(Clone, Copy, PartialEq)]
enum Cell {
    Amount(f64),
    /// « - », « néant », « rejet », « débouté » : the poste is priced at nothing.
    Nil,
    /// « réservé », « pour mémoire » : no amount was set, and none is coming.
    Skip,
    Text,
}

struct Seg {
    s: usize,
    e: usize,
    cell: Cell,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    Label,
    Cour,
    /// What the debtor owes once the share of liability is taken off: the poste
    /// reduced, never the poste.
    Du,
    Victime,
    Tiers,
    Demande,
    Offre,
    Jugement,
    Unknown,
}

struct Row {
    label: (usize, usize),
    amounts: Vec<Cell>,
}

struct Table {
    /// Character bounds of the header cells, in order.
    header: Vec<(usize, usize)>,
    rows: Vec<Row>,
    /// The text cells sitting between two rows: a poste named there is one the
    /// table details elsewhere.
    extra: Vec<(usize, usize)>,
    start: usize,
}

fn trimmed(chars: &[char], mut s: usize, mut e: usize) -> (usize, usize) {
    let junk = |c: char| c.is_whitespace() || matches!(c, '*' | '\u{2022}' | '\u{b7}' | '\u{25cf}');
    while s < e && junk(chars[s]) {
        s += 1;
    }
    while e > s && junk(chars[e - 1]) {
        e -= 1;
    }
    (s, e)
}

fn segments(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let push = |s: usize, e: usize, out: &mut Vec<(usize, usize)>| {
        let (s, e) = trimmed(chars, s, e);
        if e > s {
            out.push((s, e));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' || c == '\t' || c == '|' {
            push(start, i, &mut out);
            i += 1;
            start = i;
            continue;
        }
        if SPACES.contains(&c) {
            let mut j = i;
            while j < chars.len() && SPACES.contains(&chars[j]) {
                j += 1;
            }
            if j - i >= 2 {
                push(start, i, &mut out);
                start = j;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    push(start, chars.len(), &mut out);
    out
}

/// The cells of the document. A scan collapses every newline to a space, so a
/// table laid out one cell per line arrives as one run: the priced amounts cut
/// it back into a label and its columns.
fn cells(chars: &[char], folded: &[char], amounts: &[Amount]) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    for (s, e) in segments(chars) {
        let text = text_of(folded, s, e);
        let whole = cell_of(&text);
        // « 38 332,21 <saut> euros » : the currency word falls in a cell of its
        // own, which would cut the row in two.
        if whole == Cell::Text && without_currency(&text).is_empty() {
            continue;
        }
        let inside: Vec<&Amount> = amounts
            .iter()
            .filter(|a| a.start >= s && a.end <= e)
            .collect();
        if whole != Cell::Text || !tabular(chars, folded, s, e, &inside) {
            out.push(Seg { s, e, cell: whole });
            continue;
        }
        let mut cur = s;
        for a in inside {
            push_text(chars, folded, cur, a.start, &mut out);
            out.push(Seg {
                s: a.start,
                e: a.end,
                cell: Cell::Amount(a.value as f64 / 1000.0),
            });
            cur = a.end;
        }
        push_text(chars, folded, cur, e, &mut out);
    }
    out
}

/// A run of cells the scan has glued into one: three prices at least, each
/// labelled in a few words. Prose priced twice in a sentence is not one, and
/// cutting it would read a table where the decision writes an argument.
fn tabular(chars: &[char], folded: &[char], s: usize, e: usize, inside: &[&Amount]) -> bool {
    if inside.len() < 3 {
        return false;
    }
    let mut cur = s;
    for a in inside {
        let (ts, te) = trimmed(chars, cur, a.start);
        if te.saturating_sub(ts) > 45 {
            return false;
        }
        cur = a.end;
    }
    let (ts, te) = trimmed(chars, cur, e);
    te.saturating_sub(ts) <= 45 || text_of(folded, ts, te).is_empty()
}

/// The stretch between two amounts, its currency word dropped: `amounts_in`
/// stops at the last digit, and « euros » heads the label that follows.
fn push_text(chars: &[char], folded: &[char], s: usize, e: usize, out: &mut Vec<Seg>) {
    let (mut s, e) = trimmed(chars, s, e);
    let head = text_of(folded, s, e);
    for w in CURRENCIES {
        if head.starts_with(w) && !head[w.len()..].starts_with(|c: char| c.is_alphanumeric()) {
            s += w.chars().count();
            break;
        }
    }
    if head.starts_with('\u{20ac}') {
        s += 1;
    }
    let (s, e) = trimmed(chars, s, e);
    if e > s {
        let cell = cell_of(&text_of(folded, s, e));
        out.push(Seg { s, e, cell });
    }
}

fn parse_number(t: &str) -> Option<f64> {
    let cleaned: String = t
        .chars()
        .filter(|c| !SPACES.contains(c) && *c != '\'')
        .collect();
    let (int_part, frac) = match cleaned.rsplit_once(',') {
        Some((a, b)) if (1..=2).contains(&b.len()) && b.chars().all(|c| c.is_ascii_digit()) => {
            (a.to_string(), b.to_string())
        }
        Some(_) => return None,
        // A stray « 768.90 » among « 171.857,90 »: two digits behind a dot are
        // cents, three are a thousands group.
        None => match cleaned.rsplit_once('.') {
            Some((a, b)) if (1..=2).contains(&b.len()) && b.chars().all(|c| c.is_ascii_digit()) => {
                (a.to_string(), b.to_string())
            }
            _ => (cleaned, "0".to_string()),
        },
    };
    let ip = if int_part.contains('.') {
        let parts: Vec<&str> = int_part.split('.').collect();
        let head_ok = !parts[0].is_empty() && parts[0].len() <= 3;
        if !head_ok || !parts[1..].iter().all(|p| p.len() == 3) {
            return None;
        }
        parts.concat()
    } else {
        int_part
    };
    if ip.is_empty() || ip.len() > 12 || !ip.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    format!("{ip}.{frac}").parse().ok()
}

fn without_currency(txt: &str) -> &str {
    let t = txt.trim_end();
    if let Some(p) = t.strip_suffix('€') {
        return p.trim_end();
    }
    for w in CURRENCIES {
        if let Some(p) = t.strip_suffix(w) {
            if p.is_empty() || p.ends_with(|c: char| !c.is_alphanumeric()) {
                return p.trim_end();
            }
        }
    }
    t
}

fn amount_of(txt: &str) -> Option<f64> {
    if txt.contains('%') {
        return None;
    }
    let t = without_currency(txt)
        .trim_end_matches([':', ';', ')', '.'])
        .trim_end();
    let t = t.trim_start_matches('(').trim_start();
    if !t.starts_with(|c: char| c.is_ascii_digit())
        || !t
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == '\'' || SPACES.contains(&c))
    {
        return None;
    }
    parse_number(t)
}

fn cell_of(folded: &str) -> Cell {
    if let Some(v) = amount_of(folded) {
        return Cell::Amount(v);
    }
    let bare = without_currency(folded).trim_end_matches(['.', ':']).trim();
    if matches!(
        bare,
        "-" | "--"
            | "---"
            | "\u{2013}"
            | "\u{2014}"
            | "neant"
            | "rejet"
            | "rejete"
            | "rejetee"
            | "rejetes"
            | "rejetees"
            | "deboute"
            | "deboutee"
            | "deboutes"
            | "deboutees"
            | "sans objet"
            | "/"
            | "n/a"
            | "nc"
            | "0"
    ) {
        return Cell::Nil;
    }
    if matches!(
        bare,
        "reserve" | "reserves" | "reservee" | "pour memoire" | "memoire" | "?" | "a parfaire"
    ) {
        return Cell::Skip;
    }
    Cell::Text
}

fn text_of(folded: &[char], s: usize, e: usize) -> String {
    let raw: String = folded[s..e].iter().collect();
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has(f: &str, words: &[&str]) -> bool {
    words.iter().any(|w| f.contains(w))
}

/// « part revenant à Mme [Y] » : the share of a natural person the source has
/// pseudonymised, which is the victim's. « Part du Dr [I] » is a codebtor's.
fn person_share(f: &str) -> bool {
    f.contains('[')
        && has(
            f,
            &["a m.", "a mme", "a mlle", "a m ", "a monsieur", "a madame"],
        )
}

fn role_of(f: &str) -> Role {
    let f = f.trim_end_matches([':', '.', ',']).trim();
    if f == "prejudice" || f == "prejudices" {
        return Role::Label;
    }
    if f.starts_with("poste")
        || f.starts_with("nature du prejudice")
        || f == "nature"
        || f.starts_with("prejudices poste par poste")
        || f == "prejudice victime"
        || f == "prejudices victime"
    {
        return Role::Label;
    }
    if has(f, &["offre", "offert", "propos"]) {
        return Role::Offre;
    }
    if has(f, &["sollicit", "demande", "reclam"]) {
        return Role::Demande;
    }
    // « évaluation par le jugement ou la Cour » is the court's own column.
    if f.contains("cour") && !f.contains("recours") {
        return Role::Cour;
    }
    if has(
        f,
        &[
            "jugement",
            "tribunal",
            "premiere instance",
            "1ere instance",
            "premiers juges",
        ],
    ) {
        return Role::Jugement;
    }
    if has(
        f,
        &[
            "cpam",
            "caisse",
            "tiers payeur",
            "tiers-payeur",
            "organisme",
            "ccss",
            " cps",
            " msa",
            "mutuelle",
            "debours",
            "prestations",
            "harmonie",
            "malakoff",
            "ociane",
            "ag2r",
            "chorum",
            "mgen",
            "cnmss",
            "cramif",
            "prevoyance",
        ],
    ) {
        return Role::Tiers;
    }
    if has(f, &["victime"]) || person_share(f) {
        return Role::Victime;
    }
    if f.contains("evaluation") {
        return Role::Cour;
    }
    if has(
        f,
        &[
            "charge du responsable",
            "charge de la",
            "dette du responsable",
            "indemnite due",
            "indemnites due",
            "somme due",
            "sommes dues",
            "montant du",
        ],
    ) {
        return Role::Du;
    }
    if has(
        f,
        &[
            "montant",
            "indemnisation",
            "indemnite",
            "somme due",
            "sommes dues",
            "alloue",
            "la cour",
            "charge du responsable",
            "dette du responsable",
            "reparation",
        ],
    ) {
        return Role::Cour;
    }
    if f.starts_with("total") || f.starts_with("prejudice") || f.starts_with("evaluation") {
        return Role::Cour;
    }
    Role::Unknown
}

/// A word that opens a column of its own, even when it settles no role: without
/// it « Créance | Groupama » folds into the caisse column on its left.
fn opens_column(f: &str) -> bool {
    [
        "creance",
        "part ",
        "parts ",
        "du a",
        "due a",
        "dues a",
        "dus a",
        "indemnite",
        "somme",
        "montant",
        "solde",
        "prestations",
        "evaluation",
        "total",
        "droits",
        "pour ",
        "dette",
        "debours",
        "offre",
        "demande",
        "jugement",
        "assiette",
        "expertise",
    ]
    .iter()
    .any(|w| f.starts_with(w))
}

fn tables_of(segs: &[Seg], folded: &[char]) -> Vec<Table> {
    let mut out: Vec<Table> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut pending: Vec<(usize, usize)> = Vec::new();
    let mut header: Vec<(usize, usize)> = Vec::new();
    let mut extra: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if segs[i].cell != Cell::Text {
            if !rows.is_empty() {
                out.push(Table {
                    header: std::mem::take(&mut header),
                    extra: std::mem::take(&mut extra),
                    start: rows[0].label.0,
                    rows: std::mem::take(&mut rows),
                });
            }
            extra.clear();
            pending.clear();
            i += 1;
            continue;
        }
        let mut amounts = Vec::new();
        let mut j = i + 1;
        while j < segs.len()
            && matches!(segs[j].cell, Cell::Amount(_) | Cell::Nil | Cell::Skip)
            && amounts.len() < 6
        {
            amounts.push(segs[j].cell);
            j += 1;
        }
        if amounts.is_empty() {
            let long = segs[i].e - segs[i].s > 60;
            pending.push((segs[i].s, segs[i].e));
            if !rows.is_empty() && (long || pending.len() > 4) {
                out.push(Table {
                    header: std::mem::take(&mut header),
                    extra: std::mem::take(&mut extra),
                    start: rows[0].label.0,
                    rows: std::mem::take(&mut rows),
                });
                pending.clear();
            }
            i += 1;
            continue;
        }
        if rows.is_empty() {
            header = pending
                .iter()
                .rev()
                .take_while(|(s, e)| e - s <= 45 && !text_of(folded, *s, *e).is_empty())
                .copied()
                .collect();
            header.reverse();
        }
        if !rows.is_empty() {
            extra.append(&mut pending);
        }
        pending.clear();
        rows.push(Row {
            label: (segs[i].s, segs[i].e),
            amounts,
        });
        i = j;
    }
    if !rows.is_empty() {
        out.push(Table {
            header,
            extra,
            start: rows[0].label.0,
            rows,
        });
    }
    out.retain(|t| t.rows.len() >= 3);
    out
}

/// The header cells merged into columns: a cell that settles no role and opens
/// none continues the one on its left, and a poste label or a group heading
/// closes the header, being a row of the table rather than a column of it.
fn columns(folded: &[char], header: &[(usize, usize)], cues: &[(usize, usize, Cue)]) -> Vec<Role> {
    let mut cols: Vec<String> = Vec::new();
    for (s, e) in header {
        if code_of(cues, folded, *s, *e).is_some() || group_heading(cues, folded, *s, *e) {
            break;
        }
        let t = text_of(folded, *s, *e);
        let continues = match cols.last() {
            Some(prev) => {
                (role_of(&t) == Role::Unknown && !opens_column(&t)) || prev.ends_with('/')
            }
            None => false,
        };
        if continues {
            let prev = cols.last_mut().expect("checked");
            prev.push(' ');
            prev.push_str(t.trim_start_matches('/').trim());
        } else {
            cols.push(t);
        }
    }
    let mut roles: Vec<Role> = cols.iter().map(|c| role_of(c)).collect();
    // « Récapitulatif » heads the table, « Expertise » heads a column of it.
    while roles.first() == Some(&Role::Unknown) && !opens_column(&cols[cols.len() - roles.len()]) {
        roles.remove(0);
    }
    // « Postes | Préjudices | Victime | CPAM » : the second label heads the
    // court's column, the first only names the rows.
    if roles.first() == Some(&Role::Label) {
        roles.remove(0);
        if roles.first() == Some(&Role::Label) {
            roles[0] = Role::Cour;
        }
    }
    roles
}

/// A cell naming a family of postes rather than a column.
/// « Montant soumis au recours de la CPAM » heads a column, not a group: a cell
/// that names a caisse is that caisse's column, whatever it carries besides.
fn group_heading(cues: &[(usize, usize, Cue)], folded: &[char], s: usize, e: usize) -> bool {
    role_of(&text_of(folded, s, e)) != Role::Tiers && heading(cues, s, e)
}

fn heading(cues: &[(usize, usize, Cue)], s: usize, e: usize) -> bool {
    cues.iter()
        .any(|(cs, ce, c)| *cs >= s && *ce <= e && matches!(c, Cue::OutOfScope(_)))
}

/// The codes a label could name without the vocabulary settling which: a row the
/// table prices under one of them poisons them all.
fn ambiguous_codes(label: &str) -> Vec<&'static str> {
    vocabulary::AMBIGUOUS
        .iter()
        .filter(|(w, _)| label.contains(w))
        .flat_map(|(_, codes)| codes.iter().copied())
        .collect()
}

fn code_of(
    cues: &[(usize, usize, Cue)],
    folded: &[char],
    s: usize,
    e: usize,
) -> Option<&'static str> {
    let inside: Vec<Cue> = cues
        .iter()
        .filter(|(cs, ce, _)| *cs >= s && *ce <= e)
        .map(|(_, _, c)| *c)
        .collect();
    if inside.iter().any(|c| matches!(c, Cue::OutOfScope(_))) {
        return None;
    }
    let mut codes: Vec<&'static str> = inside
        .iter()
        .filter_map(|c| match c {
            Cue::Poste(code) => Some(*code),
            _ => None,
        })
        .collect();
    codes.dedup();
    codes.sort_unstable();
    codes.dedup();
    if let [only] = codes[..] {
        return Some(only);
    }
    if !codes.is_empty() {
        return None;
    }
    let t = text_of(folded, s, e);
    SIGLES
        .iter()
        .find(|(sigle, _)| t.trim_end_matches([':', '.']).trim() == *sigle)
        .map(|(_, code)| *code)
}

fn is_total(label: &str) -> bool {
    let l = label.trim_end_matches([':', '.']).trim();
    if l == "totaux" {
        return true;
    }
    if !l.starts_with("total") || has(l, &["sous", "apres", "partiel", "creance", "general par"]) {
        return false;
    }
    // « Total avant provision » carries the award; the row after it carries what
    // is left to pay.
    !l.contains("provision") || l.contains("avant")
}

/// The court's own recapitulation, as opposed to a party's claim or the recital
/// of the judgment: the dispositif settles it, elsewhere the intro does.
fn court_authored(folded: &[char], table: &Table, zoning: &Zoning, dispositif: usize) -> bool {
    if table.start >= dispositif {
        return true;
    }
    if zoning.zone(table.start) == Zone::Pretentions {
        return false;
    }
    let lo = table.start.saturating_sub(500);
    let intro = text_of(folded, lo, table.start);
    !has(&intro, &CLAIMED) && !has(&intro, &RICOCHET)
}

struct Plan {
    cour: Option<usize>,
    victime: Option<usize>,
    tiers: Vec<usize>,
    demande: Option<usize>,
    offre: Option<usize>,
    jugement: Option<usize>,
    /// What the column right after the court's own carries.
    second: Option<Role>,
    width: usize,
}

fn plan_of(roles: &[Role], width: usize) -> Option<Plan> {
    // A column empty on every row leaves the header longer than any row: the
    // rows then read as short ones, never by position. Under two columns there
    // is no header to speak of, and a prose list would pass for a table.
    if roles.len() < width || roles.len() < 2 {
        return None;
    }
    let width = roles.len();
    let only = |want: Role| -> Option<usize> {
        let hits: Vec<usize> = roles
            .iter()
            .enumerate()
            .filter(|(_, r)| **r == want)
            .map(|(i, _)| i)
            .collect();
        match hits[..] {
            [i] => Some(i),
            _ => None,
        }
    };
    Some(Plan {
        cour: only(Role::Cour).filter(|i| *i == 0),
        victime: only(Role::Victime),
        tiers: roles
            .iter()
            .enumerate()
            .filter(|(_, r)| **r == Role::Tiers)
            .map(|(i, _)| i)
            .collect(),
        demande: only(Role::Demande),
        offre: only(Role::Offre),
        jugement: only(Role::Jugement),
        second: roles
            .get(1)
            .copied()
            .filter(|r| matches!(r, Role::Victime | Role::Tiers)),
        width,
    })
}

/// What a row writes: the court's award, and the shares when the columns and the
/// arithmetic agree on them.
struct Read {
    alloue: Option<f64>,
    part_victime: Option<f64>,
    part_tiers: Option<f64>,
    demande: Option<f64>,
    offre: Option<f64>,
    jugement: Option<f64>,
}

fn value(cell: Cell, nil_is_zero: bool) -> Option<f64> {
    match cell {
        Cell::Amount(v) => Some(v),
        Cell::Nil if nil_is_zero => Some(0.0),
        _ => None,
    }
}

fn read_row(plan: &Plan, amounts: &[Cell], short: Short, mode: Mode) -> Option<Read> {
    let extended = mode == Mode::Extended;
    let n = amounts.len();
    if n > plan.width {
        return None;
    }
    let mut r = Read {
        alloue: None,
        part_victime: None,
        part_tiers: None,
        demande: None,
        offre: None,
        jugement: None,
    };
    if n == plan.width {
        let at = |i: Option<usize>| i.and_then(|i| value(amounts[i], extended));
        r.alloue = at(plan.cour);
        r.demande = at(plan.demande);
        r.offre = at(plan.offre);
        r.jugement = at(plan.jugement);
        let pv = plan.victime.and_then(|i| value(amounts[i], true));
        let pt: Option<f64> = plan
            .tiers
            .iter()
            .map(|i| value(amounts[*i], true))
            .try_fold(0.0, |acc, v| v.map(|v| acc + v));
        if let (Some(alloue), Some(pv)) = (r.alloue, pv) {
            let closes = pt.is_some_and(|pt| (alloue - pv - pt).abs() < 0.005);
            if closes || (extended && plan.tiers.is_empty()) {
                r.part_victime = Some(pv);
                r.part_tiers = pt;
            }
        }
        return Some(r);
    }
    // Fewer cells than columns: the first column reads by position, the others
    // only once the layout of the row is settled.
    let cour = plan.cour?;
    if cour != 0 {
        return None;
    }
    r.alloue = value(amounts[0], extended);
    match short {
        Short::Proven(layout) => {
            if let Some(cols) = layout_columns(layout, plan, n) {
                let mut victime = None;
                let mut tiers = 0.0;
                for (k, j) in cols.iter().enumerate() {
                    let Some(v) = value(amounts[k + 1], true) else {
                        return Some(r);
                    };
                    if Some(*j) == plan.victime {
                        victime = Some(v);
                    } else if plan.tiers.contains(j) {
                        tiers += v;
                    }
                }
                if victime.is_some() {
                    r.part_victime = victime;
                    r.part_tiers = Some(tiers);
                }
            }
        }
        Short::Placed(role) if n == 2 => match (role, value(amounts[1], true)) {
            (Role::Victime, Some(b)) => {
                r.part_victime = Some(b);
                r.part_tiers = Some(0.0);
            }
            (Role::Tiers, Some(b)) => {
                r.part_tiers = Some(b);
                r.part_victime = Some(0.0);
            }
            _ => {}
        },
        Short::Guess(role) if n == 2 => match (role, value(amounts[1], true)) {
            (Role::Victime, Some(b)) => {
                r.part_victime = Some(b);
                r.part_tiers = Some(0.0);
            }
            (Role::Tiers, Some(b)) => r.part_tiers = Some(b),
            _ => {}
        },
        _ => {}
    }
    Some(r)
}

/// Where the cells of a row shorter than the header land: on the leftmost
/// columns, on the rightmost ones, or on the tiers payeurs from the left with
/// the victim's share last.
#[derive(Clone, Copy, PartialEq)]
enum Layout {
    Left,
    Right,
    VictimeLast,
}

#[derive(Clone, Copy)]
enum Short {
    Blind,
    Proven(Layout),
    /// The column the total row proves for this row alone.
    Placed(Role),
    Guess(Role),
}

fn layout_columns(layout: Layout, plan: &Plan, n: usize) -> Option<Vec<usize>> {
    let (w, k) = (plan.width, n.checked_sub(1)?);
    if n > w {
        return None;
    }
    if k == 0 {
        return Some(Vec::new());
    }
    Some(match layout {
        Layout::Left => (1..=k).collect(),
        Layout::Right => (w - k..w).collect(),
        Layout::VictimeLast => {
            let last = plan.victime.filter(|i| *i == w - 1)?;
            (1..k).chain(std::iter::once(last)).collect()
        }
    })
}

/// A layout holds when every column of the total row equals what the rows below
/// put in it. Sums of positive cells: a cell laid on the wrong column leaves a
/// hole in another, so the closing is a proof and not a coincidence.
fn layout_holds(folded: &[char], table: &Table, plan: &Plan, layout: Layout) -> Option<()> {
    let total_row = table
        .rows
        .iter()
        .find(|r| is_total(&text_of(folded, r.label.0, r.label.1)))?;
    if total_row.amounts.len() != plan.width {
        return None;
    }
    let mut sums = vec![0.0; plan.width];
    for row in &table.rows {
        if is_total(&text_of(folded, row.label.0, row.label.1)) {
            continue;
        }
        let n = row.amounts.len();
        let cols: Vec<usize> = if n == plan.width {
            (1..plan.width).collect()
        } else {
            layout_columns(layout, plan, n)?
        };
        for (k, j) in cols.iter().enumerate() {
            sums[*j] += value(row.amounts[k + 1], true)?;
        }
    }
    for (j, sum) in sums.iter().enumerate().skip(1) {
        if (value(total_row.amounts[j], true)? - sum).abs() >= 0.005 {
            return None;
        }
    }
    Some(())
}

/// When no single layout fits, the rows may still split between the two columns:
/// the total row names the caisse's share, and one subset of the two-cell rows
/// reaches it. Proved only when that subset is the only one.
fn short_split(
    folded: &[char],
    table: &Table,
    plan: &Plan,
) -> Option<std::collections::HashMap<usize, Role>> {
    let victime = plan.victime?;
    if plan.tiers.is_empty() {
        return None;
    }
    let total = table
        .rows
        .iter()
        .find(|r| is_total(&text_of(folded, r.label.0, r.label.1)))?;
    if total.amounts.len() != plan.width {
        return None;
    }
    let sum = |row: &Row, cols: &[usize]| -> Option<f64> {
        cols.iter()
            .map(|i| value(row.amounts[*i], true))
            .try_fold(0.0, |acc, v| v.map(|v| acc + v))
    };
    let (mut v_full, mut t_full) = (0.0, 0.0);
    let mut items: Vec<(usize, f64)> = Vec::new();
    for row in &table.rows {
        if is_total(&text_of(folded, row.label.0, row.label.1)) {
            continue;
        }
        match row.amounts.len() {
            n if n == plan.width => {
                v_full += sum(row, &[victime])?;
                t_full += sum(row, &plan.tiers)?;
            }
            2 => items.push((row.label.0, value(row.amounts[1], true)?)),
            1 => {}
            _ => return None,
        }
    }
    // 2^16 subsets at the worst, on a table of sixteen short rows.
    if items.is_empty() || items.len() > 16 {
        return None;
    }
    let t_need = sum(total, &plan.tiers)? - t_full;
    let v_need = sum(total, &[victime])? - v_full;
    let close = |a: f64, b: f64| (a - b).abs() < 0.005;
    if !close(items.iter().map(|(_, v)| v).sum::<f64>(), t_need + v_need) {
        return None;
    }
    let mut found: Option<u32> = None;
    for mask in 0u32..(1 << items.len()) {
        let picked: f64 = items
            .iter()
            .enumerate()
            .filter(|(i, _)| mask >> i & 1 == 1)
            .map(|(_, (_, v))| v)
            .sum();
        if close(picked, t_need) {
            if found.is_some() {
                return None;
            }
            found = Some(mask);
        }
    }
    let mask = found?;
    Some(
        items
            .iter()
            .enumerate()
            .map(|(i, (at, _))| {
                let role = if mask >> i & 1 == 1 {
                    Role::Tiers
                } else {
                    Role::Victime
                };
                (*at, role)
            })
            .collect(),
    )
}

/// A row shorter than the header does not say which column it skipped: the total
/// row answers, when a single layout closes it on the widths the table carries.
fn short_layout(folded: &[char], table: &Table, plan: &Plan) -> Option<Layout> {
    plan.victime?;
    let widths: Vec<usize> = table
        .rows
        .iter()
        .filter(|r| !is_total(&text_of(folded, r.label.0, r.label.1)))
        .map(|r| r.amounts.len())
        .filter(|n| *n > 1 && *n < plan.width)
        .collect();
    let mut kept: Vec<(Layout, Vec<Vec<usize>>)> = Vec::new();
    for layout in [Layout::Left, Layout::Right, Layout::VictimeLast] {
        if layout_holds(folded, table, plan, layout).is_none() {
            continue;
        }
        let Some(sig) = widths
            .iter()
            .map(|n| layout_columns(layout, plan, *n))
            .collect::<Option<Vec<Vec<usize>>>>()
        else {
            continue;
        };
        if !kept.iter().any(|(_, s)| *s == sig) {
            kept.push((layout, sig));
        }
    }
    match kept.len() {
        1 => Some(kept[0].0),
        _ => None,
    }
}

fn fill(target: &mut Option<f64>, v: Option<f64>) {
    if target.is_none() {
        *target = v;
    }
}

/// A cell that is a party's name and nothing else: it opens the block of rows
/// beneath it, which prices that person's own postes. The civility is required,
/// so that a debtor the source lost into its own cell opens nothing.
fn person_heading(folded: &[char], s: usize, e: usize) -> bool {
    let label = text_of(folded, s, e);
    e - s <= 40
        && label.contains('[')
        && ["m.", "mme", "mlle", "monsieur", "madame", "consorts"]
            .iter()
            .any(|c| label.starts_with(c))
}

/// Whose postes a block prices: the codes written under the name vote. A block
/// that names a person inside one of its own rows names several, and settles
/// nothing.
fn block_side(cues: &[(usize, usize, Cue)], folded: &[char], rows: &[Row]) -> Option<bool> {
    let (mut direct, mut indirect) = (0usize, 0usize);
    for row in rows {
        if text_of(folded, row.label.0, row.label.1).contains('[') {
            return None;
        }
        match code_of(cues, folded, row.label.0, row.label.1).and_then(vocabulary::direct_side) {
            Some(true) => direct += 1,
            Some(false) => indirect += 1,
            None => {}
        }
    }
    match (direct, indirect) {
        (d, i) if i > d => Some(false),
        (d, i) if d > i => Some(true),
        _ => None,
    }
}

/// Where each person block of the table opens, the first one at the first row.
fn person_blocks(folded: &[char], table: &Table) -> Vec<usize> {
    let mut starts: Vec<usize> = std::iter::once(0)
        .chain(
            table
                .extra
                .iter()
                .filter(|(s, e)| person_heading(folded, *s, *e))
                .filter_map(|(s, _)| table.rows.iter().position(|r| r.label.0 > *s)),
        )
        .collect();
    starts.dedup();
    starts
}

/// The run of rows a table prices under one name, and the side its codes vote.
#[derive(Clone, Copy)]
struct Block {
    from: usize,
    side: bool,
}

fn apply_table(
    scan: &DocScan,
    folded: &[char],
    cues: &[(usize, usize, Cue)],
    table: &Table,
    block: Block,
    victime: &mut Victime,
    mode: Mode,
) -> bool {
    let Block { from, side } = block;
    // The widest row, or the commonest one: a row detailing a poste in six
    // cells adds no column to the table, but reading it as the header's width
    // shifts every figure, so only the extended mode takes that risk.
    let mut widths = [0usize; 7];
    for row in &table.rows {
        if !is_total(&text_of(folded, row.label.0, row.label.1)) {
            widths[row.amounts.len().min(6)] += 1;
        }
    }
    let width = match mode {
        Mode::Sure => (1..=6).rev().find(|n| widths[*n] > 0).unwrap_or(0),
        Mode::Extended => (1..=6)
            .max_by_key(|n| (widths[*n], *n))
            .filter(|n| widths[*n] > 0)
            .unwrap_or(0),
    };
    let Some(plan) = plan_of(&columns(folded, &table.header, cues), width) else {
        return false;
    };
    if plan.cour.is_none() && plan.demande.is_none() && plan.offre.is_none() {
        return false;
    }
    // What follows the total row belongs to another block: the relatives'
    // sections, the provisions deducted, the payers' shares.
    let end = table.rows[from..]
        .iter()
        .position(|r| is_total(&text_of(folded, r.label.0, r.label.1)))
        .map_or(table.rows.len(), |i| from + i + 1);
    let rows = &table.rows[from..end];
    let mut poisoned: HashSet<&'static str> = HashSet::new();
    let mut coded: Vec<(usize, &'static str, &Row)> = Vec::new();
    for row in rows {
        let label = text_of(folded, row.label.0, row.label.1);
        if is_total(&label) || heading(cues, row.label.0, row.label.1) {
            continue;
        }
        if has(&label, &PROSE) {
            continue;
        }
        match code_of(cues, folded, row.label.0, row.label.1) {
            Some(code) => coded.push((coded.len(), code, row)),
            None => {
                poisoned.extend(ambiguous_codes(&label));
                // « ATPP » next to « ATP » : the table shortens two postes into
                // one abbreviation and its extension, and neither is settled.
                let bare = label.trim_end_matches([':', '.']).trim();
                if bare.len() <= 6 {
                    poisoned.extend(
                        SIGLES
                            .iter()
                            .filter(|(sigle, _)| bare.starts_with(sigle))
                            .map(|(_, code)| *code),
                    );
                }
                // « Frais divers » is where the nomenclature files what it does
                // not name: a priced row read as nothing may belong there.
                poisoned.insert("FD");
            }
        }
    }
    // Two rows on one code would have to be added up; the reading stops there.
    let mut seen: HashSet<&'static str> = HashSet::new();
    for (_, code, _) in &coded {
        if !seen.insert(code) {
            poisoned.insert(code);
        }
    }
    // « Frais divers », then its components priced on their own lines: the row
    // that carries the code holds only a part of the poste.
    for (s, e) in table
        .extra
        .iter()
        .filter(|(s, _)| *s < rows[rows.len() - 1].label.1)
    {
        if let Some(code) = code_of(cues, folded, *s, *e) {
            if seen.contains(code) {
                poisoned.insert(code);
            }
        }
    }
    // A block prices a poste of the side it is about, and nothing else.
    if !coded
        .iter()
        .any(|(_, code, _)| vocabulary::direct_side(code) == Some(side))
    {
        return false;
    }
    // Failing the proof, the extended mode reads the second cell of a two-cell
    // row as the column right after the court's own.
    let mut split = std::collections::HashMap::new();
    let short = match short_layout(folded, table, &plan) {
        Some(layout) => Short::Proven(layout),
        None => {
            split = short_split(folded, table, &plan).unwrap_or_default();
            match plan.second.filter(|_| mode == Mode::Extended) {
                Some(role) => Short::Guess(role),
                None => Short::Blind,
            }
        }
    };
    let column_sum: Option<f64> = rows
        .iter()
        .filter(|r| {
            let l = text_of(folded, r.label.0, r.label.1);
            !l.starts_with("total")
                && !l.starts_with("totaux")
                && !l.starts_with("sous")
                && !l.starts_with("soit")
                && !heading(cues, r.label.0, r.label.1)
        })
        .map(|r| value(r.amounts[0], true).filter(|_| r.amounts.len() <= plan.width))
        .try_fold(0.0, |acc, v| v.map(|v| acc + v));
    // A label that is itself an amount is a cell the source lost: the row above
    // has eaten the label of this one, and every column below is shifted.
    if rows.iter().any(|r| {
        let l = text_of(folded, r.label.0, r.label.1);
        amount_of(l.trim_start_matches(['-', '\u{2013}', '\u{2014}']).trim()).is_some()
    }) {
        return false;
    }
    let mut wrote = false;
    for (_, code, row) in &coded {
        if poisoned.contains(code) || vocabulary::direct_side(code) == Some(!side) || *code == "PR"
        {
            continue;
        }
        let placed = split
            .get(&row.label.0)
            .map_or(short, |role| Short::Placed(*role));
        let Some(read) = read_row(&plan, &row.amounts, placed, mode) else {
            continue;
        };
        let held = victime
            .postes
            .iter()
            .position(|p| p.poste.as_deref() == Some(*code));
        if let (Some(k), Some(a)) = (held, read.alloue) {
            if victime.postes[k]
                .montant_alloue
                .is_some_and(|m| (m - a).abs() >= 0.005)
            {
                continue;
            }
        }
        let k = match held {
            Some(k) => k,
            None => {
                if read.alloue.is_none() {
                    continue;
                }
                victime.postes.push(Poste {
                    poste: Some((*code).to_string()),
                    libelle_decision: Some(scan.verbatim(row.label.0, row.label.1)),
                    ..Default::default()
                });
                victime.postes.len() - 1
            }
        };
        let p = &mut victime.postes[k];
        fill(&mut p.montant_alloue, read.alloue);
        fill(&mut p.part_victime, read.part_victime);
        fill(&mut p.part_tiers_payeurs, read.part_tiers);
        fill(&mut p.montant_demande, read.demande);
        fill(&mut p.montant_offert, read.offre);
        fill(&mut p.alloue_premiere_instance, read.jugement);
        wrote = true;
    }
    if !wrote {
        return false;
    }
    for row in rows {
        if !is_total(&text_of(folded, row.label.0, row.label.1)) || row.amounts.len() != plan.width
        {
            continue;
        }
        let Some(read) = read_row(&plan, &row.amounts, short, mode) else {
            continue;
        };
        // The court's own total, only where the column it closes adds up to it.
        if !matches!((read.alloue, column_sum), (Some(t), Some(c)) if (t - c).abs() < 0.005) {
            continue;
        }
        fill(&mut victime.total_alloue, read.alloue);
        fill(&mut victime.net_victime, read.part_victime);
        fill(&mut victime.creance_tiers_payeurs, read.part_tiers);
    }
    true
}

pub fn apply(scan: &DocScan, x: &mut Extraction, mode: Mode) {
    let directs: Vec<usize> = x
        .quantum
        .victimes
        .iter()
        .enumerate()
        .filter(|(_, v)| v.role == "directe")
        .map(|(i, _)| i)
        .collect();
    // A recapitulation is a liquidation: when the dispositif has named no direct
    // victim, the table is the only place she appears, and a dispositif that
    // named the proches alone does not make her any less absent.
    let fresh = directs.is_empty();
    if directs.len() > 1 {
        return;
    }
    let folded: Vec<char> = scan.folded().chars().collect();
    let chars = scan.chars();
    let amounts = leftovers::amounts_in(chars, &folded);
    let segs = cells(chars, &folded, &amounts);
    let tables = tables_of(&segs, &folded);
    if tables.is_empty() {
        return;
    }
    let zoning = Zoning::of(&folded);
    let (dispositif, _) = super::dispositif_span(scan);
    let cues: Vec<(usize, usize, Cue)> = vocabulary::cues(scan.folded(), scan.byte2char(), false)
        .into_iter()
        .map(|c| (c.s, c.e, c.cue))
        .collect();
    let mut drafted = Victime {
        cle: format!("v{}", x.quantum.victimes.len() + 1),
        role: "directe".to_string(),
        ..Default::default()
    };
    let mut wrote = false;
    let mut proches: Vec<Victime> = Vec::new();
    {
        let victime = if fresh {
            &mut drafted
        } else {
            &mut x.quantum.victimes[directs[0]]
        };
        for table in tables
            .iter()
            .rev()
            .filter(|t| court_authored(&folded, t, &zoning, dispositif))
        {
            let starts = person_blocks(&folded, table);
            let whole = Block {
                from: 0,
                side: true,
            };
            wrote |= apply_table(scan, &folded, &cues, table, whole, victime, mode);
            // One table is found under as many starts as it has openings, and
            // its relatives are read once.
            if !proches.is_empty() {
                continue;
            }
            for (b, &from) in starts.iter().enumerate().skip(1) {
                let to = starts.get(b + 1).copied().unwrap_or(table.rows.len());
                let Some(side) = block_side(&cues, &folded, &table.rows[from..to]) else {
                    continue;
                };
                let mut proche = Victime {
                    role: if side { "directe" } else { "indirecte" }.to_string(),
                    ..Default::default()
                };
                let block = Block { from, side };
                if apply_table(scan, &folded, &cues, table, block, &mut proche, mode) {
                    proches.push(proche);
                }
            }
        }
    }
    if fresh && wrote {
        x.quantum.victimes.push(drafted);
    }
    for mut v in proches {
        v.cle = format!("v{}", x.quantum.victimes.len() + 1);
        x.quantum.victimes.push(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_is_an_amount_only_when_the_whole_of_it_is_one() {
        assert_eq!(amount_of("2.813,90 €"), Some(2813.90));
        assert_eq!(amount_of("40 000,00 €"), Some(40000.0));
        assert_eq!(amount_of("18 669, 36 euros"), Some(18669.36));
        assert_eq!(amount_of("3200"), Some(3200.0));
        assert_eq!(amount_of("25 %"), None);
        assert_eq!(amount_of("12 mai 2024"), None);
        assert_eq!(amount_of("1,5"), Some(1.5));
    }

    #[test]
    fn two_digits_behind_a_dot_are_cents_and_three_are_a_thousands_group() {
        assert_eq!(amount_of("768.90 \u{20ac}"), Some(768.90));
        assert_eq!(amount_of("171.857,90 \u{20ac}"), Some(171857.90));
        assert_eq!(amount_of("48.000 \u{20ac}"), Some(48000.0));
    }

    #[test]
    fn a_header_cell_that_settles_nothing_continues_the_column_on_its_left() {
        // A scan turns every newline into a space, so a header laid out one
        // cell per line arrives with the cells separated by two of them.
        let text = "Postes  de préjudice  Évaluation  du préjudice  Solde victime";
        let n = lj_extract::compiled::Norm::new(text);
        let folded: Vec<char> = n.folded.chars().collect();
        let header = segments(&n.chars);
        let cues: Vec<(usize, usize, Cue)> = vocabulary::cues(&n.folded, &n.byte2char, false)
            .into_iter()
            .map(|c| (c.s, c.e, c.cue))
            .collect();
        assert_eq!(
            columns(&folded, &header, &cues),
            vec![Role::Cour, Role::Victime]
        );
    }

    #[test]
    fn a_caisse_heads_its_column_and_does_not_close_the_header() {
        let text = "\u{c9}valuation du pr\u{e9}judice  Montant d\u{fb} \u{e0} la victime  Montant soumis au recours de la CPAM  Pr\u{e9}judices patrimoniaux  temporaires";
        let n = lj_extract::compiled::Norm::new(text);
        let folded: Vec<char> = n.folded.chars().collect();
        let header = segments(&n.chars);
        let cues: Vec<(usize, usize, Cue)> = vocabulary::cues(&n.folded, &n.byte2char, false)
            .into_iter()
            .map(|c| (c.s, c.e, c.cue))
            .collect();
        assert_eq!(
            columns(&folded, &header, &cues),
            vec![Role::Cour, Role::Victime, Role::Tiers]
        );
    }

    #[test]
    fn a_row_shorter_than_the_table_writes_the_share_only_where_the_total_places_it() {
        let plan = Plan {
            cour: Some(0),
            victime: Some(1),
            tiers: vec![2],
            demande: None,
            offre: None,
            jugement: None,
            second: Some(Role::Victime),
            width: 3,
        };
        let row = [Cell::Amount(2822.0), Cell::Amount(2822.0)];
        let r = read_row(&plan, &row, Short::Proven(Layout::Left), Mode::Sure).expect("read");
        assert_eq!(r.alloue, Some(2822.0));
        assert_eq!(r.part_victime, Some(2822.0));
        assert_eq!(r.part_tiers, Some(0.0));
        let r = read_row(&plan, &row, Short::Blind, Mode::Sure).expect("read");
        assert_eq!(r.alloue, Some(2822.0));
        assert_eq!(r.part_victime, None);
    }

    #[test]
    fn the_shares_are_written_only_when_they_close_on_the_award() {
        let plan = Plan {
            cour: Some(0),
            victime: Some(1),
            tiers: vec![2],
            demande: None,
            offre: None,
            jugement: None,
            second: Some(Role::Victime),
            width: 3,
        };
        let full = [
            Cell::Amount(2813.90),
            Cell::Amount(73.0),
            Cell::Amount(2740.90),
        ];
        let r = read_row(&plan, &full, Short::Blind, Mode::Sure).expect("read");
        assert_eq!(r.part_victime, Some(73.0));
        assert_eq!(r.part_tiers, Some(2740.90));
        let off = [Cell::Amount(2813.90), Cell::Amount(73.0), Cell::Amount(1.0)];
        let r = read_row(&plan, &off, Short::Blind, Mode::Sure).expect("read");
        assert_eq!(r.part_victime, None);
    }

    #[test]
    fn a_recapitulation_prices_the_postes_and_splits_the_shares() {
        let text = "COUR D'APPEL\nMOTIFS DE LA DECISION\nLe préjudice corporel de M. [A] [B] se récapitule comme suit :\n\
             Poste de préjudice  Montant  Créance victime  Créance CPAM  \
             Déficit fonctionnel permanent  27 600 €  27 600 €  \
             Souffrances endurées  8 000 €  8 000 €  \
             Dépenses de santé actuelles  2 813,90 €  73 €  2 740,90 €  \
             TOTAL  38 413,90 €  35 673 €  2 740,90 €\n\n\
             PAR CES MOTIFS\n\
             CONDAMNE la société X à payer à M. [A] [B] la somme de 27 600 euros au titre du déficit fonctionnel permanent ;";
        let x = super::super::dispositif::extract(&lj_extract::scan::scan(text), Mode::Sure);
        let v = &x.quantum.victimes[0];
        let of = |code: &str| {
            let p = v
                .postes
                .iter()
                .find(|p| p.poste.as_deref() == Some(code))
                .expect("poste");
            (p.montant_alloue, p.part_victime, p.part_tiers_payeurs)
        };
        assert_eq!(of("DFP"), (Some(27600.0), Some(27600.0), Some(0.0)));
        assert_eq!(of("SE"), (Some(8000.0), Some(8000.0), Some(0.0)));
        assert_eq!(of("DSA"), (Some(2813.90), Some(73.0), Some(2740.90)));
        assert_eq!(v.total_alloue, Some(38413.90));
        assert_eq!(v.net_victime, Some(35673.0));
        assert_eq!(v.creance_tiers_payeurs, Some(2740.90));
    }

    #[test]
    fn a_recapitulation_alone_in_the_dispositif_carries_the_direct_victim() {
        let text = "COUR D'APPEL\nMOTIFS DE LA DECISION\nSur la liquidation.\nPAR CES MOTIFS\n\
             La cour,\n\
             Fixe le pr\u{e9}judice corporel de M. [A] [B] comme suit :\n\
             Pr\u{e9}judices  \u{c9}valuation  Part de la CPAM  Part revenant \u{e0} M. [B]  \
             DSA  2 813,90 \u{20ac}  2 740,90 \u{20ac}  73 \u{20ac}  \
             SE  8 000 \u{20ac}  0 \u{20ac}  8 000 \u{20ac}  \
             DFP  27 600 \u{20ac}  0 \u{20ac}  27 600 \u{20ac}  \
             TOTAL  38 413,90 \u{20ac}  2 740,90 \u{20ac}  35 673 \u{20ac}\n\n\
             Condamne la soci\u{e9}t\u{e9} X aux d\u{e9}pens.";
        let x = super::super::dispositif::extract(&lj_extract::scan::scan(text), Mode::Sure);
        assert_eq!(x.quantum.victimes.len(), 1);
        let v = &x.quantum.victimes[0];
        assert_eq!(v.role, "directe");
        assert_eq!(v.postes.len(), 3);
        assert_eq!(v.total_alloue, Some(38413.90));
        assert_eq!(v.net_victime, Some(35673.0));
        assert_eq!(v.creance_tiers_payeurs, Some(2740.90));
    }

    #[test]
    fn the_total_row_names_which_short_rows_went_to_the_caisse() {
        let text = "COUR D'APPEL\nMOTIFS DE LA DECISION\nSur la liquidation.\nPAR CES MOTIFS\n\
             La cour,\n\
             Fixe le pr\u{e9}judice corporel de M. [A] [B] ainsi qu'il suit :\n\
             Nature du pr\u{e9}judice  \u{c9}valuation du pr\u{e9}judice  Cr\u{e9}ance de la CPAM  Montant allou\u{e9} \u{e0} la victime  \
             DSA  18 202,41 \u{20ac}  18 202,41 \u{20ac}  \
             FD  6 441,91 \u{20ac}  1 201,50 \u{20ac}  5 240,41 \u{20ac}  \
             DFT  2 803 \u{20ac}  2 803 \u{20ac}  \
             SE  7 000 \u{20ac}  7 000 \u{20ac}  \
             TOTAL  34 447,32 \u{20ac}  19 403,91 \u{20ac}  15 043,41 \u{20ac}\n\n\
             Condamne la soci\u{e9}t\u{e9} X aux d\u{e9}pens.";
        let x = super::super::dispositif::extract(&lj_extract::scan::scan(text), Mode::Sure);
        let v = &x.quantum.victimes[0];
        let of = |code: &str| {
            let p = v
                .postes
                .iter()
                .find(|p| p.poste.as_deref() == Some(code))
                .expect("poste");
            (p.part_victime, p.part_tiers_payeurs)
        };
        // No layout fits: the caisse took the first row and the victim the two
        // last, and only the total row says so.
        assert_eq!(of("DSA"), (Some(0.0), Some(18202.41)));
        assert_eq!(of("DFT"), (Some(2803.0), Some(0.0)));
        assert_eq!(of("SE"), (Some(7000.0), Some(0.0)));
    }

    #[test]
    fn a_ragged_table_lays_its_short_rows_where_the_total_closes() {
        let text = "COUR D'APPEL\nMOTIFS DE LA DECISION\nLe pr\u{e9}judice corporel de M. [A] [B] se r\u{e9}capitule comme suit :\n\
             Pr\u{e9}judices  Total  CPAM  Malakoff  Solde victime  \
             DSA  173.640,52 \u{20ac}  171.857,90 \u{20ac}  768.90 \u{20ac}  1.013,72 \u{20ac}  \
             FD  31.831,34 \u{20ac}  4.869,65 \u{20ac}  26.961,69 \u{20ac}  \
             IP  80.000,00 \u{20ac}  80.000,00 \u{20ac}  \
             TOTAL  285.471,86 \u{20ac}  176.727,55 \u{20ac}  768,90 \u{20ac}  107.975,41 \u{20ac}\n\n\
             PAR CES MOTIFS\n\
             CONDAMNE la soci\u{e9}t\u{e9} X \u{e0} payer \u{e0} M. [A] [B] la somme de 80 000 euros au titre de l'incidence professionnelle ;";
        let x = super::super::dispositif::extract(&lj_extract::scan::scan(text), Mode::Sure);
        let v = &x.quantum.victimes[0];
        let of = |code: &str| {
            let p = v
                .postes
                .iter()
                .find(|p| p.poste.as_deref() == Some(code))
                .expect("poste");
            (p.montant_alloue, p.part_victime, p.part_tiers_payeurs)
        };
        // The two-cell row leaves the caisses aside, the three-cell one skips
        // the second caisse: only the total row says so.
        assert_eq!(of("IP"), (Some(80000.0), Some(80000.0), Some(0.0)));
        assert_eq!(of("FD"), (Some(31831.34), Some(26961.69), Some(4869.65)));
        assert_eq!(of("DSA"), (Some(173640.52), Some(1013.72), Some(172626.80)));
    }
    #[test]
    fn a_bare_cell_opens_a_person_block_only_under_a_civility() {
        let cell = |t: &str| {
            let f: Vec<char> = t.chars().collect();
            person_heading(&f, 0, f.len())
        };
        assert!(cell("mme [d] [o]"));
        assert!(cell("consorts [o]"));
        // The debtor column the source lost into a cell of its own.
        assert!(!cell("docteur [b]"));
        assert!(!cell("l'oniam"));
        assert!(!cell("prejudice patrimonial avant consolidation"));
    }
}
