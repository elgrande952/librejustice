//! Poste labels and amount cues of a liquidation, matched on the folded text of
//! a scan. Own automaton: the fused one is leftmost-longest, and a label that
//! extends one of its markers (« au titre ») would take the marker's role.
use std::sync::OnceLock;

use aho_corasick::{AhoCorasick, AhoCorasickKind, MatchKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    /// A label of the nomenclature, with its code. `FD` switches to `FDV` at
    /// composition for an indirect victim.
    Poste(&'static str),
    Sum,
    Total,
    Under,
    Including,
    Soit,
    PayTo,
    Fix,
    Condemn,
    Reject,
    /// An amount that is not an indemnity, with the cause the record declares.
    OutOfScope(&'static str),
}

/// Folded surfaces. Labels naming two codes at once (« préjudice moral »,
/// « préjudice esthétique » alone, « assistance par tierce personne » alone,
/// « troubles dans les conditions d'existence ») are absent on purpose.
#[rustfmt::skip]
const SURFACES: &[(&str, Cue)] = &[
    ("depenses de sante actuelles", Cue::Poste("DSA")),
    ("depenses de sante actuelle", Cue::Poste("DSA")),
    ("depenses de sante echues", Cue::Poste("DSA")),
    ("frais divers", Cue::Poste("FD")),
    ("tierce personne temporaire", Cue::Poste("FD")),
    ("assistance par tierce personne temporaire", Cue::Poste("FD")),
    ("assistance tierce personne temporaire", Cue::Poste("FD")),
    ("assistance temporaire par tierce personne", Cue::Poste("FD")),
    ("assistance temporaire tierce personne", Cue::Poste("FD")),
    ("assistance temporaire par une tierce personne", Cue::Poste("FD")),
    ("aide par tierce personne temporaire", Cue::Poste("FD")),
    ("aide humaine temporaire", Cue::Poste("FD")),
    ("aide humaine permanente", Cue::Poste("ATP")),
    ("aide par tierce personne permanente", Cue::Poste("ATP")),
    ("perte de gains professionnels actuels", Cue::Poste("PGPA")),
    ("pertes de gains professionnels actuels", Cue::Poste("PGPA")),
    ("perte de gains professionnels actuelle", Cue::Poste("PGPA")),
    ("pertes de gains professionnels actuelles", Cue::Poste("PGPA")),
    ("perte de gains professionnels avant consolidation", Cue::Poste("PGPA")),
    ("pertes de gains professionnels avant consolidation", Cue::Poste("PGPA")),
    ("assistance par tierce personne permanente", Cue::Poste("ATP")),
    ("assistance par une tierce personne permanente", Cue::Poste("ATP")),
    ("assistance tierce personne permanente", Cue::Poste("ATP")),
    ("assistance permanente par tierce personne", Cue::Poste("ATP")),
    ("assistance par tierce personne definitive", Cue::Poste("ATP")),
    ("tierce personne permanente", Cue::Poste("ATP")),
    ("tierce personne definitive", Cue::Poste("ATP")),
    ("tierce personne future", Cue::Poste("ATP")),
    ("depenses de sante futures", Cue::Poste("DSF")),
    ("depenses de sante future", Cue::Poste("DSF")),
    ("frais de logement adapte", Cue::Poste("FLA")),
    ("frais d'amenagement du logement", Cue::Poste("FLA")),
    ("frais d'adaptation du logement", Cue::Poste("FLA")),
    ("frais de vehicule adapte", Cue::Poste("FVA")),
    ("frais d'adaptation du vehicule", Cue::Poste("FVA")),
    ("incidence professionnelle", Cue::Poste("IP")),
    ("perte des droits a la retraite", Cue::Poste("IP")),
    ("perte de droits a la retraite", Cue::Poste("IP")),
    ("perte de droits a retraite", Cue::Poste("IP")),
    ("perte de retraite", Cue::Poste("IP")),
    ("prejudice de retraite", Cue::Poste("IP")),
    ("perte de gains professionnels futurs", Cue::Poste("PGPF")),
    ("pertes de gains professionnels futurs", Cue::Poste("PGPF")),
    ("perte de gains professionnels future", Cue::Poste("PGPF")),
    ("pertes de gains professionnels futures", Cue::Poste("PGPF")),
    ("perte de gains professionnels apres consolidation", Cue::Poste("PGPF")),
    ("pertes de gains professionnels apres consolidation", Cue::Poste("PGPF")),
    ("perte de gains futurs", Cue::Poste("PGPF")),
    ("pertes de gains futurs", Cue::Poste("PGPF")),
    ("prejudice scolaire", Cue::Poste("PSU")),
    ("prejudice universitaire", Cue::Poste("PSU")),
    ("prejudice de formation", Cue::Poste("PSU")),
    ("deficit fonctionnel temporaire", Cue::Poste("DFT")),
    ("prejudice esthetique temporaire", Cue::Poste("PET")),
    ("prejudice esthetique avant consolidation", Cue::Poste("PET")),
    ("souffrances endurees", Cue::Poste("SE")),
    ("souffrances physiques et morales endurees", Cue::Poste("SE")),
    ("souffrances morales et physiques endurees", Cue::Poste("SE")),
    ("pretium doloris", Cue::Poste("SE")),
    ("deficit fonctionnel permanent", Cue::Poste("DFP")),
    ("deficit fonctionnel definitif", Cue::Poste("DFP")),
    ("deficit fonctionnel sequellaire", Cue::Poste("DFP")),
    ("prejudice d'agrement", Cue::Poste("PA")),
    ("prejudice d'agrement temporaire", Cue::Poste("HORS_NOMENCLATURE")),
    ("prejudice d'etablissement", Cue::Poste("PE")),
    ("prejudice esthetique permanent", Cue::Poste("PEP")),
    ("prejudice esthetique definitif", Cue::Poste("PEP")),
    ("prejudice permanent exceptionnel", Cue::Poste("PPE")),
    ("prejudices permanents exceptionnels", Cue::Poste("PPE")),
    ("prejudice sexuel", Cue::Poste("PS")),
    ("prejudice lie a une pathologie evolutive", Cue::Poste("PEV")),
    ("prejudices lies a des pathologies evolutives", Cue::Poste("PEV")),
    ("prejudice extra-patrimonial evolutif", Cue::Poste("PEV")),
    ("prejudice extrapatrimonial evolutif", Cue::Poste("PEV")),
    ("frais divers des proches", Cue::Poste("FDV")),
    ("frais d'obseques", Cue::Poste("FO")),
    ("frais funeraires", Cue::Poste("FO")),
    ("perte de revenus des proches", Cue::Poste("PR")),
    ("pertes de revenus des proches", Cue::Poste("PR")),
    ("perte de revenus", Cue::Poste("PR")),
    ("pertes de revenus", Cue::Poste("PR")),
    ("prejudice d'accompagnement", Cue::Poste("PAC")),
    ("prejudice d'affection", Cue::Poste("PAF")),
    ("prejudice extra-patrimonial exceptionnel", Cue::Poste("PEX")),
    ("prejudice extrapatrimonial exceptionnel", Cue::Poste("PEX")),
    ("prejudice extra patrimonial exceptionnel", Cue::Poste("PEX")),
    ("prejudice d'anxiete", Cue::Poste("ANXIETE")),
    ("prejudice d'attente et d'inquietude", Cue::Poste("ATTENTE")),
    ("prejudice d'impreparation", Cue::Poste("IMPREPARATION")),
    ("la somme de", Cue::Sum),
    ("les sommes de", Cue::Sum),
    ("une somme de", Cue::Sum),
    ("celle de", Cue::Sum),
    ("la somme totale de", Cue::Total),
    ("la somme globale de", Cue::Total),
    ("montant total", Cue::Total),
    ("montant global", Cue::Total),
    ("au total", Cue::Total),
    ("total", Cue::Total),
    ("totaux", Cue::Total),
    ("sous-total", Cue::Total),
    ("sous total", Cue::Total),
    ("prejudices patrimoniaux temporaires", Cue::OutOfScope("intermediaire")),
    ("prejudices patrimoniaux permanents", Cue::OutOfScope("intermediaire")),
    ("prejudices patrimoniaux", Cue::OutOfScope("intermediaire")),
    ("prejudices extra patrimoniaux temporaires", Cue::OutOfScope("intermediaire")),
    ("prejudices extra-patrimoniaux temporaires", Cue::OutOfScope("intermediaire")),
    ("prejudices extrapatrimoniaux temporaires", Cue::OutOfScope("intermediaire")),
    ("prejudices extra patrimoniaux permanents", Cue::OutOfScope("intermediaire")),
    ("prejudices extra-patrimoniaux permanents", Cue::OutOfScope("intermediaire")),
    ("prejudices extrapatrimoniaux permanents", Cue::OutOfScope("intermediaire")),
    ("prejudices extra patrimoniaux", Cue::OutOfScope("intermediaire")),
    ("prejudices extra-patrimoniaux", Cue::OutOfScope("intermediaire")),
    ("prejudices extrapatrimoniaux", Cue::OutOfScope("intermediaire")),
    ("au titre de", Cue::Under),
    ("au titre du", Cue::Under),
    ("au titre des", Cue::Under),
    ("en reparation de", Cue::Under),
    ("en reparation du", Cue::Under),
    ("en reparation des", Cue::Under),
    ("a titre de", Cue::Under),
    ("a titre d'", Cue::Under),
    ("sur le fondement de", Cue::Under),
    ("en application de", Cue::Under),
    ("dont", Cue::Including),
    ("soit", Cue::Soit),
    ("a payer a", Cue::PayTo),
    ("a payer aux", Cue::PayTo),
    ("a verser a", Cue::PayTo),
    ("a verser aux", Cue::PayTo),
    ("a regler a", Cue::PayTo),
    ("a regler aux", Cue::PayTo),
    ("fixe", Cue::Fix),
    ("evalue", Cue::Fix),
    ("liquide", Cue::Fix),
    ("condamne", Cue::Condemn),
    ("deboute", Cue::Reject),
    ("rejette", Cue::Reject),
    ("article 700", Cue::OutOfScope("article_700")),
    ("depens", Cue::OutOfScope("depens")),
    ("frais irrepetibles", Cue::OutOfScope("article_700")),
    ("provision", Cue::OutOfScope("provision")),
    ("provisions", Cue::OutOfScope("provision")),
    ("provisionnelle", Cue::OutOfScope("provision")),
    ("provisionnel", Cue::OutOfScope("provision")),
    ("provisionnelles", Cue::OutOfScope("provision")),
    ("provisionnels", Cue::OutOfScope("provision")),
    ("debours", Cue::OutOfScope("tiers_payeur")),
    ("recours subrogatoire", Cue::OutOfScope("tiers_payeur")),
    ("indemnite forfaitaire de gestion", Cue::OutOfScope("tiers_payeur")),
    ("prestations servies", Cue::OutOfScope("tiers_payeur")),
    ("liquides et taxes", Cue::OutOfScope("depens")),
    ("761-1", Cue::OutOfScope("article_700")),
];

/// A cue positioned in chars of the scanned text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CueTok {
    pub s: usize,
    pub e: usize,
    pub cue: Cue,
}

fn automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| {
        AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .kind(Some(AhoCorasickKind::DFA))
            .build(SURFACES.iter().map(|(s, _)| *s))
            .expect("quantum vocabulary automaton")
    })
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// Every cue of `folded`, bounded by non-word characters, in text order.
/// `byte2char` maps a byte of `folded` to the index of its char.
pub fn cues(folded: &str, byte2char: &[usize]) -> Vec<CueTok> {
    let mut out = Vec::new();
    for m in automaton().find_iter(folded) {
        let (bs, be) = (m.start(), m.end());
        let before = folded[..bs].chars().next_back();
        let after = folded[be..].chars().next();
        let glued = after.is_some_and(word_char) && !folded[..be].ends_with('\'');
        if before.is_some_and(word_char) || glued {
            continue;
        }
        let s = byte2char[bs];
        let e = s + folded[bs..be].chars().count();
        out.push(CueTok {
            s,
            e,
            cue: SURFACES[m.pattern().as_usize()].1,
        });
    }
    out
}

/// `Some(true)` for a code only a direct victim receives, `Some(false)` for a
/// ricochet one, `None` when courts award it on both sides.
pub fn direct_side(code: &str) -> Option<bool> {
    match code {
        "DSA" | "PGPA" | "ATP" | "DSF" | "FLA" | "FVA" | "IP" | "PGPF" | "PSU" | "DFT" | "PET"
        | "SE" | "DFP" | "PEP" | "PPE" | "PEV" => Some(true),
        "FDV" | "PAC" | "PAF" | "PEX" | "ATTENTE" | "MORAL" => Some(false),
        _ => None,
    }
}

/// Codes a third-party payer never shares: the amount the court fixes is the
/// poste in full and the victim's share at once.
pub fn extra_patrimonial(code: &str) -> bool {
    matches!(
        code,
        "DFT"
            | "PET"
            | "SE"
            | "DFP"
            | "PA"
            | "PEP"
            | "PS"
            | "PE"
            | "PPE"
            | "PEV"
            | "PAF"
            | "PEX"
            | "PAC"
            | "ATTENTE"
            | "ANXIETE"
            | "IMPREPARATION"
            | "PSU"
            | "HORS_NOMENCLATURE"
    )
}

/// Words that name a family of postes without settling the code, with the codes
/// a line carrying them could merge into.
pub const AMBIGUOUS: &[(&str, &[&str])] = &[
    ("tierce personne", &["FD", "ATP", "FDV"]),
    ("aide humaine", &["FD", "ATP"]),
    ("assistance", &["FD", "ATP", "FDV"]),
    ("retraite", &["IP"]),
    ("prejudice esthetique", &["PET", "PEP"]),
    ("deficit fonctionnel", &["DFT", "DFP"]),
    ("perte de gains", &["PGPA", "PGPF"]),
    ("gains professionnels", &["PGPA", "PGPF"]),
    ("revenus", &["PGPA", "PGPF", "PR"]),
    ("salaire", &["PGPA", "PGPF"]),
    ("prejudice moral", &["PAF", "MORAL"]),
    ("affection", &["PAF"]),
    ("conditions d'existence", &["PA", "PAC", "PEX"]),
    ("agrement", &["PA"]),
    ("sante", &["DSA", "DSF"]),
    ("medicaux", &["DSA", "DSF"]),
    ("frais", &["FD", "FDV", "DSA", "DSF", "FLA", "FVA", "FO"]),
    ("logement", &["FLA"]),
    ("vehicule", &["FVA"]),
    ("obseques", &["FO"]),
    ("funeraires", &["FO"]),
    ("scolaire", &["PSU"]),
    ("formation", &["PSU"]),
    ("souffrances", &["SE"]),
    ("sexuel", &["PS"]),
    ("etablissement", &["PE"]),
    ("accompagnement", &["PAC"]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiled::Norm;

    fn cues_of(text: &str) -> Vec<(Cue, String)> {
        let n = Norm::new(text);
        cues(&n.folded, &n.byte2char)
            .into_iter()
            .map(|c| (c.cue, n.chars[c.s..c.e].iter().collect()))
            .collect()
    }

    #[test]
    fn labels_fold_and_stop_at_word_boundaries() {
        let got = cues_of("Déficit fonctionnel permanent : 27 600 euros ; souffrances endurées");
        assert_eq!(
            got,
            vec![
                (
                    Cue::Poste("DFP"),
                    "Déficit fonctionnel permanent".to_string()
                ),
                (Cue::Poste("SE"), "souffrances endurées".to_string()),
            ]
        );
        assert!(cues_of("les dépenses de santé actuellement engagées").is_empty());
    }

    #[test]
    fn the_longest_surface_wins_and_ambiguous_labels_are_absent() {
        let got = cues_of("préjudice d'agrément temporaire, préjudice moral, préjudice esthétique");
        assert_eq!(
            got.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            vec![Cue::Poste("HORS_NOMENCLATURE")]
        );
    }

    #[test]
    fn cues_of_a_dispositif_clause() {
        let got = cues_of(
            "CONDAMNE la société X à payer à M. [A] la somme de 38 017,83 euros dont 13 017,83 euros au titre de sa perte de revenus",
        );
        assert_eq!(
            got.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            vec![
                Cue::Condemn,
                Cue::PayTo,
                Cue::Sum,
                Cue::Including,
                Cue::Under,
                Cue::Poste("PR")
            ]
        );
    }
}
