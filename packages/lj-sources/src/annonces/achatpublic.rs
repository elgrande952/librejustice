use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_core::annonces::{echeance_ts, parse_date_fr, parse_heure, AnnonceBrute};

use super::{apres_balise, dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::Result;

const BASE: &str = "https://www.achatpublic.com";
const FORM: &str = "https://www.achatpublic.com/sdm/ent/gen/ent_recherche.do";
const ACTION: &str = "https://www.achatpublic.com/sdm/ent2/gen/rechercheCsl.action";
const PER_PAGE: u64 = 10;
const SESSIONS: u64 = 12;

async fn session() -> Result<reqwest::Client> {
    let client = super::client();
    send_text(
        client
            .get(FORM)
            .header("Accept-Language", "fr-FR,fr;q=0.9")
            .header("Referer", FORM),
        "achatpublic formulaire",
    )
    .await?;
    Ok(client)
}

/// Les 8 champs du formulaire, ni un de plus. `page` est le neuvième toléré.
async fn query(client: &reqwest::Client, page: Option<u64>) -> Result<(u64, String)> {
    let page = page.map(|p| p.to_string());
    let mut corps: Vec<(&str, &str)> = vec![
        ("searchCslBean.intitule", ""),
        ("search-location", ""),
        ("searchCslBean.departement", ""),
        ("searchCslBean.dlrpStart", ""),
        ("searchCslBean.dlrpEnd", ""),
        ("searchCslBean.procedure", "-1"),
        ("searchCslBean.codeCPV", ""),
        ("codeCPV", ""),
    ];
    if let Some(p) = page.as_deref() {
        corps.push(("searchCslBean.page", p));
    }
    let html = send_text(
        client.post(ACTION).header("Referer", FORM).form(&corps),
        "achatpublic recherche",
    )
    .await?;
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\d[\d\s]{0,7})\s*résultats?").unwrap());
    let total = TOTAL
        .captures(&txt(&html))
        .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
        .unwrap_or(0);
    Ok((total, html))
}

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    static PCSL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"PCSLID=([A-Za-z0-9_\-]+)").unwrap());
    static TITRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)class="sdmCardGeneric__title".*?<a[^>]*>(.*?)</a>"#).unwrap()
    });
    // « 12 septembre 2026 14 : 30 » — jour, mois en lettres, année, puis heure.
    static DATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(\d{1,2})\s+([A-Za-zÀ-ÿ]+)\.?\s+(\d{4})\s+(\d{1,2})\s*:\s*(\d{2})").unwrap()
    });
    static ORG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Organisme\s*:\s*(.*?)\s*(?:Référence|Allotissement|$)").unwrap()
    });
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Référence\s*:\s*(.*?)\s*(?:Allotissement|Organisme|$)").unwrap()
    });

    let mut out = Vec::new();
    for bloc in html
        .split(r#"<li id="li_consult_"#)
        .skip(1)
        .map(apres_balise)
    {
        let Some(pid) = PCSL.captures(bloc).map(|c| c[1].to_string()) else {
            continue;
        };
        let flat = txt(&bloc.chars().take(6_000).collect::<String>());
        let objet = TITRE
            .captures(bloc)
            .map(|c| txt(&c[1]))
            .unwrap_or_else(|| flat.split("Date limite").next().unwrap_or("").to_string())
            .chars()
            .take(400)
            .collect::<String>();
        let echeance = DATE.captures(&flat).and_then(|c| {
            let d = parse_date_fr(&format!("{} {} {}", &c[1], &c[2], &c[3]))?;
            Some(echeance_ts(d, parse_heure(&format!("{}h{}", &c[4], &c[5]))))
        });
        out.push(AnnonceBrute {
            moteur: "achatpublic",
            source_uid: format!("achatpublic:{pid}"),
            cles_jointure: Vec::new(),
            reference: REF
                .captures(&flat)
                .map(|c| c[1].chars().take(120).collect::<String>())
                .filter(|r| !r.is_empty()),
            acheteur: ORG
                .captures(&flat)
                .map(|c| c[1].chars().take(160).collect::<String>())
                .filter(|a| !a.is_empty()),
            siren: None,
            objet,
            nature: None,
            cpv: None,
            departement: None,
            date_publication: None,
            echeance,
            url_avis: Some(format!(
                "{BASE}/sdm/ent2/gen/ficheCsl.action?PCSLID={pid}&ongletActif=2"
            )),
            url_rc: None,
            url_dce: Some(format!(
                "{BASE}/sdm/ent2/gen/ficheCsl.action?PCSLID={pid}&ongletActif=3"
            )),
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// Une tranche de pages dans sa propre session.
async fn tranche(lo: u64, hi: u64) -> Result<Vec<AnnonceBrute>> {
    let client = session().await?;
    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    for p in lo..=hi {
        let (_, html) = query(&client, Some(p)).await?;
        let got = parse_page(&html);
        let avant = vus.len();
        for r in got {
            if vus.insert(r.source_uid.clone()) {
                rows.push(r);
            }
        }
        // La pagination sature au-delà du stock : la page rejoue la dernière.
        if vus.len() == avant {
            break;
        }
    }
    Ok(rows)
}

/// Ré-énumère le stock actif.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = session().await?;
    let (total, premiere) = query(&client, None).await?;
    let npages = total.div_ceil(PER_PAGE).max(1);
    let pas = npages.div_ceil(SESSIONS);
    let tranches: Vec<(u64, u64)> = (1..=npages)
        .step_by(pas as usize)
        .map(|i| (i, (i + pas - 1).min(npages)))
        .collect();
    let resultats: Vec<Result<Vec<AnnonceBrute>>> =
        futures::stream::iter(tranches.into_iter().map(|(lo, hi)| tranche(lo, hi)))
            .buffer_unordered(SESSIONS as usize)
            .collect()
            .await;
    let mut run = MoteurRun {
        annonce: Some(total),
        echecs: 0,
        hotes: Vec::new(),
        rows: parse_page(&premiere),
    };
    for res in resultats {
        match res {
            Ok(rows) => run.rows.extend(rows),
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(error = %e, "achatpublic : tranche en échec, stock conservé");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing. Les onglets sont rendus côté client — le serveur sert la fiche
/// entière quel que soit `ongletActif`, sans session préalable.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let html = send_text(super::client().get(url_avis), "achatpublic fiche").await?;
    Ok(parse_detail(&html))
}

/// Chaque champ est le `<span>` qui suit le `<span>` de son libellé, une seule
/// fois par fiche. Un `PCSLID` inconnu route vers le parcours d'inscription :
/// une page en 200 sans aucun des deux libellés, qui rend un détail vide.
fn parse_detail(html: &str) -> DetailAnnonce {
    // « Code CPV recherché : 45000000 - Travaux de construction. » — le code
    // entier, clé de contrôle comprise quand la fiche la porte. Le tiret du
    // libellé ne s'y colle pas : il est suivi d'une lettre, pas d'un chiffre.
    static CPV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Code CPV[^<]*</span>\s*<span[^>]*>\s*(\d{8}(?:-\d)?)").unwrap()
    });
    static PROC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Type de procédure[^<]*</span>\s*<span[^>]*>(.*?)</span>").unwrap()
    });
    DetailAnnonce {
        cpv: CPV.captures(html).map(|c| c[1].to_string()),
        procedure: PROC
            .captures(html)
            .map(|c| txt(&c[1]))
            .filter(|p| !p.is_empty()),
        acheteur: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FICHE: &str = r#"
                <li class="sdmCardConsult__listItem sdmCardConsult__listItem--halfInDesk is-deskTabOnly">
                    <span class="textLatoBold--14">Type de procédure&nbsp;: </span>


                    <span class="jqShave same-content-title">Procédure avec négociation</span>

                </li>
                <li class="sdmCardConsult__listItem sdmCardConsult__listItem--halfInDesk is-deskTabOnly">
                    <span class="textLatoBold--14">
                        Type de contrat&nbsp;: </span>
                        <span>Marché ordinaire</span>
                </li>
            <li class="sdmCardConsult__listItem">
                <span class="textLatoBold--14">Code CPV recherché&nbsp;: </span>
                <span class="textLatoReg--14">45000000 - Travaux de construction.</span>
            </li>"#;

    #[test]
    fn cpv_et_procedure_de_la_fiche() {
        let d = parse_detail(FICHE);
        assert_eq!(d.cpv.as_deref(), Some("45000000"));
        assert_eq!(d.procedure.as_deref(), Some("Procédure avec négociation"));
    }

    #[test]
    fn la_cle_de_controle_reste_dans_le_code() {
        let avec_cle = FICHE.replace("45000000 -", "45233140-2 -");
        assert_eq!(parse_detail(&avec_cle).cpv.as_deref(), Some("45233140-2"));
    }

    #[test]
    fn span_de_cpv_vide_ne_donne_pas_de_code() {
        let d = parse_detail(&FICHE.replace("45000000 - Travaux de construction.", ""));
        assert_eq!(d.cpv, None);
        assert_eq!(d.procedure.as_deref(), Some("Procédure avec négociation"));
    }

    #[test]
    fn fiche_sans_libelle_rend_un_detail_vide() {
        assert_eq!(
            parse_detail("<h1>Nouvel utilisateur ? Créez votre compte</h1>"),
            DetailAnnonce::default()
        );
    }
}
