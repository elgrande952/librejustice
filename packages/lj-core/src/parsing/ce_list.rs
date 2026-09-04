//! Parse pur des « grandes décisions depuis 1873 » du Conseil d'État : la page
//! de liste porte le titre et le chemin d'une fiche, la fiche porte les liens
//! ArianeWeb qui ancrent les décisions commentées. Le titre est la seule source
//! du nom : le texte de l'arrêt Blanco dit « Jean Y… ».

use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::text::{fold, normalize_spaces};

/// Une entrée de la liste : le chemin de sa fiche et son titre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeListEntry {
    pub path: String,
    pub title: String,
}

/// Ancre exacte d'une décision, telle que le lien ArianeWeb de la fiche la
/// grave. `jurisdiction` est replié en minuscules comme
/// `decisions.jurisdiction_code` ; `docket` garde les zéros de tête que le
/// lien pose ou non (`00012` pour Blanco, `1645` pour Dehaene).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeListAnchor {
    pub jurisdiction: String,
    /// Date de lecture ISO.
    pub date: String,
    pub docket: String,
}

/// Nom d'usage porté par un segment du titre, avec la date qui l'ancre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeListName {
    /// Date de lecture ISO.
    pub date: String,
    pub name: String,
}

static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<a class="list__item__body"[^>]*\shref="([^"]+)"[^>]*>\s*<p class="list__item__title">(.*?)</p>"#,
    )
    .unwrap()
});

/// Lien de décision ArianeWeb. Les liens `analyse/` ne sont pas des ancres :
/// la fiche « Action Française » (TC, 1935) renvoie vers une analyse CE de
/// 1979.
static ARIANE_DECISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)arianeweb/([a-z]+)/decision/(\d{4}-\d{2}-\d{2})/([a-z0-9]+)").unwrap()
});

static FR_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d{1,2}|1er)\s+(\p{L}+)\s+(\d{4})\b").unwrap());

/// Marqueur de juridiction : il ouvre la citation d'une décision dans un titre
/// qui en porte deux.
static JURIDICTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Conseil d['’]\s*[EÉ]tat|Tribunal des conflits|\bCE\b|\bTC\b").unwrap()
});

/// Queue de numéros du titre (`, n° 388902`, ` n°s 391000, 393769`).
static DOCKET_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bn\s*°").unwrap());

static ET_AUTRES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bet\s+autres?\b").unwrap());

static CIVILITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:M\.|MM\.|Mme|Mmes|Mlle|Mlles|[EÉ]poux|Dame|Demoiselle|Consorts|Sieur)(?:\s|$)",
    )
    .unwrap()
});

static MOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{L}+").unwrap());

const FR_MONTHS: [&str; 12] = [
    "janvier",
    "fevrier",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "aout",
    "septembre",
    "octobre",
    "novembre",
    "decembre",
];

/// Entrées d'une page de liste, dans l'ordre du document.
pub fn parse_ce_list_page(html: &str) -> Vec<CeListEntry> {
    LIST_ITEM
        .captures_iter(html)
        .map(|c| CeListEntry {
            path: c[1].to_string(),
            title: normalize_spaces(&html_escape::decode_html_entities(&c[2])),
        })
        .collect()
}

/// Ancres ArianeWeb d'une fiche, dans l'ordre du document.
pub fn parse_ce_list_anchors(html: &str) -> Vec<CeListAnchor> {
    ARIANE_DECISION
        .captures_iter(html)
        .map(|c| CeListAnchor {
            jurisdiction: c[1].to_lowercase(),
            date: c[2].to_string(),
            docket: c[3].to_lowercase(),
        })
        .collect()
}

/// Noms portés par un titre, un par décision citée. Le nom court de la date à
/// la juridiction qui ouvre la citation suivante, et perd la queue de numéros.
pub fn ce_list_names(title: &str) -> Vec<CeListName> {
    let dates: Vec<Captures> = FR_DATE.captures_iter(title).collect();
    let mut out = Vec::new();
    for (i, c) in dates.iter().enumerate() {
        let Some(date) = iso_date(c) else { continue };
        let debut = c.get(0).expect("groupe 0").end();
        let fin = dates
            .get(i + 1)
            .map_or(title.len(), |n| n.get(0).expect("groupe 0").start());
        let mut segment = &title[debut..fin];
        if i + 1 < dates.len() {
            if let Some(j) = JURIDICTION.find_iter(segment).last() {
                segment = &segment[..j.start()];
            }
        }
        let name = clean_name(segment);
        if !name.is_empty() {
            out.push(CeListName { date, name });
        }
    }
    out
}

/// Découpe un nom conjoint en autant de parts que la fiche ancre de décisions,
/// dans l'ordre du titre. La virgule sépare d'abord, le « et » ensuite : c'est
/// l'ordre qui garde « Société Million et Marais » entière parmi les trois noms
/// de la fiche Yonne Funéraire, et qui coupe pourtant « Hardouin et Marie » en
/// deux. Rend `None` quand aucun découpage ne tombe sur le compte : le nom
/// reste alors celui d'aucune décision.
pub fn split_joint_name(name: &str, parts: usize) -> Option<Vec<String>> {
    if parts < 2 {
        return None;
    }
    for sep in [&*COMMA, &*COMMA_OR_ET] {
        let morceaux: Vec<String> = sep
            .split(name)
            .map(clean_name)
            .filter(|m| !m.is_empty())
            .collect();
        if morceaux.len() == parts {
            return Some(morceaux);
        }
    }
    None
}

static COMMA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[,;]\s*").unwrap());

static COMMA_OR_ET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*[,;]\s*|\s+et\s+").unwrap());

/// Un nom réduit à des initiales derrière une civilité : la source
/// pseudonymise ses entrées récentes (« M. A…B… », « Mme C. et autres »), et
/// des initiales ne nomment rien. « G.I.S.T.I., C.F.D.T. et C.G.T. », sans
/// civilité, reste un nom.
pub fn is_pseudonymised(name: &str) -> bool {
    let sans_autres = ET_AUTRES.replace_all(name, "");
    let sans_autres = sans_autres.trim();
    let Some(civilite) = CIVILITE.find(sans_autres) else {
        return false;
    };
    MOT.find_iter(&sans_autres[civilite.end()..])
        .all(|m| m.as_str().chars().count() == 1)
}

fn clean_name(segment: &str) -> String {
    let sans_numeros = DOCKET_TAIL
        .find(segment)
        .map_or(segment, |m| &segment[..m.start()]);
    normalize_spaces(sans_numeros.trim_matches(|c: char| c.is_whitespace() || ",;-".contains(c)))
}

fn iso_date(c: &Captures) -> Option<String> {
    let day: u32 = if c[1].eq_ignore_ascii_case("1er") {
        1
    } else {
        c[1].parse().ok()?
    };
    let mois = fold(&c[2]);
    let month = FR_MONTHS.iter().position(|m| *m == mois)? + 1;
    (1..=31)
        .contains(&day)
        .then(|| format!("{}-{month:02}-{day:02}", &c[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTE: &str = r#"
<div class="list__item__right">
    <a class="list__item__body"  href="/decisions-de-justice/jurisprudence/les-grandes-decisions-depuis-1873/conseil-d-etat-3-juillet-1996-kone" >
                    <p class="list__item__title">Conseil d&#039;&Eacute;tat, 3 juillet 1996, Kon&eacute;</p>
                                            <p class="list__item__teasing">
    Interprétation d’un traité d’extradition
</p>
    </a>
</div>
<div class="list__item__right">
    <a class="list__item__body"  href="/decisions-de-justice/jurisprudence/les-grandes-decisions-depuis-1873/ce-21-juin-1895-cames" >
                    <p class="list__item__title">CE 21 juin 1895 Cames</p>
    </a>
</div>
"#;

    #[test]
    fn la_liste_rend_chemin_et_titre_decodes() {
        let entries = parse_ce_list_page(LISTE);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Conseil d'État, 3 juillet 1996, Koné");
        assert!(entries[0]
            .path
            .ends_with("/conseil-d-etat-3-juillet-1996-kone"));
        assert_eq!(entries[1].title, "CE 21 juin 1895 Cames");
    }

    #[test]
    fn la_fiche_rend_ses_ancres_de_decision() {
        let html = r#"<a href="/decisions-de-justice/jurisprudence/rechercher-une-decision-arianeweb">Rechercher</a>
            <p><a href="http://www.conseil-etat.fr/fr/arianeweb/TC/decision/1873-02-08/00012">&gt; Lire la décision</a></p>"#;
        assert_eq!(
            parse_ce_list_anchors(html),
            vec![CeListAnchor {
                jurisdiction: "tc".to_string(),
                date: "1873-02-08".to_string(),
                docket: "00012".to_string(),
            }]
        );
    }

    #[test]
    fn une_analyse_nest_pas_une_ancre() {
        let html = r#"<a href="https://www.conseil-etat.fr/fr/arianeweb/CE/analyse/1979-12-07/00822">analyse</a>"#;
        assert!(parse_ce_list_anchors(html).is_empty());
    }

    #[test]
    fn le_nom_court_de_la_date_a_la_queue_de_numeros() {
        for (titre, date, nom) in [
            (
                "Tribunal des conflits, 8 février 1873, Blanco",
                "1873-02-08",
                "Blanco",
            ),
            ("CE 21 juin 1895 Cames", "1895-06-21", "Cames"),
            (
                "Conseil d'État, 26 octobre 2001 - M. Ternon",
                "2001-10-26",
                "M. Ternon",
            ),
            (
                "CE, 14 janvier 1938 - Société anonyme des produits laitiers \"La Fleurette\"",
                "1938-01-14",
                "Société anonyme des produits laitiers \"La Fleurette\"",
            ),
            (
                "CE, 5 mai 2017 , Commune de Saint Bon Tarentaise, n° 388902",
                "2017-05-05",
                "Commune de Saint Bon Tarentaise",
            ),
            (
                "CE, Ass., 24 juin 2014, Mme Lambert, n°s 375081, 375090 et 37509",
                "2014-06-24",
                "Mme Lambert",
            ),
            (
                "Conseil d'État, 25 juin 1948, Société du journal « L’Aurore »",
                "1948-06-25",
                "Société du journal « L’Aurore »",
            ),
            (
                "Conseil d’État, 20 avril 1956, Époux Bertin et Ministre de l’agriculture c/ consorts Grimouard et autres",
                "1956-04-20",
                "Époux Bertin et Ministre de l’agriculture c/ consorts Grimouard et autres",
            ),
        ] {
            assert_eq!(
                ce_list_names(titre),
                vec![CeListName {
                    date: date.to_string(),
                    name: nom.to_string()
                }],
                "titre {titre:?}"
            );
        }
    }

    #[test]
    fn un_titre_porte_parfois_deux_decisions() {
        assert_eq!(
            ce_list_names(
                "Conseil d'État, 21 mars 2016, Société Fairvesta International GMBH et autres ; \
                 Conseil d'État Assemblée, 21 mars 2016, Société NC Numericable"
            ),
            vec![
                CeListName {
                    date: "2016-03-21".to_string(),
                    name: "Société Fairvesta International GMBH et autres".to_string(),
                },
                CeListName {
                    date: "2016-03-21".to_string(),
                    name: "Société NC Numericable".to_string(),
                },
            ]
        );
        assert_eq!(
            ce_list_names(
                "CE, Section, 11 décembre 2015, M. D. n° 395009  \
                 CE, Section, 11 décembre 2015, M. G. n°394990"
            )
            .iter()
            .map(|n| n.name.as_str())
            .collect::<Vec<_>>(),
            vec!["M. D.", "M. G."]
        );
    }

    #[test]
    fn le_nom_conjoint_se_coupe_au_compte_des_ancres() {
        assert_eq!(
            split_joint_name("Hardouin et Marie", 2),
            Some(vec!["Hardouin".into(), "Marie".into()])
        );
        assert_eq!(
            split_joint_name("Commune de Morsang-sur-Orge et Ville d'Aix-en-Provence", 2),
            Some(vec![
                "Commune de Morsang-sur-Orge".into(),
                "Ville d'Aix-en-Provence".into()
            ])
        );
        // La virgule passe avant le « et », qui vit ici dans un nom de société.
        assert_eq!(
            split_joint_name(
                "Société Yonne Funéraire, Société Intermarbres, Société Million et Marais",
                3
            ),
            Some(vec![
                "Société Yonne Funéraire".into(),
                "Société Intermarbres".into(),
                "Société Million et Marais".into()
            ])
        );
        assert_eq!(
            split_joint_name("Mme M'Rida, Mme Delannoy et M. Verzele, Mme Lazare", 3),
            Some(vec![
                "Mme M'Rida".into(),
                "Mme Delannoy et M. Verzele".into(),
                "Mme Lazare".into()
            ])
        );
    }

    #[test]
    fn un_nom_qui_ne_tombe_pas_sur_le_compte_ne_se_coupe_pas() {
        assert_eq!(split_joint_name("Société Million et Marais", 3), None);
        assert_eq!(split_joint_name("Danthony", 2), None);
        assert_eq!(split_joint_name("Hardouin et Marie", 1), None);
    }

    #[test]
    fn les_initiales_derriere_une_civilite_ne_nomment_rien() {
        for nom in [
            "M. A…B…",
            "Mme C. A.",
            "Mme C. et autres",
            "M. D.",
            "M. V.",
            "Époux V",
        ] {
            assert!(is_pseudonymised(nom), "nom {nom:?}");
        }
        for nom in [
            "M. Ternon",
            "M. Dahan",
            "Mme Duvignères",
            "Mme Lambert",
            "M. Bergoend c/ Société ERDF Annecy Léman",
            "Époux Bertin et Ministre de l’agriculture c/ consorts Grimouard et autres",
            "Dame veuve Trompier-Gravier",
            "G.I.S.T.I., C.F.D.T. et C.G.T.",
            "Association AC ! et autres",
            "Blanco",
        ] {
            assert!(!is_pseudonymised(nom), "nom {nom:?}");
        }
    }
}
