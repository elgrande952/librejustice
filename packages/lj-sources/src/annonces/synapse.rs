//! Synapse (`*.synapse-entreprises.com`, `synapse-consult.com`).
//!
//! Les portails Synapse ne sont qu'une façade : la dématérialisation est
//! opérée par **EU-Supply**, qui expose la liste publique sur
//! `eu.eu-supply.com/ctm/supplier/publictenders?B=SYNAPSE`, paginée par
//! `SearchFilter.PagingInfo.PageNumber`. Le branding `SYNAPSE` couvre tous les
//! acheteurs français de l'éditeur ; on s'arrête dès qu'une page n'apporte
//! plus rien de neuf (le serveur rejoue la dernière au-delà du stock).

use std::sync::LazyLock;

use regex::Regex;

use lj_core::annonces::{echeance_ts, parse_date_fr, parse_heure, AnnonceBrute};

use super::{apres_balise, dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use crate::error::{Result, SourceError};

const BASE: &str = "https://eu.eu-supply.com/ctm/supplier/publictenders";
const BRANDING: &str = "SYNAPSE";
const MAX_PAGES: u64 = 60;

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    static LIEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)href="(/app/rfq/rwlentrance_s\.asp\?PID=(\d+)[^"]*)""#).unwrap()
    });
    static TITRE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">([^<]{8,300})</a>").unwrap());
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{2}/\d{2}/\d{4})(?:\s+(\d{2}:\d{2}))?").unwrap());

    let mut out = Vec::new();
    for bloc in html.split("<tr").skip(1).map(apres_balise) {
        let Some(lien) = LIEN.captures(bloc) else {
            continue;
        };
        let (href, pid) = (lien[1].replace("&amp;", "&"), lien[2].to_string());
        let flat = txt(bloc);
        let cellules: Vec<&str> = flat
            .split('|')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect();
        let dates: Vec<_> = DATE.captures_iter(&flat).collect();
        let titre = TITRE
            .captures(bloc)
            .map(|c| txt(&c[1]))
            .or_else(|| cellules.first().map(|c| c.to_string()))
            .unwrap_or_default();
        // Première date = publication, dernière = limite (une seule date ⇒
        // publication seule, l'échéance reste inconnue).
        let echeance = (dates.len() > 1)
            .then(|| dates.last())
            .flatten()
            .and_then(|c| {
                let d = parse_date_fr(&c[1])?;
                Some(echeance_ts(
                    d,
                    c.get(2).and_then(|h| parse_heure(h.as_str())),
                ))
            });
        out.push(AnnonceBrute {
            moteur: "synapse",
            source_uid: format!("synapse:{pid}"),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: cellules.get(1).map(|c| c.to_string()),
            siren: None,
            objet: titre.chars().take(300).collect(),
            nature: None,
            cpv: None,
            departement: None,
            date_publication: dates.first().and_then(|c| parse_date_fr(&c[1])),
            echeance,
            url_avis: Some(format!("https://eu.eu-supply.com{href}")),
            url_rc: None,
            url_dce: Some(format!("https://eu.eu-supply.com{href}")),
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// Ré-énumère la liste publique. Pas de total déclaré exploitable.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    for p in 1..=MAX_PAGES {
        let html = send_text(
            client.get(BASE).query(&[
                ("B", BRANDING.to_string()),
                ("SearchFilter.PagingInfo.PageNumber", p.to_string()),
            ]),
            &format!("synapse page {p}"),
        )
        .await?;
        let avant = vus.len();
        for r in parse_page(&html) {
            if vus.insert(r.source_uid.clone()) {
                rows.push(r);
            }
        }
        if vus.len() == avant {
            break;
        }
    }
    Ok(MoteurRun {
        annonce: None,
        echecs: 0,
        rows: dedup_source_uid(rows),
    })
}

/// Lit la fiche CTM. Le gabarit ne rend des codes CPV que quand l'acheteur en a
/// saisi : un libellé `ctm-content-label` suivi d'un `<p>` d'un `<span>` par
/// code, le principal en tête. Chaque fiche est servie dans la langue de son
/// acheteur — « Codes CPV » chez l'un, « CPV codes » chez l'autre au sein d'une
/// même session —, d'où l'ancrage sur le seul acronyme. La procédure n'est pas
/// de la fiche : le moteur la garde en colonne de listing (« Marché privé »,
/// « Appel d'offres ouvert », « Procédure Adaptée. »).
///
/// Sans `tenderInfoSection`, la page n'est pas une fiche : un PID retiré du
/// public rend un 200 « Access denied », qui est un échec de lecture et non un
/// avis muet.
fn parse_detail(html: &str) -> Result<DetailAnnonce> {
    static BLOC_CPV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)class="ctm-content-label"[^>]*>[^<]*CPV[^<]*</div>\s*<p>(.*?)</p>"#)
            .unwrap()
    });
    static CODE_CPV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{8}(?:-\d)?\b").unwrap());

    if !html.contains("tenderInfoSection") {
        return Err(SourceError::Invalid(
            "synapse détail: fiche non servie".into(),
        ));
    }
    Ok(DetailAnnonce {
        cpv: BLOC_CPV
            .captures(html)
            .and_then(|c| CODE_CPV.find(&txt(&c[1])).map(|m| m.as_str().to_string())),
        procedure: None,
        acheteur: None,
    })
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing.
///
/// `rwlentrance_s.asp` rebondit en 302 vers `/ctm/Supplier/PublicPurchase/<PID>`
/// et la chaîne ne se referme qu'une fois posé le cookie d'affinité du
/// répartiteur — sans jar elle boucle jusqu'à épuisement du compteur de
/// redirections, celui de [`super::client`] suffit.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let html = send_text(super::client().get(url_avis), "synapse détail").await?;
    parse_detail(&html)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le bloc CPV d'une fiche renseignée, dans les deux langues où le moteur
    /// le sert : le code retenu est le premier listé, complet (clé de contrôle
    /// comprise), et la fiche ne dit rien de la procédure.
    #[test]
    fn le_premier_code_cpv_est_lu_quelle_que_soit_la_langue_de_la_fiche() {
        let fr = r#"<div id="tenderInfoSection" class="ctm-section-box">
            <div class="ctm-content-label">Type de contrat:</div>
            <p>Fournitures</p>
            <div class="ctm-content-label">Date limite de réponse :</div>
            <p>30/09/2026 11:00:00</p>
            <div class="ctm-content-label">Codes CPV:</div>
            <p>
                    <span class="">34144400-2&nbsp;&nbsp;Véhicules d'entretien routier</span><br />
                    <span class="">39713410-0&nbsp;&nbsp;Machines de nettoyage des sols</span><br />
                    <span class="">34144000-8&nbsp;&nbsp;Véhicules automobiles à usage spécifique</span><br />
            </p>"#;
        assert_eq!(
            parse_detail(fr).unwrap(),
            DetailAnnonce {
                cpv: Some("34144400-2".into()),
                procedure: None,
                acheteur: None,
            }
        );

        let en = r#"<div id="tenderInfoSection" class="ctm-section-box">
            <div class="ctm-content-label">CPV codes:</div>
            <p>
                    <span class="">34144431-8&nbsp;&nbsp;Suction-sweeper vehicles</span><br />
                    <span class="">34144430-1&nbsp;&nbsp;Road-sweeping vehicles</span><br />
                    <span class="">34921100-0&nbsp;&nbsp;Road sweepers</span><br />
            </p>"#;
        assert_eq!(parse_detail(en).unwrap().cpv.as_deref(), Some("34144431-8"));
    }

    /// La fiche que servent les acheteurs qui n'ont rempli aucun CPV — le cas
    /// courant du branding : lue, muette, la ligne sort de la file.
    #[test]
    fn la_fiche_sans_bloc_cpv_est_muette() {
        let html = r#"<div id="tenderInfoSection" class="ctm-section-box">
            <div class="ctm-content-label">Description détaillée:</div>
            <p class="ctm-multi-line"></p>
            <div class="ctm-content-label">Date limite de réponse :</div>
            <p>02/10/2026 12:00:00</p>
            <div class="ctm-content-label">Donneur d'ordre:</div>
            <p><br />Cisn coop&#233;rative<br />44613<br />SAINT-NAZAIRE CEDEX<br />France</p>"#;
        assert_eq!(parse_detail(html).unwrap(), DetailAnnonce::default());
    }

    /// Un PID retiré du public rend un 200 sans fiche : c'est une lecture en
    /// échec, la ligne reste en file — pas un avis sans CPV.
    #[test]
    fn l_acces_refuse_n_est_pas_une_fiche_muette() {
        let html = r#"<div class="ctm-header-pane"><h1></h1></div>
            <div class="container-default">
                <div class="header"><h2>Information</h2></div>
                <div class="content"><div class="error-img">
                    <i class="icon-lock icon-2x yellow-icon"></i>
                    <div class="error-content"><h3 class="error">Access denied</h3><br />
                    You&nbsp;are&nbsp;not&nbsp;authorized&nbsp;to&nbsp;view the requested&nbsp;page!
                </div></div></div>
            </div>"#;
        assert!(parse_detail(html).is_err());
    }
}
