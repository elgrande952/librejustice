//! Le chemin qu'une correction nomme, résolu sur le record.
//!
//! ```text
//! victimes[v1].postes[DFP].montant_alloue
//! victimes[v1].postes[ATP].composantes[1].quantite
//! victimes[*].creance_tiers_payeurs
//! total_enonce
//! ```
//!
//! La victime se nomme par sa `cle` et le poste par son code, jamais par son
//! rang, qui change à la relecture. `*` vise toutes les victimes : une décision
//! où aucun tiers payeur ne recourt le dit une fois, pas une fois par victime.

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
#[error("« {path} » : {what}")]
pub struct PathError {
    pub path: String,
    pub what: String,
}

type PathResult<T> = std::result::Result<T, PathError>;

fn err<T>(path: &str, what: impl Into<String>) -> PathResult<T> {
    Err(PathError {
        path: path.to_string(),
        what: what.into(),
    })
}

enum Step {
    Victimes(String),
    Postes(String),
    Composantes(usize),
    Leaf(String),
}

fn parse_path(path: &str) -> PathResult<Vec<Step>> {
    let mut steps = Vec::new();
    for seg in path.split('.') {
        let (head, index) = match seg.split_once('[') {
            Some((t, rest)) => match rest.strip_suffix(']') {
                Some(i) => (t, Some(i)),
                None => return err(path, format!("crochet non fermé dans « {seg} »")),
            },
            None => (seg, None),
        };
        steps.push(match (head, index) {
            ("victimes", Some(key)) => Step::Victimes(key.to_string()),
            ("postes", Some(code)) => Step::Postes(code.to_string()),
            ("composantes", Some(i)) => match i.parse() {
                Ok(n) => Step::Composantes(n),
                Err(_) => return err(path, format!("« {i} » n'est pas un rang")),
            },
            (name, None) => Step::Leaf(name.to_string()),
            (name, Some(_)) => return err(path, format!("« {name} » ne s'indexe pas")),
        });
    }
    match steps.last() {
        Some(Step::Leaf(_)) => Ok(steps),
        _ => err(path, "ne se termine pas sur un nom de champ"),
    }
}

/// Les objets à un pas de la feuille, et le nom de celle-ci. Un chemin qui ne
/// mène à rien est une erreur, jamais une liste vide.
fn descend<'a>(record: &'a mut Value, path: &str) -> PathResult<(Vec<&'a mut Value>, String)> {
    let steps = parse_path(path)?;
    let (chemin, leaf) = steps.split_at(steps.len() - 1);
    let Step::Leaf(name) = &leaf[0] else {
        unreachable!("analyse garantit une feuille en dernier")
    };
    let mut here: Vec<&mut Value> = vec![record];
    for p in chemin {
        let mut next_level = Vec::new();
        for o in here {
            match p {
                Step::Victimes(key) => match o.get_mut("victimes").and_then(Value::as_array_mut) {
                    Some(a) => next_level.extend(a.iter_mut().filter(|v| {
                        key == "*" || v.get("cle").and_then(Value::as_str) == Some(key.as_str())
                    })),
                    None => return err(path, "aucune victime dans ce record"),
                },
                Step::Postes(code) => {
                    match o.get_mut("postes").and_then(Value::as_array_mut) {
                        Some(a) => next_level.extend(a.iter_mut().filter(|p| {
                            p.get("poste").and_then(Value::as_str) == Some(code.as_str())
                        })),
                        None => return err(path, "aucun poste dans cette victime"),
                    }
                }
                Step::Composantes(i) => {
                    match o
                        .get_mut("composantes")
                        .and_then(Value::as_array_mut)
                        .and_then(|a| a.get_mut(*i))
                    {
                        Some(c) => next_level.push(c),
                        None => return err(path, format!("pas de composante {i}")),
                    }
                }
                Step::Leaf(name) => match o.get_mut(name) {
                    Some(v) => next_level.push(v),
                    None => return err(path, format!("« {name} » absent")),
                },
            }
        }
        if next_level.is_empty() {
            return err(path, "ne désigne rien dans ce record");
        }
        here = next_level;
    }
    Ok((here, name.clone()))
}

/// Ce que le record porte à ce chemin.
pub fn read(record: &Value, path: &str) -> PathResult<Vec<Value>> {
    let mut copy = record.clone();
    let (targets, name) = descend(&mut copy, path)?;
    let mut out = Vec::with_capacity(targets.len());
    for c in targets {
        match c.get(&name) {
            Some(v) => out.push(v.clone()),
            None => return err(path, format!("« {name} » absent de la cible")),
        }
    }
    Ok(out)
}

/// Réécrit ce chemin, et rend le nombre de cibles touchées. Sert à demander
/// « et si le record portait le chiffre que la décision écrit ? ».
pub fn set(record: &mut Value, path: &str, v: &Value) -> PathResult<usize> {
    let (targets, name) = descend(record, path)?;
    let mut n = 0;
    for c in targets {
        match c.as_object_mut() {
            Some(o) if o.contains_key(&name) => {
                o.insert(name.clone(), v.clone());
                n += 1;
            }
            Some(_) => return err(path, format!("« {name} » absent de la cible")),
            None => return err(path, "la cible n'est pas un objet"),
        }
    }
    Ok(n)
}
