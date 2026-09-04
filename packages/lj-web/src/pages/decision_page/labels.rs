//! Libellés des codes de publication bruts (`publication_codes`). Les autres
//! libellés métier (solution, voie, office, domaine, juridictions) arrivent
//! résolus par l'API en `FacetTag`.

/// Table `code → libellé` des publications.
const PUBLICATION_LABELS: &[(&str, &str)] = &[
    ("b", "Publié au bulletin"),
    ("r", "Publié au rapport"),
    ("l", "Lettre de chambre"),
    ("c", "Communiqué"),
    ("n", "Inédit"),
    ("A", "Recueil Lebon"),
    ("B", "Tables du recueil Lebon"),
    ("C+", "Signalée"),
    ("R", "Signalée"),
    ("C", "Inédit au recueil Lebon"),
    ("D", "Inédit au recueil Lebon"),
    ("Z", "Inédit au recueil Lebon"),
];

fn publication_code_label(code: &str) -> Option<&'static str> {
    PUBLICATION_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, label)| *label)
}

/// Libellé joint depuis les codes (« Publié au bulletin · Rapport »), dédupliqué,
/// ordre préservé, codes inconnus ignorés. `None` si
/// vide / aucun code connu.
pub fn publication_label(codes: &[String]) -> Option<String> {
    if codes.is_empty() {
        return None;
    }
    let mut labels: Vec<&'static str> = Vec::new();
    for code in codes {
        if let Some(label) = publication_code_label(code) {
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
    }
    if labels.is_empty() {
        None
    } else {
        Some(labels.join(" · "))
    }
}

/// Libellé des seules publications notables (exclut les « Inédit… ») : filtre
/// les codes dont le label commence par `"Inédit"`, puis applique
/// `publication_label`.
pub fn publication_badge(codes: &[String]) -> Option<String> {
    let notable: Vec<String> = codes
        .iter()
        .filter(|c| {
            publication_code_label(c)
                .map(|label| !label.starts_with("Inédit"))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    publication_label(&notable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_label_joins_and_dedups() {
        let c = vec!["b".to_string(), "r".to_string()];
        assert_eq!(
            publication_label(&c).as_deref(),
            Some("Publié au bulletin · Publié au rapport")
        );
        // C/D/Z partagent un libellé : dédup.
        let c = vec!["C".to_string(), "D".to_string(), "Z".to_string()];
        assert_eq!(
            publication_label(&c).as_deref(),
            Some("Inédit au recueil Lebon")
        );
        assert_eq!(publication_label(&[]), None);
    }

    #[test]
    fn publication_badge_drops_inedit() {
        // "n" = "Inédit", "b" = notable.
        let c = vec!["n".to_string(), "b".to_string()];
        assert_eq!(publication_badge(&c).as_deref(), Some("Publié au bulletin"));
        // Que des inédits → None.
        let c = vec!["n".to_string(), "C".to_string()];
        assert_eq!(publication_badge(&c), None);
    }
}
