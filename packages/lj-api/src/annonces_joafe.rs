//! Backend du fond `joafe` : le Journal officiel des associations, proxy live
//! sur l'API Opendatasoft DILA.
//!
//! Zéro ligne stockée : l'API sert filtres, facettes et tri sur les parutions,
//! donc on traduit l'état de filtres du pot en ODSQL et on projette le résultat
//! dans le `AnnonceHit` commun — la recherche du pot et la chronologie d'un RNA
//! (fiche entité) parlent au même dataset. Le PDF officiel hébergé par la DILA
//! est le « où agir » de la ligne : un lien direct, jamais proxifié.
//!
//! Ce fond n'a ni échéance (une parution constate un fait déjà survenu) ni CPV.
//! Ses axes déclarés sont le type d'avis (« Création », « Modification »,
//! « Dissolution » — la valeur est son propre libellé) et le département.

use lj_dtos::{
    annonce_acteur_role, annonce_fond_label, AnnonceFacetAxis, AnnonceHit, AnnonceSearchRequest,
    FacetChoice, FacetTag, ANNONCE_FOND_JOAFE,
};
use serde::Deserialize;

use crate::error::{ApiError, Result};
use crate::registre::{get_json, OdsRecords};
use crate::state::AppState;

const RECORDS: &str =
    "https://journal-officiel-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/jo_associations/records";
const FACETS: &str =
    "https://journal-officiel-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/jo_associations/facets";

/// Champs projetés dans le hit commun — la même liste pour la recherche du pot
/// et pour la chronologie d'une entité.
const SELECT: &str = "id,dateparution,typeavis,titre,departement_code,url_pdf";

#[derive(Debug, Deserialize)]
struct Record {
    id: Option<String>,
    dateparution: Option<String>,
    typeavis: Option<String>,
    titre: Option<String>,
    departement_code: Option<String>,
    url_pdf: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FacetPayload {
    facets: Vec<FacetGroup>,
}

#[derive(Debug, Deserialize)]
struct FacetGroup {
    name: String,
    facets: Vec<FacetItem>,
}

#[derive(Debug, Deserialize)]
struct FacetItem {
    name: String,
    value: String,
    count: i64,
}

/// Clause `where` ODSQL pour l'état de filtres courant. Les valeurs sont
/// passées entre guillemets après neutralisation : c'est la frontière de
/// validation de ce backend, une injection ODSQL n'a pas d'autre porte.
fn where_clause(req: &AnnonceSearchRequest) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    if let Some(q) = req.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        clauses.push(format!("search(*, \"{}\")", echappe(q)));
    }
    let ou = |champ: &str, valeurs: &[String]| {
        let liste: Vec<String> = valeurs
            .iter()
            .map(|v| format!("{champ}=\"{}\"", echappe(v)))
            .collect();
        (!liste.is_empty()).then(|| format!("({})", liste.join(" OR ")))
    };
    clauses.extend(ou("departement_code", &req.departement));
    clauses.extend(ou("typeavis", &req.famille));
    (!clauses.is_empty()).then(|| clauses.join(" AND "))
}

/// Neutralise ce qui pourrait fermer une chaîne ODSQL. Les valeurs légitimes du
/// fond (codes de département, types d'avis, mots d'une requête) n'en
/// contiennent pas.
fn echappe(value: &str) -> String {
    value.replace(['"', '\\'], " ")
}

/// Une page du fond, plus son total. Le tri est toujours la parution
/// décroissante : le fond n'a pas d'échéance.
pub async fn page(
    state: &AppState,
    req: &AnnonceSearchRequest,
    avec_hits: bool,
) -> Result<(Vec<AnnonceHit>, i64)> {
    let limit = if avec_hits { req.limit.min(100) } else { 1 };
    let offset = if avec_hits { req.offset } else { 0 };
    let (limit, offset) = (limit.to_string(), offset.to_string());
    let mut query: Vec<(&str, &str)> = vec![
        ("order_by", "dateparution desc"),
        ("select", SELECT),
        ("limit", limit.as_str()),
        ("offset", offset.as_str()),
    ];
    let ou = where_clause(req);
    if let Some(w) = ou.as_deref() {
        query.push(("where", w));
    }
    let payload: OdsRecords<Record> = get_json(state, RECORDS, &query)
        .await
        .map_err(|()| ApiError::BadGateway("joafe".into()))?;
    let hits = payload.results.iter().map(hit_from).collect();
    Ok((hits, payload.total_count))
}

/// La chronologie JOAFE d'un RNA. L'identifiant est déjà filtré en
/// alphanumérique par l'appelant (frontière de validation du volet registre).
pub async fn pour_entite(
    state: &AppState,
    rna: &str,
    limit: usize,
) -> std::result::Result<(Vec<AnnonceHit>, i64), ()> {
    let where_clause = format!("numero_rna=\"{rna}\"");
    let limit = limit.to_string();
    let payload: OdsRecords<Record> = get_json(
        state,
        RECORDS,
        &[
            ("where", where_clause.as_str()),
            ("order_by", "dateparution desc"),
            ("select", SELECT),
            ("limit", limit.as_str()),
        ],
    )
    .await?;
    Ok((
        payload.results.iter().map(hit_from).collect(),
        payload.total_count,
    ))
}

/// Les axes que ce backend sait exécuter, comptés par l'API elle-même.
pub async fn axes(state: &AppState, req: &AnnonceSearchRequest) -> Result<Vec<AnnonceFacetAxis>> {
    // Chaque axe se compte sans son propre filtre : deux appels.
    let sans_famille = AnnonceSearchRequest {
        famille: Vec::new(),
        ..req.clone()
    };
    let sans_departement = AnnonceSearchRequest {
        departement: Vec::new(),
        ..req.clone()
    };
    let (types, departements) = tokio::try_join!(
        facette(state, &sans_famille, "typeavis"),
        facette(state, &sans_departement, "departement_code"),
    )?;
    Ok(vec![
        AnnonceFacetAxis {
            key: "famille".to_string(),
            label: "Type d'avis".to_string(),
            choices: types,
        },
        AnnonceFacetAxis {
            key: "departement".to_string(),
            label: "Département".to_string(),
            choices: departements,
        },
    ])
}

/// Un axe compté par l'API — endpoint `/facets`, syntaxe
/// `facet(name=…, limit=100)`, la clause `where` courante s'appliquant telle
/// quelle.
async fn facette(
    state: &AppState,
    req: &AnnonceSearchRequest,
    champ: &str,
) -> Result<Vec<FacetChoice>> {
    let facet = format!("facet(name=\"{champ}\", limit=100)");
    let mut query: Vec<(&str, &str)> = vec![("facet", facet.as_str())];
    let ou = where_clause(req);
    if let Some(w) = ou.as_deref() {
        query.push(("where", w));
    }
    let payload: FacetPayload = get_json(state, FACETS, &query)
        .await
        .map_err(|()| ApiError::BadGateway("joafe".into()))?;
    Ok(payload
        .facets
        .into_iter()
        .find(|g| g.name == champ)
        .map(|g| {
            g.facets
                .into_iter()
                .map(|f| FacetChoice {
                    label: libelle(champ, &f.name),
                    value: f.value,
                    count: f.count,
                    parent: None,
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Le libellé d'un jeton de facette : un type d'avis est son propre libellé
/// (« Création ») ; le département se lit par la table du référentiel, la même
/// que les autres fonds.
fn libelle(champ: &str, valeur: &str) -> String {
    match champ {
        "departement_code" => lj_core::referential_labels::departement_label(valeur)
            .map(|n| format!("{valeur} — {n}"))
            .unwrap_or_else(|| valeur.to_string()),
        _ => valeur.to_string(),
    }
}

fn hit_from(r: &Record) -> AnnonceHit {
    let type_avis = r.typeavis.as_deref().unwrap_or("Annonce");
    let titre = match &r.titre {
        Some(nom) => format!("{type_avis} — {nom}"),
        None => type_avis.to_string(),
    };
    AnnonceHit {
        uid: format!("joafe:{}", r.id.as_deref().unwrap_or_default()),
        fond: ANNONCE_FOND_JOAFE.to_string(),
        fond_label: annonce_fond_label(ANNONCE_FOND_JOAFE)
            .unwrap_or_default()
            .to_string(),
        titre,
        acteur: r.titre.clone().unwrap_or_default(),
        acteur_role: annonce_acteur_role(ANNONCE_FOND_JOAFE).to_string(),
        departement: r.departement_code.clone(),
        departement_label: r
            .departement_code
            .as_deref()
            .and_then(lj_core::referential_labels::departement_label)
            .map(str::to_string),
        famille: r.typeavis.as_ref().map(|t| FacetTag {
            key: t.clone(),
            label: t.clone(),
        }),
        cpv: None,
        date_publication: r.dateparution.clone().unwrap_or_default(),
        echeance: None,
        closed: false,
        attribue: false,
        url_action: r.url_pdf.clone().unwrap_or_default(),
        reference: r.id.clone(),
        procedure: r.typeavis.clone(),
        first_seen_at: None,
        canaux: vec![ANNONCE_FOND_JOAFE.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // L'égalité ODSQL est sensible à la casse ; les types d'avis circulent en
    // casse mixte.
    #[test]
    fn la_clause_odsql_combine_les_axes() {
        let req = AnnonceSearchRequest {
            q: Some("pêche".into()),
            departement: vec!["31".into()],
            famille: vec!["Création".into(), "Dissolution".into()],
            ..Default::default()
        };
        assert_eq!(
            where_clause(&req).unwrap(),
            "search(*, \"pêche\") AND (departement_code=\"31\") \
             AND (typeavis=\"Création\" OR typeavis=\"Dissolution\")"
        );
    }

    #[test]
    fn une_valeur_ne_peut_pas_fermer_la_chaine() {
        let req = AnnonceSearchRequest {
            q: Some("a\" OR typeavis=\"Dissolution".into()),
            ..Default::default()
        };
        let clause = where_clause(&req).unwrap();
        assert_eq!(clause.matches('"').count(), 2, "{clause}");
    }

    #[test]
    fn sans_filtre_aucune_clause() {
        assert!(where_clause(&AnnonceSearchRequest::default()).is_none());
    }

    #[test]
    fn projette_une_parution_dans_le_hit_commun() {
        let r = Record {
            id: Some("associations_1234".into()),
            dateparution: Some("2026-03-14".into()),
            typeavis: Some("Création".into()),
            titre: Some("LES AMIS DU CANAL".into()),
            departement_code: Some("31".into()),
            url_pdf: Some(
                "https://www.journal-officiel.gouv.fr/document/associations_b_1234".into(),
            ),
        };
        let hit = hit_from(&r);
        assert_eq!(hit.uid, "joafe:associations_1234");
        assert_eq!(hit.titre, "Création — LES AMIS DU CANAL");
        assert_eq!(hit.acteur_role, "Association");
        assert_eq!(hit.departement_label.as_deref(), Some("Haute-Garonne"));
        assert_eq!(
            hit.famille.as_ref().map(|f| f.label.as_str()),
            Some("Création")
        );
        assert!(hit.echeance.is_none());
    }
}
