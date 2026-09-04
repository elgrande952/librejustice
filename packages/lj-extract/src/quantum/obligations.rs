//! Does a record hold what the dispositif writes: each obligation is held by a
//! field at the cent, declared in `hors_record` with a cause its context bears,
//! refused when declared against its context, or unmet.
use std::collections::{HashMap, HashSet};

use super::dispositif::{Nature, Obligation};
use super::Quantum;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Held,
    Declared,
    Refused,
    Unmet,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Held => "tenue",
            Status::Declared => "declaree",
            Status::Refused => "declaree a tort",
            Status::Unmet => "manquante",
        }
    }
}

/// Numeric fields that are not amounts: positions, ages, rates, counts.
const NOT_AMOUNTS: [&str; 16] = [
    "ancres",
    "lecture",
    "hors_record",
    "age_fait",
    "age_consolidation",
    "age_decision",
    "taux_dfp_pct",
    "taux_pct",
    "cotation",
    "quantite",
    "partage_responsabilite_pct",
    "perte_de_chance_pct",
    "duree_ans",
    "euro_de_rente",
    "passes",
    "version",
];

pub fn cents(x: f64) -> i64 {
    (x * 100.0).round() as i64
}

fn collect(v: &serde_json::Value, out: &mut HashSet<i64>) {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(x) = n.as_f64() {
                out.insert(cents(x));
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|x| collect(x, out)),
        serde_json::Value::Object(map) => {
            for (k, x) in map {
                if !NOT_AMOUNTS.contains(&k.as_str()) {
                    collect(x, out);
                }
            }
        }
        _ => {}
    }
}

/// Every amount a field of the record holds, in cents.
pub fn amounts(q: &Quantum) -> HashSet<i64> {
    let mut out = HashSet::new();
    collect(
        &serde_json::to_value(q).expect("a record serialises"),
        &mut out,
    );
    out
}

/// A caisse's amount may be declared as its debours or its rente; an amount the
/// extractor could not name takes the cause the reader gives it.
fn compatible(read: &str, declared: &str) -> bool {
    read == declared
        || read == "autre"
        || (read == "tiers_payeur" && matches!(declared, "debours_de_caisse" | "rente_servie"))
}

pub fn statuses(q: &Quantum, obligations: &[Obligation]) -> Vec<Status> {
    let held = amounts(q);
    let mut declared: HashMap<i64, Vec<String>> = HashMap::new();
    for h in &q.hors_record {
        let cause = serde_json::to_value(h.cause).expect("a cause serialises");
        declared
            .entry(cents(h.montant))
            .or_default()
            .push(cause.as_str().expect("a cause is a string").to_string());
    }
    obligations
        .iter()
        .map(|o| {
            let c = cents(o.montant);
            let costs = matches!(o.nature, Nature::HorsRecord("article_700" | "depens"));
            if !costs && held.contains(&c) {
                return Status::Held;
            }
            match (declared.get(&c), &o.nature) {
                (None, _) => Status::Unmet,
                (Some(causes), Nature::HorsRecord(read))
                    if causes.iter().any(|d| compatible(read, d)) =>
                {
                    Status::Declared
                }
                (Some(_), _) => Status::Refused,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantum::{CauseHorsRecord, HorsRecord, Poste, Victime};

    fn ob(montant: f64, nature: Nature) -> Obligation {
        Obligation {
            montant,
            s: 0,
            e: 1,
            nature,
        }
    }

    #[test]
    fn an_amount_is_held_by_any_field_declared_by_its_cause_or_unmet() {
        let q = Quantum {
            hors_record: vec![
                HorsRecord {
                    montant: 3000.0,
                    cause: CauseHorsRecord::Article700,
                },
                HorsRecord {
                    montant: 45.0,
                    cause: CauseHorsRecord::AutreLitige,
                },
                HorsRecord {
                    montant: 8712.45,
                    cause: CauseHorsRecord::DeboursDeCaisse,
                },
            ],
            victimes: vec![Victime {
                cle: "v1".into(),
                role: "directe".into(),
                age_fait: Some(45.0),
                postes: vec![Poste {
                    poste: Some("DFP".into()),
                    montant_alloue: Some(27600.0),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let obs = [
            ob(
                27600.0,
                Nature::Poste {
                    cle: Some("v1".into()),
                    code: "DFP".into(),
                },
            ),
            ob(3000.0, Nature::HorsRecord("article_700")),
            ob(45.0, Nature::Indemnite { cle: None }),
            ob(8712.45, Nature::HorsRecord("tiers_payeur")),
            ob(500.0, Nature::Total { cle: None }),
        ];
        assert_eq!(
            statuses(&q, &obs),
            vec![
                Status::Held,
                Status::Declared,
                Status::Refused,
                Status::Declared,
                Status::Unmet
            ]
        );
    }
}
