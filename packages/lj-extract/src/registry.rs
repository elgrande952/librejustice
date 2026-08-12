//! Registre de parties : le point de convergence des moissons NER. Une entité
//! = (valeur, côté, qualité) ; les champs plats (`companies`, `counsel`,
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
    /// Structure d'exercice du conseil (SCP, SELARL…).
    LawFirm,
    /// Avocat nommé.
    CounselName,
    /// Intervenant.
    Intervenor,
}

/// Une entité du document : tranche verbatim + coordonnées (côté, qualité).
#[derive(Debug, Clone)]
pub struct Entity {
    pub value: String,
    pub side: Option<Side>,
    pub quality: Quality,
}

/// Registre par document, mémoïsé sur `DocScan`.
#[derive(Debug, Clone, Default)]
pub struct PartyRegistry {
    pub entities: Vec<Entity>,
}

impl PartyRegistry {
    pub(crate) fn push(&mut self, values: Vec<String>, side: Option<Side>, quality: Quality) {
        self.entities.extend(values.into_iter().map(|value| Entity {
            value,
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
}
