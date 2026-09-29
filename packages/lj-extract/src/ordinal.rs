//! Ordinal names the practice gives a line of decisions: « Béziers I »,
//! « Béziers II ». A citing text writes beside the ordinal the number or the
//! day of the decision it means, and the votes name the bearer of each one.

use std::collections::HashMap;

use lj_fold::fold;

pub const ROMANS: [&str; 5] = ["i", "ii", "iii", "iv", "v"];

/// What announces a citation before the name, a locution or a court. « centre
/// commercial Béziers II » and « canton de Béziers I » announce a mall and a
/// constituency.
const ANNOUNCERS: [&str; 15] = [
    "jurisprudence",
    "decision",
    "arret",
    "recours",
    "affaire",
    "ce",
    "conseil",
    "section",
    "assemblee",
    "cass",
    "cassation",
    "cour",
    "cjue",
    "cjce",
    "cedh",
];

/// Words on each side of the ordinal where its announcer and its anchors are
/// read. The longest writing measured: « (CE, Section, 21 mars 2011, Commune de
/// Béziers, n° 304806 (Béziers II) » puts the day nine words before.
const WINDOW: usize = 15;

const MONTHS: [&[&str]; 12] = [
    &["janvier", "janv", "jan"],
    &["fevrier", "fevr", "fev"],
    &["mars"],
    &["avril", "avr"],
    &["mai"],
    &["juin"],
    &["juillet", "juil"],
    &["aout"],
    &["septembre", "sept", "sep"],
    &["octobre", "oct"],
    &["novembre", "nov"],
    &["decembre", "dec"],
];

/// A decision an ordinal may designate: its docket keys and its reading day, ISO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: i64,
    pub dockets: Vec<String>,
    pub date: Option<String>,
}

/// One writing of `<name> <roman>` that a locution announces, with the numbers
/// and the days written within [`WINDOW`] words of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub roman: &'static str,
    pub numbers: Vec<String>,
    pub days: Vec<String>,
}

/// The ordinal writings of `key`, a folded case name key, in a text.
pub fn ordinal_occurrences(text: &str, key: &str) -> Vec<Occurrence> {
    let folded = fold(text);
    let words: Vec<&str> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let name: Vec<&str> = key.split(' ').collect();
    let mut out = Vec::new();
    if words.len() <= name.len() {
        return out;
    }
    for start in 0..words.len() - name.len() {
        if words[start..start + name.len()] != name[..] {
            continue;
        }
        let after = start + name.len();
        let Some(roman) = ROMANS.iter().copied().find(|r| *r == words[after]) else {
            continue;
        };
        let announced = words[start.saturating_sub(WINDOW)..start].iter().any(|w| {
            ANNOUNCERS
                .iter()
                .any(|l| w == l || w.strip_suffix('s') == Some(l))
        });
        if !announced {
            continue;
        }
        let window = &words[start.saturating_sub(WINDOW)..(after + 1 + WINDOW).min(words.len())];
        out.push(Occurrence {
            roman,
            numbers: numbers_in(window),
            days: days_in(window),
        });
    }
    out
}

/// The numbers a word sequence writes, a year left out: `5911` is a number of
/// the Conseil d'État, `2015` a year.
fn numbers_in(words: &[&str]) -> Vec<String> {
    words
        .iter()
        .filter(|w| (4..=10).contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| !w.parse::<u32>().is_ok_and(|n| (1800..=2100).contains(&n)))
        .map(|w| w.to_string())
        .collect()
}

/// The days a word sequence writes, ISO: `21 mars 2011`, `28 déc 2009`,
/// `21-03-2011` and `1er avril 2025` once folded into words.
fn days_in(words: &[&str]) -> Vec<String> {
    let mut days = Vec::new();
    for w in words.windows(3) {
        let Ok(day) = w[0].trim_end_matches("er").parse::<u32>() else {
            continue;
        };
        let month = MONTHS
            .iter()
            .position(|names| names.contains(&w[1]))
            .map(|i| i as u32 + 1)
            .or_else(|| {
                (w[1].len() <= 2)
                    .then(|| w[1].parse::<u32>().ok())
                    .flatten()
            });
        let (Some(month), Ok(year)) = (month, w[2].parse::<i32>()) else {
            continue;
        };
        if (1..=31).contains(&day) && (1..=12).contains(&month) && (1800..=2100).contains(&year) {
            days.push(format!("{year:04}-{month:02}-{day:02}"));
        }
    }
    days
}

fn same_number(a: &str, b: &str) -> bool {
    let a = a.trim_start_matches('0');
    !a.is_empty() && a == b.trim_start_matches('0')
}

fn votes(o: &Occurrence, c: &Candidate) -> u32 {
    let by_number = o
        .numbers
        .iter()
        .any(|n| c.dockets.iter().any(|d| same_number(n, d)));
    let by_day = c
        .date
        .as_deref()
        .is_some_and(|d| o.days.iter().any(|x| x == d));
    u32::from(by_number) + u32::from(by_day)
}

/// The bearer each ordinal designates. A number written beside the ordinal is a
/// vote, a day is another; the winner holds at least two and twice as many as
/// any other candidate.
pub fn ordinal_winners(
    occurrences: &[Occurrence],
    candidates: &[Candidate],
) -> Vec<(&'static str, i64)> {
    let mut tally: HashMap<(&'static str, i64), u32> = HashMap::new();
    for o in occurrences {
        for c in candidates {
            let v = votes(o, c);
            if v > 0 {
                *tally.entry((o.roman, c.id)).or_default() += v;
            }
        }
    }
    let mut winners = Vec::new();
    for roman in ROMANS {
        let mut scores: Vec<(u32, i64)> = tally
            .iter()
            .filter(|((r, _), _)| *r == roman)
            .map(|((_, id), v)| (*v, *id))
            .collect();
        scores.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        if let [(best, id), rest @ ..] = &scores[..] {
            if *best >= 2 && rest.first().is_none_or(|(second, _)| *best >= 2 * second) {
                winners.push((roman, *id));
            }
        }
    }
    winners
}

/// The anchors no candidate answers, a day and a number written together beside
/// an ordinal: the decision they name may bear no name yet.
pub fn unresolved_anchors(
    occurrences: &[Occurrence],
    candidates: &[Candidate],
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for o in occurrences {
        if candidates.iter().any(|c| votes(o, c) > 0) {
            continue;
        }
        for day in &o.days {
            for number in &o.numbers {
                let pair = (day.clone(), number.clone());
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beziers() -> Vec<Candidate> {
        vec![
            Candidate {
                id: 2001,
                dockets: vec!["237137".into()],
                date: Some("2001-12-28".into()),
            },
            Candidate {
                id: 2009,
                dockets: vec!["304802".into()],
                date: Some("2009-12-28".into()),
            },
            Candidate {
                id: 2011,
                dockets: vec!["304806".into()],
                date: Some("2011-03-21".into()),
            },
        ]
    }

    #[test]
    fn l_ordinal_annonce_par_une_locution_se_lit_avec_ses_ancres() {
        let text = "les relations contractuelles (CE, Section, 21 mars 2011, Commune de \
                    Béziers, n° 304806 (Béziers II). Or de telles conclusions";
        let o = ordinal_occurrences(text, "beziers");
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].roman, "ii");
        assert!(o[0].numbers.contains(&"304806".to_string()));
        assert_eq!(o[0].days, vec!["2011-03-21".to_string()]);

        let o = ordinal_occurrences("(CE, Béziers I, 28 déc 2009, 304802, A)", "beziers");
        assert_eq!(o.len(), 1, "the court announces the name");
        assert_eq!(o[0].numbers, vec!["304802".to_string()]);
        assert_eq!(o[0].days, vec!["2009-12-28".to_string()]);
        let o = ordinal_occurrences(
            "au sens de la décision du Conseil d'Etat \" Béziers I \" du 28 décembre 2009 ;",
            "beziers",
        );
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].days, vec!["2009-12-28".to_string()]);
        let o = ordinal_occurrences(
            "dite Béziers II (CE Section 21-03-2011 n° 304806)",
            "beziers",
        );
        assert_eq!(o.len(), 0, "« dite » is not a locution on its own");
        let o = ordinal_occurrences(
            "la jurisprudence dite Béziers II (CE Section 21-03-2011 n° 304806)",
            "beziers",
        );
        assert_eq!(o[0].days, vec!["2011-03-21".to_string()]);
    }

    #[test]
    fn le_lieu_dit_ne_cite_rien() {
        assert!(ordinal_occurrences(
            "dont le siège est centre commercial Béziers II, route de Bessan",
            "beziers"
        )
        .is_empty());
        assert!(ordinal_occurrences(
            "du conseiller général du canton de Béziers I ; 2° d'annuler",
            "beziers"
        )
        .is_empty());
    }

    #[test]
    fn les_ancres_votent_et_le_gagnant_tient_deux_voix() {
        let texts = [
            "la jurisprudence dite Béziers II (CE Section 21-03-2011 n° 304806)",
            "de la jurisprudence dite \" Béziers II \" du 21 mars 2011 ;",
            "la décision du Conseil d'Etat \" Béziers I \" du 28 décembre 2009 ;",
            "eu égard aux particularités du recours contentieux défini dans la décision dite \" Béziers II \", à l'étendue",
        ];
        let occurrences: Vec<Occurrence> = texts
            .iter()
            .flat_map(|t| ordinal_occurrences(t, "beziers"))
            .collect();
        assert_eq!(occurrences.len(), 4);
        let winners = ordinal_winners(&occurrences, &beziers());
        assert_eq!(
            winners,
            vec![("ii", 2011)],
            "one day alone does not name Béziers I"
        );
    }

    #[test]
    fn l_ancre_sans_porteuse_designe_une_decision_a_resoudre() {
        let text = "requis par la décision dite \" Béziers III \" (CE 27 février 2015, \
                    n°357028, Commune de Béziers) - applicable en l'espèce";
        let occurrences = ordinal_occurrences(text, "beziers");
        let pending = unresolved_anchors(&occurrences, &beziers());
        assert_eq!(
            pending,
            vec![("2015-02-27".to_string(), "357028".to_string())]
        );
        let mut candidates = beziers();
        candidates.push(Candidate {
            id: 2015,
            dockets: vec!["357028".into()],
            date: Some("2015-02-27".into()),
        });
        assert_eq!(
            ordinal_winners(&occurrences, &candidates),
            vec![("iii", 2015)]
        );
        assert!(unresolved_anchors(&occurrences, &candidates).is_empty());
    }
}
