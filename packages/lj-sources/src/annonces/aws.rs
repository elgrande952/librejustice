use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_core::annonces::{
    departement_from_cp, echeance_ts, normalise_departement, parse_date_fr, parse_heure,
    AnnonceBrute, NatureMarche,
};

use super::{dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::{Result, SourceError};

const BASE: &str = "https://www.marches-publics.info";
const NATURES: [(&str, NatureMarche); 3] = [
    ("T", NatureMarche::Travaux),
    ("S", NatureMarche::Services),
    ("F", NatureMarche::Fournitures),
];
const MAX_PAGES: u64 = 60;
const CELLULES_PARALLELES: usize = 12;
/// Procédures du formulaire (`<select name="IDP">`), seul axe qui morde en plus
/// de la nature et du département. Elles laissent ~2 % de la cellule dehors, des
/// avis sans procédure déclarée qu'aucune valeur ne rattrape.
const PROCEDURES: [&str; 9] = ["PA", "AOO", "AOE", "MN", "CO", "CR", "DC", "AU", "DSP"];

/// Ce que `lister` rend : la page servie, ou le refus au-delà du plafond.
enum Cellule {
    Liste {
        annonce: u64,
        rows: Vec<AnnonceBrute>,
    },
    Plafond {
        annonce: u64,
    },
}

async fn session() -> Result<reqwest::Client> {
    let client = super::client();
    send_text(
        client
            .get(format!("{BASE}/Annonces/rechercher"))
            .header("Referer", format!("{BASE}/Annonces/rechercher")),
        "aws rechercher",
    )
    .await?;
    Ok(client)
}

/// Départements offerts par le formulaire (`<select name="IDR">`).
async fn departements() -> Result<Vec<String>> {
    let page = send_text(
        super::client().get(format!("{BASE}/Annonces/rechercher")),
        "aws form",
    )
    .await?;
    static SELECT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<select[^>]*name="IDR".*?</select>"#).unwrap());
    static OPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<option[^>]*value="([^"]*)""#).unwrap());
    static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,3}|2A|2B)$").unwrap());
    let select = SELECT
        .find(&page)
        .ok_or_else(|| SourceError::Invalid("aws: select IDR absent du formulaire".into()))?;
    Ok(OPT
        .captures_iter(select.as_str())
        .map(|c| c[1].to_string())
        .filter(|v| CODE.is_match(v))
        .collect())
}

/// Une page d'une cellule : session neuve (contexte serveur global).
async fn page_query(dept: &str, nature: &str, idp: &str, page: u64) -> Result<Cellule> {
    let client = session().await?;
    let mut url = format!("{BASE}/Annonces/lister");
    if page > 1 {
        url = format!("{url}?pager_{}={page}", nature.to_lowercase());
    }
    let corps = send_text(
        client.post(url).form(&[
            ("IDE", "EC"),
            ("IDN", nature),
            ("IDR", dept),
            ("IDP", idp),
            ("dateParution", ""),
            ("Rechercher", "Rechercher"),
        ]),
        &format!("aws lister {dept}/{nature}/{idp} p{page}"),
    )
    .await?;
    Ok(lecture(&corps, dept, nature))
}

/// Le refus se reconnaît à son propre décompte, servi à la place de la liste.
fn lecture(page: &str, dept: &str, nature: &str) -> Cellule {
    static PLAFOND: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)Votre recherche trouve\s*(\d[\d\s]*)\s*résultats?").unwrap()
    });
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\d[\d\s]*)\s*Avis\s+correspond").unwrap());
    let plat = txt(page);
    let nombre = |re: &Regex| -> Option<u64> {
        re.captures(&plat)
            .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
    };
    match nombre(&PLAFOND) {
        Some(annonce) => Cellule::Plafond { annonce },
        None => Cellule::Liste {
            annonce: nombre(&TOTAL).unwrap_or(0),
            rows: parse_rows(page, dept, nature),
        },
    }
}

fn parse_rows(page: &str, dept: &str, nature: &str) -> Vec<AnnonceBrute> {
    // La balise ouvrante entière : un match qui démarre à `class=` laisse un `<div `
    // orphelin dans l'objet après le `replace`.
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<div[^>]*class="[^"]*ref-acheteur[^"]*"[^>]*>(.*?)</div>"#).unwrap()
    });
    static BOX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)id='titre_box'>(.*?)</div>\s*</div>").unwrap());
    static IDM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"IDM=(\d+)").unwrap());
    static AVIS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"href="(/Annonces/((?:MPI|DL)-pub-[^".]+)\.htm)""#).unwrap());
    static PUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Publié le\s*([\d/]{6,10})").unwrap());
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Date limite\s*:?\s*le\s*([\d/]{6,10})(?:\s*à\s*([\dh:]+))?").unwrap()
    });
    static CP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d{5})\)").unwrap());

    let nature = NATURES
        .iter()
        .find(|(code, _)| *code == nature)
        .map(|(_, n)| *n);
    let mut out = Vec::new();
    for block in page.split(r#"<h2 class="h2-avis">"#).skip(1) {
        let b: String = block.chars().take(9_000).collect();
        let acheteur_brut = txt(b.split("</h2>").next().unwrap_or(""));
        let (mut reference, mut objet) = (None, String::new());
        if let Some(cap) = BOX.captures(&b) {
            let mut inner = cap[1].to_string();
            if let Some(r) = REF.captures(&inner) {
                reference = Some(
                    txt(&r[1])
                        .trim_start_matches("[réf.")
                        .trim_end_matches(']')
                        .trim()
                        .to_string(),
                );
                inner = inner.replace(&r[0], " ");
            }
            objet = txt(&inner);
        }
        let idm = IDM.captures(&b).map(|c| c[1].to_string());
        let avis = AVIS.captures(&b);
        let source_uid = match (&idm, &avis) {
            (Some(idm), _) => format!("aws:{idm}"),
            (None, Some(a)) => format!("aws:pub:{}", &a[2]),
            (None, None) => continue,
        };
        let flat = txt(&b);
        let echeance = LIM.captures(&flat).and_then(|c| {
            let date = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                date,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        });
        let cp = CP.captures(&acheteur_brut).map(|c| c[1].to_string());
        out.push(AnnonceBrute {
            moteur: "aws",
            source_uid,
            cles_jointure: Vec::new(),
            reference: reference.filter(|r| !r.is_empty()),
            acheteur: Some(CP.replace(&acheteur_brut, "").trim().to_string())
                .filter(|a| !a.is_empty()),
            siren: None,
            objet,
            nature,
            cpv: None,
            departement: normalise_departement(dept)
                .or_else(|| cp.as_deref().and_then(departement_from_cp)),
            date_publication: PUB.captures(&flat).and_then(|c| parse_date_fr(&c[1])),
            echeance,
            url_avis: avis.map(|a| format!("{BASE}{}", &a[1])),
            url_rc: idm
                .as_ref()
                .filter(|_| b.contains("fuseaction=dce.dRC"))
                .map(|idm| format!("{BASE}/mpiaws/index.cfm?fuseaction=dce.dRC&IDM={idm}")),
            url_dce: idm
                .as_ref()
                .filter(|_| b.contains("fuseaction=dematEnt.login"))
                .map(|idm| {
                    format!("{BASE}/mpiaws/index.cfm?fuseaction=dematEnt.login&type=DCE&IDM={idm}")
                }),
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// Toutes les pages d'une cellule servie (département × nature × procédure).
async fn pages(dept: &str, nature: &str, idp: &str) -> Result<Cellule> {
    let (annonce, mut rows) = match page_query(dept, nature, idp, 1).await? {
        Cellule::Plafond { annonce } => return Ok(Cellule::Plafond { annonce }),
        Cellule::Liste { annonce, rows } => (annonce, rows),
    };
    let mut vus: std::collections::HashSet<String> =
        rows.iter().map(|r| r.source_uid.clone()).collect();
    let mut page = 1;
    while (vus.len() as u64) < annonce && page < MAX_PAGES && !rows.is_empty() {
        page += 1;
        let Cellule::Liste { rows: suite, .. } = page_query(dept, nature, idp, page).await? else {
            break;
        };
        let avant = vus.len();
        for r in suite {
            if vus.insert(r.source_uid.clone()) {
                rows.push(r);
            }
        }
        if vus.len() == avant {
            break;
        }
    }
    Ok(Cellule::Liste { annonce, rows })
}

/// Une cellule, redécoupée par procédure quand le moteur refuse de la lister.
/// Le déclaré retenu reste celui du refus, pour que le résidu se voie au ratio.
async fn enumerate_cell(dept: String, nature: &'static str) -> super::HoteRun {
    let plafond = match pages(&dept, nature, "X").await? {
        Cellule::Liste { annonce, rows } => return Ok((annonce, rows)),
        Cellule::Plafond { annonce } => annonce,
    };
    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    for idp in PROCEDURES {
        match pages(&dept, nature, idp).await? {
            Cellule::Liste { rows: lot, .. } => {
                for r in lot {
                    if vus.insert(r.source_uid.clone()) {
                        rows.push(r);
                    }
                }
            }
            Cellule::Plafond { annonce } => {
                return Err(SourceError::Invalid(format!(
                    "aws: {dept}/{nature}/{idp} pèse {annonce} avis, plus rien ne le redécoupe"
                )))
            }
        }
    }
    Ok((plafond, rows))
}

/// Ré-énumère le stock actif France entière, cellule par cellule.
pub async fn enumerate() -> Result<MoteurRun> {
    let departements = departements().await?;
    let cellules: Vec<(String, &'static str)> = departements
        .iter()
        .flat_map(|d| NATURES.iter().map(|(n, _)| (d.clone(), *n)))
        .collect();
    let resultats: Vec<super::HoteRun> =
        futures::stream::iter(cellules.into_iter().map(|(d, n)| enumerate_cell(d, n)))
            .buffer_unordered(CELLULES_PARALLELES)
            .collect()
            .await;
    let mut run = MoteurRun {
        annonce: Some(0),
        echecs: 0,
        hotes: Vec::new(),
        rows: Vec::new(),
    };
    for res in resultats {
        match res {
            Ok((annonce, rows)) => {
                // Le total par cellule double-compte les avis multi-départements ; le
                // dédoublonnage par source_uid ci-dessous ramène les lignes, pas le déclaré.
                run.annonce = run.annonce.map(|a| a + annonce);
                run.rows.extend(rows);
            }
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(error = %e, "aws : cellule en échec, stock conservé");
            }
        }
    }
    let avant = run.rows.len() as u64;
    run.rows = dedup_source_uid(run.rows);
    // Le déclaré compte des doublons multi-départements : on le rabat du même
    // facteur que les lignes pour garder un ratio de sanity check honnête.
    if avant > 0 {
        run.annonce = run.annonce.map(|a| a * run.rows.len() as u64 / avant);
    }
    Ok(run)
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing. Même carte pour les deux formes d'URL, `MPI-pub-…` comme `DL-pub-…` ;
/// pas de session, la page est servie en GET nu.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let page = send_text(
        super::client().get(url_avis),
        &format!("aws détail {url_avis}"),
    )
    .await?;
    Ok(parse_detail(&page))
}

/// La carte d'avis est une table d'étiquettes en gras : `Mode` porte le libellé
/// de procédure, `Code CPV principal` le code (8 chiffres ici, la clé de
/// contrôle `-N` restant admise). Un MAPA court se publie sans aucune ligne
/// CPV — la page est lue, `cpv` reste `None`.
fn parse_detail(page: &str) -> DetailAnnonce {
    static CPV: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)Code CPV principal\s*(\d{8}(?:-\d)?)").unwrap());
    // Le libellé se lit dans la cellule qui suit l'étiquette : à plat, rien ne
    // marque où il s'arrête (l'étiquette suivante le prolongerait).
    static MODE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<b>\s*Mode\s*</b>\s*</td>\s*<td[^>]*>(.*?)</td>").unwrap()
    });
    DetailAnnonce {
        cpv: CPV.captures(&txt(page)).map(|c| c[1].to_string()),
        procedure: MODE
            .captures(page)
            .map(|c| txt(&c[1]))
            .filter(|p| !p.is_empty()),
        acheteur: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_objet_ne_garde_rien_de_la_div_reference() {
        let page = r#"<h2 class="h2-avis">Ville de Colmar (68000)</h2>
<a href="index.cfm?fuseaction=marches.certif&IDM=1848353">certificat</a>
<div id='titre_box'><div class="ref-acheteur">[réf. 26AC01]</div><div>Montage et démontage des illuminations</div></div>
Publié le 07/08/2026 Date limite : le 09/09/2026 à 10h00"#;
        let rows = parse_rows(page, "68", "2");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].objet, "Montage et démontage des illuminations");
        assert_eq!(rows[0].reference.as_deref(), Some("26AC01"));
        assert_eq!(rows[0].source_uid, "aws:1848353");
    }

    /// Le refus compte lui aussi ses avis : le lire comme un total ferait passer
    /// la cellule pour servie et vide.
    #[test]
    fn le_refus_de_lister_ne_se_lit_pas_comme_un_total() {
        let refus = r#"<h2>Votre recherche trouve 115 résultats.
            Veuillez préciser votre recherche à l&#039;aide des critères suivants :</h2>"#;
        assert!(matches!(
            lecture(refus, "75", "S"),
            Cellule::Plafond { annonce: 115 }
        ));
        let servie = r#"<h2 id="nbResultAvis"><strong>
                6
            </strong>
            Avis correspondent à votre recherche</h2>"#;
        assert!(matches!(
            lecture(servie, "68", "T"),
            Cellule::Liste { annonce: 6, .. }
        ));
    }

    /// Extrait de `MPI-pub-20261911503.htm` : les deux lignes de la carte.
    #[test]
    fn la_carte_d_avis_donne_le_cpv_principal_et_le_mode() {
        let page = r#"<tr>
    <td class="AW_TableM_Bloc1_Clair" valign="top"><b>Mode</b></td>
    <td class="AW_TableM_Bloc1_Clair">
    Procédure adaptée ouverte
    </td>
</tr>
<tr>
    <td class="AW_TableM_Bloc1_Clair" valign="top"><b>Code CPV principal</b></td>
    <td class="AW_TableM_Bloc1_Clair"><strong>45233140</strong> - Travaux routiers</td>
</tr>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                cpv: Some("45233140".into()),
                procedure: Some("Procédure adaptée ouverte".into()),
                acheteur: None,
            }
        );
    }

    /// Un MAPA court se publie sans ligne CPV : page lue, pas d'échec.
    #[test]
    fn une_carte_sans_ligne_cpv_garde_la_procedure() {
        let page = r#"<tr>
    <td class="AW_TableM_Bloc1_Clair" valign="top"><b>Mode</b></td>
    <td class="AW_TableM_Bloc1_Clair">
    Procédure ouverte
    </td>
</tr>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                cpv: None,
                procedure: Some("Procédure ouverte".into()),
                acheteur: None,
            }
        );
    }
}
