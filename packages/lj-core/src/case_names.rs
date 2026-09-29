//! Noms d'usage d'arrêts : « Danthony », « Duvignères », « Tropic ». Le nom est
//! absent du texte de sa propre décision, la pseudonymisation le retirant ; il
//! est porté par `decisions.case_name`. Ce module tient ce qui reste PUR : le
//! pliage qui sert de clé, les n-grammes qu'une requête offre au catalogue, et
//! le canon que les renvois ne donnent pas. La résolution, elle, est une
//! lecture en base (`decision_by_case_name`).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::text::normalize_spaces;
use lj_fold::fold;

const CASE_NAMES_JSON: &str = include_str!("../data/case_names.json");

/// A name the renvois do not give: the canon before 1965, which the Conseil
/// d'État no longer cites, and the shortcut no generic head derives (« Tropic »
/// for « Société Tropic Travaux Signalisation »). Anchored against the corpus.
#[derive(Debug, Clone, Deserialize)]
pub struct CanonicalCaseName {
    /// Le nom tel qu'il s'écrit ; sa clé se calcule par [`case_name_key`].
    pub nom: String,
    pub jurisdiction: String,
    /// Date de lecture, ISO.
    pub date: String,
    pub dockets: Vec<String>,
}

static CANON: LazyLock<Vec<CanonicalCaseName>> = LazyLock::new(|| {
    serde_json::from_str(CASE_NAMES_JSON).expect("case_names.json embarqué valide")
});

/// Les noms écrits à la main, à unir aux noms élus par la projection.
pub fn canon() -> &'static [CanonicalCaseName] {
    &CANON
}

/// Le nom d'usage le plus long qu'une requête puisse porter, en mots :
/// « Section française de l'Observatoire international des prisons » en fait
/// sept.
const MAX_NAME_WORDS: usize = 8;

/// Clé de comparaison d'un nom d'usage : accents, casse et ponctuation repliés,
/// pour que « Association AC ! » et « Association AC! » soient un seul nom. La
/// projection et la résolution passent toutes deux par elle.
pub fn case_name_key(nom: &str) -> String {
    let sans_ponctuation: String = fold(nom)
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    normalize_spaces(&sans_ponctuation).trim().to_string()
}

/// The heads a named party carries before what distinguishes it. The corpus
/// cites the remainder alone: « jurisprudence Béziers » 12 times against 6 for
/// « jurisprudence Commune de Béziers », « Béziers II » 36 times.
const GENERIC_HEADS: &[&str] = &[
    "commune",
    "ville",
    "departement",
    "region",
    "societe",
    "ste",
    "sa",
    "sarl",
    "sas",
    "sci",
];

const HEAD_ARTICLES: &[&str] = &["de", "du", "des", "d", "la", "le", "les", "l"];

/// The short key a full key derives: what follows a generic head, articles
/// dropped. `commune de beziers` gives `beziers`; a patronym gives nothing.
pub fn short_case_name_key(key: &str) -> Option<String> {
    let mut words = key.split(' ');
    if !GENERIC_HEADS.contains(&words.next()?) {
        return None;
    }
    let short = words
        .skip_while(|w| HEAD_ARTICLES.contains(w))
        .collect::<Vec<_>>()
        .join(" ");
    (short.chars().count() >= 3).then_some(short)
}

/// Contexte qu'un nom supporte autour de lui, en mots. Mesuré sur le trafic
/// externe : une requête qui nomme son arrêt va jusqu'à quinze mots de contexte,
/// au-delà de vingt ce sont des paragraphes collés. Le budget de la référence
/// numérotée, dix mots, coupait les premières.
const MAX_CONTEXT_WORDS: usize = 20;

/// Les clés que la requête peut désigner, la plus longue d'abord : tous ses
/// n-grammes contigus jusqu'à [`MAX_NAME_WORDS`] mots. Vide au-delà du contexte
/// que [`MAX_CONTEXT_WORDS`] autorise.
pub fn candidate_keys(query: &str) -> Vec<String> {
    let cle = case_name_key(query);
    let mots: Vec<&str> = cle.split_whitespace().collect();
    if mots.is_empty() || mots.len() > MAX_NAME_WORDS + MAX_CONTEXT_WORDS {
        return Vec::new();
    }
    let mut cles = Vec::new();
    for taille in (1..=mots.len().min(MAX_NAME_WORDS)).rev() {
        if mots.len() - taille > MAX_CONTEXT_WORDS {
            continue;
        }
        for debut in 0..=mots.len() - taille {
            cles.push(mots[debut..debut + taille].join(" "));
        }
    }
    cles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_canon_est_coherent() {
        for a in canon() {
            assert!(!a.dockets.is_empty(), "{:?} sans numéro", a.nom);
            assert_eq!(a.date.len(), 10, "{:?} sans date ISO", a.nom);
            assert!(!case_name_key(&a.nom).is_empty(), "{:?} sans clé", a.nom);
        }
    }

    #[test]
    fn la_ponctuation_du_nom_ne_compte_pas() {
        assert_eq!(case_name_key("Association AC !"), "association ac");
        assert_eq!(
            case_name_key("Département de Tarn-et-Garonne"),
            case_name_key("departement de tarn et garonne")
        );
    }

    #[test]
    fn le_nom_court_suit_la_tete_generique() {
        for (cle, court) in [
            ("commune de beziers", Some("beziers")),
            ("commune de la garde", Some("garde")),
            ("departement de l eure", Some("eure")),
            ("ville de paris", Some("paris")),
            (
                "societe tropic travaux signalisation",
                Some("tropic travaux signalisation"),
            ),
            ("sa x", None),
            ("danthony", None),
            ("commission c italie", None),
            ("commune", None),
        ] {
            assert_eq!(short_case_name_key(cle).as_deref(), court, "clé {cle:?}");
        }
    }

    #[test]
    fn le_nom_seul_ou_avec_son_contexte_est_candidat() {
        for q in [
            "danthony",
            "arrêt Danthony",
            "CE 2011 Danthony vice de procédure",
        ] {
            assert!(
                candidate_keys(q).contains(&"danthony".to_string()),
                "requête {q:?}"
            );
        }
    }

    /// Trois requêtes du trafic externe, vérifiées en prod sans nom résolu quand
    /// le budget valait dix mots.
    #[test]
    fn le_nom_survit_a_une_question_de_quinze_mots() {
        for (q, nom) in [
            (
                "jurisprudence Danthony vice de procédure influence sur le sens de la décision privation d'une garantie",
                "danthony",
            ),
            (
                "délai raisonnable Czabaj inapplicable recours indemnitaire mise en jeu de la responsabilité personne publique prescription quadriennale",
                "czabaj",
            ),
            (
                "GISTI documents de portée générale effets notables droits situation autres personnes agents chargés mettre en oeuvre",
                "gisti",
            ),
        ] {
            assert!(
                candidate_keys(q).contains(&nom.to_string()),
                "requête {q:?}"
            );
        }
    }

    /// Un paragraphe collé n'est pas une demande d'arrêt : le nom qu'il contient
    /// y est de passage.
    #[test]
    fn le_paragraphe_colle_ne_nomme_rien() {
        let colle = "l'intégrité physique ou psychique supérieure à 25 % ainsi que les décès provoqués par ces infections, l'ONIAM étant seul tenu d'indemniser les victimes au titre de la solidarité nationale";
        assert!(candidate_keys(colle).is_empty());
    }

    #[test]
    fn le_nom_le_plus_long_est_propose_avant_son_raccourci() {
        let cles = candidate_keys("société tropic travaux signalisation");
        let rang = |c: &str| cles.iter().position(|k| k == c).expect(c);
        assert!(rang("societe tropic travaux signalisation") < rang("tropic"));
    }

    /// Le patronyme courant noyé dans une question concourt désormais, et ce
    /// n'est plus la longueur qui l'écarte : « Marie » sort dans plus de 20 000
    /// décisions, donc la projection ne lui laisse aucune clé à désigner.
    #[test]
    fn le_patronyme_courant_reste_a_la_projection() {
        assert!(candidate_keys(
            "un détenu nommé Marie conteste une sanction disciplinaire prise par le \
             chef d'établissement sans que la commission ait été consultée"
        )
        .contains(&"marie".to_string()));
    }
}
