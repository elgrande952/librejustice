//! Backend du fond `bodacc` : proxy live sur l'API Opendatasoft DILA.
//!
//! Zéro ligne stockée : l'API sert déjà filtres, facettes et tri sur les
//! 50 millions d'annonces commerciales, donc on n'a rien à agréger — on traduit
//! l'état de filtres du pot en ODSQL et on projette le résultat dans le
//! `AnnonceHit` commun.
//!
//! Ce que ce fond n'a pas : d'échéance (une annonce constate, elle n'ouvre pas de
//! délai d'action) et de CPV. Ses axes déclarés sont donc la famille d'avis et le
//! département — on n'affiche jamais un filtre que la source ne sait pas exécuter.

use lj_dtos::{
    annonce_acteur_role, annonce_fond_label, AnnonceFacetAxis, AnnonceHit, AnnonceSearchRequest,
    FacetChoice, FacetTag,
};
use lj_vocab::ANNONCE_FOND_BODACC;
use serde::Deserialize;

use crate::error::{ApiError, Result};
use crate::registre::{get_json, OdsRecords};
use crate::state::AppState;

const RECORDS: &str =
    "https://bodacc-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/annonces-commerciales/records";
const FACETS: &str =
    "https://bodacc-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/annonces-commerciales/facets";

/// Page consultable de l'annonce sur le site officiel — le « où agir » du fond.
fn url_annonce(r: &Record) -> String {
    format!(
        "https://www.bodacc.fr/pages/annonce/detail-annonce/{}/{}/{}",
        r.publicationavis, r.parution, r.numeroannonce
    )
}

#[derive(Debug, Deserialize)]
struct Record {
    id: String,
    publicationavis: String,
    parution: String,
    numeroannonce: i64,
    dateparution: Option<String>,
    familleavis: Option<String>,
    familleavis_lib: Option<String>,
    numerodepartement: Option<String>,
    departement_nom_officiel: Option<String>,
    typeavis_lib: Option<String>,
    tribunal: Option<String>,
    commercant: Option<String>,
    ville: Option<String>,
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
/// passées entre guillemets après filtrage alphanumérique : c'est la frontière
/// de validation de ce backend, une injection ODSQL n'a pas d'autre porte.
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
    clauses.extend(ou("numerodepartement", &req.zone));
    clauses.extend(ou("familleavis", &req.famille));
    (!clauses.is_empty()).then(|| clauses.join(" AND "))
}

/// Neutralise ce qui pourrait fermer une chaîne ODSQL. Les valeurs légitimes du
/// fond (codes de département, familles d'avis, mots d'une requête) n'en
/// contiennent pas.
fn echappe(value: &str) -> String {
    value.replace(['"', '\\'], " ")
}

/// Une page du fond, plus son total. Le tri est toujours la parution
/// décroissante : le fond n'a pas d'échéance, et « les dernières annonces »
/// est la seule lecture qu'il offre.
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
        .map_err(|()| ApiError::BadGateway("bodacc".into()))?;
    let hits = payload.results.iter().map(hit_from).collect();
    Ok((hits, payload.total_count))
}

/// Champs projetés dans le hit commun — la même liste pour la recherche du pot
/// et pour la chronologie d'une entité.
const SELECT: &str = "id,publicationavis,parution,numeroannonce,dateparution,familleavis,\
                      familleavis_lib,numerodepartement,departement_nom_officiel,typeavis_lib,\
                      tribunal,commercant,ville";

/// La chronologie BODACC d'un SIREN pour la fiche entité : le même fond, la même
/// projection, une clause de rattachement au lieu d'un état de filtres. Le champ
/// `registre` est composite (`siren,greffe`) → `like`.
pub(crate) async fn pour_entite(
    state: &AppState,
    siren: &str,
    limit: usize,
) -> std::result::Result<(Vec<AnnonceHit>, i64), ()> {
    let where_clause = format!("registre like \"{}\"", echappe(siren));
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
    // Chaque axe se compte **sans son propre filtre** — deux appels, comme les
    // deux agrégats SQL du fond `marche`.
    let sans_famille = AnnonceSearchRequest {
        famille: Vec::new(),
        ..req.clone()
    };
    let sans_departement = AnnonceSearchRequest {
        zone: Vec::new(),
        ..req.clone()
    };
    let (familles, departements) = tokio::try_join!(
        facette(state, &sans_famille, "familleavis"),
        facette(state, &sans_departement, "numerodepartement"),
    )?;
    Ok(vec![
        AnnonceFacetAxis {
            key: "famille".to_string(),
            label: "Famille d'avis".to_string(),
            choices: familles,
        },
        AnnonceFacetAxis {
            key: "zone".to_string(),
            label: "Département".to_string(),
            choices: departements,
        },
    ])
}

/// Un axe compté par l'API. Les facettes ont leur propre endpoint (`/facets`,
/// pas `/records`) et leur propre syntaxe : `facet(name="champ", limit=N)`, où
/// `limit` plafonne à 100 côté DILA. La clause `where` courante s'y applique
/// telle quelle.
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
        .map_err(|()| ApiError::BadGateway("bodacc".into()))?;
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
                    hint: None,
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Le libellé d'un jeton de facette. L'API ne rend que le code technique sur
/// `familleavis` ; le département se lit par la table du référentiel, la même
/// que le fond `marche` — un code de département est un code de département.
fn libelle(champ: &str, valeur: &str) -> String {
    match champ {
        "numerodepartement" => lj_norm::referential_labels::departement_label(valeur)
            .map(|n| format!("{valeur} — {n}"))
            .unwrap_or_else(|| valeur.to_string()),
        _ => famille_label(valeur).to_string(),
    }
}

/// Familles d'avis du BODACC, libellées comme le site officiel les nomme.
fn famille_label(code: &str) -> &str {
    match code {
        "creation" => "Créations",
        "immatriculation" => "Immatriculations",
        "modification" => "Modifications",
        "radiation" => "Radiations",
        "vente" => "Ventes et cessions",
        "dpc" => "Dépôts des comptes",
        "collective" => "Procédures collectives",
        "retablissement_personnel" => "Rétablissements personnels",
        "avis" => "Avis divers",
        autre => autre,
    }
}

fn hit_from(r: &Record) -> AnnonceHit {
    // L'acteur d'une annonce BODACC est le commerçant ; à défaut, la juridiction
    // qui la publie situe encore la ligne.
    let acteur = r
        .commercant
        .clone()
        .or_else(|| r.tribunal.clone())
        .unwrap_or_default();
    let titre = match (&r.typeavis_lib, &r.familleavis_lib, &r.ville) {
        (Some(t), Some(f), Some(v)) => format!("{f} — {t} ({v})"),
        (_, Some(f), Some(v)) => format!("{f} ({v})"),
        (_, Some(f), None) => f.clone(),
        _ => "Annonce BODACC".to_string(),
    };
    AnnonceHit {
        uid: format!("bodacc:{}", r.id),
        fond: ANNONCE_FOND_BODACC.to_string(),
        fond_label: annonce_fond_label(ANNONCE_FOND_BODACC)
            .unwrap_or_default()
            .to_string(),
        titre,
        description: None,
        acteur,
        acteur_role: annonce_acteur_role(ANNONCE_FOND_BODACC).to_string(),
        zones: r
            .numerodepartement
            .iter()
            .map(|c| FacetTag {
                label: r
                    .departement_nom_officiel
                    .clone()
                    .unwrap_or_else(|| c.clone()),
                key: c.clone(),
            })
            .collect(),
        famille: r.familleavis.as_ref().map(|f| FacetTag {
            key: f.clone(),
            label: famille_label(f).to_string(),
        }),
        cpv: None,
        cpv_deduit: false,
        metiers: Vec::new(),
        metiers_deduits: false,
        date_publication: r.dateparution.clone().unwrap_or_default(),
        // Une annonce constate un fait déjà survenu : aucune échéance, donc
        // aucun `VEVENT` dans un agenda et aucune fenêtre « ferme sous N jours ».
        echeance: None,
        closed: false,
        attribue: false,
        url_action: url_annonce(r),
        reference: Some(r.id.clone()),
        procedure: r.typeavis_lib.clone(),
        first_seen_at: None,
        canaux: vec!["bodacc".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_clause_odsql_combine_les_axes() {
        let req = AnnonceSearchRequest {
            q: Some("boulangerie".into()),
            zone: vec!["31".into(), "75".into()],
            famille: vec!["collective".into()],
            ..Default::default()
        };
        assert_eq!(
            where_clause(&req).unwrap(),
            "search(*, \"boulangerie\") AND (numerodepartement=\"31\" OR numerodepartement=\"75\") \
             AND (familleavis=\"collective\")"
        );
    }

    #[test]
    fn une_valeur_ne_peut_pas_fermer_la_chaine() {
        let req = AnnonceSearchRequest {
            q: Some("a\" OR familleavis=\"dpc".into()),
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
    fn projette_un_enregistrement_dans_le_hit_commun() {
        let r = Record {
            id: "A20260147510".into(),
            publicationavis: "A".into(),
            parution: "20260147".into(),
            numeroannonce: 510,
            dateparution: Some("2026-08-05".into()),
            familleavis: Some("creation".into()),
            familleavis_lib: Some("Créations".into()),
            numerodepartement: Some("31".into()),
            departement_nom_officiel: Some("Haute-Garonne".into()),
            typeavis_lib: Some("Avis initial".into()),
            tribunal: Some("Greffe du Tribunal de Commerce de Toulouse".into()),
            commercant: Some("TULU, Stéphanie".into()),
            ville: Some("Toulouse".into()),
        };
        let hit = hit_from(&r);
        assert_eq!(hit.uid, "bodacc:A20260147510");
        assert_eq!(hit.acteur, "TULU, Stéphanie");
        assert_eq!(hit.acteur_role, "Entreprise");
        assert_eq!(hit.titre, "Créations — Avis initial (Toulouse)");
        assert_eq!(
            hit.zones.iter().map(|z| z.key.as_str()).collect::<Vec<_>>(),
            ["31"]
        );
        assert!(hit.echeance.is_none() && hit.cpv.is_none());
        assert_eq!(
            hit.url_action,
            "https://www.bodacc.fr/pages/annonce/detail-annonce/A/20260147/510"
        );
    }
}
