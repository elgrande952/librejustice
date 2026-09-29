//! The normative tables of the reading protocol, written here and nowhere else.
//!
//! The markdown handed to a reader is rendered from these constants, and a test
//! refuses any drift between the two. A rule two artifacts state ends up saying
//! two things, and a reader who follows the document is then right against the
//! code.

use super::{Lien, ModeEmploi, NatureAide, Regime, Statut, Unite};

/// The poste codes and the three axes that settle a doubtful case: direct or
/// indirect victim, patrimonial or extra-patrimonial harm, temporary or
/// permanent phase. `HORS_NOMENCLATURE` is absent on purpose, being the refusal
/// to pick among these; rule 20 describes it in prose.
pub const POSTES: [(&str, &str, &str, &str, &str); 29] = [
    (
        "DSA",
        "directe",
        "patrimonial",
        "temporaire",
        "dépenses de santé actuelles",
    ),
    (
        "FD",
        "directe",
        "patrimonial",
        "temporaire",
        "frais divers, dont tierce personne temporaire",
    ),
    (
        "PGPA",
        "directe",
        "patrimonial",
        "temporaire",
        "perte de gains professionnels actuels",
    ),
    (
        "ATP",
        "directe",
        "patrimonial",
        "permanent",
        "assistance par tierce personne",
    ),
    (
        "DSF",
        "directe",
        "patrimonial",
        "permanent",
        "dépenses de santé futures",
    ),
    (
        "FLA",
        "directe",
        "patrimonial",
        "permanent",
        "frais de logement adapté",
    ),
    (
        "FVA",
        "directe",
        "patrimonial",
        "permanent",
        "frais de véhicule adapté",
    ),
    (
        "IP",
        "directe",
        "patrimonial",
        "permanent",
        "incidence professionnelle",
    ),
    (
        "PGPF",
        "directe",
        "patrimonial",
        "permanent",
        "perte de gains professionnels futurs",
    ),
    (
        "PSU",
        "directe",
        "patrimonial",
        "permanent",
        "préjudice scolaire, universitaire ou de formation",
    ),
    (
        "DFT",
        "directe",
        "extra-patrimonial",
        "temporaire",
        "déficit fonctionnel temporaire",
    ),
    (
        "PET",
        "directe",
        "extra-patrimonial",
        "temporaire",
        "préjudice esthétique temporaire",
    ),
    (
        "SE",
        "directe",
        "extra-patrimonial",
        "temporaire",
        "souffrances endurées",
    ),
    (
        "DFP",
        "directe",
        "extra-patrimonial",
        "permanent",
        "déficit fonctionnel permanent",
    ),
    (
        "PA",
        "directe",
        "extra-patrimonial",
        "permanent",
        "préjudice d'agrément",
    ),
    (
        "PE",
        "directe",
        "extra-patrimonial",
        "permanent",
        "préjudice d'établissement",
    ),
    (
        "PEP",
        "directe",
        "extra-patrimonial",
        "permanent",
        "préjudice esthétique permanent",
    ),
    (
        "PPE",
        "directe",
        "extra-patrimonial",
        "permanent",
        "préjudice permanent exceptionnel",
    ),
    (
        "PS",
        "directe",
        "extra-patrimonial",
        "permanent",
        "préjudice sexuel",
    ),
    (
        "PEV",
        "directe",
        "extra-patrimonial",
        "évolutif",
        "préjudice lié à une pathologie évolutive",
    ),
    (
        "FDV",
        "indirecte",
        "patrimonial",
        "",
        "frais divers des proches",
    ),
    ("FO", "indirecte", "patrimonial", "", "frais d'obsèques"),
    (
        "PR",
        "indirecte",
        "patrimonial",
        "",
        "perte de revenus des proches",
    ),
    (
        "PAC",
        "indirecte",
        "extra-patrimonial",
        "",
        "préjudice d'accompagnement",
    ),
    (
        "PAF",
        "indirecte",
        "extra-patrimonial",
        "",
        "préjudice d'affection",
    ),
    (
        "PEX",
        "indirecte",
        "extra-patrimonial",
        "",
        "préjudice extra-patrimonial exceptionnel",
    ),
    (
        "ANXIETE",
        "directe",
        "extra-patrimonial",
        "permanent",
        "préjudice d'anxiété, hors nomenclature",
    ),
    (
        "ATTENTE",
        "indirecte",
        "extra-patrimonial",
        "",
        "préjudice d'attente et d'inquiétude, hors nomenclature",
    ),
    (
        "IMPREPARATION",
        "directe",
        "extra-patrimonial",
        "temporaire",
        "défaut d'information, hors nomenclature",
    ),
];

impl Lien {
    pub const ALL: [Lien; 14] = [
        Lien::Conjoint,
        Lien::Concubin,
        Lien::Partenaire,
        Lien::Enfant,
        Lien::Parent,
        Lien::Fratrie,
        Lien::GrandParent,
        Lien::PetitEnfant,
        Lien::BeauParent,
        Lien::BeauFils,
        Lien::OncleTante,
        Lien::NeveuNiece,
        Lien::Cousin,
        Lien::Autre,
    ];
}

impl Unite {
    pub const ALL: [Unite; 8] = [
        Unite::Point,
        Unite::Jour,
        Unite::Semaine,
        Unite::Mois,
        Unite::An,
        Unite::Heure,
        Unite::Seance,
        Unite::EuroDeRente,
    ];
}

impl Statut {
    pub const ALL: [Statut; 7] = [
        Statut::Actif,
        Statut::Independant,
        Statut::SansEmploi,
        Statut::Retraite,
        Statut::Etudiant,
        Statut::Enfant,
        Statut::AuFoyer,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Actif => "actif",
            Self::Independant => "independant",
            Self::SansEmploi => "sans_emploi",
            Self::Retraite => "retraite",
            Self::Etudiant => "etudiant",
            Self::Enfant => "enfant",
            Self::AuFoyer => "au_foyer",
        }
    }
}

impl NatureAide {
    pub const ALL: [NatureAide; 3] = [
        NatureAide::Active,
        NatureAide::PresenceResponsable,
        NatureAide::Autre,
    ];
}

impl ModeEmploi {
    pub const ALL: [ModeEmploi; 2] = [ModeEmploi::Mandataire, ModeEmploi::Prestataire];
}

impl Regime {
    pub const ALL: [Regime; 10] = [
        Regime::Badinter,
        Regime::MedicaleFaute,
        Regime::MedicaleSolidarite,
        Regime::Nosocomiale,
        Regime::FauteInexcusable,
        Regime::Fgti,
        Regime::Fiva,
        Regime::Civen,
        Regime::Gav,
        Regime::DroitCommun,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Badinter => "badinter",
            Self::MedicaleFaute => "medicale_faute",
            Self::MedicaleSolidarite => "medicale_solidarite",
            Self::Nosocomiale => "nosocomiale",
            Self::FauteInexcusable => "faute_inexcusable",
            Self::Fgti => "fgti",
            Self::Fiva => "fiva",
            Self::Civen => "civen",
            Self::Gav => "gav",
            Self::DroitCommun => "droit_commun",
        }
    }
}

/// The fields where a value outside the list makes the whole record refused.
/// They are aggregation axes, and one more spelling is one more population.
pub fn vocabularies() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("lien", Lien::ALL.iter().map(|x| x.name()).collect()),
        ("unite", Unite::ALL.iter().map(|x| x.name()).collect()),
        (
            "statut_professionnel",
            Statut::ALL.iter().map(|x| x.name()).collect(),
        ),
        (
            "nature_aide",
            NatureAide::ALL.iter().map(|x| x.name()).collect(),
        ),
        (
            "mode_emploi",
            ModeEmploi::ALL.iter().map(|x| x.name()).collect(),
        ),
        ("regimes", Regime::ALL.iter().map(|x| x.name()).collect()),
    ]
}

/// The rows of the code table, as the protocol carries them.
pub fn postes_markdown() -> String {
    POSTES
        .iter()
        .map(|(c, v, n, p, l)| format!("| {c} | {v} | {n} | {p} | {l} |"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The rows of the closed-vocabulary table, without `regimes`, which the
/// protocol describes in prose at its rule 27.
pub fn vocabularies_markdown() -> String {
    vocabularies()
        .iter()
        .filter(|(nom, _)| *nom != "regimes")
        .map(|(nom, vals)| {
            let l = vals
                .iter()
                .map(|v| format!("`{v}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("| `{nom}` | {l} |")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_vocabulary_names_each_value_once_and_serde_answers_the_same() {
        for (nom, vals) in vocabularies() {
            let mut vus = vals.clone();
            vus.sort_unstable();
            vus.dedup();
            assert_eq!(vus.len(), vals.len(), "{nom} : une valeur en double");
        }
        for l in Lien::ALL {
            let back: Lien = serde_json::from_str(&format!("\"{}\"", l.name())).unwrap();
            assert_eq!(back.name(), l.name());
        }
        for r in Regime::ALL {
            let back: Regime = serde_json::from_str(&format!("\"{}\"", r.name())).unwrap();
            assert_eq!(back.name(), r.name());
        }
        for u in Unite::ALL {
            let back: Unite = serde_json::from_str(&format!("\"{}\"", u.name())).unwrap();
            assert_eq!(back.name(), u.name());
        }
        for st in Statut::ALL {
            let back: Statut = serde_json::from_str(&format!("\"{}\"", st.name())).unwrap();
            assert_eq!(back.name(), st.name());
        }
        for m in ModeEmploi::ALL {
            let back: ModeEmploi = serde_json::from_str(&format!("\"{}\"", m.name())).unwrap();
            assert_eq!(back.name(), m.name());
        }
    }

    #[test]
    fn the_three_axes_of_a_poste_take_their_values_from_a_closed_set() {
        for (code, victime, nature, phase, libelle) in POSTES {
            assert!(matches!(victime, "directe" | "indirecte"), "{code}");
            assert!(
                matches!(nature, "patrimonial" | "extra-patrimonial"),
                "{code}"
            );
            assert!(
                matches!(phase, "temporaire" | "permanent" | "évolutif" | ""),
                "{code}"
            );
            assert!(!libelle.is_empty(), "{code}");
        }
    }
}
