//! Amounts the decision writes that no field of the record holds: the converse
//! of anchoring, which looks every record value up in the text.
//!
//! An unheld amount is at best out of scope on purpose (article 700, costs, a
//! provision, the reversed judgment recited), at worst a check the reading gave
//! up on. The flag that matters is `derivable`: an amount equal to a record
//! amount times one of the record's rates would have confirmed a cell.

use std::collections::HashSet;
use std::sync::LazyLock;

use lj_core::text::fold_char;
use regex::Regex;
use serde_json::Value;

use super::ancrage::{jetons, millimes};
use super::Quantum;

const SPACES: [char; 3] = [' ', '\u{a0}', '\u{202f}'];

/// Currency words, on folded text; the euro sign and the apostrophe are read on
/// the raw characters.
const CURRENCIES: [&str; 12] = [
    "euros",
    "euro",
    "eur",
    "f cfp",
    "f. cfp",
    "f.cfp",
    "fcfp",
    "xpf",
    "francs cfp",
    "franc cfp",
    "francs pacifique",
    "franc pacifique",
];

/// Where the motifs open: the first anchor found before the dispositif wins.
/// The last six are those of the administrative order, which writes neither
/// « motifs » nor « sur ce ».
const MOTIFS_ANCHORS: [&str; 16] = [
    "motifs de la decision",
    "motifs de l'arret",
    "motifs de la cour",
    "expose du litige",
    "faits et procedure",
    "sur ce, la cour",
    "cela etant expose",
    "discussion",
    "motifs",
    "sur ce",
    "il resulte de l'instruction",
    "sur les conclusions",
    "sur la responsabilite",
    "sur les prejudices",
    "considerant que",
    "sur le fond",
];

/// The dispositif opens with one of these, in the last 65 % of the text.
const DISPOSITIF_ANCHORS: [&str; 3] = ["par ces motifs", "d e c i d e", "decide :"];

/// What an unheld amount usually is, read on the folded characters around it.
static OUT_OF_SCOPE: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r"article\s+700|frais irrepetibles|l\.?\s*761",
            "article 700",
        ),
        (
            r"depens|article\s*699|expertise, liquid",
            "dépens et frais d'expertise",
        ),
        (r"provision|consign", "provision"),
        (
            r"\brg\b|repertoire|minute|\bn°\s*\d",
            "référence de dossier",
        ),
        (
            r"bulletin|salaire mentionnant|revenu mensuel moyen",
            "pièce de revenu",
        ),
        (r"indemnite kilom|reste a charge", "pièce justificative"),
        (r"^- |condamne la |alloue a ", "rappel du jugement"),
    ]
    .into_iter()
    .map(|(rx, cause)| (Regex::new(rx).expect("out-of-scope pattern"), cause))
    .collect()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Zone {
    Pretentions,
    Motifs,
    Dispositif,
}

impl Zone {
    pub fn label(self) -> &'static str {
        match self {
            Zone::Pretentions => "pretentions",
            Zone::Motifs => "motifs",
            Zone::Dispositif => "dispositif",
        }
    }
}

/// An amount of the text: value in thousandths, bounds in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount {
    pub value: i64,
    pub start: usize,
    pub end: usize,
}

/// An amount of the text that the record does not hold.
#[derive(Debug, Clone)]
pub struct Leftover {
    pub value: f64,
    pub start: usize,
    pub zone: Zone,
    pub derivable: bool,
    pub cause: Option<&'static str>,
    /// The 130 characters around the amount, newlines flattened.
    pub excerpt: String,
}

pub struct Report {
    /// Amounts the text prices, held or not.
    pub amounts: usize,
    pub leftovers: Vec<Leftover>,
}

fn word_at(folded: &[char], i: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    folded.len() >= i + w.len()
        && folded[i..i + w.len()] == w[..]
        && !folded.get(i + w.len()).is_some_and(|c| c.is_alphabetic())
}

/// Is the number priced? An apostrophe counts when the source lost its euro
/// signs, which it then writes as « 740 ' x 1,193 ».
fn currency_after(chars: &[char], folded: &[char], end: usize, apostrophe: bool) -> bool {
    let mut i = end;
    while folded.get(i).is_some_and(|c| SPACES.contains(c)) {
        i += 1;
    }
    match chars.get(i) {
        Some('€') => true,
        Some('\'') | Some('\u{2019}') => apostrophe,
        _ => CURRENCIES.iter().any(|w| word_at(folded, i, w)),
    }
}

pub(super) fn amounts_in(chars: &[char], folded: &[char]) -> Vec<Amount> {
    let apostrophe = !chars.contains(&'€');
    let mut out: Vec<Amount> = Vec::new();
    for j in jetons(chars) {
        // A leading zero is the tail of a badly cut number: « 07 euros ».
        let cut =
            chars[j.debut] == '0' && chars.get(j.debut + 1).is_some_and(|c| c.is_ascii_digit());
        // « 42.469 » yields two readings; the thousands come first and win here.
        let seen = out.last().is_some_and(|a| a.start == j.debut);
        // « 1'729'346,03 » : a fragment of a number the apostrophe separates.
        let fragment = (chars.get(j.fin) == Some(&'\'')
            && chars[j.fin + 1..]
                .iter()
                .take(3)
                .all(|c| c.is_ascii_digit())
            && chars[j.fin + 1..].iter().take(3).count() == 3)
            || (j.debut >= 2 && chars[j.debut - 1] == '\'' && chars[j.debut - 2].is_ascii_digit());
        if cut
            || seen
            || fragment
            || j.valeur <= 0
            || !currency_after(chars, folded, j.fin, apostrophe)
        {
            continue;
        }
        out.push(Amount {
            value: j.valeur,
            start: j.debut,
            end: j.fin,
        });
    }
    out
}

/// Every amount of the text, in order: a number the text prices in a currency.
pub fn amounts(text: &str) -> Vec<Amount> {
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().copied().map(fold_char).collect();
    amounts_in(&chars, &folded)
}

fn find(hay: &[char], needle: &str, from: usize, to: usize, last: bool) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || hay.len() < n.len() {
        return None;
    }
    let range = from..to.min(hay.len() - n.len() + 1);
    let hit = |i: &usize| hay[*i..*i + n.len()] == n[..];
    if last {
        range.rev().find(hit)
    } else {
        range.into_iter().find(hit)
    }
}

/// The zone of every offset: prétentions until the motifs open, motifs until
/// the dispositif does.
pub struct Zoning {
    motifs: Option<usize>,
    dispositif: Option<usize>,
}

impl Zoning {
    pub fn of(folded: &[char]) -> Self {
        let floor = folded.len() * 35 / 100;
        let dispositif = DISPOSITIF_ANCHORS
            .iter()
            .filter_map(|a| find(folded, a, floor, folded.len(), true))
            .max();
        let limit = dispositif.unwrap_or(folded.len());
        let motifs = MOTIFS_ANCHORS
            .iter()
            .find_map(|a| find(folded, a, 0, limit, false).filter(|&i| i > 0));
        Zoning { motifs, dispositif }
    }

    pub fn zone(&self, pos: usize) -> Zone {
        if self.dispositif.is_some_and(|d| pos >= d) {
            Zone::Dispositif
        } else if self.motifs.is_some_and(|m| pos >= m) {
            Zone::Motifs
        } else {
            Zone::Pretentions
        }
    }
}

/// Every number the record holds anywhere, its prose included: a figure quoted
/// in a comment is not left over.
fn held(record: &Value, out: &mut HashSet<i64>) {
    match record {
        Value::Number(n) => {
            if let Some(x) = n.as_f64() {
                out.insert(millimes(x));
            }
        }
        Value::String(s) => {
            for j in jetons(&s.chars().collect::<Vec<_>>()) {
                out.insert(j.valeur);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| held(v, out)),
        Value::Object(o) => o.values().for_each(|v| held(v, out)),
        _ => {}
    }
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn items<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    v.get(key).and_then(Value::as_array).into_iter().flatten()
}

/// What the record prices, and the rates by which it reduces.
fn amounts_and_rates(record: &Value) -> (Vec<i64>, Vec<f64>) {
    let mut amounts = Vec::new();
    let mut rates = Vec::new();
    amounts.extend(num(record, "total_enonce").map(millimes));
    for v in items(record, "victimes") {
        for k in [
            "total_alloue",
            "net_victime",
            "creance_tiers_payeurs",
            "assiette_reduite",
        ] {
            amounts.extend(num(v, k).map(millimes));
        }
        for k in ["partage_responsabilite_pct", "perte_de_chance_pct"] {
            rates.extend(num(v, k).filter(|p| *p != 100.0).map(|p| p / 100.0));
        }
        for p in items(v, "postes") {
            for k in ["montant_alloue", "part_victime", "part_tiers_payeurs"] {
                amounts.extend(num(p, k).map(millimes));
            }
        }
    }
    (amounts, rates)
}

/// Within two cents of a record amount reduced by one of the record's rates.
fn derivable(value: i64, amounts: &[i64], rates: &[f64]) -> bool {
    let v = value as f64;
    amounts.iter().any(|&m| {
        let m = m as f64;
        rates
            .iter()
            .any(|&t| (v - m * t).abs() < 20.0 || (v - m * (1.0 - t)).abs() < 20.0)
    })
}

pub fn leftovers(q: &Quantum, text: &str) -> Report {
    let record = serde_json::to_value(q).unwrap_or(Value::Null);
    let mut held_values = HashSet::new();
    held(&record, &mut held_values);
    let (record_amounts, rates) = amounts_and_rates(&record);

    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().copied().map(fold_char).collect();
    let zoning = Zoning::of(&folded);
    let found = amounts_in(&chars, &folded);

    let leftovers = found
        .iter()
        .filter(|a| !held_values.contains(&a.value))
        .map(|a| {
            let lo = a.start.saturating_sub(90);
            let hi = (a.start + 40).min(chars.len());
            let excerpt: String = chars[lo..hi]
                .iter()
                .map(|c| if *c == '\n' { ' ' } else { *c })
                .collect();
            let around: String = folded[lo..hi].iter().collect::<String>().trim().to_string();
            Leftover {
                value: a.value as f64 / 1000.0,
                start: a.start,
                zone: zoning.zone(a.start),
                derivable: derivable(a.value, &record_amounts, &rates),
                cause: OUT_OF_SCOPE
                    .iter()
                    .find(|(rx, _)| rx.is_match(&around))
                    .map(|(_, c)| *c),
                excerpt,
            }
        })
        .collect();
    Report {
        amounts: found.len(),
        leftovers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(text: &str) -> Vec<i64> {
        amounts(text).iter().map(|a| a.value).collect()
    }

    #[test]
    fn only_a_priced_number_is_an_amount() {
        assert_eq!(
            values("alloue 12 345,67 euros, 1.500 € et 1 500 000 F CFP le 12 mai 2024, soit 25 %"),
            vec![12_345_670, 1_500_000, 1_500_000_000]
        );
    }

    #[test]
    fn a_leading_zero_is_the_tail_of_a_cut_number() {
        assert_eq!(values("07 euros"), Vec::<i64>::new());
        assert_eq!(values("600 272, 07 euros"), vec![600_272_070]);
    }

    #[test]
    fn the_apostrophe_prices_a_number_only_where_the_euro_sign_is_lost() {
        assert_eq!(
            values("740 ' x 1,193 (IPC 2013) = 882,82 '"),
            vec![740_000, 882_820]
        );
        assert_eq!(values("740 ' et 1 500 €"), vec![1_500_000]);
    }

    #[test]
    fn zones_follow_the_motifs_and_the_dispositif() {
        let text =
            "Les parties demandent 10 euros. MOTIFS DE LA DÉCISION : la cour alloue 20 euros. \
                    PAR CES MOTIFS, condamne à payer 30 euros.";
        let folded: Vec<char> = text.chars().map(fold_char).collect();
        let zoning = Zoning::of(&folded);
        let zones: Vec<Zone> = amounts(text).iter().map(|a| zoning.zone(a.start)).collect();
        assert_eq!(
            zones,
            vec![Zone::Pretentions, Zone::Motifs, Zone::Dispositif]
        );
    }
}
