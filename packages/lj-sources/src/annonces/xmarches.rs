use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_core::annonces::{echeance_ts, parse_date_fr, parse_heure, AnnonceBrute};

use super::{apres_balise, dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::Result;

const BASE: &str = "https://www.xmarches.fr/entreprise";
const MAX_PAGES: u64 = 80;
const FICHES_PARALLELES: usize = 10;
/// Relectures de la liste au plus. Quatre suffisent à ramasser les 296
/// consultations du 27/08 (271, puis 288, 295, 296) ; la sixième est la marge.
const PASSES_MAX: u32 = 6;

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    static KEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"detailConsultation\.php\?key=(\d+)").unwrap());
    static TITRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)href="\./detailConsultation\.php\?key=\d+"[^>]*>(.*?)</a>"#).unwrap()
    });
    let mut out = Vec::new();
    for bloc in html.split("<tr").skip(1).map(apres_balise) {
        let Some(k) = KEY.captures(bloc).map(|c| c[1].to_string()) else {
            continue;
        };
        let objet = TITRE
            .captures(bloc)
            .map(|c| txt(&c[1]))
            .unwrap_or_else(|| txt(bloc))
            .chars()
            .take(400)
            .collect::<String>();
        out.push(AnnonceBrute {
            moteur: "xmarches",
            source_uid: format!("xmarches:{k}"),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: None,
            siren: None,
            objet,
            nature: None,
            cpv: None,
            departement: None,
            date_publication: None,
            echeance: None,
            url_avis: Some(format!("{BASE}/detailConsultation.php?key={k}")),
            url_rc: None,
            url_dce: Some(format!("{BASE}/detailConsultation.php?key={k}")),
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// Acheteur d'une fiche : le libellé placé juste avant « Vérifier les
/// pré-requis ». Il faut couper après la dernière date ou heure qui le
/// précède : la fiche peut porter une phrase de report (« Ancienne date
/// limite : 19/10/2028 à 21:00:00 ») entre l'échéance et l'acheteur, et un
/// motif ancré sur la première date avale tout.
fn acheteur(flat: &str) -> Option<String> {
    static DATE_OU_HEURE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{1,2}[hH:]\d{2}(?::\d{2})?|\d{2}/\d{2}/\d{2,4}").unwrap());
    let fin = flat.find("Vérifier les pré-requis")?;
    let avant = &flat[..fin];
    let debut = DATE_OU_HEURE
        .find_iter(avant)
        .last()
        .map(|m| m.end())
        .unwrap_or(0);
    let nom = avant[debut..].trim().trim_start_matches('.').trim();
    (nom.len() >= 4 && nom.chars().count() <= 160).then(|| nom.to_string())
}

/// Complète une ligne par sa fiche : référence, acheteur, dates.
async fn completer_par_fiche(client: &reqwest::Client, mut row: AnnonceBrute) -> AnnonceBrute {
    static REF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Réf\s*:\s*(.*?)\s*Date de publication").unwrap());
    static PUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Date de publication\s*:\s*(\d{2}/\d{2}/\d{4})").unwrap());
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"Date limite de remise des plis\s*:\s*(\d{2}/\d{2}/\d{4})(?:\s*à\s*([\dhH:]+))?",
        )
        .unwrap()
    });
    let url = row.url_avis.clone().unwrap_or_default();
    let html = match send_text(client.get(&url), "xmarches fiche").await {
        Ok(h) => h,
        // Fiche en échec : la ligne de liste reste servie sans dates plutôt que
        // d'être perdue.
        Err(e) => {
            tracing::debug!(url = %url, error = %e, "xmarches : fiche en échec");
            return row;
        }
    };
    let flat = txt(&html);
    row.reference = REF
        .captures(&flat)
        .map(|c| c[1].chars().take(120).collect::<String>())
        .filter(|r| !r.is_empty());
    row.acheteur = acheteur(&flat);
    row.date_publication = PUB.captures(&flat).and_then(|c| parse_date_fr(&c[1]));
    row.echeance = LIM.captures(&flat).and_then(|c| {
        let d = parse_date_fr(&c[1])?;
        Some(echeance_ts(
            d,
            c.get(2).and_then(|h| parse_heure(h.as_str())),
        ))
    });
    row
}

/// Ré-énumère la liste puis complète chaque ligne par sa fiche.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    send_text(client.get(format!("{BASE}/")), "xmarches accueil").await?;
    let premiere = send_text(
        client.get(format!("{BASE}/liste-des-consultations,1.html")),
        "xmarches liste 1",
    )
    .await?;
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\d[\d\s]{0,6})\s*résultats?").unwrap());
    let total: u64 = TOTAL
        .captures(&txt(&premiere))
        .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
        .unwrap_or(0);

    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    let mut absorber = |lot: Vec<AnnonceBrute>, rows: &mut Vec<AnnonceBrute>| {
        for r in lot {
            if vus.insert(r.source_uid.clone()) {
                rows.push(r);
            }
        }
    };
    absorber(parse_page(&premiere), &mut rows);
    for _ in 0..PASSES_MAX {
        let avant = rows.len();
        for p in 1..=MAX_PAGES {
            let html = send_text(
                client.get(format!("{BASE}/liste-des-consultations,{p}.html")),
                &format!("xmarches liste {p}"),
            )
            .await?;
            // Fin de liste : la page est vide. Une page qui n'apprend rien ne
            // l'annonce pas, le réordonnancement en produit au milieu du stock.
            let lot = parse_page(&html);
            if lot.is_empty() {
                break;
            }
            absorber(lot, &mut rows);
        }
        if rows.len() == avant || total > 0 && rows.len() as u64 >= total {
            break;
        }
    }

    let rows: Vec<AnnonceBrute> = futures::stream::iter(rows.into_iter().map(|r| {
        let client = &client;
        async move { completer_par_fiche(client, r).await }
    }))
    .buffer_unordered(FICHES_PARALLELES)
    .collect()
    .await;

    Ok(MoteurRun {
        annonce: (total > 0).then_some(total),
        echecs: 0,
        hotes: Vec::new(),
        rows: dedup_source_uid(rows),
    })
}

/// Ce qu'une fiche apprend : la procédure, sous le libellé « Mode de
/// passation » — texte nu derrière le `</strong>` du libellé (« Procédure
/// adaptée ouverte », « Appel d'offres ouvert », « Avis d'information »).
///
/// Jamais de CPV : le moteur n'en publie nulle part. Ni la fiche, ni l'avis PDF
/// qu'elle offre au téléchargement (`redirect.php?…&telequoi=AVIS`), qui décrit
/// la nature du marché en clair (« Nature : Travaux », « Détail : Travaux
/// d'infrastructures ») et rien de codé.
fn champs_fiche(html: &str) -> DetailAnnonce {
    static PASSATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Mode de passation\s*:\s*</strong>([^<]+)").unwrap());
    DetailAnnonce {
        cpv: None,
        procedure: PASSATION
            .captures(html)
            .map(|c| txt(&c[1]))
            .filter(|p| !p.is_empty()),
        acheteur: None,
    }
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing. La fiche se lit sans session, sans passage préalable par l'accueil.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let client = super::client();
    let html = send_text(client.get(url_avis), "xmarches détail").await?;
    Ok(champs_fiche(&html))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acheteur_apres_la_derniere_date() {
        let flat = "Détail de la consultation Consultation de Test Réf : TEST \
             Date de publication : 01/10/2018 Date limite de remise des plis : \
             14/10/2027 à 14h52 La date limite a fait l'objet d'une modification \
             le 28/12/23 à 09:15. Ancienne date limite : 19/10/2028 à 21:00:00 \
             Commune de TestVille Vérifier les pré-requis à la réponse électronique";
        assert_eq!(acheteur(flat).as_deref(), Some("Commune de TestVille"));
    }

    #[test]
    fn acheteur_juste_apres_l_echeance() {
        let flat = "Date limite de remise des plis : 30/09/2026 à 17h00 \
             Ville de Reims Vérifier les pré-requis";
        assert_eq!(acheteur(flat).as_deref(), Some("Ville de Reims"));
    }

    #[test]
    fn sans_repere_pas_d_acheteur() {
        assert_eq!(acheteur("Se connecter Mot de passe oublié ?"), None);
    }

    #[test]
    fn procedure_dune_fiche_sans_cpv() {
        let html = r#"<li><strong>Objet de la consultation : </strong>
                        La présente procédure a pour objet l'Assistance à Maîtrise
                        d'Ouvrage.</li>
                    <li>
                        <strong>Mode de passation : </strong>Appel d'offres ouvert                    </li>
                    <li>
                        <strong>Type de marché : </strong>Services                    </li>"#;
        assert_eq!(
            champs_fiche(html),
            DetailAnnonce {
                cpv: None,
                procedure: Some("Appel d'offres ouvert".to_string()),
                acheteur: None,
            }
        );
    }

    #[test]
    fn fiche_muette_rend_un_bloc_vide() {
        assert_eq!(
            champs_fiche("Se connecter Mot de passe oublié ?"),
            DetailAnnonce::default()
        );
    }

    #[test]
    fn residu_de_balise_ecarte() {
        let html = r#"<tr class="ligne" id="l1"><td>
             <a href="./detailConsultation.php?key=42">Travaux de voirie</a></td></tr>"#;
        let rows = parse_page(html);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].objet, "Travaux de voirie");
        assert_eq!(rows[0].source_uid, "xmarches:42");
    }
}
