//! Où chaque valeur du record se lit dans l'arrêt.
//!
//! Un chiffre se répète : « 68,13 » s'écrit six fois dans un même arrêt, dans
//! les prétentions des parties comme dans les motifs. L'ancre porte donc **tous**
//! ses passages, jamais un seul choisi : retenir le bon est un jugement, et la
//! donnée n'a pas à le rendre à la place du lecteur. Les bornes sont en
//! caractères, non en octets.
//!
//! Le poste s'y nomme par son rang, là où une correction le nomme par son code :
//! une correction est écrite à la main et doit survivre à une relecture qui
//! réordonne, une ancre est recalculée avec le record qu'elle accompagne, et
//! deux postes d'une même victime partagent parfois un code.

use lj_core::text::fold_char;
use rustc_hash::FxHashMap;
use serde_json::Value;

use super::{Ancre, Quantum, Span};

/// Les seules chaînes recopiées de l'arrêt. Ailleurs, le record porte les mots de
/// la lecture, un nom de vocabulaire ou une date recomposée : chercher
/// « badinter » ou « euro_de_rente » dans le texte ne désigne aucun passage, et
/// les compter ferait passer une couverture pour un trou.
const CITATIONS: [&str; 5] = [
    "libelle",
    "libelle_decision",
    "profession",
    "referentiel_cite",
    "regime_libelle",
];

/// Les branches qui décrivent la lecture et non la décision : leurs nombres sont
/// des rangs et des positions, sans passage dans l'arrêt.
const HORS: [&str; 4] = ["ancres", "contradictions", "corrections", "lecture"];

const SEPARATEURS: [char; 5] = ['.', ' ', '\u{a0}', '\u{202f}', '\u{2009}'];

/// Un nombre du texte, en millièmes pour que le coefficient de capitalisation à
/// trois décimales entre dans une clé entière.
#[derive(Clone, Copy)]
pub(super) struct Jeton {
    pub(super) valeur: i64,
    pub(super) debut: usize,
    pub(super) fin: usize,
}

fn est_chiffre(c: &char) -> bool {
    c.is_ascii_digit()
}

/// Un groupe de milliers : le séparateur n'en est un que suivi d'exactement
/// trois chiffres, sans quoi « 10 25 » vaudrait 1 025.
fn groupe(chars: &[char], i: usize) -> bool {
    SEPARATEURS.contains(chars.get(i).unwrap_or(&'!'))
        && chars.len() >= i + 4
        && chars[i + 1..i + 4].iter().all(est_chiffre)
        && !chars.get(i + 4).is_some_and(est_chiffre)
}

pub(super) fn jetons(chars: &[char]) -> Vec<Jeton> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let debut = i;
        let mut entier = String::new();
        while i < chars.len() && chars[i].is_ascii_digit() {
            entier.push(chars[i]);
            i += 1;
        }
        let tete = entier.len();
        let (mut groupes, mut points) = (0usize, 0usize);
        while groupe(chars, i) {
            groupes += 1;
            points += usize::from(chars[i] == '.');
            entier.extend(&chars[i + 1..i + 4]);
            i += 4;
        }
        let mut frac = String::new();
        if matches!(chars.get(i), Some(',') | Some('.')) {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            // Un point suivi de trois chiffres est déjà passé en milliers.
            if j > i + 1 && (chars[i] == ',' || j - i - 1 != 3) {
                frac.extend(&chars[i + 1..j]);
                i = j;
            } else if chars[i] == ','
                && chars.get(i + 1) == Some(&' ')
                && chars.len() > i + 3
                && chars[i + 2..i + 4].iter().all(est_chiffre)
                && !chars.get(i + 4).is_some_and(est_chiffre)
            {
                // « 18.485, 36 € » : the cents after a comma and a space, as the
                // registry writes them in some decisions.
                frac.extend(&chars[i + 2..i + 4]);
                i += 4;
            }
        }
        let (Ok(e), Ok(q)) = (
            entier.parse::<i64>(),
            frac.chars()
                .chain("0000".chars())
                .take(4)
                .collect::<String>()
                .parse::<i64>(),
        ) else {
            continue;
        };
        if let Some(valeur) = e.checked_mul(1000).map(|x| x + (q + 5) / 10) {
            out.push(Jeton {
                valeur,
                debut,
                fin: i,
            });
        }
        // « 42.469 » est le coefficient 42,469 autant que le millier 42 469, et
        // seul le contexte tranche : les deux lectures deviennent candidates.
        if groupes == 1 && points == 1 && frac.is_empty() {
            if let Ok(t) = entier[..tete].parse::<i64>() {
                let d: i64 = entier[tete..].parse().unwrap_or_default();
                out.push(Jeton {
                    valeur: t * 1000 + d,
                    debut,
                    fin: i,
                });
            }
        }
    }
    out
}

pub(super) fn millimes(x: f64) -> i64 {
    (x.abs() * 1000.0).round() as i64
}

/// Les feuilles du record, chacune avec son chemin. La marche est générique : un
/// champ ajouté au schéma s'ancre sans qu'on y revienne.
fn feuilles(v: &Value, at: &str, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Number(_) | Value::String(_) => out.push((at.to_string(), v.clone())),
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                let index = match x.get("cle").and_then(Value::as_str) {
                    Some(cle) if !cle.is_empty() => cle.to_string(),
                    _ => i.to_string(),
                };
                feuilles(x, &format!("{at}[{index}]"), out);
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                if at.is_empty() && HORS.contains(&k.as_str()) {
                    continue;
                }
                let sep = if at.is_empty() { "" } else { "." };
                feuilles(x, &format!("{at}{sep}{k}"), out);
            }
        }
        _ => {}
    }
}

/// Ce qu'une valeur doit être pour qu'on la cherche dans le texte. Un zéro ne
/// s'écrit pas : la cour rejette un poste, elle ne le chiffre pas à 0.
fn retenue(champ: &str, v: &Value) -> bool {
    match v {
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(_) => CITATIONS.contains(&champ.rsplit('.').next().unwrap_or(champ)),
        _ => false,
    }
}

/// Les chemins que l'ancrage cherche, texte ou pas : le dénominateur de sa
/// couverture.
pub fn ancrables(q: &Quantum) -> Vec<String> {
    let Ok(json) = serde_json::to_value(q) else {
        return Vec::new();
    };
    let mut leaves = Vec::new();
    feuilles(&json, "", &mut leaves);
    leaves
        .into_iter()
        .filter(|(c, v)| retenue(c, v))
        .map(|(c, _)| c)
        .collect()
}

fn positions(foin: &[char], aiguille: &[char]) -> Vec<usize> {
    if aiguille.is_empty() || aiguille.len() > foin.len() {
        return Vec::new();
    }
    (0..=foin.len() - aiguille.len())
        .filter(|&i| foin[i..i + aiguille.len()] == *aiguille)
        .collect()
}

/// Les ancres du record, dans l'ordre des chemins. Un champ dont le texte
/// n'écrit pas la valeur n'en a pas : c'est un chiffre dérivé, et le taire vaut
/// mieux que le poser au hasard.
pub fn ancrer(q: &Quantum, texte: &str) -> Vec<Ancre> {
    let Ok(json) = serde_json::to_value(q) else {
        return Vec::new();
    };
    let chars: Vec<char> = texte.chars().collect();
    let plie: Vec<char> = chars.iter().copied().map(fold_char).collect();

    let mut par_valeur: FxHashMap<i64, Vec<Jeton>> = FxHashMap::default();
    for j in jetons(&chars) {
        par_valeur.entry(j.valeur).or_default().push(j);
    }

    let mut leaves = Vec::new();
    feuilles(&json, "", &mut leaves);

    let mut ancres: Vec<Ancre> = Vec::new();
    for (champ, v) in leaves.iter().filter(|(c, v)| retenue(c, v)) {
        let spans: Vec<Span> = match v {
            Value::Number(n) => par_valeur
                .get(&millimes(n.as_f64().unwrap_or_default()))
                .map(|js| {
                    js.iter()
                        .map(|j| Span {
                            char_start: j.debut,
                            char_end: j.fin,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Value::String(s) => {
                let aiguille: Vec<char> = s.chars().map(fold_char).collect();
                let n = aiguille.len();
                positions(&plie, &aiguille)
                    .into_iter()
                    .map(|p| Span {
                        char_start: p,
                        char_end: p + n,
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        if !spans.is_empty() {
            ancres.push(Ancre {
                champ: champ.clone(),
                spans,
            });
        }
    }
    ancres.sort_by(|a, b| a.champ.cmp(&b.champ));
    ancres
}

#[cfg(test)]
mod tests {
    use super::super::{Poste, Victime};
    use super::*;

    fn valeurs(texte: &str) -> Vec<i64> {
        jetons(&texte.chars().collect::<Vec<_>>())
            .iter()
            .map(|j| j.valeur)
            .collect()
    }

    #[test]
    fn les_graphies_francaises_du_meme_nombre_donnent_la_meme_valeur() {
        for graphie in ["307.142,27", "307 142,27", "307142,27", "307\u{202f}142,27"] {
            assert_eq!(valeurs(graphie), vec![307_142_270], "{graphie}");
        }
    }

    #[test]
    fn cents_after_a_comma_and_a_space_belong_to_the_number() {
        assert_eq!(valeurs("18.485, 36 €"), vec![18_485_360]);
        assert_eq!(valeurs("43, 70 euros"), vec![43_700]);
        // Two amounts in a list stay two amounts.
        assert_eq!(
            valeurs("1 500, 2 000 et 3 000 euros"),
            vec![1_500_000, 2_000_000, 3_000_000]
        );
    }

    #[test]
    fn un_separateur_ne_groupe_que_trois_chiffres() {
        assert_eq!(valeurs("(10 heures x 25 euros)"), vec![10_000, 25_000]);
        assert_eq!(valeurs("10 25"), vec![10_000, 25_000]);
        assert_eq!(valeurs("1 000 25"), vec![1_000_000, 25_000]);
    }

    #[test]
    fn le_coefficient_garde_ses_trois_decimales() {
        assert_eq!(valeurs("1 872 x 35,597"), vec![1_872_000, 35_597]);
    }

    /// Une cour écrit « prix d'un euro de rente [...] de 42.469 », qui est le
    /// coefficient 42,469 : la graphie ne tranche pas, les deux se cherchent.
    #[test]
    fn le_point_suivi_de_trois_chiffres_est_un_millier_ou_une_decimale() {
        assert_eq!(valeurs("15.773,11 euros"), vec![15_773_110]);
        assert_eq!(valeurs("de 42.469, les besoins"), vec![42_469_000, 42_469]);
    }

    #[test]
    fn une_valeur_unique_dans_le_texte_a_un_seul_span() {
        let q = Quantum {
            total_enonce: Some(307_142.27),
            ..Default::default()
        };
        let a = ancrer(&q, "une dépense totale de 307.142,27€ au titre de laquelle");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].champ, "total_enonce");
        assert_eq!(a[0].spans.len(), 1);
        assert_eq!(a[0].spans[0].char_start, 22);
    }

    /// Le doublon de chiffres : 68,13 s'écrit deux fois, une fois dans les
    /// prétentions et une fois dans les motifs. Les deux sont notées.
    #[test]
    fn un_chiffre_repete_porte_tous_ses_passages() {
        let texte = "les consorts demandent 68,13 euros. \
            Sur les dépenses de santé actuelles, le tribunal a chiffré 136,26 \
            euros et alloué 68,13 euros après réduction de moitié.";
        let q = Quantum {
            victimes: vec![Victime {
                cle: "v1".to_string(),
                postes: vec![Poste {
                    poste: Some("DSA".to_string()),
                    libelle_decision: Some("depenses de sante actuelles".to_string()),
                    montant_alloue: Some(136.26),
                    part_victime: Some(68.13),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let a = ancrer(&q, texte);
        let pv = a
            .iter()
            .find(|x| x.champ.ends_with("part_victime"))
            .expect("la part victime s'ancre");
        assert_eq!(pv.spans.len(), 2);
        assert!(pv.spans[0].char_start < 40 && pv.spans[1].char_start > 100);
        assert_eq!(
            a.iter()
                .find(|x| x.champ.ends_with("libelle_decision"))
                .map(|x| x.spans.len()),
            Some(1)
        );
    }
}
