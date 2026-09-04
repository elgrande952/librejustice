//! La projection d'un record en observations : une ligne par poste lu, avec son
//! abscisse quand elle existe et la raison de son absence sinon.
//!
//! Ce que le référentiel publie en regard n'est pas ici : l'écart dépend de
//! l'édition à laquelle on le mesure, et se calcule à la lecture.

use serde::{Deserialize, Serialize};

use super::spreadsheet::{Proof, Spreadsheet};
use super::{Bareme, Correction, Lien, Poste, Quantum, Regime, Unite, Victime};

/// Les quatre natures d'abscisse, qui ne se mélangent pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Valeur du point de DFP : une cellule d'un croisement taux par âge, donc
    /// ce qui s'y mesure est l'écart à la cellule, jamais la médiane des cellules.
    Grille,
    /// Un tarif par jour, par heure, par mois, dont la médiane a un sens.
    Unite,
    /// Une note sur 7, sous-niveau de la nomenclature.
    Cotation,
    /// Le préjudice d'affection, indexé sur le lien découpé par ses modalités.
    Categorie,
}

impl Axis {
    pub fn name(self) -> &'static str {
        match self {
            Self::Grille => "grille",
            Self::Unite => "unite",
            Self::Cotation => "cotation",
            Self::Categorie => "categorie",
        }
    }
}

/// De quelle cellule une ligne relève, sans relire l'arrêt. Le taux est celui de
/// la victime directe même sur la ligne d'un proche : à catégorie égale, un
/// préjudice d'affection va de 500 à 30 000 euros selon lui. `referentiel_cite`
/// rempli dit une table recopiée, donc pas un témoignage indépendant.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Comparabilite {
    pub regimes: Vec<Regime>,
    pub regime_libelle: Option<String>,
    pub fait: Option<String>,
    pub referentiel_cite: Option<String>,
    pub dfp_victime_directe: Option<f64>,
    pub bareme_capitalisation: Option<Bareme>,
}

impl Comparabilite {
    fn from_record(q: &Quantum) -> Self {
        Self {
            regimes: q.regimes.clone(),
            regime_libelle: q.regime_libelle.clone(),
            fait: q.fait_generateur.clone(),
            referentiel_cite: q.referentiel_cite.clone(),
            dfp_victime_directe: q
                .victimes
                .iter()
                .find(|v| v.role == "directe")
                .and_then(|v| v.taux_dfp_pct),
            bareme_capitalisation: q.bareme_capitalisation.clone(),
        }
    }
}

/// Une ligne de la distribution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Observation {
    pub victime_ord: i16,
    pub poste_ord: i16,
    pub poste: String,
    pub axe: Option<String>,
    /// Rempli quand la ligne ne se compare à rien. Elle reste écrite : sans
    /// elle, la couverture de la mesure serait invisible.
    pub hors_table: Option<String>,
    pub montant: Option<f64>,
    pub valeur_unitaire: Option<f64>,
    pub quantite: Option<f64>,
    pub unite: Option<String>,
    /// Une heure d'aide active et une heure de présence responsable sont deux
    /// tarifs, jamais la même médiane.
    pub nature_aide: Option<String>,
    pub taux_pct: Option<f64>,
    pub cotation: Option<f64>,
    pub categorie: Option<String>,
    /// Le grain qui manque pour affiner la catégorie : la ligne s'agrège plus
    /// grossièrement au lieu de disparaître.
    pub categorie_manque: Option<String>,
    /// La victime directe est décédée. Deux populations disjointes, et le
    /// référentiel ne chiffre que la première.
    pub deces: Option<bool>,
    /// Rempli pour les seules lignes qui ont une abscisse : ce sont les seules
    /// que la distribution compare, donc à reclasser.
    pub comparabilite: Option<Comparabilite>,
    pub taux_dfp_pct: Option<f64>,
    pub age: Option<f64>,
    pub sexe: Option<String>,
    pub role: Option<String>,
    pub lien: Option<String>,
    /// What determines the amount, by the spreadsheet: `confirmee` by two
    /// paths, `sommee` by the victim's total closing on it, `lue` alone,
    /// `derivee` from a single relation, or nothing when no cell holds it.
    pub preuve: Option<String>,
    pub equations: i16,
    /// The nature of the correction that put the amount there, and `cite` what
    /// the text carries at that spot. Empty when the value is as written.
    pub origine: Option<String>,
    pub cite: Option<String>,
    /// Sum of what earlier liquidations already paid on the poste; `montant`
    /// then adds it to the complement the decision awards.
    pub indemnites_acquises: Option<f64>,
}

/// The fields of a poste that a correction may touch without moving its value.
const BESIDE_THE_VALUE: &[&str] = &[
    "montant_demande",
    "montant_offert",
    "alloue_premiere_instance",
    "libelle_decision",
];

/// The correction that moved this poste's value, if any: one that names the
/// victim, or every victim, and the poste's code, on a field the table reads.
fn correction_on<'a>(
    corrections: &'a [Correction],
    cle: &str,
    code: &str,
) -> Option<&'a Correction> {
    corrections.iter().find(|c| {
        let Some(rest) = c.champ.strip_prefix("victimes[") else {
            return false;
        };
        let Some((who, rest)) = rest.split_once("].postes[") else {
            return false;
        };
        let Some((which, leaf)) = rest.split_once("].") else {
            return false;
        };
        (who == "*" || who == cle) && which == code && !BESIDE_THE_VALUE.contains(&leaf)
    })
}

/// A read amount is summed when an aggregate the court writes, the total of
/// the postes, the assiette of what is due or the net, closes on their sum:
/// that sum is its second path, shared with its siblings. An aggregate the
/// spreadsheet only derives proves nothing.
fn summed(t: &Spreadsheet, victime_ord: i16) -> bool {
    ["total", "assiette", "net"].iter().any(|k| {
        let key = format!("v{victime_ord}.{k}");
        t.proof(&key) == Some(Proof::Confirmed) && t.source(&key) == Some("lu")
    })
}

/// Le poste est-il chiffré à 100 % ? La réponse ne se déduit pas du
/// pourcentage : la cour chiffre le plus souvent ses postes pleins et réduit
/// ensuite, donc seul `postes_reduits` exclut.
fn full_base(v: &Victime, code: &str) -> Option<String> {
    if v.partage_responsabilite_pct.is_none() {
        return Some("base des montants non établie".to_string());
    }
    if v.postes_reduits.iter().any(|c| c == code) {
        let pct = v.perte_de_chance_pct.or(v.partage_responsabilite_pct);
        return Some(match pct {
            Some(p) => format!("poste chiffré réduit à {p} %"),
            None => "poste chiffré réduit".to_string(),
        });
    }
    None
}

/// La sous-cotation du préjudice d'affection, et ce qui manque pour l'affiner.
/// À parenté égale, un enfant mineur, un enfant majeur au foyer et un enfant
/// majeur ailleurs relèvent de trois fourchettes disjointes.
pub fn paf_category(v: &Victime) -> (String, Option<String>) {
    let mineur = v.mineur.or_else(|| v.age_fait.map(|a| a < 18.0));
    let (coh, freq) = (v.cohabitation, v.frequence_relations.as_deref());

    match v.lien {
        Some(Lien::Conjoint) | Some(Lien::Concubin) | Some(Lien::Partenaire) => {
            ("conjoint".to_string(), None)
        }
        Some(Lien::Parent) => ("parent".to_string(), None),
        Some(Lien::Enfant) => match (mineur, coh) {
            (Some(true), _) => ("enfant mineur".to_string(), None),
            (None, _) => ("enfant".to_string(), Some("minorité".to_string())),
            (Some(false), Some(true)) => ("enfant majeur au foyer".to_string(), None),
            (Some(false), Some(false)) => ("enfant majeur hors foyer".to_string(), None),
            (Some(false), None) => (
                "enfant majeur".to_string(),
                Some("cohabitation".to_string()),
            ),
        },
        Some(Lien::Fratrie) => match coh {
            Some(true) => ("fratrie au foyer".to_string(), None),
            Some(false) => ("fratrie hors foyer".to_string(), None),
            None => ("fratrie".to_string(), Some("cohabitation".to_string())),
        },
        Some(l @ (Lien::GrandParent | Lien::PetitEnfant)) => {
            let stem = if l == Lien::GrandParent {
                "grand-parent"
            } else {
                "petit-enfant"
            };
            let low = freq.map(|f| f.trim().to_lowercase());
            let said = match low.as_deref() {
                Some(x) if x.contains("peu") && x.contains("fr") => Some("peu fréquentes"),
                Some(x) if x.contains("fr") => Some("fréquentes"),
                _ => None,
            };
            match said {
                Some(f) => (format!("{stem}, relations {f}"), None),
                None => (
                    stem.to_string(),
                    Some("fréquence des relations".to_string()),
                ),
            }
        }
        Some(Lien::BeauParent) => ("beau-parent".to_string(), None),
        Some(Lien::BeauFils) => ("beau-fils".to_string(), None),
        Some(Lien::OncleTante) => ("oncle ou tante".to_string(), None),
        Some(Lien::NeveuNiece) => ("neveu ou nièce".to_string(), None),
        Some(Lien::Cousin) => ("cousin".to_string(), None),
        Some(Lien::Autre) | None => ("autre proche".to_string(), None),
    }
}

/// Les seuls postes de proche dont le lien est l'abscisse : des frais d'obsèques
/// se mesurent en frais réels, et ranger un « conjoint » en abscisse a mis une
/// perte de revenus de 337 782 euros dans la médiane du préjudice d'affection.
const ON_LINK: &[&str] = &["PAF", "PAC"];

/// The CFP franc has been pegged to the euro since 1999: 1 000 F CFP = 8,38 euros.
const XPF_PER_EUR: f64 = 119.331_74;

/// Euros per unit of the record's currency; `None` for a currency without a
/// fixed parity, which no table can hold.
fn euro_rate(devise: Option<&str>) -> Option<f64> {
    match devise {
        None | Some("EUR") => Some(1.0),
        Some("XPF") => Some(1.0 / XPF_PER_EUR),
        Some(_) => None,
    }
}

fn set_axis(o: &mut Observation, v: &Victime, code: &str, deces: Option<bool>) {
    if code == "DFP" && o.unite.as_deref() == Some("point") {
        o.axe = Some(Axis::Grille.name().to_string());
    } else if o.valeur_unitaire.is_some() {
        o.axe = Some(Axis::Unite.name().to_string());
    } else if o.cotation.is_some() {
        o.axe = Some(Axis::Cotation.name().to_string());
    } else if ON_LINK.contains(&code) && v.role == "indirecte" && v.lien.is_some() {
        let (cat, missing) = paf_category(v);
        o.axe = Some(Axis::Categorie.name().to_string());
        o.categorie = Some(cat);
        o.categorie_manque = missing.or_else(|| {
            deces
                .is_none()
                .then(|| "issue de la victime directe".to_string())
        });
        o.deces = deces;
    }
}

fn project(
    q: &Quantum,
    v: &Victime,
    p: &Poste,
    t: &Spreadsheet,
    victime_ord: i16,
    poste_ord: i16,
    comparabilite: &Comparabilite,
) -> Observation {
    let code = p
        .poste
        .clone()
        .unwrap_or_else(|| "HORS_NOMENCLATURE".to_string());
    let mut unite = p.unite;
    let mut quantite = p.quantite;
    if code == "DFP" && quantite.is_none() {
        // Le nombre de points d'un DFP est le taux d'AIPP de la victime, qui ne
        // vit qu'une fois dans le record.
        if let Some(taux) = v.taux_dfp_pct {
            quantite = Some(taux);
            unite = unite.or(Some(Unite::Point));
        }
    }

    let cell = format!("v{victime_ord}.p{poste_ord}.plein");
    let acquis =
        (!p.indemnites_acquises.is_empty()).then(|| p.indemnites_acquises.iter().sum::<f64>());
    let viager = p.prorata_survie.as_ref().map(|pr| pr.montant_viager);
    let correction = correction_on(&q.corrections, &v.cle, &code);
    let mut o = Observation {
        victime_ord,
        poste_ord,
        poste: code.clone(),
        preuve: t.proof(&cell).map(|pr| match pr {
            Proof::Read if summed(t, victime_ord) => "sommee".to_string(),
            other => other.name().to_string(),
        }),
        equations: t.constraints(&cell) as i16,
        origine: correction.map(|c| c.nature.name().to_string()),
        cite: correction.and_then(|c| c.cite.as_ref()).map(|c| match c {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }),
        indemnites_acquises: acquis,
        montant: viager.or_else(|| p.montant_alloue.map(|m| m + acquis.unwrap_or(0.0))),
        valeur_unitaire: p.valeur_unitaire,
        quantite,
        unite: unite.map(|u| u.name().to_string()),
        nature_aide: p.nature_aide.map(|n| n.name().to_string()),
        taux_pct: p.taux_pct,
        cotation: p.cotation,
        taux_dfp_pct: v.taux_dfp_pct,
        age: v.age_consolidation.or(v.age_fait),
        sexe: v.sexe.clone(),
        role: Some(v.role.clone()),
        lien: v.lien.map(|l| l.name().to_string()),
        ..Default::default()
    };
    set_axis(&mut o, v, &code, q.deces);
    if o.axe.is_some() {
        o.comparabilite = Some(comparabilite.clone());
    }

    let rate = euro_rate(q.devise.as_deref());
    if let Some(r) = rate.filter(|r| *r != 1.0) {
        o.montant = o.montant.map(|m| m * r);
        o.valeur_unitaire = o.valeur_unitaire.map(|x| x * r);
        o.indemnites_acquises = o.indemnites_acquises.map(|a| a * r);
    }
    o.hors_table = rate
        .is_none()
        .then(|| {
            format!(
                "devise {} : aucune parité fixe",
                q.devise.as_deref().unwrap_or("")
            )
        })
        .or_else(|| full_base(v, &code))
        .or_else(|| match (o.montant, o.valeur_unitaire, o.axe.is_some()) {
            (Some(0.0), None, _) => Some("poste rejeté : la cour n'alloue rien".to_string()),
            (None, None, _) => Some("aucun montant relevé".to_string()),
            (_, _, false) => Some("ni tarif ni cotation : pas d'abscisse".to_string()),
            _ => None,
        });
    o
}

/// Toutes les observations d'un record, dans l'ordre du record : l'ordinal est
/// la clé, donc il ne se dérive pas d'un tri.
pub fn observations(q: &Quantum, t: &Spreadsheet) -> Vec<Observation> {
    let comparabilite = Comparabilite::from_record(q);
    let mut out = Vec::new();
    for (iv, v) in q.victimes.iter().enumerate() {
        for (ip, p) in v.postes.iter().enumerate() {
            out.push(project(q, v, p, t, iv as i16, ip as i16, &comparabilite));
        }
    }
    out
}
