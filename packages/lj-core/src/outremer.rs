//! Ville-siège ↔ ressort ultramarin. Les deux ordres nomment différemment la
//! même juridiction d'outre-mer : l'ordre administratif par territoire
//! (« tribunal administratif de la Guyane »), l'ordre judiciaire par ville de
//! siège (« cour d'appel de Cayenne »), le registre CNB par un mélange des
//! deux. Cette table porte les classes d'équivalence ; chaque consommateur y
//! résout sa propre cible fermée.

/// Classes d'équivalence, ressort en tête puis villes-sièges et graphies
/// d'usage, en slug (`lj_fold::slug`). Un lieu sans nom de ville
/// d'usage distinct n'y figure pas, et « saint-pierre » est hors table : il
/// désigne aussi bien Saint-Pierre de La Réunion que la collectivité.
const RESSORTS: &[&[&str]] = &[
    &["guyane", "cayenne"],
    &["martinique", "fort-de-france"],
    &["guadeloupe", "basse-terre", "pointe-a-pitre"],
    &[
        "reunion",
        "la-reunion",
        "saint-denis-de-la-reunion",
        "saint-denis",
    ],
    &["mayotte", "mamoudzou"],
    &["nouvelle-caledonie", "noumea"],
    &["polynesie-francaise", "papeete", "tahiti"],
];

/// Graphies équivalentes d'un lieu ultramarin, `slug` compris ; vide hors
/// outre-mer.
pub fn equivalents(slug: &str) -> &'static [&'static str] {
    RESSORTS
        .iter()
        .copied()
        .find(|classe| classe.contains(&slug))
        .unwrap_or(&[])
}

/// Toutes les graphies de la table, ressorts et villes confondus — le
/// vocabulaire ultramarin qu'un consommateur ajoute à ses clés reconnues.
pub fn graphies() -> impl Iterator<Item = &'static str> {
    RESSORTS.iter().flat_map(|classe| classe.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalents_are_symmetric_and_empty_off_outremer() {
        assert!(equivalents("cayenne").contains(&"guyane"));
        assert!(equivalents("guyane").contains(&"cayenne"));
        assert!(equivalents("pointe-a-pitre").contains(&"guadeloupe"));
        assert!(equivalents("paris").is_empty());
        assert!(equivalents("saint-pierre").is_empty());
    }
}
