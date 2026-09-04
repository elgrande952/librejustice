//! What the two writers of `decisions.case_name_keys`, `renvois` and
//! `sync-ce-list`, share: the rarity thresholds and the short key.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::Result;
use lj_core::case_names::short_case_name_key;
use lj_store::repository::{DecisionRepository, ElectedCaseName};

/// Rarity threshold of a name, in decisions writing it. Elected names go from 4
/// (Duvignères) to 5 829 (Boulay), the parsing scrap from 48 493 (Loire) up;
/// « Ville de Paris » and « La Poste » pass 10 000, « Commune de Béziers » makes 470.
pub const NOM_TROP_COURANT: i64 = 10_000;

/// Rarity threshold of a short key, a place or a company written far more than
/// cited: Béziers 13 760, Agen 15 144, Chambéry 29 085, la Garde 253 518. Under
/// it, the keys fire on 0.55 % of 60 days of queries, above it « garde à vue » too.
pub const NOM_COURT_TROP_COURANT: i64 = 20_000;

/// Adds to every entry the short key its keys derive, unless the corpus writes it
/// in [`NOM_COURT_TROP_COURANT`] decisions or more. Returns how many were refused.
pub async fn add_short_keys(
    repo: &DecisionRepository<'_>,
    by_decision: &mut BTreeMap<i64, ElectedCaseName>,
) -> Result<usize> {
    let shorts: BTreeSet<String> = by_decision
        .values()
        .flat_map(|e| e.keys.iter().filter_map(|k| short_case_name_key(k)))
        .collect();
    let uniques: Vec<&str> = shorts.iter().map(String::as_str).collect();
    let courants: HashSet<String> = repo
        .frequent_names(&uniques, NOM_COURT_TROP_COURANT)
        .await?
        .into_iter()
        .collect();
    for e in by_decision.values_mut() {
        let derived: Vec<String> = e
            .keys
            .iter()
            .filter_map(|k| short_case_name_key(k))
            .collect();
        for short in derived {
            if !courants.contains(&short) && !e.keys.contains(&short) {
                e.keys.push(short);
            }
        }
    }
    Ok(courants.len())
}
