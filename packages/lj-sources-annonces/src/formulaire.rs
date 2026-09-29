//! The notice form, read from the PDF a platform renders it to. Three forms
//! share one shape, a `libellé : valeur` a line: the national AAPC form, the
//! eForms notice, which numbers its sections and opens a block per lot, and the
//! European form, which numbers its own in roman.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use lj_annonces::LotAnnonce;

use super::{fiche, DetailAnnonce};

/// A numbered heading names a section, never a field: it closes the value above
/// it, which would otherwise run on to the end of the form.
static TITRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:\d+(?:\.\d+)+\s+\S|section\s+\d)").expect("regex titre de section")
});

/// The heading that opens a lot, numbered by the eForms notice and bare in the
/// national form.
static LOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:\d+(?:\.\d+)*\s+)?lot(?:\s*n[°o])?\s*:").expect("regex en-tête de lot")
});

/// The description, as each form names it. `Description` alone is the eForms
/// field, and it ranks last: a form that has both writes its object under the
/// longer label.
const DESCRIPTIONS: [&str; 8] = [
    "description succinte du marche",
    "description succincte du marche",
    "description succincte",
    "description des prestations",
    "objet du marche",
    "intitule du marche",
    "objet",
    "description",
];

const CPV: [&str; 4] = [
    "nomenclature principale (cpv)",
    "classification cpv",
    "code cpv principal",
    "code cpv",
];

/// The lines as the label walk reads them: one field a line. A form quotes a
/// threshold as `> 90 k€`, so the markup it is handed is escaped.
fn html(lignes: &[&str]) -> String {
    lignes
        .iter()
        .map(|l| l.replace('&', "&amp;").replace('<', "&lt;"))
        .collect::<Vec<_>>()
        .join("<br>")
}

/// The fields of a run of lines, its section headings left out.
fn champs(lignes: &[&str]) -> Vec<fiche::Champ> {
    let corps: Vec<&str> = lignes
        .iter()
        .copied()
        .filter(|l| !TITRE.is_match(l))
        .collect();
    fiche::champs(&html(&corps))
}

/// The allotment, block by block, each heading opening a lot and the fields
/// under it naming it.
fn lots(lignes: &[&str]) -> Vec<LotAnnonce> {
    let bornes: Vec<usize> = lignes
        .iter()
        .enumerate()
        .filter(|(_, l)| LOT.is_match(l))
        .map(|(i, _)| i)
        .collect();
    let mut out = Vec::new();
    for (n, &debut) in bornes.iter().enumerate() {
        let fin = bornes.get(n + 1).copied().unwrap_or(lignes.len());
        let bloc = &lignes[debut + 1..fin];
        let champs = champs(bloc);
        // The eForms notice labels the lot name; the national form writes it
        // bare on the line under the heading.
        let nom = fiche::premier(&champs, &["titre"])
            .or_else(|| bloc.first().map(|l| l.trim_end_matches('.').to_string()));
        out.push(LotAnnonce {
            nom: nom.filter(|n| !n.is_empty()),
            description: fiche::premier(&champs, &["description"]),
            cpv: fiche::premier(&champs, &CPV)
                .as_deref()
                .and_then(fiche::cpv_de),
        });
    }
    out
}

/// The eForms notice lays its scale in a section of its own, a block a
/// criterion: its name under `Description`, its weight under `Pondération`,
/// whose label carries the unit. The block repeats identically for each lot.
fn bareme(lignes: &[&str]) -> Vec<String> {
    static ATTRIBUTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)crit[èe]res\s+d['\x{2019}]?attribution")
            .expect("regex critères d'attribution")
    });
    static NOM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^description\s*:\s*(\S.*?)\s*\.?$").expect("regex nom de critère")
    });
    static POIDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^pond[ée]ration\s*\(([^)]*)\)\s*:\s*([0-9]+(?:[.,][0-9]+)?)")
            .expect("regex pondération")
    });
    let mut out: Vec<String> = Vec::new();
    let mut dedans = false;
    let mut nom: Option<String> = None;
    for ligne in lignes {
        if TITRE.is_match(ligne) {
            dedans = ATTRIBUTION.is_match(ligne);
            nom = None;
            continue;
        }
        if !dedans {
            continue;
        }
        if let Some(m) = NOM.captures(ligne) {
            nom = Some(m[1].to_string());
        } else if let Some(m) = POIDS.captures(ligne) {
            if let Some(n) = nom.take() {
                let unite = match m[1].to_lowercase().contains("pourcentage") {
                    true => "%",
                    false => "points",
                };
                out.push(format!("{n} ({} {unite})", m[2].trim()));
            }
        }
    }
    let mut vus = HashSet::new();
    out.retain(|c| vus.insert(c.clone()));
    out
}

/// The object as a buyer that drafts its own notice names it, `Objet de la
/// consultation` or `Objet du marché et allotissement`: the head of the label,
/// in document order, once the ranked list above has found nothing.
fn objet_libre(champs: &[fiche::Champ]) -> Option<String> {
    let cles: Vec<&str> = champs
        .iter()
        .map(|c| c.cle.as_str())
        .filter(|c| c.starts_with("objet") || c.starts_with("description"))
        .collect();
    fiche::premier(champs, &cles)
}

/// What the form carries that a listing row does not. The place is not part of
/// it: the only address every form fills is the buyer's own, which is not where
/// the market applies.
pub(crate) fn detail(texte: &str) -> DetailAnnonce {
    let lignes: Vec<&str> = texte
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let champs = champs(&lignes);
    let objet = fiche::premier(&champs, &DESCRIPTIONS).or_else(|| objet_libre(&champs));
    let mut contenu = fiche::contenu(&champs, objet);
    let lots = lots(&lignes);
    if !lots.is_empty() {
        contenu.lots = lots;
    }
    if contenu.criteres.is_empty() {
        contenu.criteres = bareme(&lignes);
    }
    DetailAnnonce {
        cpv: fiche::premier(&champs, &CPV)
            .as_deref()
            .and_then(fiche::cpv_de),
        procedure: fiche::premier(&champs, &["type de procedure"]),
        contenu,
        ..DetailAnnonce::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_annonces::MontantEstime;

    /// Real eForms notice (SICTIAM, 01/09/2025), cut to one lot, with the scale
    /// of a second notice grafted in: the sampled ones state instead that the
    /// weighting lives in the rulebook.
    const EFORMS: &str = r#"Section 2 - Procédure
2.1 Procédure
Titre: SYSTÈME D’ACQUISITION DYNAMIQUE DE SOLUTIONS BASÉES SUR L’INTELLIGENCE ARTIFICIELLE.
Description: Le SICTIAM met en place un Système d'Acquisition Dynamique (SAD) visant à acquérir des outils basés sur
l'intelligence artificielle (IA) pour optimiser divers services publics.
Identifiant interne: 2025-AFFAIRE-14.
Type de Procédure: Autre procédure en plusieurs étapes.
2.1.1 Objectif
Nature du marché: fournitures.
Nomenclature principale (cpv): 48900000. 72212900.
2.1.3 Valeur
Valeur estimée hors TVA: 112000000 EUR.
2.1.6 Motifs d'exclusion
Description: Sans objet.
Section 5 - Lot
5.1 LOT N° : LOT-0001
Identifiant interne: Catégorie 1.
Titre: Performance administrative.
Description: Les solutions destinées à améliorer l’efficacité des fonctions supports et des services de gestion des
services publics.
5.1.1 Objectif
Type de marché: fournitures.
Classification CPV: 48900000. 30200000.
5.1.5 Valeur
Valeur (hors TVA): 20000000 EUR.
5.1.10 Critères d'attribution
Critère Prix :
Description : Prix.
Pondération (pourcentage, valeur exacte) : 60
Critère Qualité :
Description : Valeur technique.
Pondération (pourcentage, valeur exacte) : 40
5.1.11 Documents de marché
Pas de restriction en matière d’accès aux documents."#;

    #[test]
    fn la_notice_eforms_rend_les_quatre_champs() {
        let d = detail(EFORMS);
        assert_eq!(d.cpv.as_deref(), Some("48900000"));
        assert_eq!(
            d.procedure.as_deref(),
            Some("Autre procédure en plusieurs étapes.")
        );
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "Le SICTIAM met en place un Système d'Acquisition Dynamique (SAD) visant à \
                 acquérir des outils basés sur l'intelligence artificielle (IA) pour optimiser \
                 divers services publics."
            )
        );
        assert_eq!(
            d.contenu.lots,
            [LotAnnonce {
                nom: Some("Performance administrative.".to_string()),
                description: Some(
                    "Les solutions destinées à améliorer l’efficacité des fonctions supports \
                     et des services de gestion des services publics."
                        .to_string()
                ),
                cpv: Some("48900000".to_string()),
            }]
        );
        assert_eq!(
            d.contenu.criteres,
            ["Prix (60 %)", "Valeur technique (40 %)"]
        );
        assert_eq!(
            d.contenu.montant_estime,
            Some(MontantEstime {
                valeur: 112_000_000.0,
                devise: Some("EUR".to_string()),
            })
        );
    }

    /// Real national AAPC form (Conseil départemental de l'Ariège, 24/09/2025),
    /// cut to two lots. Its `Description succinte` is the typo the form ships.
    const NATIONAL: &str = r#"Section 3 : Procédure
Type de procédure : Procédure adaptée ouverte
Technique d'achat : Système d'acquisition dynamique (SAD)
Critères d'attribution : Offre économiquement la plus avantageuse :
- prix des prestations : 40 %
- valeur technique : 60 %
Section 4 : Identification du marché
Intitulé du marché : SAD ETUDES ET PRESTATIONS TECHNIQUES
Classification CPV : 71313000
Description succinte du marché : SYSTÈME D'ACQUISITION DYNAMIQUE POUR DES ÉTUDES ET PRESTATIONS TECHNIQUES
DANS LE CADRE DU PROJET DE SÉCURISATION DU REMPLISSAGE DU BARRAGE DE MONTBEL
Valeur estimée hors TVA : 131800 euros
Marché alloti : Oui
Section 5 : Informations sur les lots
LOT :
Catégorie 1 : Étude de résistivité
Classification CPV : 71351000
Valeur estimée du lot hors TVA : 4800 euros
LOT :
Catégorie 2 : Détection des réseaux
Classification CPV : 71351900
Valeur estimée du lot hors TVA : 6000 euros"#;

    #[test]
    fn le_formulaire_national_rend_les_quatre_champs() {
        let d = detail(NATIONAL);
        assert_eq!(d.cpv.as_deref(), Some("71313000"));
        assert_eq!(d.procedure.as_deref(), Some("Procédure adaptée ouverte"));
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "SYSTÈME D'ACQUISITION DYNAMIQUE POUR DES ÉTUDES ET PRESTATIONS TECHNIQUES \
                 DANS LE CADRE DU PROJET DE SÉCURISATION DU REMPLISSAGE DU BARRAGE DE MONTBEL"
            )
        );
        assert_eq!(
            d.contenu.lots,
            [
                LotAnnonce {
                    nom: Some("Catégorie 1 : Étude de résistivité".to_string()),
                    description: None,
                    cpv: Some("71351000".to_string()),
                },
                LotAnnonce {
                    nom: Some("Catégorie 2 : Détection des réseaux".to_string()),
                    description: None,
                    cpv: Some("71351900".to_string()),
                },
            ]
        );
        assert_eq!(
            d.contenu.criteres,
            ["prix des prestations (40 %)", "valeur technique (60 %)"]
        );
        assert_eq!(
            d.contenu.montant_estime,
            Some(MontantEstime {
                valeur: 131_800.0,
                devise: Some("EUR".to_string()),
            })
        );
    }

    /// Real European form (communauté urbaine de Dunkerque), whose sections are
    /// numbered in roman and whose labels carry that numbering.
    const EUROPEEN: &str = r#"II.1.4) Description succincte : La présente consultation a pour objet la mise en place d'un système
d'acquisition dynamique portant sur la fourniture et l'acheminement de gaz naturel.
II.1.5) Valeur totale estimée :
Valeur hors TVA : 100 000 000 euros
IV.1.1) Type de procédure : Procédure restreinte"#;

    /// Real notice a buyer laid out itself (commune de Buzancy), which numbers
    /// its own sections and closes that numbering on a slash.
    #[test]
    fn la_numerotation_a_barre_oblique_se_lit_comme_celle_a_parenthese() {
        let d = detail(
            "COMMUNE DE BUZANCY\n\
             I/ IDENTIFICATION DE L'ORGANISME QUI PASSE LE MARCHE : Commune de Buzancy\n\
             IV/ OBJET DU MARCHE : Aménagement de la rue des Barrières\n\
             VI/ LIEU D'EXECUTION : Rue des Barrières - Buzancy",
        );
        assert_eq!(
            d.contenu.description.as_deref(),
            Some("Aménagement de la rue des Barrières")
        );
    }

    #[test]
    fn la_numerotation_romaine_ne_masque_pas_le_libelle() {
        let d = detail(EUROPEEN);
        assert_eq!(d.procedure.as_deref(), Some("Procédure restreinte"));
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "La présente consultation a pour objet la mise en place d'un système \
                 d'acquisition dynamique portant sur la fourniture et l'acheminement de gaz \
                 naturel."
            )
        );
        assert_eq!(
            d.contenu.montant_estime,
            Some(MontantEstime {
                valeur: 100_000_000.0,
                devise: Some("EUR".to_string()),
            })
        );
    }

    /// A notice the buyer published as a scan: poppler renders the page and
    /// nothing else. Reading it yields nothing rather than a fragment.
    #[test]
    fn un_avis_scanne_ne_rend_rien() {
        assert_eq!(detail("AAPC"), DetailAnnonce::default());
    }

    /// Real notice drafted by the buyer (Terres de Montaigu, 08/09/2026): it
    /// numbers its own labels, and names the object in its own words.
    #[test]
    fn le_libelle_numerote_en_chiffres_se_lit_comme_celui_numerote_en_romain() {
        let d = detail(
            "AVIS D’APPEL PUBLIC A LA CONCURRENCE (AAPC)\n\
             APPEL D’OFFRES OUVERT\n\
             1. OBJET DE LA CONSULTATION : Fourniture, évolution, sécurisation et maintenance \
             des infrastructures du système d’informations de la Communauté d’agglomération\n\
             2. POUVOIR ADJUDICATEUR : Terres de Montaigu, Communauté d’agglomération\n",
        );
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "Fourniture, évolution, sécurisation et maintenance des infrastructures du \
                 système d’informations de la Communauté d’agglomération"
            )
        );
    }

    /// Real notice (Grand Delta Habitat, 09/09/2026): the section heading repeats
    /// word for word the label of the field under it, and carries nothing.
    #[test]
    fn un_titre_de_section_qui_repete_le_libelle_ne_masque_pas_le_champ() {
        let d = detail(
            "AVIS D'APPEL PUBLIC A LA CONCURRENCE\n\
             OBJET DU MARCHE\n\
             Objet du marché : Réhabilitation de 40 logements collectifs de la résidence \
             «Saint-Joseph»\n\
             Type de Marché de travaux : Exécution - Lieu d'exécution : APT (84400)\n",
        );
        assert_eq!(
            d.contenu.description.as_deref(),
            Some("Réhabilitation de 40 logements collectifs de la résidence «Saint-Joseph»")
        );
    }
}
