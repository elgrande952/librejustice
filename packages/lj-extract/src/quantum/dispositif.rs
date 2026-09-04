//! Sure reading of the dispositif: a poste takes an amount only through the
//! written link « X au titre de P » or « P : X », a victim is a named natural
//! person, and every other amount is an obligation with its best cause.
use super::leftovers::{self, Amount};
use super::vocabulary::{self, Cue, CueTok};
use super::{Composante, Lecture, Poste, Quantum, Victime};
use crate::scan::DocScan;

/// What a final record owes to an amount the dispositif writes.
#[derive(Clone, Debug, PartialEq)]
pub enum Nature {
    /// An amount of a poste of a victim, in whichever field the record settles.
    Poste { cle: Option<String>, code: String },
    /// An indemnity paid to a victim whose poste the text does not name.
    Indemnite { cle: Option<String> },
    /// What a victim receives in all.
    Net { cle: String },
    /// A total the court writes.
    Total { cle: Option<String> },
    /// Not an indemnity of a victim, with its cause.
    HorsRecord(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Obligation {
    pub montant: f64,
    pub s: usize,
    pub e: usize,
    pub nature: Nature,
}

#[derive(Debug, Default)]
pub struct Extraction {
    pub quantum: Quantum,
    pub obligations: Vec<Obligation>,
}

#[derive(Clone, Copy)]
enum Event {
    Amount(Amount),
    Cue(CueTok),
}

impl Event {
    fn pos(&self) -> usize {
        match self {
            Event::Amount(a) => a.start,
            Event::Cue(c) => c.s,
        }
    }

    fn end(&self) -> usize {
        match self {
            Event::Amount(a) => a.end,
            Event::Cue(c) => c.e,
        }
    }

    fn cue(&self) -> Option<Cue> {
        match self {
            Event::Cue(c) => Some(c.cue),
            Event::Amount(_) => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Condemn,
    Fix,
    Reject,
    Recital,
    Other,
}

const RECITAL_HEADS: [&str; 8] = [
    "confirme", "infirme", "reforme", "annule", "casse", "sauf", "emendant", "emende",
];
const RECITAL_BEFORE: [&str; 4] = ["qu'il", "qu'elle", "qui a", "qui ont"];
const CIVILITIES: [&str; 7] = [
    "monsieur ",
    "madame ",
    "mademoiselle ",
    "mme ",
    "mlle ",
    "m. ",
    "m ",
];
const REPRESENTING: [&str; 5] = [
    "qualite",
    "agissant",
    "au nom de",
    "pour le compte de",
    "representant legal",
];
const NAME_STOPS: [&str; 22] = [
    ",",
    ":",
    ";",
    " la somme",
    " les sommes",
    " une somme",
    " celle de",
    " en deniers",
    " a titre",
    " au titre",
    " en reparation",
    " sur le fondement",
    " le montant",
    " les montants",
    " comme suit",
    " ainsi",
    " consecutif",
    " resultant",
    " suite",
    " du fait",
    " victime",
    " de la maniere",
];
const RICOCHET_WORDS: [&str; 11] = [
    "ricochet",
    "ayant droit",
    "ayants droit",
    "victime indirecte",
    "victimes indirectes",
    "en sa qualite de",
    "ses enfants",
    "ses parents",
    "sa mere",
    "son pere",
    "ses proches",
];
const SHARE_WORDS: [&str; 6] = [
    "hors creance",
    "a charge",
    "revenant a",
    "part de la victime",
    "apres deduction",
    "deduction faite",
];
const GLOBAL: [&str; 3] = [
    "prejudice corporel",
    "prejudices corporels",
    "ses prejudices",
];
const GLOBAL_TERMINATED: [&str; 2] = ["son prejudice", "leur prejudice"];
/// Wordings that make the sum the whole of what the victim receives.
const GLOBAL_STRONG: [&str; 7] = [
    "l'ensemble de ses prejudices",
    "l'ensemble de son prejudice",
    "son entier prejudice",
    "l'entier prejudice",
    "l'integralite de son prejudice",
    "toutes causes de prejudice",
    "tous prejudices confondus",
];
const TERMINATORS: [&str; 16] = [
    ",",
    ";",
    ".",
    ")",
    " avec",
    " outre",
    " en deniers",
    " apres",
    " deduction",
    " sous",
    " sauf",
    " et ",
    " a la suite",
    " resultant",
    " consecutif",
    " subi",
];
const CONNECTORS: &[&str] = &[
    "euros",
    "euro",
    "eur",
    "brut",
    "bruts",
    "net",
    "nets",
    "ttc",
    "ht",
    "hors",
    "taxes",
    "en",
    "capital",
    "deniers",
    "ou",
    "quittances",
    "chacun",
    "chacune",
    "principal",
];
const ARTICLES: &[&str] = &[
    "le",
    "la",
    "les",
    "l",
    "son",
    "sa",
    "ses",
    "leur",
    "leurs",
    "du",
    "de",
    "des",
    "d",
    "un",
    "une",
    "poste",
    "chef",
    "prejudice",
    "indemnisation",
    "reparation",
];
const AFTER_LABEL: &[&str] = &[
    "a",
    "la",
    "le",
    "les",
    "l",
    "de",
    "d",
    "du",
    "des",
    "somme",
    "sommes",
    "hauteur",
    "pour",
    "un",
    "une",
    "montant",
    "euros",
    "euro",
    "brut",
    "net",
    "evalue",
    "evaluee",
    "evalues",
    "evaluees",
    "fixe",
    "fixee",
    "fixes",
    "fixees",
    "liquide",
    "liquidee",
    "chiffre",
    "chiffree",
    "arrete",
    "arretee",
    "retenu",
    "retenue",
    "restant",
    "restants",
    "restante",
    "restantes",
    "restee",
    "restees",
    "reste",
    "restes",
    "charge",
    "victime",
    "en",
    "capital",
    "sa",
    "son",
    "hors",
    "creance",
    "creances",
    "tiers",
    "payeurs",
    "part",
    "revenant",
    "apres",
    "deduction",
    "faite",
    "s",
    "elevant",
    "avant",
    "consolidation",
    "temporaire",
    "temporaires",
    "permanent",
    "permanents",
    "permanente",
    "permanentes",
    "futur",
    "futurs",
    "future",
    "futures",
    "echu",
    "echus",
    "echue",
    "echues",
    "echoir",
    "definitif",
    "definitifs",
    "definitive",
];
const TEMPORARY: [&str; 3] = [
    "temporaire",
    "avant consolidation",
    "avant la consolidation",
];
const PERMANENT: [&str; 5] = [
    "apres consolidation",
    "apres la consolidation",
    "futur",
    "permanent",
    "definitif",
];

/// A label qualified in the text after it: temporary costs of a permanent poste
/// are frais divers, permanent frais divers are another poste.
fn qualified(code: &'static str, around: &str) -> Option<&'static str> {
    let temporary = TEMPORARY.iter().any(|w| around.contains(w));
    let permanent = PERMANENT.iter().any(|w| around.contains(w));
    match (code, temporary, permanent) {
        ("FVA" | "FLA" | "DSF" | "ATP", true, false) => Some("FD"),
        ("FD", _, true) => None,
        (c, _, _) => Some(c),
    }
}
const BEFORE_LABEL: &[&str] = &["euros", "euro", "le", "la", "les", "l", "son", "sa", "ses"];
const AFTER_SUM: &[&str] = &[
    "de",
    "d",
    "i",
    "ii",
    "iii",
    "iv",
    "restant",
    "du",
    "general",
    "generale",
    "un",
    "montant",
    "euros",
    "totale",
    "globale",
    "la",
    "somme",
    "en",
    "principal",
    "capital",
];
/// A list separator between a label and an amount: the amount belongs to the next line.
const SEPARATORS: [&str; 5] = [";", " -", " *", " •", "|"];
const REDUCTION_WORDS: [&str; 10] = [
    "partage",
    "perte de chance",
    "reduction",
    "reduit",
    "limit",
    "faute",
    "concurrence",
    "responsabilit",
    "droit a",
    "imputab",
];
const REDUCTION_NOISE: [&str; 11] = [
    "dfp",
    "deficit",
    "execution provisoire",
    "interet",
    "tva",
    "horaire",
    "capitalis",
    "100 %",
    "100%",
    "aipp",
    "incapacite",
];
const FRACTIONS: [&str; 6] = [
    "de moitie",
    "la moitie",
    "un tiers",
    "deux tiers",
    "trois quarts",
    "un quart",
];

fn euros(a: &Amount) -> f64 {
    a.value as f64 / 1000.0
}

fn text_of(folded: &[char], s: usize, e: usize) -> String {
    folded[s.min(e)..e].iter().collect()
}

/// Does the text reduce a right to compensation: a percentage or a fraction
/// near a word of reduction, anywhere, since a missed motifs marker would
/// otherwise let a reduced amount pass as the full one.
fn reduction_cue(folded: &[char]) -> bool {
    let from = 0usize;
    let window =
        |i: usize, e: usize| text_of(folded, i.saturating_sub(80), (e + 80).min(folded.len()));
    let reducing = |w: &str| {
        REDUCTION_WORDS.iter().any(|x| w.contains(x))
            && !REDUCTION_NOISE.iter().any(|x| w.contains(x))
    };
    for i in from..folded.len() {
        if folded[i] != '%' {
            continue;
        }
        let mut j = i;
        while j > 0 && folded[j - 1] == ' ' {
            j -= 1;
        }
        if j > 0 && folded[j - 1].is_ascii_digit() && reducing(&window(i, i + 1)) {
            return true;
        }
    }
    let tail: String = folded[from..].iter().collect();
    FRACTIONS.iter().any(|f| {
        tail.match_indices(f).any(|(b, _)| {
            reducing(&window(
                from + tail[..b].chars().count(),
                from + tail[..b].chars().count() + f.chars().count(),
            ))
        })
    })
}

/// A « 0 » the label is priced at, which the amount lexer never emits.
fn zero_after(folded: &[char], from: usize, limit: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i < limit && matches!(folded[i], ' ' | ':' | '-' | '*' | '\'') {
        i += 1;
    }
    if i >= limit || folded[i] != '0' {
        return None;
    }
    let mut e = i + 1;
    if e < limit && matches!(folded[e], ',' | '.') && folded.get(e + 1) == Some(&'0') {
        e += 1;
        while e < limit && folded[e] == '0' {
            e += 1;
        }
    }
    let next = folded.get(e);
    if next.is_some_and(|c| c.is_ascii_digit() || matches!(c, ',' | '.')) {
        return None;
    }
    Some((i, e))
}

/// A chef opens on a capital, or on a list mark; a verb after « qu'il » or in
/// the past participle recites the judgment and opens nothing.
fn head_kind(chars: &[char], folded: &[char], p: usize) -> Option<Kind> {
    let word: String = folded[p..]
        .iter()
        .take_while(|c| c.is_alphabetic())
        .collect();
    let kind = match word.as_str() {
        w if RECITAL_HEADS.contains(&w) => Kind::Recital,
        "condamne" => Kind::Condemn,
        "fixe" | "evalue" | "liquide" => Kind::Fix,
        "deboute" | "rejette" => Kind::Reject,
        _ => Kind::Other,
    };
    let last = chars[p + word.chars().count() - 1];
    if matches!(last, 'é' | 'É') && kind != Kind::Recital {
        return None;
    }
    let before = text_of(folded, p.saturating_sub(40), p);
    if RECITAL_BEFORE.iter().any(|w| before.contains(w)) {
        return None;
    }
    if chars[p].is_uppercase() {
        return Some(kind);
    }
    let prev = folded[..p].iter().rev().find(|c| **c != ' ');
    matches!(
        prev,
        None | Some('-' | ';' | '.' | ':' | '*' | '•' | '>' | '|')
    )
    .then_some(kind)
}

enum Person {
    Single(String),
    Several,
    Organism,
}

/// The party a payment names, folded, civility dropped: a natural person, or not.
fn person(raw: &str) -> Person {
    let mut name = raw.trim();
    for stop in NAME_STOPS {
        if let Some(i) = name.find(stop) {
            name = name[..i].trim();
        }
    }
    if REPRESENTING.iter().any(|w| raw.contains(w))
        || name.contains(" et ")
        || name.starts_with("les ")
        || name.starts_with("aux ")
        || name.starts_with("chacun")
    {
        return Person::Several;
    }
    let mut civil = false;
    for civ in CIVILITIES {
        if let Some(rest) = name.strip_prefix(civ) {
            name = rest.trim();
            civil = true;
            break;
        }
    }
    if !civil && !name.starts_with('[') {
        return Person::Organism;
    }
    let name = name.trim_matches(|c: char| !c.is_alphanumeric() && c != '[' && c != ']');
    if name.len() < 2 {
        Person::Organism
    } else {
        Person::Single(name.to_string())
    }
}

/// The victim a fixing clause names in its opening: « de M. X », « subis par Mme Y ».
fn fixed_person(intro: &str) -> Option<Person> {
    let mut best: Option<usize> = None;
    for lead in ["de ", "par ", "a "] {
        for civ in CIVILITIES.iter().copied().chain(["["]) {
            let pat = format!("{lead}{civ}");
            if let Some(i) = intro.find(&pat) {
                let at = i + lead.len();
                if best.is_none_or(|b| at < b) {
                    best = Some(at);
                }
            }
        }
    }
    best.map(|at| person(&intro[at..]))
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Pv,
    Alloue,
    None,
}

struct Line {
    code: &'static str,
    label: String,
    amount: f64,
    field: Field,
}

#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Draft(usize),
    List(usize),
    None,
}

enum ObKind {
    Poste(String),
    Indemnite,
    Net,
    Total,
    Hors(&'static str),
}

struct Ob {
    montant: f64,
    s: usize,
    e: usize,
    kind: ObKind,
    owner: Owner,
}

#[derive(Default)]
struct Draft {
    name: String,
    ricochet: bool,
    /// The heirs claim the deceased's own damages: the victim is the direct one.
    successoral: bool,
    lines: Vec<Line>,
    nets: Vec<f64>,
    /// Codes a line of the text could merge into without the vocabulary settling it.
    unsure: Vec<&'static str>,
}

struct FixList {
    person: Option<Person>,
    ricochet: bool,
    successoral: bool,
    lines: Vec<Line>,
    unsure: Vec<&'static str>,
}

enum Link {
    Poste {
        code: &'static str,
        s: usize,
        e: usize,
        share: bool,
    },
    Out(&'static str),
    Global {
        strong: bool,
    },
    None,
}

struct Composer<'a> {
    scan: &'a DocScan,
    folded: Vec<char>,
    reduced: bool,
    drafts: Vec<Draft>,
    lists: Vec<FixList>,
    obs: Vec<Ob>,
    /// Codes a confirmed chief of the judgment names: the appeal may add to them.
    confirmed: Vec<&'static str>,
    /// The dispositif confirms part of the judgment: what the header recites of
    /// it stays awarded, and the appeal's own lines may add to the same code.
    confirms: bool,
}

impl Composer<'_> {
    /// « M. [G] [K] » and « M. [K] » are one party when no other draft shares the
    /// shorter name.
    fn draft(&mut self, name: &str) -> usize {
        if let Some(i) = self.drafts.iter().position(|d| d.name == name) {
            return i;
        }
        let tokens: Vec<&str> = name.split(' ').collect();
        let nested: Vec<usize> = self
            .drafts
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                let theirs: Vec<&str> = d.name.split(' ').collect();
                !d.name.is_empty()
                    && (tokens.iter().all(|t| theirs.contains(t))
                        || theirs.iter().all(|t| tokens.contains(t)))
            })
            .map(|(i, _)| i)
            .collect();
        if let [i] = nested[..] {
            return i;
        }
        self.drafts.push(Draft {
            name: name.to_string(),
            ..Default::default()
        });
        self.drafts.len() - 1
    }

    fn text(&self, s: usize, e: usize) -> String {
        text_of(&self.folded, s, e)
    }

    /// Only allowed words between `s` and `e`, parentheses skipped, no digit.
    fn gap_ok(&self, s: usize, e: usize, allowed: &[&str]) -> bool {
        if e < s || e - s > 80 {
            return false;
        }
        let mut depth = 0usize;
        let mut word = String::new();
        for &c in &self.folded[s..e] {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth > 0 => {}
                c if c.is_alphanumeric() => word.push(c),
                _ => {
                    if !word.is_empty() && !allowed.contains(&word.as_str()) {
                        return false;
                    }
                    word.clear();
                }
            }
        }
        word.is_empty() || allowed.contains(&word.as_str())
    }

    /// `Some(strong)` when the sum repairs the prejudice as a whole rather than a poste.
    fn global_after(&self, from: usize) -> Option<bool> {
        let text = self.text(from, (from + 90).min(self.folded.len()));
        let candidates = GLOBAL_STRONG
            .iter()
            .map(|p| (*p, false, true))
            .chain(GLOBAL.iter().map(|p| (*p, false, false)))
            .chain(GLOBAL_TERMINATED.iter().map(|p| (*p, true, false)));
        for (phrase, terminated, strong) in candidates {
            let Some(b) = text.find(phrase) else { continue };
            let k = text[..b].chars().count();
            if k > 40 || !self.gap_ok(from, from + k, ARTICLES) {
                continue;
            }
            let rest = &text[b + phrase.len()..];
            if !terminated
                || rest.trim().is_empty()
                || TERMINATORS.iter().any(|t| rest.starts_with(t))
            {
                return Some(strong);
            }
        }
        None
    }

    /// Codes the clause leaves unsettled: a label with no amount, or a family word
    /// (« tierce personne », « frais ») outside every label.
    fn unsure_codes(
        &self,
        ev: &[Event],
        s: usize,
        e: usize,
        linked: &[usize],
    ) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for x in ev {
            let Event::Cue(c) = x else { continue };
            match c.cue {
                Cue::Poste(code) => {
                    spans.push((c.s, c.e));
                    if !linked.contains(&c.s) {
                        out.push(code);
                    }
                }
                Cue::OutOfScope(_) => spans.push((c.s, c.e)),
                _ => {}
            }
        }
        let text = self.text(s, e);
        for (word, codes) in vocabulary::AMBIGUOUS {
            let loose = text.match_indices(word).any(|(b, _)| {
                let at = s + text[..b].chars().count();
                !spans.iter().any(|(ps, pe)| at >= *ps && at < *pe)
            });
            if loose {
                out.extend(codes.iter().copied());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// « X au titre de P » : the amount, then the link word, then the label.
    fn forward(&self, ev: &[Event], i: usize) -> Link {
        let Event::Amount(a) = ev[i] else {
            return Link::None;
        };
        let Some(Event::Cue(u)) = ev.get(i + 1) else {
            return Link::None;
        };
        if u.cue != Cue::Under || !self.gap_ok(a.end, u.s, CONNECTORS) {
            return Link::None;
        }
        match ev.get(i + 2) {
            Some(Event::Cue(p)) if self.gap_ok(u.e, p.s, ARTICLES) => match p.cue {
                Cue::Poste(code) => {
                    let until = ev.get(i + 3).map_or(p.e + 40, |n| n.pos()).min(p.e + 40);
                    let Some(code) = qualified(code, &self.text(p.e, until.min(self.folded.len())))
                    else {
                        return Link::None;
                    };
                    return Link::Poste {
                        code,
                        s: p.s,
                        e: p.e,
                        share: false,
                    };
                }
                Cue::OutOfScope(cause) => return Link::Out(cause),
                _ => {}
            },
            _ => {}
        }
        match self.global_after(u.e) {
            Some(strong) => Link::Global { strong },
            None => Link::None,
        }
    }

    /// « P : X » : the label, then the amount, nothing else between.
    fn backward(&self, ev: &[Event], i: usize) -> Link {
        let Event::Amount(a) = ev[i] else {
            return Link::None;
        };
        if i == 0 {
            return Link::None;
        }
        let Event::Cue(p) = ev[i - 1] else {
            return Link::None;
        };
        if !self.gap_ok(p.e, a.start, AFTER_LABEL) {
            return Link::None;
        }
        let gap = self.text(p.e, a.start);
        if SEPARATORS.iter().any(|sep| gap.contains(sep)) {
            return Link::None;
        }
        let code = match p.cue {
            Cue::Poste(code) => code,
            Cue::OutOfScope(cause) => return Link::Out(cause),
            _ => return Link::None,
        };
        if i >= 2 && matches!(ev[i - 2].cue(), Some(Cue::Poste(_))) {
            return Link::None;
        }
        let Some(code) = qualified(code, &gap) else {
            return Link::None;
        };
        Link::Poste {
            code,
            s: p.s,
            e: p.e,
            share: SHARE_WORDS.iter().any(|w| gap.contains(w)),
        }
    }

    fn link(&self, ev: &[Event], i: usize, fixing: bool) -> Link {
        let Event::Amount(a) = ev[i] else {
            return Link::None;
        };
        let forward = self.forward(ev, i);
        let backward = self.backward(ev, i);
        match (&forward, &backward) {
            (Link::Poste { .. }, Link::Poste { .. }) => return Link::None,
            (Link::None, Link::Poste { .. } | Link::Out(_)) => return backward,
            (Link::None, _) => {}
            _ => return forward,
        }
        if fixing {
            if let Some(Event::Cue(p)) = ev.get(i + 1) {
                if let Cue::Poste(code) = p.cue {
                    let gap = self.text(a.end, p.s);
                    if self.gap_ok(a.end, p.s, BEFORE_LABEL)
                        && !gap.contains([';', ',', '.', ':'])
                        && gap
                            .split(|c: char| !c.is_alphanumeric())
                            .any(|w| ARTICLES.contains(&w) && w != "euros")
                    {
                        return Link::Poste {
                            code,
                            s: p.s,
                            e: p.e,
                            share: false,
                        };
                    }
                }
            }
        }
        Link::None
    }

    fn ob(&mut self, a: &Amount, kind: ObKind, owner: Owner) {
        self.obs.push(Ob {
            montant: euros(a),
            s: a.start,
            e: a.end,
            kind,
            owner,
        });
    }

    fn clause_cause(ev: &[Event]) -> &'static str {
        ev.iter()
            .find_map(|e| match e.cue() {
                Some(Cue::OutOfScope(c)) => Some(c),
                _ => None,
            })
            .unwrap_or("autre")
    }

    fn all_hors(&mut self, ev: &[Event], cause: &'static str) {
        for e in ev {
            if let Event::Amount(a) = e {
                self.ob(a, ObKind::Hors(cause), Owner::None);
            }
        }
    }

    fn payment(&mut self, ev: &[Event], s: usize, e: usize) {
        let Some(pi) = ev.iter().position(|x| x.cue() == Some(Cue::PayTo)) else {
            let cause = Self::clause_cause(ev);
            return self.all_hors(ev, cause);
        };
        let pay = ev[pi].end();
        let stop = ev[pi + 1..]
            .iter()
            .find(|x| {
                matches!(x, Event::Amount(_)) || matches!(x.cue(), Some(Cue::Sum | Cue::Total))
            })
            .map_or(e, Event::pos);
        let raw = self.text(pay, stop);
        let owner = match person(&raw) {
            Person::Single(name) => Owner::Draft(self.draft(&name)),
            Person::Several => Owner::None,
            Person::Organism => {
                let cause = match Self::clause_cause(ev) {
                    "autre" => "tiers_payeur",
                    c => c,
                };
                return self.all_hors(ev, cause);
            }
        };
        let text = self.text(s, e);
        if let Owner::Draft(d) = owner {
            self.drafts[d].ricochet |= RICOCHET_WORDS.iter().any(|w| text.contains(w));
            self.drafts[d].successoral |= text.contains("successoral");
        }
        let has_provision = ev
            .iter()
            .any(|x| x.cue() == Some(Cue::OutOfScope("provision")));
        let total = ev
            .iter()
            .position(|x| x.pos() >= pay && matches!(x.cue(), Some(Cue::Sum | Cue::Total)))
            .and_then(|si| {
                let ti = si + 1;
                match ev.get(ti) {
                    Some(Event::Amount(t)) if self.gap_ok(ev[si].end(), t.start, AFTER_SUM) => {
                        Some(ti)
                    }
                    _ => None,
                }
            });
        let total_after_total_cue =
            total.is_some_and(|ti| ti > 0 && ev[ti - 1].cue() == Some(Cue::Total));
        let mut linked_after_including = false;
        let mut including_seen = false;
        let mut linked: Vec<usize> = Vec::new();
        for i in 0..ev.len() {
            let Event::Amount(a) = ev[i] else {
                if ev[i].cue() == Some(Cue::Including) {
                    including_seen = true;
                }
                continue;
            };
            if a.start < pay {
                let cause = Self::clause_cause(ev);
                self.ob(&a, ObKind::Hors(cause), Owner::None);
                continue;
            }
            match self.link(ev, i, false) {
                Link::Poste { code, s, e, .. } => {
                    if including_seen {
                        linked_after_including = true;
                    }
                    linked.push(s);
                    let columns = matches!(ev.get(i + 1), Some(Event::Amount(_)));
                    let label = self.scan.verbatim(s, e);
                    if let Owner::Draft(d) = owner {
                        self.drafts[d].lines.push(Line {
                            code,
                            label,
                            amount: euros(&a),
                            field: if columns { Field::None } else { Field::Pv },
                        });
                    }
                    self.ob(&a, ObKind::Poste(code.to_string()), owner);
                }
                Link::Out(cause) => self.ob(&a, ObKind::Hors(cause), Owner::None),
                Link::Global { strong: true } if Some(i) == total && !has_provision => {
                    self.ob(&a, ObKind::Net, owner)
                }
                Link::Global { .. } | Link::None if Some(i) == total => {
                    self.ob(&a, ObKind::Indemnite, owner);
                }
                Link::Global { .. } | Link::None => self.ob(&a, ObKind::Indemnite, owner),
            }
        }
        if let (Some(ti), Owner::Draft(d)) = (total, owner) {
            let Event::Amount(t) = ev[ti] else { return };
            let ob = self.obs.iter_mut().rev().find(|o| o.s == t.start);
            if let Some(o) = ob {
                if matches!(o.kind, ObKind::Indemnite)
                    && (linked_after_including || total_after_total_cue)
                    && !has_provision
                {
                    o.kind = ObKind::Net;
                }
                if matches!(o.kind, ObKind::Net) {
                    self.drafts[d].nets.push(euros(&t));
                }
            }
        }
        if let Owner::Draft(d) = owner {
            let unsure = self.unsure_codes(ev, pay, e, &linked);
            self.drafts[d].unsure.extend(unsure);
        }
    }

    fn fixing(&mut self, ev: &[Event], s: usize, e: usize) {
        let first_label = ev
            .iter()
            .find(|x| matches!(x.cue(), Some(Cue::Poste(_))))
            .map_or(e, Event::pos);
        let intro = self.text(s, first_label.min(s + 600));
        let ricochet = RICOCHET_WORDS.iter().any(|w| intro.contains(w));
        let person = if first_label > s + 600 {
            Some(Person::Several)
        } else {
            match fixed_person(&intro) {
                None if ricochet || intro.contains("chacun") || intro.contains("epoux") => {
                    Some(Person::Several)
                }
                p => p,
            }
        };
        let share = SHARE_WORDS.iter().any(|w| intro.contains(w));
        let provisional = ev
            .iter()
            .any(|x| x.cue() == Some(Cue::OutOfScope("provision")));
        let li = self.lists.len();
        self.lists.push(FixList {
            person,
            ricochet,
            successoral: self.text(s, e).contains("successoral"),
            lines: Vec::new(),
            unsure: Vec::new(),
        });
        let owner = Owner::List(li);
        let mut row: Option<&'static str> = None;
        let mut linked: Vec<usize> = Vec::new();
        for i in 0..ev.len() {
            if let Some(Cue::Poste(code)) = ev[i].cue() {
                row = Some(code);
            }
            let Event::Amount(a) = ev[i] else { continue };
            match self.link(ev, i, true) {
                Link::Poste {
                    code,
                    s,
                    e,
                    share: row_share,
                } => {
                    let columns = matches!(ev.get(i + 1), Some(Event::Amount(_)));
                    let field = if columns || provisional {
                        Field::None
                    } else if share || row_share {
                        Field::Pv
                    } else if self.reduced || !vocabulary::extra_patrimonial(code) {
                        Field::None
                    } else {
                        Field::Alloue
                    };
                    linked.push(s);
                    let label = self.scan.verbatim(s, e);
                    self.lists[li].lines.push(Line {
                        code,
                        label,
                        amount: euros(&a),
                        field,
                    });
                    row = Some(code);
                    self.ob(&a, ObKind::Poste(code.to_string()), owner);
                }
                Link::Out(cause) => self.ob(&a, ObKind::Hors(cause), Owner::None),
                Link::Global { .. } | Link::None => {
                    let after_total = i > 0
                        && matches!(ev[i - 1].cue(), Some(Cue::Total | Cue::Sum))
                        && self.gap_ok(ev[i - 1].end(), a.start, AFTER_LABEL);
                    match row {
                        _ if after_total => self.ob(&a, ObKind::Total, owner),
                        Some(code) => self.ob(&a, ObKind::Poste(code.to_string()), owner),
                        None => self.ob(&a, ObKind::Indemnite, owner),
                    }
                }
            }
        }
        let unsure = self.unsure_codes(ev, s, e, &linked);
        self.lists[li].unsure = unsure;
    }
}

/// `Some(true)` direct, `Some(false)` ricochet, `None` when neither the codes
/// nor the wording settle it.
fn role_of(lines: &[Line], ricochet: bool, successoral: bool) -> Option<bool> {
    let votes: Vec<bool> = lines
        .iter()
        .filter_map(|l| vocabulary::direct_side(l.code))
        .collect();
    if votes.iter().any(|v| *v) || successoral {
        Some(true)
    } else if !votes.is_empty() || ricochet {
        Some(false)
    } else {
        None
    }
}

/// The record and the obligations the dispositif alone fixes.
pub fn extract(scan: &DocScan) -> Extraction {
    let (ds, de) = scan.dispositif_span();
    if ds >= de {
        return Extraction::default();
    }
    let folded: Vec<char> = scan.folded().chars().collect();
    let chars = scan.chars();
    let cues: Vec<CueTok> = vocabulary::cues(scan.folded(), scan.byte2char())
        .into_iter()
        .filter(|c| c.s >= ds && c.e <= de)
        .collect();
    let mut amounts: Vec<Amount> = leftovers::amounts_in(chars, &folded)
        .into_iter()
        .filter(|a| a.start >= ds && a.end <= de)
        .collect();
    for c in cues.iter().filter(|c| matches!(c.cue, Cue::Poste(_))) {
        if let Some((s, e)) = zero_after(&folded, c.e, de) {
            if !amounts.iter().any(|a| a.start == s) {
                amounts.push(Amount {
                    value: 0,
                    start: s,
                    end: e,
                });
            }
        }
    }
    let mut events: Vec<Event> = amounts
        .into_iter()
        .map(Event::Amount)
        .chain(cues.iter().copied().map(Event::Cue))
        .collect();
    events.sort_by_key(Event::pos);

    let mut heads: Vec<(usize, Kind)> = scan
        .dispositif_clause_starts()
        .into_iter()
        .chain(
            cues.iter()
                .filter(|c| matches!(c.cue, Cue::Condemn | Cue::Fix | Cue::Reject))
                .map(|c| c.s),
        )
        .filter_map(|p| head_kind(chars, &folded, p).map(|k| (p, k)))
        .collect();
    heads.push((ds, Kind::Other));
    heads.sort_by_key(|h| h.0);
    heads.dedup_by_key(|h| h.0);

    let mut cp = Composer {
        scan,
        folded,
        reduced: false,
        drafts: Vec::new(),
        lists: Vec::new(),
        obs: Vec::new(),
        confirmed: Vec::new(),
        confirms: false,
    };
    cp.reduced = reduction_cue(&cp.folded);
    let header_end = scan.motifs_start().min(ds);
    let header: Vec<Event> = leftovers::amounts_in(chars, &cp.folded)
        .into_iter()
        .filter(|a| a.end <= header_end)
        .map(Event::Amount)
        .chain(
            vocabulary::cues(scan.folded(), scan.byte2char())
                .into_iter()
                .filter(|c| c.e <= header_end)
                .map(Event::Cue),
        )
        .collect();
    let mut header = header;
    header.sort_by_key(Event::pos);
    let header_codes: Vec<&'static str> = (0..header.len())
        .filter_map(|i| match cp.link(&header, i, true) {
            Link::Poste { code, .. } => Some(code),
            _ => None,
        })
        .collect();
    for (n, &(s, kind)) in heads.iter().enumerate() {
        let e = heads.get(n + 1).map_or(de, |h| h.0);
        let clause: Vec<Event> = events
            .iter()
            .filter(|ev| ev.pos() >= s && ev.pos() < e)
            .copied()
            .collect();
        match kind {
            Kind::Condemn => cp.payment(&clause, s, e),
            Kind::Fix => cp.fixing(&clause, s, e),
            Kind::Reject => cp.all_hors(&clause, "pretention"),
            Kind::Recital => {
                if cp.text(s, s + 8) == "confirme" {
                    cp.confirms = true;
                    cp.confirmed
                        .extend(clause.iter().filter_map(|x| match x.cue() {
                            Some(Cue::Poste(code)) => Some(code),
                            _ => None,
                        }));
                }
                cp.all_hors(&clause, "rappel_du_jugement")
            }
            Kind::Other => {
                let cause = Composer::clause_cause(&clause);
                cp.all_hors(&clause, cause)
            }
        }
    }

    if cp.confirms {
        cp.confirmed.extend(header_codes);
    }
    let names: Vec<Option<String>> = cp
        .lists
        .iter()
        .map(|l| match &l.person {
            Some(Person::Single(name)) => Some(name.clone()),
            _ => None,
        })
        .collect();
    let mut list_draft: Vec<Option<usize>> = names
        .iter()
        .map(|n| n.as_deref().map(|name| cp.draft(name)))
        .collect();
    let unnamed: Vec<usize> = (0..cp.lists.len())
        .filter(|li| cp.lists[*li].person.is_none())
        .collect();
    for li in unnamed.iter().copied() {
        if cp.drafts.len() == 1 {
            list_draft[li] = Some(0);
        } else if cp.drafts.is_empty() && unnamed.len() == 1 {
            list_draft[li] = Some(cp.draft(""));
        }
    }
    for (li, target) in list_draft.iter().enumerate() {
        let Some(d) = *target else { continue };
        let list = &mut cp.lists[li];
        let lines = std::mem::take(&mut list.lines);
        let unsure = std::mem::take(&mut list.unsure);
        cp.drafts[d].ricochet |= list.ricochet;
        cp.drafts[d].successoral |= list.successoral;
        cp.drafts[d].lines.extend(lines);
        cp.drafts[d].unsure.extend(unsure);
    }

    let mut victimes = Vec::new();
    let mut draft_cle: Vec<Option<String>> = vec![None; cp.drafts.len()];
    for (i, d) in cp.drafts.iter().enumerate() {
        let Some(directe) = role_of(&d.lines, d.ricochet, d.successoral) else {
            continue;
        };
        let mut postes: Vec<Poste> = Vec::new();
        for line in &d.lines {
            let code = match (line.code, directe) {
                ("FD", false) => "FDV",
                ("PS", false) => "PEX",
                ("PR", true) => continue,
                (c, false) if vocabulary::direct_side(c) == Some(true) => continue,
                (c, _) => c,
            };
            let unsettled = |c: &str| d.unsure.contains(&c) || cp.confirmed.contains(&c);
            let field = if unsettled(line.code) || unsettled(code) {
                Field::None
            } else {
                line.field
            };
            let p = match postes.iter().position(|p| p.poste.as_deref() == Some(code)) {
                Some(k) => &mut postes[k],
                None => {
                    postes.push(Poste {
                        poste: Some(code.to_string()),
                        ..Default::default()
                    });
                    postes.last_mut().expect("just pushed")
                }
            };
            let label = line.label.to_lowercase();
            p.libelle_decision = Some(match &p.libelle_decision {
                Some(l) if l.contains(&label) => l.clone(),
                Some(l) => format!("{l} ; {label}"),
                None => label,
            });
            match field {
                Field::Pv => p.part_victime = Some(p.part_victime.unwrap_or(0.0) + line.amount),
                Field::Alloue => {
                    p.montant_alloue = Some(p.montant_alloue.unwrap_or(0.0) + line.amount);
                    p.composantes.push(Composante {
                        libelle: Some(line.label.to_lowercase()),
                        montant: Some(line.amount),
                        ..Default::default()
                    });
                }
                Field::None => {}
            }
        }
        for p in &mut postes {
            if p.composantes.len() < 2 {
                p.composantes.clear();
            }
            if let Some(v) = p.part_victime {
                p.part_victime = Some((v * 100.0).round() / 100.0);
            }
            if let Some(v) = p.montant_alloue {
                p.montant_alloue = Some((v * 100.0).round() / 100.0);
            }
        }
        let net_victime = match d.nets.as_slice() {
            [n] => Some(*n),
            _ => None,
        };
        if postes.is_empty() && net_victime.is_none() {
            continue;
        }
        let cle = format!("v{}", victimes.len() + 1);
        draft_cle[i] = Some(cle.clone());
        victimes.push(Victime {
            cle,
            role: if directe { "directe" } else { "indirecte" }.to_string(),
            net_victime,
            postes,
            ..Default::default()
        });
    }

    let cle_of = |owner: Owner| -> Option<String> {
        match owner {
            Owner::Draft(d) => draft_cle[d].clone(),
            Owner::List(li) => list_draft[li].and_then(|d| draft_cle[d].clone()),
            Owner::None => None,
        }
    };
    let mut obligations: Vec<Obligation> = cp
        .obs
        .iter()
        .filter_map(|o| {
            let cle = cle_of(o.owner);
            let nature = match &o.kind {
                ObKind::Poste(code) => Nature::Poste {
                    cle,
                    code: code.clone(),
                },
                ObKind::Indemnite => Nature::Indemnite { cle },
                ObKind::Net => Nature::Net { cle: cle? },
                ObKind::Total => Nature::Total { cle },
                ObKind::Hors(c) => Nature::HorsRecord(c),
            };
            Some(Obligation {
                montant: o.montant,
                s: o.s,
                e: o.e,
                nature,
            })
        })
        .collect();
    for ev in &events {
        let Event::Amount(a) = ev else { continue };
        if !obligations.iter().any(|o| o.s == a.start) {
            obligations.push(Obligation {
                montant: euros(a),
                s: a.start,
                e: a.end,
                nature: Nature::HorsRecord("autre"),
            });
        }
    }
    obligations.sort_by_key(|o| o.s);
    Extraction {
        quantum: Quantum {
            est_liquidation: !victimes.is_empty(),
            lecture: Some(Lecture {
                agent: Some("extracteur-sur".to_string()),
                passes: 1,
                ..Default::default()
            }),
            victimes,
            ..Default::default()
        },
        obligations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::scan;

    const TJ: &str = "TRIBUNAL JUDICIAIRE\nJUGEMENT\nMOTIFS DE LA DECISION\nSur les demandes.\nPAR CES MOTIFS\nLe tribunal,\nCONDAMNE in solidum les sociétés CARDIF IARD et AREAS DOMMAGES à payer à Monsieur [A] [W] la somme de 38 017,83 euros, en deniers ou quittances, en réparation de ses préjudices personnels subis en sa qualité de victime par ricochet, dont 13 017,83 euros au titre de sa perte de revenus et 25 000 euros au titre de son préjudice d'affection ;\nCONDAMNE in solidum les sociétés CARDIF IARD et AREAS DOMMAGES à payer aux demandeurs la somme de 15 000 euros au titre de l'article 700 du code de procédure civile ;\nCONDAMNE les défenderesses aux dépens.";

    fn postes(v: &Victime) -> Vec<(&str, Option<f64>, Option<f64>)> {
        v.postes
            .iter()
            .map(|p| {
                (
                    p.poste.as_deref().unwrap(),
                    p.montant_alloue,
                    p.part_victime,
                )
            })
            .collect()
    }

    #[test]
    fn a_payment_with_its_postes_makes_a_ricochet_victim() {
        let x = extract(&scan(TJ));
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].role, "indirecte");
        assert_eq!(v[0].net_victime, Some(38017.83));
        assert_eq!(
            postes(&v[0]),
            vec![("PR", None, Some(13017.83)), ("PAF", None, Some(25000.0))]
        );
        assert!(x
            .obligations
            .iter()
            .any(|o| o.montant == 15000.0 && o.nature == Nature::HorsRecord("article_700")));
    }

    #[test]
    fn a_fixed_list_links_by_adjacency_in_both_directions() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nSur le préjudice.\nPAR CES MOTIFS\nLa cour,\nFixe à la somme globale de 3 025 euros les préjudices extra-patrimoniaux de M. [K], à savoir, 1 025 euros au titre du déficit fonctionnel temporaire et 2 000 euros au titre des souffrances endurées ;\nFIXE le préjudice corporel de M. [K] comme suit : déficit fonctionnel permanent : 27 600 euros ; préjudice d'agrément : 0 ; Total : 27 600 euros ;\nCONDAMNE la société AXA à payer à M. [K] la somme totale de 47 600 euros en réparation de son préjudice corporel ;";
        let x = extract(&scan(text));
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].role, "directe");
        assert_eq!(v[0].net_victime, Some(47600.0));
        assert_eq!(
            postes(&v[0]),
            vec![
                ("DFT", Some(1025.0), None),
                ("SE", Some(2000.0), None),
                ("DFP", Some(27600.0), None),
                ("PA", Some(0.0), None),
            ]
        );
        let totals: Vec<f64> = x
            .obligations
            .iter()
            .filter(|o| matches!(o.nature, Nature::Total { .. }))
            .map(|o| o.montant)
            .collect();
        assert_eq!(totals, vec![3025.0, 27600.0]);
    }

    #[test]
    fn recitals_organisms_and_reductions_write_no_field() {
        let text = "COUR D'APPEL\nARRET\nMOTIFS\nLa faute de la victime réduit son droit à indemnisation à hauteur de 50 %.\nPAR CES MOTIFS\nLa cour,\nConfirme le jugement sauf en ce qu'il a fixé à la somme de 4 356,75 euros la perte de gains professionnels actuels de M. [D] ;\nStatuant à nouveau,\nFixe le préjudice de M. [D] comme suit : perte de gains professionnels actuels : 6 331,77 euros ;\nCondamne la MAIF à payer à la CPAM la somme de 1 200 euros au titre des dépenses de santé actuelles ;";
        let x = extract(&scan(text));
        let v = &x.quantum.victimes;
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(postes(&v[0]), vec![("PGPA", None, None)]);
        let hors: Vec<(f64, &str)> = x
            .obligations
            .iter()
            .filter_map(|o| match o.nature {
                Nature::HorsRecord(c) => Some((o.montant, c)),
                _ => None,
            })
            .collect();
        assert_eq!(
            hors,
            vec![(4356.75, "rappel_du_jugement"), (1200.0, "tiers_payeur")]
        );
    }
}
