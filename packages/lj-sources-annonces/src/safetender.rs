use futures::StreamExt;

use lj_annonces::zones::{departement_cite, zones_departements, Zone};
use lj_annonces::{
    departement_from_cp, ts_from_epoch_ms, AnnonceBrute, ContenuAnnonce, LotAnnonce,
};

use super::{dedup_source_uid, send_text, DetailAnnonce, MoteurRun};
use lj_sources::error::{Result, SourceError};

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
        rows.extend(tenders.iter().filter_map(|t| row(t, host)));
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
        hotes: Vec::new(),
        rows: Vec::new(),
    };
    for (host, res) in resultats {
        let joignable = res.is_ok();
        match res {
            Ok((size, rows)) => {
                run.annonce = run.annonce.map(|a| a + size);
                run.rows.extend(rows);
            }
            Err(e) => tracing::warn!(host, error = %e, "safetender: portal failed"),
        }
        run.hotes.push(super::HoteVerdict { host, joignable });
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

/// Identifiant d'un avis. Les portails passés au socle à UUID gardent leur
/// entier d'origine en `tenderOldId` : le retenir en priorité tient le
/// `source_uid` et l'URL stables des deux côtés de la migration.
fn tender_id(t: &serde_json::Value) -> Option<String> {
    ["tenderOldId", "tenderId"]
        .into_iter()
        .find_map(|k| match t.get(k) {
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
}

fn row(t: &serde_json::Value, host: &str) -> Option<AnnonceBrute> {
    let texte = |k: &str| {
        t.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let id = tender_id(t)?;
    let url = format!("https://{host}{SALLE}/tender/{id}");
    Some(AnnonceBrute {
        contenu: ContenuAnnonce::default(),
        descripteurs: Vec::new(),
        moteur: "safetender",
        source_uid: format!("safetender:{host}:{id}"),
        cles_jointure: Vec::new(),
        reference: texte("reference"),
        acheteur: texte("organismName"),
        siren: None,
        objet: texte("object").unwrap_or_default(),
        nature: texte("tenderType").and_then(|n| lj_annonces::NatureMarche::parse(&n)),
        cpv: None,
        zones: Vec::new(),
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
    })
}

/// The SPA routes a consultation under the marketplace root only: the same
/// route without it bounces to the landing page, on the four portals sampled.
const SALLE: &str = "/#/home";

fn hote_et_id(url_avis: &str) -> Option<(&str, &str)> {
    let (hote, route) = url_avis.strip_prefix("https://")?.split_once(SALLE)?;
    let id = route.strip_prefix("/tender/")?;
    (!hote.is_empty()
        && !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    .then_some((hote, id))
}

/// Champ texte non vide d'une réponse de l'API, par pointeur JSON.
fn champ_json(corps: &str, pointeur: &str, quoi: &str) -> Result<Option<String>> {
    let j: serde_json::Value = serde_json::from_str(corps)
        .map_err(|e| SourceError::Invalid(format!("{quoi}: JSON: {e}")))?;
    Ok(champ(&j, pointeur))
}

fn champ(j: &serde_json::Value, pointeur: &str) -> Option<String> {
    j.pointer(pointeur)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Place of execution, which the listing never carries. The buyer's own address
/// is the fallback: the sampled portals fill it when `placeOfExecution` is null.
fn zones_fiche(j: &serde_json::Value) -> Vec<Zone> {
    const CP: [&str; 2] = [
        "/tenders_details/placeOfExecution/postalCode",
        "/tenders_details/postCode",
    ];
    const NOM: [&str; 2] = [
        "/tenders_details/placeOfExecution/department",
        "/tenders_details/departement",
    ];
    let dep = CP
        .iter()
        .find_map(|p| champ(j, p).and_then(|cp| departement_from_cp(&cp)))
        .or_else(|| {
            NOM.iter()
                .find_map(|p| champ(j, p).and_then(|n| departement_cite(&n).map(str::to_string)))
        });
    zones_departements(dep)
}

/// The allotment, which the fiche announces without carrying: a route of its
/// own names the lots, and names them only.
async fn lots(client: &wreq::Client, base: &str, id: &str, quoi: &str) -> Result<Vec<LotAnnonce>> {
    let corps = send_text(client.get(format!("{base}/supplier_tender/{id}/lot")), quoi).await?;
    let j: serde_json::Value = serde_json::from_str(&corps)
        .map_err(|e| SourceError::Invalid(format!("{quoi}: JSON: {e}")))?;
    Ok(j.get("lots")
        .and_then(|v| v.as_array())
        .map(|lots| {
            lots.iter()
                .filter_map(|l| champ(l, "/name"))
                .map(|nom| LotAnnonce {
                    nom: Some(nom),
                    description: None,
                    cpv: None,
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Six consultations in ten are not allotted, and the fiche says so: the round
/// trip to the lots is spared on those.
fn alloti(j: &serde_json::Value) -> bool {
    j.pointer("/tenders_details/advancedOptionsTender/activateLot")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Fiche d'un avis. The supplier API publishes **no CPV**: neither the fiche,
/// nor its lots, nor its model carries a code, the classification living in the
/// awarded-contracts module alone, which covers no open market. It publishes no
/// description either, `object` being its only prose and already the title.
/// What it does carry is the procedure, as the code of a model (`OPEN_MAPA`)
/// whose label (« MAPA ouvert ») `/anonymous/tender_model/<code>` resolves, and
/// the allotment, behind a route the fiche does not mention.
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
    let j: serde_json::Value = serde_json::from_str(&fiche)
        .map_err(|e| SourceError::Invalid(format!("{quoi}: JSON: {e}")))?;
    let zones = zones_fiche(&j);
    let contenu = ContenuAnnonce {
        lots: match alloti(&j) {
            true => lots(&client, &base, id, &format!("safetender {hote} lots {id}")).await?,
            false => Vec::new(),
        },
        ..ContenuAnnonce::default()
    };
    let Some(code) = champ(&j, "/tenders_details/model") else {
        return Ok(DetailAnnonce {
            contenu,
            zones,
            ..DetailAnnonce::default()
        });
    };
    let quoi = format!("safetender {hote} modèle {code}");
    let modele = send_text(
        client.get(format!("{base}/anonymous/tender_model/{code}")),
        &quoi,
    )
    .await?;
    Ok(DetailAnnonce {
        contenu,
        zones,
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
        let hote = "asf-consultations.safetender.com";
        assert_eq!(
            hote_et_id(&format!("https://{hote}{SALLE}/tender/1744")),
            Some((hote, "1744"))
        );
        let uuid = "2b0ff31b-56e9-4df5-a950-88b0faca4ff4";
        assert_eq!(
            hote_et_id(&format!("https://{hote}{SALLE}/tender/{uuid}")),
            Some((hote, uuid))
        );
        assert_eq!(hote_et_id(&format!("https://{hote}/#/tender/1744")), None);
        assert_eq!(
            hote_et_id("https://www.boamp.fr/pages/avis/?q=idweb:26-1"),
            None
        );
    }

    /// Lignes réelles des deux socles : `marchespublics-aude` sur l'entier,
    /// `asf-consultations` migré, qui porte l'UUID et garde l'entier.
    #[test]
    fn l_entier_d_origine_prime_l_uuid_du_socle_migre() {
        let ancien = serde_json::json!({"tenderId": 35873, "reference": "26MA-SMMAR"});
        assert_eq!(tender_id(&ancien).as_deref(), Some("35873"));

        let migre = serde_json::json!({
            "tenderId": "2b0ff31b-56e9-4df5-a950-88b0faca4ff4",
            "tenderOldId": 1572,
            "reference": "222-2026-DMA-3007"
        });
        assert_eq!(tender_id(&migre).as_deref(), Some("1572"));

        let sans_entier = serde_json::json!({"tenderId": "2b0ff31b-56e9-4df5-a950-88b0faca4ff4"});
        assert_eq!(
            tender_id(&sans_entier).as_deref(),
            Some("2b0ff31b-56e9-4df5-a950-88b0faca4ff4")
        );

        assert_eq!(tender_id(&serde_json::json!({"reference": "x"})), None);
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

    /// Fiche réelle (marchespublics-aude, avis 36097), réduite aux champs de lieu.
    #[test]
    fn le_lieu_d_execution_donne_la_zone_et_l_adresse_le_relaie() {
        let fiche = serde_json::json!({"tenders_details": {
            "postCode": "11400", "departement": "AUDE", "city": "CASTELNAUDARY ",
            "placeOfExecution": {"postalCode": "11400", "department": "AUDE", "country": "France"}
        }});
        assert_eq!(lj_annonces::zones::codes(&zones_fiche(&fiche)), ["11"]);

        let sans_bloc = serde_json::json!({"tenders_details": {
            "postCode": "20260", "departement": "HAUTE-CORSE", "placeOfExecution": null
        }});
        assert_eq!(lj_annonces::zones::codes(&zones_fiche(&sans_bloc)), ["2B"]);

        let nom_seul = serde_json::json!({"tenders_details": {
            "postCode": null, "placeOfExecution": {"postalCode": "", "department": "La Réunion"}
        }});
        assert_eq!(lj_annonces::zones::codes(&zones_fiche(&nom_seul)), ["974"]);

        let muette = serde_json::json!({"tenders_details": {"reference": "x"}});
        assert!(zones_fiche(&muette).is_empty());
    }

    /// Real fiche (Carcassonne Agglo, avis 36079) and the answer of its lots
    /// route, cut to two: a lot is a name, and nothing else.
    #[test]
    fn la_fiche_dit_si_elle_est_allotie_et_la_route_des_lots_les_nomme() {
        let alloties = serde_json::json!({"tenders_details": {
            "advancedOptionsTender": {"oldId": 36090, "activateLot": true, "activateDume": false}
        }});
        assert!(alloti(&alloties));

        let seule = serde_json::json!({"tenders_details": {
            "advancedOptionsTender": {"oldId": 1806, "activateLot": false}
        }});
        assert!(!alloti(&seule));
        assert!(!alloti(
            &serde_json::json!({"tenders_details": {"reference": "x"}})
        ));

        let corps = r#"{"lots":[
            {"id":"b44c50e8-1e29-4233-bf97-abae1412094b","oldId":49211,"name":"Lot n°1 Démolition / Gros-oeuvre ","rank":1,"lotState":"SUCCESSFUL"},
            {"id":"23250e3f-0a1c-4f2a-9d64-7c5a9a0f1b22","oldId":49212,"name":"Lot n°2 Charpente","rank":2,"lotState":"SUCCESSFUL"}],
            "size":2,"status":{"message":"ok"}}"#;
        let j: serde_json::Value = serde_json::from_str(corps).expect("JSON de lots");
        let noms: Vec<_> = j["lots"]
            .as_array()
            .expect("tableau de lots")
            .iter()
            .filter_map(|l| champ(l, "/name"))
            .collect();
        assert_eq!(
            noms,
            ["Lot n°1 Démolition / Gros-oeuvre", "Lot n°2 Charpente"]
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
