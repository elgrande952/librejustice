//! SafeTender / Omnikles (`*.safetender.com` + domaines en marque blanche).
//!
//! SPA adossée à une API REST publique propre : `GET /tenderlink-rest/
//! tenderlink/session/` pose le cookie, puis `/supplier_tender/search/` rend
//! `{tenders: [...], size}`. Un portail par tenant — les hôtes viennent du
//! recensement `profil_acheteur`. La fiche d'un avis se lit sur la même API
//! (`/supplier_tender/tender_details/<id>`), sans session : c'est la recherche
//! qui a besoin du cookie, pas la fiche.

use futures::StreamExt;

use lj_core::annonces::{ts_from_epoch_ms, AnnonceBrute};

use super::{dedup_source_uid, send_text, DetailAnnonce, MoteurRun};
use crate::error::{Result, SourceError};

const LIMITE: u64 = 500;
const HOTES_PARALLELES: usize = 8;

async fn enumerate_host(host: &str) -> super::HoteRun {
    // Client par hôte : le cookie de session est propre au tenant.
    let client = super::client();
    let base = format!("https://{host}/tenderlink-rest/tenderlink");
    send_text(client.get(format!("{base}/session/")), "safetender session").await?;
    let mut rows = Vec::new();
    let mut first = 0;
    let size = loop {
        let body = send_text(
            client
                .get(format!("{base}/supplier_tender/search/"))
                .query(&[
                    ("first", first.to_string()),
                    ("is_asc", "true".into()),
                    ("limit", LIMITE.to_string()),
                    ("notice", "0".into()),
                    ("order_by", "deadline".into()),
                ]),
            &format!("safetender {host} search"),
        )
        .await?;
        let j: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| SourceError::Invalid(format!("safetender {host}: JSON: {e}")))?;
        let size = j.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        let tenders = j
            .get("tenders")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if tenders.is_empty() {
            break size;
        }
        first += tenders.len() as u64;
        rows.extend(tenders.iter().map(|t| row(t, host)));
        if first >= size {
            break size;
        }
    };
    Ok((size, rows))
}

/// Ré-énumère tous les portails recensés.
pub async fn enumerate(hosts: &[String]) -> Result<MoteurRun> {
    let resultats: Vec<(String, super::HoteRun)> = futures::stream::iter(hosts.iter().map(|h| {
        let h = h.clone();
        async move {
            let r = enumerate_host(&h).await;
            (h, r)
        }
    }))
    .buffer_unordered(HOTES_PARALLELES)
    .collect()
    .await;
    let mut run = MoteurRun {
        annonce: Some(0),
        echecs: 0,
        rows: Vec::new(),
    };
    for (host, res) in resultats {
        match res {
            Ok((size, rows)) => {
                run.annonce = run.annonce.map(|a| a + size);
                run.rows.extend(rows);
            }
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(host, error = %e, "safetender : portail en échec");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

fn row(t: &serde_json::Value, host: &str) -> AnnonceBrute {
    let texte = |k: &str| {
        t.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let id = t
        .get("tenderId")
        .and_then(|v| v.as_i64())
        .unwrap_or_default();
    let url = format!("https://{host}/#/tender/{id}");
    AnnonceBrute {
        moteur: "safetender",
        source_uid: format!("safetender:{host}:{id}"),
        cles_jointure: Vec::new(),
        reference: texte("reference"),
        acheteur: texte("organismName"),
        siren: None,
        objet: texte("object").unwrap_or_default(),
        nature: texte("tenderType").and_then(|n| lj_core::annonces::NatureMarche::parse(&n)),
        cpv: None,
        departement: None,
        date_publication: None,
        echeance: t
            .get("deadline")
            .and_then(|v| v.as_i64())
            .and_then(ts_from_epoch_ms),
        url_avis: Some(url.clone()),
        url_rc: None,
        url_dce: Some(url),
        url_depot: None,
        procedure: texte("model"),
    }
}

/// Hôte et identifiant portés par l'URL d'avis que [`row`] écrit
/// (`https://<hôte>/#/tender/<id>`) — le routage est en fragment, donc l'`id`
/// ne se lit que là.
fn hote_et_id(url_avis: &str) -> Option<(&str, &str)> {
    let (hote, id) = url_avis
        .strip_prefix("https://")?
        .split_once("/#/tender/")?;
    (!hote.is_empty() && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .then_some((hote, id))
}

/// Champ texte non vide d'une réponse de l'API, par pointeur JSON.
fn champ_json(corps: &str, pointeur: &str, quoi: &str) -> Result<Option<String>> {
    let j: serde_json::Value = serde_json::from_str(corps)
        .map_err(|e| SourceError::Invalid(format!("{quoi}: JSON: {e}")))?;
    Ok(j.pointer(pointeur)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string))
}

/// Fiche d'un avis. L'API côté fournisseur **ne publie aucun CPV** : ni la
/// fiche, ni ses lots, ni son modèle ne portent de code — la classification
/// n'existe que dans le module « données essentielles », qui ne couvre que les
/// marchés déjà attribués. Reste la procédure : la fiche donne le code du
/// modèle (`OPEN_MAPA`), déjà porté par le listing, et affiche son libellé
/// (« MAPA ouvert ») résolu par `/anonymous/tender_model/<code>` ; c'est ce
/// libellé qu'on retient.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let (hote, id) = hote_et_id(url_avis).ok_or_else(|| {
        SourceError::Invalid(format!("safetender: URL d'avis illisible « {url_avis} »"))
    })?;
    let client = super::client();
    let base = format!("https://{hote}/tenderlink-rest/tenderlink");
    let quoi = format!("safetender {hote} fiche {id}");
    let fiche = send_text(
        client.get(format!("{base}/supplier_tender/tender_details/{id}")),
        &quoi,
    )
    .await?;
    let Some(code) = champ_json(&fiche, "/tenders_details/model", &quoi)? else {
        return Ok(DetailAnnonce::default());
    };
    let quoi = format!("safetender {hote} modèle {code}");
    let modele = send_text(
        client.get(format!("{base}/anonymous/tender_model/{code}")),
        &quoi,
    )
    .await?;
    Ok(DetailAnnonce {
        cpv: None,
        procedure: champ_json(&modele, "/model/name_fr_FR", &quoi)?,
        acheteur: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_url_d_avis_porte_l_hote_et_l_id_dans_son_fragment() {
        assert_eq!(
            hote_et_id("https://asf-consultations.safetender.com/#/tender/1744"),
            Some(("asf-consultations.safetender.com", "1744"))
        );
        assert_eq!(
            hote_et_id("https://www.boamp.fr/pages/avis/?q=idweb:26-1"),
            None
        );
    }

    /// Réponses réelles (asf-consultations, avis 1744), réduites aux champs
    /// consultés.
    #[test]
    fn la_procedure_est_le_libelle_du_modele_pas_son_code() {
        let fiche = r#"{"tenders_details":{"tenderId":1744,"reference":"2026-08-06_PARA","object":"A7-A54-A8-Salon de Provence","tenderType":"SERVICE","typeProcedure":"OFFER","model":"OPEN_MAPA","tenderState":"PUBLISHED"},"status":{"message":"ok"}}"#;
        assert_eq!(
            champ_json(fiche, "/tenders_details/model", "t")
                .unwrap()
                .as_deref(),
            Some("OPEN_MAPA")
        );

        let modele = r#"{"model":{"id":19,"code":"OPEN_MAPA","acceptLot":true,"name_fr_FR":"MAPA ouvert","name_en_GB":"Open MAPA","name_es_ES":"Contrato con procedimiento adaptado abierto"},"status":{"message":"ok"}}"#;
        assert_eq!(
            champ_json(modele, "/model/name_fr_FR", "t")
                .unwrap()
                .as_deref(),
            Some("MAPA ouvert")
        );
    }

    #[test]
    fn une_fiche_sans_modele_ne_rend_rien() {
        let fiche =
            r#"{"tenders_details":{"tenderId":1744,"model":null},"status":{"message":"ok"}}"#;
        assert_eq!(
            champ_json(fiche, "/tenders_details/model", "t").unwrap(),
            None
        );
    }
}
