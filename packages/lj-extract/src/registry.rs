//! Registre de parties : le point de convergence des moissons NER. Une entité
//! = (mention, côté, qualité) ; les champs plats (`companies`, `counsel`,
//! `intervenors`) sont des projections [`PartyRegistry::view`].

/// Côté procédural, normalisé par degré (appelant→demandeur…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Applicant,
    Defendant,
}

/// Qualité de l'entité dans l'instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Partie personne morale.
    Party,
    /// Partie personne physique, pseudonymisée par la source.
    Individual,
    /// Structure d'exercice du conseil (SCP, SELARL…).
    LawFirm,
    /// Avocat nommé.
    CounselName,
    /// Intervenant.
    Intervenor,
}

/// Une mention : la valeur imprimée et la tranche `[s..e)` de caractères d'où
/// elle a été lue. `value` est le rendu de cette tranche (forme sociale
/// recollée, intérieur nettoyé), pas sa copie octet à octet.
#[derive(Debug, Clone)]
pub struct Mention {
    pub s: usize,
    pub e: usize,
    pub value: String,
}

impl Mention {
    pub fn new(s: usize, e: usize, value: String) -> Self {
        Self { s, e, value }
    }
}

/// Une entité du document : mention ancrée + coordonnées (côté, qualité).
#[derive(Debug, Clone)]
pub struct Entity {
    pub value: String,
    pub s: usize,
    pub e: usize,
    pub side: Option<Side>,
    pub quality: Quality,
}

/// Registre par document, mémoïsé sur `DocScan`.
#[derive(Debug, Clone, Default)]
pub struct PartyRegistry {
    pub entities: Vec<Entity>,
}

impl PartyRegistry {
    pub(crate) fn push(&mut self, mentions: Vec<Mention>, side: Option<Side>, quality: Quality) {
        self.entities.extend(mentions.into_iter().map(|m| Entity {
            value: m.value,
            s: m.s,
            e: m.e,
            side,
            quality,
        }));
    }

    /// Projection plate d'une cellule (côté, qualité), en ordre d'insertion.
    pub fn view(&self, side: Option<Side>, quality: Quality) -> Vec<String> {
        self.entities
            .iter()
            .filter(|e| e.side == side && e.quality == quality)
            .map(|e| e.value.clone())
            .collect()
    }

    /// Mentions ancrées d'une cellule : la tranche décisive de chaque valeur,
    /// que la projection plate perd.
    pub fn mentions(&self, side: Option<Side>, quality: Quality) -> Vec<&Entity> {
        self.entities
            .iter()
            .filter(|e| e.side == side && e.quality == quality)
            .collect()
    }
}
