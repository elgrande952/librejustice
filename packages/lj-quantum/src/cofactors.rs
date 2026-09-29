//! Cofactors of a victim and abscissae of a poste: the cotation on seven, the
//! deficit rate, the value of the point, the date and the age at consolidation.
//!
//! A cofactor is written when the text settles it alone, or when the court's own
//! arithmetic settles it: the rate and the value of the point are kept together
//! only when their product is the sum awarded on the permanent deficit.
use super::dispositif::{Extraction, Mode};
use super::vocabulary::{self, Cue, AMBIGUOUS};
use super::{leftovers, Composante, Unite};
use lj_extract::scan::DocScan;

/// Labels of the permanent deficit, the rate of which is the victim's, not a
/// poste's. « incapacité permanente » is absent: it names the social security
/// rate of an inexcusable fault, which is not an AIPP.
const DFP_LABELS: [&str; 6] = [
    "deficit fonctionnel permanent",
    "deficit fonctionnel definitif",
    "deficit fonctionnel sequellaire",
    "dfp",
    "aipp",
    "atteinte permanente a l'integrite physique et psychique",
];

/// « le taux du deficit fonctionnel permanent doit etre evalue a 25 % » : an
/// obligation standing between the label and the figure is a party asking for
/// that figure, not the court writing it.
const CLAIMED: [&str; 5] = [
    "doit etre",
    "doivent etre",
    "devrait etre",
    "devraient etre",
    "sollicite",
];

const CIVILITIES: [(&str, &str); 6] = [
    ("monsieur", "M"),
    ("madame", "F"),
    ("mademoiselle", "F"),
    ("mme", "F"),
    ("mlle", "F"),
    ("m.", "M"),
];

/// Titles the pseudonymiser leaves in front of a name that is not a party's.
const TITLES: [&str; 6] = ["docteur", "dr", "professeur", "pr", "maitre", "me"];

const REDUCTION_WORDS: [&str; 11] = [
    "responsable",
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
/// The phrases that name a reduction of the right to compensation, where the
/// words above only sit near one: « une faute » beside a rate is enough to give
/// up a full liquidation, not enough to refuse it.
const REDUCTION_NAMED: [&str; 6] = [
    "droit a indemnisation",
    "droit a reparation",
    "droit a etre indemnise",
    "partage de responsabilite",
    "partage de responsabilites",
    "perte de chance",
];
const FRACTIONS: [&str; 6] = [
    "de moitie",
    "la moitie",
    "un tiers",
    "deux tiers",
    "trois quarts",
    "un quart",
];

const MONTHS: [(&str, u32); 12] = [
    ("janvier", 1),
    ("fevrier", 2),
    ("mars", 3),
    ("avril", 4),
    ("mai", 5),
    ("juin", 6),
    ("juillet", 7),
    ("aout", 8),
    ("septembre", 9),
    ("octobre", 10),
    ("novembre", 11),
    ("decembre", 12),
];

const CONSOLIDATION: [&str; 5] = [
    "consolidation",
    "consolide",
    "consolidee",
    "consolides",
    "consolidees",
];

/// A date of consolidation reads through one of these; anything else between
/// the word and the date names another event.
const DATE_GAP: [&str; 23] = [
    "est",
    "a",
    "ete",
    "sera",
    "etait",
    "fixe",
    "fixee",
    "fixees",
    "acquise",
    "intervenue",
    "arretee",
    "retenue",
    "au",
    "le",
    "la",
    "du",
    "de",
    "en",
    "date",
    "soit",
    "medico",
    "medicale",
    "legale",
];

/// Verbs that hand the consolidation to a party rather than to the court.
const PARTY_DATE: [&str; 5] = [
    "se prevalent",
    "se prevaut",
    "se prevaloir",
    "revendiqu",
    "sollicit",
];

/// « de la consolidation au 3 avril 2019 » opens a period, and its second bound
/// is not the date the state was fixed.
const PERIOD_BEFORE: [&str; 7] = [
    "de la",
    "depuis la",
    "apres la",
    "avant la",
    "posterieure a la",
    "a compter de la",
    "entre la",
];

const AGE_BEFORE_GAP: [&str; 16] = [
    "au", "jour", "de", "a", "la", "sa", "leur", "date", "lors", "moment", "temps", "du",
    "revolus", "ans", "mois", "et",
];
const AGE_HEAD: [&str; 12] = [
    "age",
    "agee",
    "ages",
    "agees",
    "avait",
    "avaient",
    "etait",
    "etaient",
    "de",
    "que",
    "seulement",
    "avoir",
];
const AGE_AFTER_GAP: [&str; 11] = [
    "soit", "en", "l", "espece", "il", "elle", "avait", "de", "a", "etant", "age",
];

/// Words a court writes between a poste and the note it gives it.
const COTATION_GAP: [&str; 17] = [
    "de", "du", "a", "au", "cote", "cotee", "cotees", "cotes", "evalue", "evaluee", "evaluees",
    "evalues", "estime", "estimee", "estimees", "fixe", "fixee",
];

const COTATION_CODES: [&str; 3] = ["SE", "PET", "PEP"];

pub fn apply(scan: &DocScan, x: &mut Extraction, mode: Mode) {
    let folded: Vec<char> = scan.folded().chars().collect();
    let labels = poste_labels(scan, &folded);
    tariffs(x, scan.chars(), &folded, &labels);

    let directs: Vec<usize> = x
        .quantum
        .victimes
        .iter()
        .enumerate()
        .filter(|(_, v)| v.role == "directe")
        .map(|(i, _)| i)
        .collect();
    let [vi] = directs[..] else { return };

    let v = &mut x.quantum.victimes[vi];
    if v.partage_responsabilite_pct.is_none() && !reduced(&folded, mode) {
        v.partage_responsabilite_pct = Some(100.0);
    }
    if v.sexe.is_none() {
        v.sexe = sexe(&folded, super::dispositif_span(scan).0, mode).map(str::to_string);
    }
    if v.date_consolidation.is_none() {
        v.date_consolidation = consolidation_date(&folded, mode);
    }
    if v.age_consolidation.is_none() {
        v.age_consolidation = consolidation_age(&folded, mode);
    }
    for code in COTATION_CODES {
        let Some(p) = v
            .postes
            .iter_mut()
            .find(|p| p.poste.as_deref() == Some(code))
        else {
            continue;
        };
        if p.cotation.is_none() {
            p.cotation = cotation(&folded, &labels, code, mode);
        }
    }

    let rates = dfp_rates(&folded);
    let award = v
        .postes
        .iter()
        .find(|p| p.poste.as_deref() == Some("DFP"))
        .and_then(|p| p.montant_alloue.or(p.part_victime));
    let points = award.map_or_else(Vec::new, |_| point_values(scan.chars(), &folded));
    let settled =
        award.and_then(|m| settle(&folded, v.taux_dfp_pct.iter().copied(), &rates, &points, m));
    if v.taux_dfp_pct.is_none() {
        v.taux_dfp_pct = settled
            .map(|(rate, _)| rate)
            .or_else(|| pick_rate(&rates, mode));
    }
    if let Some((_, unit)) = settled {
        let Some(p) = v
            .postes
            .iter_mut()
            .find(|p| p.poste.as_deref() == Some("DFP"))
        else {
            return;
        };
        if p.valeur_unitaire.is_none() {
            p.valeur_unitaire = Some(unit);
            p.unite = p.unite.or(Some(Unite::Point));
        }
    }
}

/// The rate and the value of the point whose product is what the court awarded
/// on the permanent deficit. Reading both in the text and letting the award pick
/// the pair proves them; no division ever poses either.
fn settle(
    f: &[char],
    known: impl Iterator<Item = f64>,
    rates: &[f64],
    points: &[f64],
    award: f64,
) -> Option<(f64, f64)> {
    let mut candidates: Vec<(f64, f64)> = Vec::new();
    let known: Vec<f64> = known.collect();
    let rates = if known.is_empty() { rates } else { &known };
    for rate in rates {
        for point in points {
            if (rate * point - award).abs() < 0.02
                && !candidates
                    .iter()
                    .any(|(r, p)| (r - rate).abs() < 0.005 && (p - point).abs() < 0.005)
            {
                candidates.push((*rate, *point));
            }
        }
    }
    // « 299.200 (4.675 x 64) » under a report at 58 %: the court's own
    // arithmetic names the rate, and the quotient counts when the text also
    // writes it as a rate.
    if candidates.is_empty() && known.is_empty() {
        for point in points.iter().filter(|p| (*p - award).abs() > 0.005) {
            let quotient = award / point;
            let rate = (quotient * 2.0).round() / 2.0;
            if (quotient - rate).abs() < 0.001
                && (1.0..=100.0).contains(&rate)
                && rate_written(f, rate)
            {
                candidates.push((rate, *point));
            }
        }
    }
    match candidates[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// « 64% » somewhere in the text, and not « -1% », the rate of a capitalisation table.
fn rate_written(f: &[char], rate: f64) -> bool {
    let figure = format!("{rate}").replace('.', ",");
    [format!("{figure}%"), format!("{figure} %")]
        .iter()
        .any(|w| {
            find_all(f, w).iter().any(|(s, _)| {
                let before = f[..*s].iter().rev().find(|c| **c != ' ');
                before.is_none_or(|c| !c.is_ascii_digit() && *c != '-')
            })
        })
}

/// « 4.675 », « 1 850 », « 12 000,50 » : digits by groups of three, a comma
/// before the cents.
fn grouped_number_at(f: &[char], i: usize) -> Option<(f64, usize)> {
    let (head, mut j) = digits_at(f, i)?;
    let mut value = f64::from(head);
    while matches!(f.get(j), Some('.') | Some(' ')) {
        let Some((group, k)) = digits_at(f, j + 1) else {
            break;
        };
        if k - j - 1 != 3 || f.get(k).is_some_and(|c| c.is_ascii_digit()) {
            break;
        }
        value = value * 1000.0 + f64::from(group);
        j = k;
    }
    if f.get(j) == Some(&',') {
        if let Some((frac, k)) = digits_at(f, j + 1) {
            if k - j - 1 <= 2 {
                value += f64::from(frac) / 10f64.powi((k - j - 1) as i32);
                j = k;
            }
        }
    }
    Some((value, j))
}

/// Postes a court prices by the unit, a day of temporary deficit or an hour of
/// third-party assistance. On a lump sum, a product that lands on the award is
/// a coincidence and not a tariff.
const BY_UNIT: [&str; 9] = [
    "DFT", "FD", "ATP", "PGPA", "PGPF", "DSA", "DSF", "PR", "FVA",
];

/// The units a court counts a poste in. « 183 j x 28 euros »: the abbreviation
/// stands beside the word in the same arrets.
const UNITS: &[(&str, Unite)] = &[
    ("journees", Unite::Jour),
    ("journee", Unite::Jour),
    ("jours", Unite::Jour),
    ("jour", Unite::Jour),
    ("j", Unite::Jour),
    ("semaines", Unite::Semaine),
    ("semaine", Unite::Semaine),
    ("mois", Unite::Mois),
    ("annees", Unite::An),
    ("annee", Unite::An),
    ("ans", Unite::An),
    ("an", Unite::An),
    ("heures", Unite::Heure),
    ("heure", Unite::Heure),
    ("seances", Unite::Seance),
    ("seance", Unite::Seance),
];

/// What names the tariff of one unit beside the sum that prices it. Matched on
/// the raw slice and not on a word, so « journaliere » reads through
/// « journalier ».
const PER: &[(&str, Unite)] = &[
    ("par jour", Unite::Jour),
    ("par journee", Unite::Jour),
    ("/ jour", Unite::Jour),
    ("/jour", Unite::Jour),
    ("journalier", Unite::Jour),
    ("quotidien", Unite::Jour),
    ("par heure", Unite::Heure),
    ("/ heure", Unite::Heure),
    ("/heure", Unite::Heure),
    ("horaire", Unite::Heure),
    ("par mois", Unite::Mois),
    ("/ mois", Unite::Mois),
    ("/mois", Unite::Mois),
    ("mensuel", Unite::Mois),
    ("par semaine", Unite::Semaine),
    ("hebdomadaire", Unite::Semaine),
    ("par an", Unite::An),
    ("par annee", Unite::An),
    ("/ an", Unite::An),
    ("annuel", Unite::An),
    ("par seance", Unite::Seance),
    ("la seance", Unite::Seance),
];

/// How far past a label of the poste its computation runs.
const TARIFF_WINDOW: usize = 1400;

/// The reading a product closes on: the whole award, or the periods the court
/// adds up to reach it.
#[derive(Debug, Clone, PartialEq)]
enum Tariff {
    Single { quantite: f64, taux: Option<f64> },
    Split { periods: Vec<(f64, f64)> },
}

fn unit_at(f: &[char], i: usize) -> Option<Unite> {
    UNITS
        .iter()
        .find(|(w, _)| word_at(f, i, w))
        .map(|(_, u)| *u)
}

/// The fraction that prorates a period. A percentage the court writes right
/// after the quantity belongs to it, « 16 jours : 16 x 32 x 25 % »; otherwise
/// the last one before it, « partiel a 50 % pendant 156 jours ».
fn period_rate(f: &[char], s: usize, e: usize) -> f64 {
    // « (1 jour x 30 euros) + (11 jours x 30 euros x 10 %) » : the fraction
    // of the next term is not this one's.
    let after = (e..(e + 45).min(f.len()))
        .take_while(|i| !matches!(f[*i], ')' | '+' | '=' | ';'))
        .find_map(|i| percent_at(f, i));
    let before = (s.saturating_sub(90)..s)
        .rev()
        .find_map(|i| percent_at(f, i));
    match after.or(before) {
        Some((v, _)) if v > 0.0 && v <= 100.0 => v,
        _ => 100.0,
    }
}

/// Every « 156 jours », « 183 j », « 18 mois » of the window, with the fraction
/// that prorates it.
fn periods(f: &[char], s: usize, e: usize) -> Vec<(f64, Unite, f64)> {
    let mut out = Vec::new();
    for i in s..e {
        let Some((value, end)) = number_at(f, i) else {
            continue;
        };
        let j = skip_spaces(f, end);
        if let Some(u) = unit_at(f, j) {
            if value > 0.0 {
                out.push((value, u, period_rate(f, i, j)));
            }
        }
    }
    out
}

/// Every sum of the window the court prices by a unit.
fn unit_prices(f: &[char], amounts: &[leftovers::Amount], s: usize, e: usize) -> Vec<(f64, Unite)> {
    let mut out = Vec::new();
    for a in amounts.iter().filter(|a| a.start >= s && a.end <= e) {
        let after = text_of(f, a.end, (a.end + 30).min(f.len()));
        let beyond = text_of(f, a.end, (a.end + 45).min(f.len()));
        let before = text_of(f, a.start.saturating_sub(45), a.start);
        for (mark, u) in PER {
            // The mark of a tariff written after the sum (« base journalière
            // de 30 euros ») belongs to the sum that follows, not to this one.
            let next = after.find(mark).is_some_and(|i| {
                let tail = beyond[i + mark.len()..]
                    .trim_start_matches(|c: char| c.is_alphabetic())
                    .trim_start();
                let tail = tail.strip_prefix("de ").or_else(|| tail.strip_prefix("d'"));
                tail.is_some_and(|t| t.trim_start().starts_with(|c: char| c.is_ascii_digit()))
            });
            if ((after.contains(mark) && !next) || before.contains(mark))
                && !out.contains(&(a.value as f64 / 1000.0, *u))
            {
                out.push((a.value as f64 / 1000.0, *u));
            }
        }
    }
    out
}

/// The tariff and the quantity whose product is what the court awarded on the
/// poste. Reading both in the text and letting the award pick the pair proves
/// them; no division ever poses either, so a prorated period never contaminates
/// the price of the full day.
fn settle_tariff(
    f: &[char],
    amounts: &[leftovers::Amount],
    labels: &[(usize, usize, Option<&'static str>)],
    code: &str,
    award: f64,
) -> Option<(f64, Unite, Tariff)> {
    let mut found: Vec<(f64, Unite, Tariff)> = Vec::new();
    for (s, e, _) in labels.iter().filter(|(_, _, c)| *c == Some(code)) {
        let (ws, we) = (*s, (e + TARIFF_WINDOW).min(f.len()));
        let prices = unit_prices(f, amounts, ws, we);
        if prices.is_empty() {
            continue;
        }
        let all = periods(f, ws, we);
        for (price, unite) in prices {
            let ps: Vec<(f64, f64)> = all
                .iter()
                .filter(|(_, u, _)| *u == unite)
                .map(|(q, _, t)| (*q, *t))
                .collect();
            let closes = |v: f64, n: usize| (v - award).abs() < 0.02_f64.max(0.005 * n as f64);
            for (q, taux) in &ps {
                let read = if closes(q * price, 1) {
                    Tariff::Single {
                        quantite: *q,
                        taux: None,
                    }
                } else if closes(q * price * taux / 100.0, 1) {
                    Tariff::Single {
                        quantite: *q,
                        taux: Some(*taux),
                    }
                } else {
                    continue;
                };
                if !found.contains(&(price, unite, read.clone())) {
                    found.push((price, unite, read));
                }
            }
            let whole: f64 = ps.iter().map(|(q, t)| q * t / 100.0 * price).sum();
            let split = Tariff::Split {
                periods: ps.clone(),
            };
            if ps.len() > 1
                && closes(whole, ps.len())
                && !found.contains(&(price, unite, split.clone()))
            {
                found.push((price, unite, split));
            }
        }
    }
    match found.len() {
        1 => found.pop(),
        _ => None,
    }
}

/// Writes on each poste priced by the unit the tariff its award proves.
fn tariffs(
    x: &mut Extraction,
    chars: &[char],
    f: &[char],
    labels: &[(usize, usize, Option<&'static str>)],
) {
    let wanted = |p: &super::Poste| {
        p.valeur_unitaire.is_none()
            && p.quantite.is_none()
            && p.composantes.is_empty()
            && p.montant_alloue.is_some_and(|m| m > 0.0)
            && p.poste.as_deref().is_some_and(|c| BY_UNIT.contains(&c))
    };
    if !x
        .quantum
        .victimes
        .iter()
        .any(|v| v.postes.iter().any(wanted))
    {
        return;
    }
    let amounts = leftovers::amounts_in(chars, f);
    for v in &mut x.quantum.victimes {
        for p in v.postes.iter_mut().filter(|p| wanted(p)) {
            let (code, award) = (p.poste.clone().unwrap(), p.montant_alloue.unwrap());
            let Some((price, unite, read)) = settle_tariff(f, &amounts, labels, &code, award)
            else {
                continue;
            };
            p.valeur_unitaire = Some(price);
            p.unite = Some(unite);
            match read {
                Tariff::Single { quantite, taux } => {
                    p.quantite = Some(quantite);
                    p.taux_pct = taux.filter(|t| *t != 100.0);
                }
                Tariff::Split { periods } => {
                    p.composantes = periods
                        .into_iter()
                        .map(|(quantite, taux)| Composante {
                            quantite: Some(quantite),
                            unite: Some(unite),
                            valeur_unitaire: Some(price),
                            taux_pct: (taux != 100.0).then_some(taux),
                            ..Default::default()
                        })
                        .collect();
                }
            }
        }
    }
}

// ── the text, in characters ────────────────────────────────────────────────

fn word_at(f: &[char], i: usize, w: &str) -> bool {
    let n = w.chars().count();
    f.len() >= i + n
        && f[i..i + n].iter().copied().eq(w.chars())
        && !f.get(i + n).is_some_and(|c| c.is_alphanumeric())
        && !(i > 0 && f[i - 1].is_alphanumeric())
}

fn find_all(f: &[char], w: &str) -> Vec<(usize, usize)> {
    let n = w.chars().count();
    (0..f.len().saturating_sub(n.saturating_sub(1)))
        .filter(|i| word_at(f, *i, w))
        .map(|i| (i, i + n))
        .collect()
}

fn text_of(f: &[char], s: usize, e: usize) -> String {
    f[s.min(f.len())..e.min(f.len())].iter().collect()
}

fn digits_at(f: &[char], i: usize) -> Option<(u32, usize)> {
    if !f.get(i)?.is_ascii_digit() {
        return None;
    }
    let mut j = i;
    while f.get(j).is_some_and(char::is_ascii_digit) {
        j += 1;
    }
    f[i..j]
        .iter()
        .collect::<String>()
        .parse()
        .ok()
        .map(|v| (v, j))
}

/// An unsigned integer, refused when it continues a longer number.
fn uint_at(f: &[char], i: usize) -> Option<(u32, usize)> {
    if i > 0 && matches!(f[i - 1], '0'..='9' | ',' | '.') {
        return None;
    }
    digits_at(f, i)
}

/// A decimal number in plain digits: no thousands group, the amounts of the
/// text being read by [`leftovers::amounts_in`].
fn number_at(f: &[char], i: usize) -> Option<(f64, usize)> {
    let (whole, mut j) = uint_at(f, i)?;
    let mut out = f64::from(whole);
    if matches!(f.get(j), Some(',') | Some('.')) {
        if let Some((frac, k)) = digits_at(f, j + 1) {
            if k - j - 1 <= 2 {
                out += f64::from(frac) / 10f64.powi((k - j - 1) as i32);
                j = k;
            }
        }
    }
    Some((out, j))
}

/// Only allowed words in the gap, and never a digit.
fn gap_ok(gap: &str, allowed: &[&str]) -> bool {
    if gap.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    gap.split(|c: char| !c.is_alphabetic())
        .all(|w| w.is_empty() || allowed.contains(&w))
}

fn skip_spaces(f: &[char], mut i: usize) -> usize {
    while f.get(i) == Some(&' ') {
        i += 1;
    }
    i
}

/// The single value a multiset agrees on.
fn unanimous(vals: &[f64]) -> Option<f64> {
    let first = *vals.first()?;
    vals.iter()
        .all(|v| (v - first).abs() < 0.005)
        .then_some(first)
}

fn majority(vals: &[f64]) -> Option<f64> {
    let count = |x: f64| vals.iter().filter(|v| (**v - x).abs() < 0.005).count();
    let best = vals.iter().copied().max_by_key(|v| count(*v))?;
    let top = count(best);
    vals.iter()
        .all(|v| (v - best).abs() < 0.005 || count(*v) < top)
        .then_some(best)
}

// ── partage de responsabilité ──────────────────────────────────────────────

/// A rate or a fraction the text ties to a reduction of the right to
/// compensation: the whole liquidation is then not certain.
fn reduced(f: &[char], mode: Mode) -> bool {
    let words: &[&str] = match mode {
        Mode::Sure => &REDUCTION_WORDS,
        Mode::Extended => &REDUCTION_NAMED,
    };
    let window = |s: usize, e: usize| text_of(f, s.saturating_sub(80), (e + 80).min(f.len()));
    let reducing = |w: &str| {
        words.iter().any(|x| w.contains(x)) && !REDUCTION_NOISE.iter().any(|x| w.contains(x))
    };
    for i in 0..f.len() {
        if f[i] != '%' {
            continue;
        }
        let mut j = i;
        while j > 0 && f[j - 1] == ' ' {
            j -= 1;
        }
        if j > 0 && f[j - 1].is_ascii_digit() && reducing(&window(i, i + 1)) {
            return true;
        }
    }
    FRACTIONS.iter().any(|fr| {
        find_all(f, fr)
            .iter()
            .any(|(s, e)| reducing(&window(*s, *e)))
    })
}

// ── sexe ───────────────────────────────────────────────────────────────────

/// The pseudonymised name a civility introduces: « [G] [K] », at most three
/// tokens.
fn name_at(f: &[char], i: usize) -> Option<(String, usize)> {
    let mut j = i;
    let mut name = String::new();
    for _ in 0..3 {
        let k = skip_spaces(f, j);
        if f.get(k) != Some(&'[') {
            break;
        }
        let Some(end) = (k..f.len()).find(|p| f[*p] == ']') else {
            break;
        };
        if end - k > 42 {
            break;
        }
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(&text_of(f, k, end + 1));
        j = end + 1;
    }
    (!name.is_empty()).then_some((name, j))
}

fn civilities(f: &[char], from: usize) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for (surface, sex) in CIVILITIES {
        for (s, e) in find_all(f, surface) {
            if s < from {
                continue;
            }
            let before = text_of(f, s.saturating_sub(14), s);
            let before = before.trim_end();
            if TITLES
                .iter()
                .any(|t| before.split(|c: char| !c.is_alphabetic()).next_back() == Some(*t))
            {
                continue;
            }
            if let Some((name, _)) = name_at(f, e) {
                out.push((name, sex));
            }
        }
    }
    out
}

/// The sex of the name the decision names most, when one name stands out.
fn best_sex(seen: &[(String, &'static str)], margin: f64) -> Option<&'static str> {
    let count = |n: &str| seen.iter().filter(|(m, _)| m == n).count();
    let top = seen.iter().max_by_key(|(n, _)| count(n))?;
    let best = count(&top.0);
    if seen
        .iter()
        .any(|(n, _)| *n != top.0 && (count(n) as f64) * margin > best as f64)
    {
        return None;
    }
    let male = seen
        .iter()
        .filter(|(n, s)| *n == top.0 && *s == "M")
        .count();
    let female = best - male;
    match male.cmp(&female) {
        std::cmp::Ordering::Greater => Some("M"),
        std::cmp::Ordering::Less => Some("F"),
        std::cmp::Ordering::Equal => None,
    }
}

fn sexe(f: &[char], dispositif: usize, mode: Mode) -> Option<&'static str> {
    let seen = civilities(f, 0);
    if mode == Mode::Extended {
        return best_sex(&seen, 1.0);
    }
    let sure = best_sex(&seen, 1.6)?;
    match best_sex(&civilities(f, dispositif), 1.0) {
        Some(other) if other != sure => None,
        _ => Some(sure),
    }
}

// ── taux de déficit fonctionnel permanent ──────────────────────────────────

fn percent_at(f: &[char], i: usize) -> Option<(f64, usize)> {
    let (value, end) = number_at(f, i)?;
    let j = skip_spaces(f, end);
    if f.get(j) == Some(&'%') {
        return Some((value, j + 1));
    }
    ["pour cent", "p. 100", "p.100"]
        .iter()
        .find(|w| word_at(f, j, w))
        .map(|w| (value, j + w.chars().count()))
}

/// « : 22. Les experts ont evalue a » : the administrative order numbers the
/// paragraph its heading announces, and that number is not a figure.
fn past_paragraph_number(gap: &str) -> &str {
    let head = gap.trim_start().trim_start_matches(':').trim_start();
    let rest = head.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() < head.len() && rest.starts_with(". ") {
        &rest[2..]
    } else {
        gap
    }
}

/// The percentage that follows a label, when nothing priced comes between.
fn rate_after(f: &[char], from: usize, window: usize) -> Option<f64> {
    let stop = (from + window).min(f.len());
    for i in from..stop {
        let Some((value, _)) = percent_at(f, i) else {
            continue;
        };
        let gap = text_of(f, from, i);
        let gap = past_paragraph_number(&gap);
        if gap.contains("euro")
            || gap.chars().any(|c| c.is_ascii_digit())
            || CLAIMED.iter().any(|w| gap.contains(w))
        {
            return None;
        }
        return Some(value);
    }
    None
}

/// « à 18 %, soit 15 % pour les cervicalgies, 3 % au titre du syndrome »: the
/// text adds its own terms up, and a term is a part of a rate, not the rate.
fn decompositions(f: &[char]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for i in 0..f.len() {
        let Some((total, end)) = percent_at(f, i) else {
            continue;
        };
        let mut k = skip_spaces(f, end);
        if f.get(k) == Some(&',') {
            k = skip_spaces(f, k + 1);
        }
        if !word_at(f, k, "soit") {
            continue;
        }
        let stop = (k + 300).min(f.len());
        let mut parts: Vec<f64> = Vec::new();
        while k < stop {
            match percent_at(f, k) {
                Some((v, e)) => {
                    parts.push(v);
                    k = e;
                }
                None => k += 1,
            }
        }
        let mut sum = 0.0;
        for (n, v) in parts.iter().enumerate() {
            sum += v;
            if n > 0 && (sum - total).abs() < 0.005 {
                out.extend(parts[..=n].iter().map(|p| (*p, total)));
                break;
            }
        }
    }
    out
}

/// The rates a label points at, and the total each of them is a term of. Reading
/// a term leaves the choice open between it and the total: the award settles it,
/// unanimity no longer can.
fn dfp_rates(f: &[char]) -> Vec<f64> {
    let mut seen: Vec<(usize, f64)> = Vec::new();
    for label in DFP_LABELS {
        for (s, e) in find_all(f, label) {
            if let Some(v) = rate_after(f, e, 45) {
                seen.push((s, v));
            }
        }
    }
    seen.sort_by_key(|(s, _)| *s);
    let mut out: Vec<f64> = seen.into_iter().map(|(_, v)| v).collect();
    for (part, total) in decompositions(f) {
        if out.iter().any(|v| (v - part).abs() < 0.005)
            && !out.iter().any(|v| (v - total).abs() < 0.005)
        {
            out.push(total);
        }
    }
    out
}

fn pick_rate(vals: &[f64], mode: Mode) -> Option<f64> {
    match mode {
        Mode::Extended => majority(vals),
        Mode::Sure => unanimous(vals),
    }
}

// ── date et âge à la consolidation ─────────────────────────────────────────

fn date_at(f: &[char], i: usize) -> Option<(String, usize)> {
    let (day, mut j) = uint_at(f, i)?;
    if !(1..=31).contains(&day) {
        return None;
    }
    // « 1er avril 2019 »: the ordinal is glued to the digit.
    if f.get(j) == Some(&'e') && f.get(j + 1) == Some(&'r') {
        j += 2;
    }
    if matches!(f.get(j), Some('/') | Some('.') | Some('-')) {
        let sep = f[j];
        let (month, k) = uint_at(f, j + 1)?;
        if !(1..=12).contains(&month) || f.get(k) != Some(&sep) {
            return None;
        }
        let (year, e) = uint_at(f, k + 1)?;
        return (1900..=2100)
            .contains(&year)
            .then(|| (format!("{year:04}-{month:02}-{day:02}"), e));
    }
    let k = skip_spaces(f, j);
    let (month, k) = MONTHS
        .iter()
        .find(|(name, _)| word_at(f, k, name))
        .map(|(name, n)| (*n, k + name.chars().count()))?;
    let p = skip_spaces(f, k);
    let (year, e) = uint_at(f, p)?;
    (1900..=2100)
        .contains(&year)
        .then(|| (format!("{year:04}-{month:02}-{day:02}"), e))
}

/// Every mention of the consolidation. `dated` drops those that open a period,
/// « de la consolidation au 1er novembre 2023 » giving its second bound and not
/// the day the state was fixed; an age reads on the same mentions all the same.
/// Does a party claim this consolidation? « Si les requérants se prévalent de
/// la date de consolidation du 18 septembre 2013 » recites what the court then
/// refuses, and the clause the cue sits in is what says so.
fn claimed(f: &[char], at: usize) -> bool {
    let from = at.saturating_sub(300);
    let open = (from..at)
        .rev()
        .find(|i| matches!(f[*i], '.' | ';' | ':') || (f[*i] == '-' && *i > 0 && f[*i - 1] == ' '))
        .map_or(from, |i| i + 1);
    let clause = text_of(f, open, at);
    PARTY_DATE.iter().any(|w| clause.contains(w))
}

fn consolidation_cues(f: &[char], dated: bool) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = CONSOLIDATION
        .iter()
        .flat_map(|w| find_all(f, w))
        .filter(|(s, _)| {
            let before = text_of(f, s.saturating_sub(40), *s);
            let trimmed = before.trim_end();
            let period =
                PERIOD_BEFORE.iter().any(|p| trimmed.ends_with(p)) && !trimmed.contains("date");
            // « le lendemain de la date de consolidation » names another day, and
            // a date an expert or a party merely proposes is not the one fixed.
            let derived = before.contains("lendemain") || before.contains("veille");
            // « l'etat de sante de la patiente n'etait pas consolide » fixes
            // nothing, and the date that follows is another event's.
            let denied = ["pas", "non", "jamais"]
                .iter()
                .any(|w| trimmed.ends_with(w));
            !denied
                && !(dated && (period || derived || before.contains("propos") || claimed(f, *s)))
        })
        .collect();
    out.sort_unstable();
    out
}

fn consolidation_date(f: &[char], mode: Mode) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for (_, e) in consolidation_cues(f, true) {
        let stop = (e + 70).min(f.len());
        let mut i = e;
        let mut taken = false;
        while i < stop {
            let Some((date, end)) = date_at(f, i) else {
                i += 1;
                continue;
            };
            if !taken && !gap_ok(&text_of(f, e, i), &DATE_GAP) {
                break;
            }
            // « consolidé le 22 juillet 2011 pour l'épaule gauche et le 6 janvier
            // 2012 pour l'épaule droite » fixes two states, and neither is the one.
            seen.push(date);
            taken = true;
            i = end;
        }
    }
    let count = |d: &String| seen.iter().filter(|x| *x == d).count();
    let best = seen.iter().max_by_key(|d| count(d))?;
    let unique = seen.iter().all(|d| d == best || count(d) < count(best));
    match mode {
        Mode::Extended => unique.then(|| best.clone()),
        Mode::Sure => seen.iter().all(|d| d == best).then(|| best.clone()),
    }
}

/// « âgée de 49 ans au jour de la consolidation », and the reverse order.
fn age_before(f: &[char], cue: usize) -> Option<f64> {
    let from = cue.saturating_sub(120);
    let mut found = None;
    for i in from..cue {
        if !word_at(f, i, "ans") && !word_at(f, i, "annees") {
            continue;
        }
        let end = i + if word_at(f, i, "ans") { 3 } else { 6 };
        let mut j = i;
        while j > from && f[j - 1] == ' ' {
            j -= 1;
        }
        let Some(start) = (from..j)
            .rev()
            .find(|k| uint_at(f, *k).is_some_and(|(_, e)| e == j))
        else {
            continue;
        };
        let (age, _) = uint_at(f, start)?;
        let head = text_of(f, from, start);
        let head = head.trim_end();
        let head = head.strip_suffix("de").map_or(head, str::trim_end);
        if !AGE_HEAD
            .iter()
            .any(|w| head.split(|c: char| !c.is_alphabetic()).next_back() == Some(*w))
        {
            continue;
        }
        if gap_ok(&text_of(f, end, cue), &AGE_BEFORE_GAP) && cue - end <= 40 {
            found = Some(f64::from(age));
        }
    }
    found
}

/// « à la consolidation (58 ans) », « la consolidation, soit 49 ans ».
fn age_after(f: &[char], cue_end: usize) -> Option<f64> {
    let stop = (cue_end + 60).min(f.len());
    for i in cue_end..stop {
        let Some((age, end)) = uint_at(f, i) else {
            continue;
        };
        let j = skip_spaces(f, end);
        if !word_at(f, j, "ans") && !word_at(f, j, "annees") {
            return None;
        }
        return gap_ok(&text_of(f, cue_end, i), &AGE_AFTER_GAP).then(|| f64::from(age));
    }
    None
}

fn consolidation_age(f: &[char], mode: Mode) -> Option<f64> {
    let mut seen: Vec<f64> = Vec::new();
    for (s, e) in consolidation_cues(f, false) {
        seen.extend(age_before(f, s));
        // « entre l'accident et la consolidation (6 ans) » counts years.
        if !text_of(f, s.saturating_sub(80), s).contains(" entre ") {
            seen.extend(age_after(f, e));
        }
    }
    let one = unanimous(&seen)?;
    match mode {
        Mode::Extended => Some(one),
        Mode::Sure => (seen.len() >= 2).then_some(one),
    }
}

// ── cotation sur sept ──────────────────────────────────────────────────────

/// Every poste label of the text, with its code when the vocabulary settles it:
/// a note belongs to the label it follows, and an ambiguous label still ends
/// the section of the previous one.
fn poste_labels(scan: &DocScan, f: &[char]) -> Vec<(usize, usize, Option<&'static str>)> {
    let mut out: Vec<(usize, usize, Option<&'static str>)> =
        vocabulary::cues(scan.folded(), scan.byte2char(), false)
            .into_iter()
            .filter_map(|c| match c.cue {
                Cue::Poste(code) => Some((c.s, c.e, Some(code))),
                _ => None,
            })
            .collect();
    for (surface, _) in AMBIGUOUS {
        for (s, e) in find_all(f, surface) {
            if !out.iter().any(|(a, b, _)| s < *b && *a < e) {
                out.push((s, e, None));
            }
        }
    }
    out.sort_unstable();
    out
}

/// A note on seven: « 3/7 », « 3,5/7 », « 4 sur 7 ».
fn note_at(f: &[char], i: usize) -> Option<f64> {
    let (value, end) = number_at(f, i)?;
    if value <= 0.0 || value > 7.0 || (value.fract() != 0.0 && (value.fract() - 0.5).abs() > 1e-9) {
        return None;
    }
    let j = skip_spaces(f, end);
    let j = match f.get(j) {
        Some('/') => j + 1,
        _ if word_at(f, j, "sur") => j + 3,
        _ => return None,
    };
    let j = skip_spaces(f, j);
    let j = if word_at(f, j, "une echelle de") {
        skip_spaces(f, j + 14)
    } else {
        j
    };
    (word_at(f, j, "sept")
        || (f.get(j) == Some(&'7') && !f.get(j + 1).is_some_and(char::is_ascii_alphanumeric)))
    .then_some(value)
}

fn cotation(
    f: &[char],
    labels: &[(usize, usize, Option<&'static str>)],
    code: &str,
    mode: Mode,
) -> Option<f64> {
    let mut seen: Vec<(f64, bool)> = Vec::new();
    for i in 0..f.len() {
        let Some(value) = note_at(f, i) else { continue };
        let Some((_, end, owner)) = labels.iter().rev().find(|(_, e, _)| *e <= i) else {
            continue;
        };
        if *owner != Some(code) || i - end > 900 {
            continue;
        }
        let gap = text_of(f, *end, i);
        seen.push((
            value,
            gap.chars().count() <= 30 && gap_ok(&gap, &COTATION_GAP),
        ));
    }
    let vals: Vec<f64> = seen.iter().map(|(v, _)| *v).collect();
    let one = unanimous(&vals)?;
    match mode {
        Mode::Extended => Some(one),
        Mode::Sure => (seen.len() >= 2 && seen.iter().any(|(_, tight)| *tight)).then_some(one),
    }
}

// ── valeur du point du déficit permanent ───────────────────────────────────

/// Every amount the text writes next to the word « point ».
fn point_values(chars: &[char], f: &[char]) -> Vec<f64> {
    // The word, not « Pointe-à-Pitre ».
    let words: Vec<usize> = find_all(f, "point")
        .into_iter()
        .filter(|(s, e)| {
            (*s == 0 || !f[s - 1].is_alphabetic())
                && f.get(*e).is_none_or(|c| !c.is_alphabetic() || *c == 's')
        })
        .map(|(s, _)| s)
        .collect();
    let mut out: Vec<f64> = leftovers::amounts_in(chars, f)
        .into_iter()
        .filter(|a| {
            words.iter().any(|w| {
                (a.start.saturating_sub(110)..a.start).contains(w)
                    || (a.end..a.end + 60).contains(w)
            })
        })
        .map(|a| a.value as f64 / 1000.0)
        .collect();
    // « la valeur du point … est fixée à 4.675 » : the point stands bare, and
    // a point is never worth less than a few hundred euros.
    for (_, e) in find_all(f, "valeur du point") {
        let stop = (e + 90).min(f.len());
        let Some(i) = (e..stop).find(|&i| f[i].is_ascii_digit() && !f[i - 1].is_ascii_digit())
        else {
            continue;
        };
        let Some((value, end)) = grouped_number_at(f, i) else {
            continue;
        };
        if value >= 500.0
            && f.get(end).is_none_or(|c| *c != '%')
            && !out.iter().any(|x| (x - value).abs() < 0.005)
        {
            out.push(value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispositif::extract;
    use lj_extract::scan::scan;

    const HEAD: &str = "COUR D'APPEL\nARRET\nMOTIFS DE LA DECISION\n";

    fn victim(text: &str, mode: Mode) -> crate::Victime {
        let x = extract(&scan(text), mode);
        x.quantum.victimes.into_iter().next().expect("a victim")
    }

    fn dispositif(body: &str) -> String {
        format!("{HEAD}{body}\nPAR CES MOTIFS\nLa cour,\nCONDAMNE la société AXA à payer à M. [K] la somme de 21 600 euros au titre du déficit fonctionnel permanent ;")
    }

    #[test]
    fn a_full_liquidation_shares_nothing() {
        let v = victim(&dispositif("Sur les préjudices."), Mode::Sure);
        assert_eq!(v.partage_responsabilite_pct, Some(100.0));
        let text = dispositif(
            "Le droit à indemnisation de M. [K] est réduit de moitié en raison de sa faute.",
        );
        assert_eq!(victim(&text, Mode::Sure).partage_responsabilite_pct, None);
    }

    #[test]
    fn the_rate_of_the_permanent_deficit_reads_after_its_label() {
        let v = victim(
            &dispositif("Le déficit fonctionnel permanent est fixé à 12 %."),
            Mode::Sure,
        );
        assert_eq!(v.taux_dfp_pct, Some(12.0));
        assert_eq!(v.sexe.as_deref(), Some("M"));
        let two = dispositif("Le déficit fonctionnel permanent est de 12 % selon l'expert. Le DFP retenu est de 15 %, et le déficit fonctionnel permanent de 15 % justifie une valeur du point plus élevée.");
        assert_eq!(victim(&two, Mode::Sure).taux_dfp_pct, None);
        assert_eq!(victim(&two, Mode::Extended).taux_dfp_pct, Some(15.0));
    }

    fn dft(body: &str, montant: &str) -> String {
        format!("{HEAD}{body}\nPAR CES MOTIFS\nLa cour,\nCONDAMNE la société AXA à payer à M. [K] la somme de {montant} euros au titre du déficit fonctionnel temporaire ;")
    }

    fn poste_dft(text: &str) -> crate::Poste {
        victim(text, Mode::Sure)
            .postes
            .into_iter()
            .find(|p| p.poste.as_deref() == Some("DFT"))
            .expect("un DFT")
    }

    #[test]
    fn a_daily_tariff_is_kept_only_when_the_product_gives_the_poste() {
        let p = poste_dft(&dft(
            "Le déficit fonctionnel temporaire total a duré 51 jours, indemnisés sur la base de 25 euros par jour.",
            "1 275",
        ));
        assert_eq!(p.valeur_unitaire, Some(25.0));
        assert_eq!(p.quantite, Some(51.0));
        assert_eq!(p.unite, Some(Unite::Jour));
        // The same passage, another award: no pair closes any more.
        let muet = poste_dft(&dft(
            "Le déficit fonctionnel temporaire total a duré 51 jours, indemnisés sur la base de 25 euros par jour.",
            "1 400",
        ));
        assert_eq!(muet.valeur_unitaire, None);
        assert_eq!(muet.quantite, None);
    }

    #[test]
    fn the_periods_a_court_adds_up_become_the_components_of_the_poste() {
        let p = poste_dft(
            &dft("Sur le déficit fonctionnel temporaire. Une base de calcul à hauteur de 32 euros par jour est adaptée et sera retenue. Il sera donc alloué : déficit fonctionnel temporaire partiel classe 2 du 19/03/2023 au 03/04/2023, soit 16 jours : 16 x 32 x 25 % = 128 euros ; déficit fonctionnel temporaire partiel classe 1 du 04/04/2023 au 18/03/2024, soit 350 jours : 350 x 32 x 10 % = 1 120 euros.", "1 248"),
        );
        assert_eq!(p.valeur_unitaire, Some(32.0));
        assert_eq!(p.unite, Some(Unite::Jour));
        assert_eq!(p.quantite, None);
        let parts: Vec<(Option<f64>, Option<f64>)> = p
            .composantes
            .iter()
            .map(|c| (c.quantite, c.taux_pct))
            .collect();
        assert_eq!(parts, [(Some(16.0), Some(25.0)), (Some(350.0), Some(10.0))]);
    }

    #[test]
    fn two_tariffs_that_both_close_write_nothing() {
        let p = poste_dft(
            &dft("La victime sollicite 30 jours sur la base de 20 euros par jour. L'assureur propose 20 jours sur la base de 30 euros par jour.", "600"),
        );
        assert_eq!(p.valeur_unitaire, None);
        assert_eq!(p.unite, None);
    }

    #[test]
    fn the_point_value_is_kept_only_when_the_product_gives_the_poste() {
        let v = victim(
            &dispositif("Le déficit fonctionnel permanent est fixé à 12 %, sur la base d'une valeur du point de 1 800 euros."),
            Mode::Sure,
        );
        let p = v
            .postes
            .iter()
            .find(|p| p.poste.as_deref() == Some("DFP"))
            .expect("DFP");
        assert_eq!(p.valeur_unitaire, Some(1800.0));
        assert_eq!(p.unite, Some(Unite::Point));
        let off = dispositif("Le déficit fonctionnel permanent est fixé à 12 %, sur la base d'une valeur du point de 1 900 euros.");
        let v = victim(&off, Mode::Sure);
        let p = v
            .postes
            .iter()
            .find(|p| p.poste.as_deref() == Some("DFP"))
            .expect("DFP");
        assert_eq!(p.valeur_unitaire, None);
    }

    #[test]
    fn the_award_settles_the_rate_the_text_writes_twice_over() {
        let text = dispositif(
            "Le déficit fonctionnel permanent est de 10 % selon l'expert. Le déficit fonctionnel permanent retenu est de 12 %, pour une valeur du point de 1 800 euros.",
        );
        let v = victim(&text, Mode::Sure);
        assert_eq!(v.taux_dfp_pct, Some(12.0));
        let p = v
            .postes
            .iter()
            .find(|p| p.poste.as_deref() == Some("DFP"))
            .expect("DFP");
        assert_eq!(p.valeur_unitaire, Some(1800.0));
    }

    #[test]
    fn the_consolidation_carries_its_date_and_its_age() {
        let text = dispositif(
            "L'état de M. [K] a été déclaré consolidé le 30 juin 2021, alors qu'il était âgé de 42 ans à la date de la consolidation.",
        );
        let v = victim(&text, Mode::Extended);
        assert_eq!(v.date_consolidation.as_deref(), Some("2021-06-30"));
        assert_eq!(v.age_consolidation, Some(42.0));
    }

    #[test]
    fn a_note_belongs_to_the_poste_whose_section_holds_it() {
        let text = format!(
            "{HEAD}Les souffrances endurées sont cotées à 3,5/7 et le préjudice esthétique permanent à 2/7.\nPAR CES MOTIFS\nLa cour,\nCONDAMNE la société AXA à payer à M. [K] la somme de 8 000 euros au titre des souffrances endurées cotées à 3,5/7 et 2 000 euros au titre du préjudice esthétique permanent coté à 2/7 ;"
        );
        let v = victim(&text, Mode::Sure);
        let note = |code: &str| {
            v.postes
                .iter()
                .find(|p| p.poste.as_deref() == Some(code))
                .and_then(|p| p.cotation)
        };
        assert_eq!(note("SE"), Some(3.5));
        assert_eq!(note("PEP"), Some(2.0));
    }

    #[test]
    fn a_term_of_a_sum_leaves_the_rate_to_the_award() {
        let body = "L'expert a évalué ce déficit à 18 %, soit 15 % pour les cervicalgies et 3 % pour le syndrome post-commotionnel. Le déficit fonctionnel permanent est fixé à 15 % pour les cervicalgies.";
        let text = dispositif(body);
        assert_eq!(victim(&text, Mode::Sure).taux_dfp_pct, None);
        assert_eq!(victim(&text, Mode::Extended).taux_dfp_pct, None);
        let priced = dispositif(&format!("{body} La valeur du point est de 1 200 euros."));
        let v = victim(&priced, Mode::Sure);
        assert_eq!(v.taux_dfp_pct, Some(18.0));
    }

    #[test]
    fn the_day_after_the_consolidation_is_not_the_consolidation() {
        let text = dispositif(
            "La consolidation médico-légale est fixée le 20 décembre 2018, date du dernier certificat du psychiatre. L'indemnité de tierce personne court du 21 décembre 2018, lendemain de la date de consolidation au 8 novembre 2021.",
        );
        let v = victim(&text, Mode::Sure);
        assert_eq!(v.date_consolidation.as_deref(), Some("2018-12-20"));
    }

    #[test]
    fn the_date_a_party_claims_is_not_the_one_the_court_fixes() {
        let text = dispositif(
            "Si les requérants se prévalent de la date de consolidation du 18 septembre 2013 retenue par le premier rapport, l'enfant n'était âgée que de 4 ans à cette date. La consolidation médico-légale est fixée le 9 mars 2015.",
        );
        let v = victim(&text, Mode::Sure);
        assert_eq!(v.date_consolidation.as_deref(), Some("2015-03-09"));
    }

    #[test]
    fn two_notes_in_the_same_section_settle_nothing() {
        let text = dispositif(
            "Les souffrances endurées sont évaluées à 3/7 par le premier expert et à 4/7 par le second.",
        );
        let v = victim(&text, Mode::Extended);
        assert!(v.postes.iter().all(|p| p.cotation.is_none()));
    }

    #[test]
    fn the_fraction_of_the_next_term_and_the_tariff_of_the_next_sum_stay_theirs() {
        let text = dft("Sur le déficit fonctionnel temporaire\n\nMadame [M] [Z] et Monsieur [N] [B] sollicitent la somme de 63 euros sur une base journalière de 30 euros tandis que la société AXA propose 30 euros.\n\nLe déficit fonctionnel temporaire sera indemnisé sur une base journalière de 30 euros comme suit :\n(1 jour x 30 euros) + (11 jours x 30 euros x 10%)\n=30 + 33\n= 63 euros.\n\nLe préjudice sera évalué à la somme de 63 euros.", "63");
        let p = poste_dft(&text);
        assert_eq!(p.valeur_unitaire, Some(30.0), "{p:?}");
        assert_eq!(p.quantite, None, "{p:?}");
        assert_eq!(p.composantes.len(), 2, "{p:?}");
    }

    #[test]
    fn the_court_s_arithmetic_names_the_rate_the_expert_did_not_write() {
        let text = format!("{HEAD}Sur le déficit fonctionnel permanent : le médecin expert conclut à un déficit fonctionnel permanent de 58 %. Au regard de la gravité des séquelles psychologiques, il y a lieu de retenir un taux de 64%. Au regard de la situation de M. [H], âgé de 32 ans à la date de sa consolidation et atteint d'un taux d'incapacité de 64%, la valeur du point d'indemnisation du déficit fonctionnel permanent est fixée à 4.675. Dès lors, il convient d'allouer à M. [H] la somme de 299.200 € (4.675 x 64), au titre du déficit fonctionnel permanent.\nPAR CES MOTIFS\nLe tribunal,\nCONDAMNE la société [X] à payer à M. [H] la somme de 299 200 euros au titre du déficit fonctionnel permanent ;");
        let v = victim(&text, Mode::Sure);
        assert_eq!(v.taux_dfp_pct, Some(64.0));
        let dfp = v
            .postes
            .iter()
            .find(|p| p.poste.as_deref() == Some("DFP"))
            .expect("DFP");
        assert_eq!(dfp.valeur_unitaire, Some(4675.0));
    }

    #[test]
    fn the_years_between_the_accident_and_the_consolidation_are_no_age() {
        let text = dispositif("Les souffrances doivent être prises en compte sur une longue durée entre la survenance de l'accident et la consolidation (6 ans).");
        assert_eq!(victim(&text, Mode::Extended).age_consolidation, None);
    }
}
