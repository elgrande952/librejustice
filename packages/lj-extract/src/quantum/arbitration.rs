//! Devant un compte qui ne tombe pas, dire quelle correction serait la plus
//! performante, et à quel titre. Rien n'est appliqué : une correction se déclare,
//! avec sa raison.

use rustc_hash::FxHashMap;

use super::spreadsheet::{compute_with, Spreadsheet};
use super::Quantum;

/// Une correction candidate, avec de quoi la juger.
#[derive(Debug, Clone)]
pub struct Proposition {
    pub cell: String,
    pub written: f64,
    pub wanted: f64,
    /// La relation qui réclame l'autre valeur.
    pub by_relation: String,
    pub arity: usize,
    pub typo: Option<String>,
    /// Contradictions restantes si la case portait `demande`, et celles d'avant.
    pub remaining: usize,
    pub start_count: usize,
}

impl Proposition {
    /// Une relation de deux ou trois termes lus désigne la case fautive et une
    /// seule : la corriger est une déduction. Une somme de seize termes ne
    /// désigne rien, l'écart pouvant tenir à n'importe lequel.
    pub fn is_deduction(&self) -> bool {
        self.arity <= 3
    }
}

/// L'espace des coquilles de frappe et d'OCR : un chiffre substitué, deux
/// chiffres échangés, et rien d'autre. Clés en centimes, pour qu'un flottant ne
/// serve jamais de clé.
///
/// Le signal est asymétrique. Absent, il exclut l'erreur de saisie ; présent, il
/// ne prouve rien, une substitution produisant un écart de la forme `m x 10^k`
/// que les écarts comptables ronds ont aussi.
pub fn variants(v: f64) -> FxHashMap<i64, String> {
    let cents = (v.abs() * 100.0).round() as i64;
    let digits: Vec<u8> = format!("{cents}").into_bytes();
    let mut out = FxHashMap::default();
    for (i, &c) in digits.iter().enumerate() {
        for d in b'0'..=b'9' {
            if d == c {
                continue;
            }
            let mut m = digits.clone();
            m[i] = d;
            if let Ok(n) = String::from_utf8_lossy(&m).parse::<i64>() {
                out.insert(
                    n,
                    format!("{}->{} en {}e position", c as char, d as char, i + 1),
                );
            }
        }
    }
    for i in 0..digits.len().saturating_sub(1) {
        if digits[i] == digits[i + 1] {
            continue;
        }
        let mut m = digits.clone();
        m.swap(i, i + 1);
        if let Ok(n) = String::from_utf8_lossy(&m).parse::<i64>() {
            out.insert(
                n,
                format!(
                    "{}{} lu {}{}",
                    digits[i] as char,
                    digits[i + 1] as char,
                    digits[i + 1] as char,
                    digits[i] as char
                ),
            );
        }
    }
    out.remove(&cents);
    out
}

/// Si l'écart entre le chiffre écrit et celui que les relations réclament
/// s'explique par une erreur de saisie, dire laquelle.
pub fn typo(written: f64, wanted: f64) -> Option<String> {
    if written < 0.0 || wanted < 0.0 {
        return None;
    }
    variants(written).remove(&((wanted * 100.0).round() as i64))
}

fn contradictions_if(r: &Quantum, cell: &str, v: f64) -> usize {
    let mut f = FxHashMap::default();
    f.insert(cell.to_string(), v);
    compute_with(r, &f).conflicts.len()
}

/// Les corrections candidates, la plus déductible d'abord.
///
/// Le critère n'est pas le nombre de contradictions qu'une valeur fait tomber :
/// on en fait tomber n'importe laquelle en adoptant ce que les relations
/// réclament, et on fabrique alors le chiffre au lieu de le lire. Le critère est
/// l'arité de la relation qui réclame l'autre valeur.
pub fn arbitrate(r: &Quantum, t: &Spreadsheet) -> Vec<Proposition> {
    let mut out = Vec::new();
    for c in &t.conflicts {
        if c.first_source != "lu" {
            continue;
        }
        let arity = t.arity_of(&c.cell, &c.second_source);
        out.push(Proposition {
            cell: c.cell.clone(),
            written: c.first,
            wanted: c.second,
            by_relation: c.second_source.clone(),
            arity,
            typo: typo(c.first, c.second),
            remaining: contradictions_if(r, &c.cell, c.second),
            start_count: t.conflicts.len(),
        });
    }
    out.sort_by_key(|p| (p.arity, p.remaining));
    out
}

/// Le plus petit jeu de déductions qui referme le record. Chaque membre en est
/// une, donc l'ensemble n'invente aucun chiffre, et `None` dit que rien ne
/// referme le compte sans en fabriquer un. Rien n'est appliqué.
///
/// Au-delà de douze candidates, le compte porte trop de contradictions
/// localisées pour que « le plus petit jeu » soit la question : les suivantes
/// sont ignorées, et un jeu peut alors être manqué.
pub fn resolution(r: &Quantum, t: &Spreadsheet) -> Option<Vec<Proposition>> {
    let candidates: Vec<Proposition> = arbitrate(r, t)
        .into_iter()
        .filter(Proposition::is_deduction)
        .take(12)
        .collect();
    let n = candidates.len() as u32;
    // Par taille croissante, pour rendre le plus petit jeu sans énumérer le reste.
    for k in 1..=n {
        for mask in (1u32..(1 << n)).filter(|m| m.count_ones() == k) {
            let chosen: Vec<&Proposition> = candidates
                .iter()
                .enumerate()
                .filter(|(i, _)| mask >> i & 1 == 1)
                .map(|(_, p)| p)
                .collect();
            // Une case ne porte qu'une valeur : deux propositions sur la même
            // case s'excluent, et le jeu qui les mêle n'est pas un candidat.
            let mut forced = FxHashMap::default();
            for p in &chosen {
                forced.insert(p.cell.clone(), p.wanted);
            }
            if forced.len() != chosen.len() || !compute_with(r, &forced).conflicts.is_empty() {
                continue;
            }
            return Some(chosen.into_iter().cloned().collect());
        }
    }
    None
}
