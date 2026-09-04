use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_core::annonces::{
    departement_from_cp, echeance_ts, parse_date_fr, parse_heure, AnnonceBrute, NatureMarche,
};

use super::{dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::{Result, SourceError};

const BASE: &str = "https://www.e-marchespublics.com";
const PER_PAGE: u64 = 10;
const SESSIONS: u64 = 12;
/// Borne dure de pages explorées ; au-delà du stock actif les cartes sont
/// vides et la tranche s'arrête d'elle-même.
const MAX_PAGES: u64 = 400;

struct Recherche {
    client: reqwest::Client,
    token: String,
}

impl Recherche {
    async fn ouvrir() -> Result<Self> {
        let client = super::client();
        let page = send_text(
            client.get(format!("{BASE}/appel-offre")),
            "dematis appel-offre",
        )
        .await?;
        static TOKEN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#"name="_token"[^>]*value="([^"]*)""#).unwrap());
        let token = TOKEN
            .captures(&page)
            .ok_or_else(|| SourceError::Invalid("dematis: jeton _token absent".into()))?[1]
            .to_string();
        Ok(Self { client, token })
    }

    async fn page(&self, n: u64) -> Result<(u64, Vec<AnnonceBrute>)> {
        let body = send_text(
            self.client
                .post(format!("{BASE}/notices/search/update"))
                .header("X-Requested-With", "XMLHttpRequest")
                .header("Referer", format!("{BASE}/appel-offre"))
                .form(&[("_token", self.token.as_str()), ("page", &n.to_string())]),
            &format!("dematis page {n}"),
        )
        .await?;
        let j: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| SourceError::Invalid(format!("dematis: JSON: {e}")))?;
        let vue = j.get("view").and_then(|v| v.as_str()).unwrap_or_default();
        Ok((
            j.get("nbResult").and_then(|v| v.as_u64()).unwrap_or(0),
            parse_view(vue),
        ))
    }
}

fn parse_view(vue: &str) -> Vec<AnnonceBrute> {
    static LIEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"href=\x22?(/appel-offre/([a-z0-9\-]+)/([a-z0-9\-]+)/([a-z0-9\-]+)/(\d+)/(\d+))",
        )
        .unwrap()
    });
    static TITRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)class="box-header-title".*?<div[^>]*>(.*?)</div>"#).unwrap()
    });
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Date limite de l'offre\s*:?\s*(\d{2}/\d{2}/\d{4})(?:\s*à\s*([\dhH:]+))?")
            .unwrap()
    });
    static PUB: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:Publi\w+|Mise en ligne)[^\d]{0,20}(\d{2}/\d{2}/\d{4})").unwrap()
    });
    // L'acheteur suit l'objet et précède « CP Commune » puis la nature.
    static ACHETEUR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?s)^\s*(.{2,140}?)\s*\b(\d{5})\b\s*(.{0,60}?)\s*(?:Travaux|Services|Fournitures|Autres|Date limite|$)",
        )
        .unwrap()
    });

    let mut out = Vec::new();
    for bloc in vue.split(r#"<div class="box">"#).skip(1) {
        let Some(lien) = LIEN.captures(bloc) else {
            continue;
        };
        let (url, aid) = (lien[1].to_string(), lien[5].to_string());
        let flat = txt(bloc);
        let objet = TITRE
            .captures(bloc)
            .map(|c| txt(&c[1]).chars().take(400).collect::<String>())
            .unwrap_or_default();
        let reste = flat.strip_prefix(objet.as_str()).unwrap_or(&flat);
        let ach = ACHETEUR.captures(reste);
        let cp = ach.as_ref().map(|c| c[2].to_string());
        let echeance = LIM.captures(&flat).and_then(|c| {
            let d = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                d,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        });
        out.push(AnnonceBrute {
            moteur: "dematis",
            source_uid: format!("dematis:{aid}"),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: ach.as_ref().map(|c| {
                c[1].trim()
                    .trim_end_matches("Accéder au site acheteur")
                    .trim()
                    .to_string()
            }),
            siren: None,
            objet,
            nature: NatureMarche::parse(&flat),
            cpv: None,
            departement: cp.as_deref().and_then(departement_from_cp),
            date_publication: PUB.captures(&flat).and_then(|c| parse_date_fr(&c[1])),
            echeance,
            url_avis: Some(format!("{BASE}{url}")),
            url_rc: Some(format!("{BASE}{url}#tabs-2")),
            url_dce: Some(format!("{BASE}{url}#tabs-3")),
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// Une tranche de pages, dans sa propre session (curseur serveur propre).
async fn tranche(lo: u64, hi: u64) -> Result<Vec<AnnonceBrute>> {
    let recherche = Recherche::ouvrir().await?;
    let mut rows = Vec::new();
    for p in lo..=hi {
        let (_, got) = recherche.page(p).await?;
        if got.is_empty() {
            break;
        }
        rows.extend(got);
    }
    Ok(rows)
}

/// Ré-énumère le stock actif. `annonce: None` : `nbResult` compte les clos.
pub async fn enumerate() -> Result<MoteurRun> {
    let tete = Recherche::ouvrir().await?;
    let (nb_result, premiere) = tete.page(1).await?;
    let npages = nb_result.div_ceil(PER_PAGE).clamp(1, MAX_PAGES);
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
        annonce: None,
        echecs: 0,
        hotes: Vec::new(),
        rows: premiere,
    };
    for res in resultats {
        match res {
            Ok(rows) => run.rows.extend(rows),
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(error = %e, "dematis : tranche en échec, stock conservé");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

/// L'en-tête de fiche porte « `Nature - Procédure` » derrière l'icône
/// `coll-ico far fa-file-alt`, seul repère commun aux deux formes d'avis servies
/// (formulaire eForms rendu en sections numérotées, ou corps libre de
/// l'acheteur). Le CPV ne vit que dans le corps et sous trois libellés selon la
/// forme ; le premier du document est celui du marché, les suivants ceux des
/// lots.
fn parse_detail(page: &str) -> DetailAnnonce {
    static PROCEDURE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"coll-ico far fa-file-alt"></i>\s*([^<]*)</p>"#).unwrap());
    static CPV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(?:Nomenclature principale\s*\(\s*cpv\s*\)|Classification CPV\s*:\s*Principale|CPV\s*-\s*Objet principal)\s*:\s*(\d{8}(?:-\d)?)",
        )
        .unwrap()
    });

    DetailAnnonce {
        cpv: CPV.captures(&txt(page)).map(|c| c[1].to_string()),
        procedure: PROCEDURE.captures(page).and_then(|c| {
            let entete = txt(&c[1]);
            entete
                .split_once(" - ")
                .map(|(_nature, p)| p.trim().to_string())
        }),
        acheteur: None,
    }
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let page = send_text(super::client().get(url_avis), "dematis détail").await?;
    Ok(parse_detail(&page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eforms_prend_la_nomenclature_principale() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Fourniture - Appel d&#039;Offres Ouvert
             </p>
             <div title="block02011003">
                <div><span class="label">Nomenclature principale</span><span class="text"> </span><span class="text">(</span><span class="value">cpv</span><span class="text">): </span><span class="value">38652100</span><span class="text"> </span><span class="dynamic-label">Appareils de projection</span></div>
             </div>
             <div title="block04010404">
                <div><span class="label">Nomenclature complémentaire</span><span class="text"> </span><span class="text">(</span><span class="value">cpv</span><span class="text">): </span><span class="value">32321200</span><span class="text"> </span><span class="dynamic-label">Equipement audiovisuel</span></div>
             </div>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                cpv: Some("38652100".into()),
                procedure: Some("Appel d'Offres Ouvert".into()),
                acheteur: None,
            }
        );
    }

    #[test]
    fn corps_libre_prend_le_cpv_du_marche_pas_celui_du_lot() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Travaux - Procédure Adaptée
             </p>
             <div class="html notice-html-content"><p><strong>Classification CPV :</strong></p><p><strong>Principale :</strong> 45454100 - Travaux de réfection</p><p><strong>Lot N° 1</strong> - Terrassement- Maçonnerie - CPV 45112500</p></div>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                cpv: Some("45454100".into()),
                procedure: Some("Procédure Adaptée".into()),
                acheteur: None,
            }
        );
    }

    #[test]
    fn formulaire_joue_cpv_objet_principal() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Travaux - Procédure Adaptée
             </p>
             <p><strong>Intitulé du marché :</strong> Restructuration intérieure de la mairie</p><p>CPV - Objet principal : 45421000.</p><p>Type de marché : Travaux</p>"#;
        assert_eq!(parse_detail(page).cpv.as_deref(), Some("45421000"));
    }

    #[test]
    fn avis_sans_cpv_rend_la_procedure_seule() {
        let page = r#"<p><i aria-hidden="true" class="coll-ico far fa-file-alt"></i>
             Autre - Procédure Adaptée
             </p>
             <div class="html notice-html-content"><p>2.Mode de passation du marché</p><p>Procédure adaptée.</p></div>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                cpv: None,
                procedure: Some("Procédure Adaptée".into()),
                acheteur: None,
            }
        );
    }
}
