//! BOAMP Opendatasoft — la colonne structurée du fond marchés.
//!
//! `boamp-datadila.opendatasoft.com`, dataset `boamp`, API Explore v2.1. Stock
//! actif = `datefindiffusion >= now()`. L'offset ODS est plafonné à 10 000 et le
//! stock actif le dépasse — des avis SAD restent diffusés des années : on fenêtre
//! par `dateparution` en coupant récursivement toute fenêtre trop pleine. Le blob
//! `donnees` (eForms/UBL, FNSimple ou legacy, 10-50 Ko) donne le SIRET acheteur et
//! les liens profonds vers les profils d'acheteurs — parsé puis **jeté** : seuls
//! les champs normalisés survivent.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use regex::Regex;
use serde::Deserialize;

use lj_core::annonces::{
    cles_jointure_depuis_urls, echeance_ts, embellir_titre, nettoyer_acheteur,
    normalise_departement, parse_attribution, parse_date_fr, parse_ts_iso, AnnonceBrute,
    NatureMarche, ResultatMarche,
};

use super::{dedup_source_uid, send_text, MoteurRun};
use crate::error::{Result, SourceError};

const RECORDS: &str =
    "https://boamp-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/boamp/records";
/// L'export JSONL ignore le plafond d'offset de `records` : une fenêtre entière
/// en une requête, une ligne JSON par avis. C'est ce qui rend l'historique
/// atteignable sans pagination récursive.
const EXPORTS: &str =
    "https://boamp-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/boamp/exports/jsonl";
const SELECT: &str = "idweb,nomacheteur,objet,dateparution,datelimitereponse,type_marche,\
     code_departement,url_avis,etat,annonce_lie,donnees";
/// Marge sous le plafond d'offset ODS (10 000).
const FENETRE_MAX: u64 = 9_000;
const PAGE: u64 = 100;
/// Politesse : l'upstream `*-datadila` sert aussi le proxy live des fiches
/// entité.
const PAUSE: Duration = Duration::from_millis(150);

#[derive(Deserialize)]
struct Ods {
    total_count: u64,
    results: Vec<serde_json::Value>,
}

fn filtre(nature: &str, lo: Date, hi: Date) -> String {
    format!(
        "datefindiffusion >= now() and nature_libelle = \"{nature}\" \
         and dateparution >= date'{lo}' and dateparution < date'{hi}'"
    )
}

async fn fetch(
    client: &reqwest::Client,
    where_: &str,
    select: &str,
    limit: u64,
    offset: u64,
) -> Result<Ods> {
    let body = send_text(
        client.get(RECORDS).query(&[
            ("where", where_),
            ("select", select),
            ("limit", &limit.to_string()),
            ("offset", &offset.to_string()),
            ("order_by", "dateparution desc,idweb"),
        ]),
        "boamp records",
    )
    .await?;
    serde_json::from_str(&body)
        .map_err(|e| SourceError::Invalid(format!("boamp: réponse ODS illisible: {e}")))
}

/// Parcourt tous les avis actifs d'une nature, fenêtre par fenêtre de
/// `dateparution`. Rend le total déclaré, cumulé sur les fenêtres.
async fn parcourir<F>(nature: &str, select: &str, mut f: F) -> Result<u64>
where
    F: FnMut(&serde_json::Value) -> Result<()>,
{
    let client = super::client();
    let demain = Timestamp::now()
        .to_zoned(TimeZone::get("Europe/Paris").expect("tzdb embarquée"))
        .date()
        .tomorrow()
        .expect("lendemain calculable");
    // Plancher 2010 : le dataset ne porte pas d'avis encore diffusé plus ancien.
    let mut pile = vec![(Date::new(2010, 1, 1).expect("date fixe"), demain)];
    let mut annonce = 0;
    while let Some((lo, hi)) = pile.pop() {
        let where_ = filtre(nature, lo, hi);
        let total = fetch(&client, &where_, "idweb", 1, 0).await?.total_count;
        if total == 0 {
            continue;
        }
        if total > FENETRE_MAX {
            let jours = lo
                .until(hi)
                .map_err(|e| SourceError::Invalid(e.to_string()))?
                .get_days();
            let mitan = lo
                .checked_add(jiff::Span::new().days(jours / 2))
                .map_err(|e| SourceError::Invalid(e.to_string()))?;
            if mitan <= lo || mitan >= hi {
                return Err(SourceError::Invalid(format!(
                    "boamp: {total} avis sur le seul jour {lo} — au-delà du plafond d'offset ODS"
                )));
            }
            pile.push((lo, mitan));
            pile.push((mitan, hi));
            continue;
        }
        annonce += total;
        let mut offset = 0;
        while offset < total {
            let page = fetch(&client, &where_, select, PAGE, offset).await?;
            if page.results.is_empty() {
                break;
            }
            offset += page.results.len() as u64;
            for rec in &page.results {
                f(rec)?;
            }
            tokio::time::sleep(PAUSE).await;
        }
    }
    Ok(annonce)
}

/// Ré-énumère les avis de marché actifs France entière. `plateformes` : les
/// hôtes recensés et classés, validateurs du dépôt de dernier ressort.
pub async fn enumerate(plateformes: &HashSet<String>) -> Result<MoteurRun> {
    let mut rows = Vec::new();
    let annonce = parcourir("Avis de marché", SELECT, |rec| {
        rows.push(row(rec, plateformes)?);
        Ok(())
    })
    .await?;
    Ok(MoteurRun {
        annonce: Some(annonce),
        echecs: 0,
        rows: dedup_source_uid(rows),
    })
}

const SELECT_RESULTATS: &str = "idweb,nomacheteur,objet,dateparution,type_marche,\
     code_departement,url_avis,annonce_lie,donnees";

/// Ré-énumère les avis de résultat de marché actifs — le fond `attribution`. Un
/// seul canal, une ligne par idweb, pas de réconciliation.
pub async fn resultats() -> Result<Vec<ResultatMarche>> {
    let mut rows = Vec::new();
    parcourir("Résultat de marché", SELECT_RESULTATS, |rec| {
        rows.push(row_resultat(rec)?);
        Ok(())
    })
    .await?;
    Ok(rows)
}

/// Le filtre de l'historique : la borne `datefindiffusion >= now()` du stock
/// actif tombe — c'est précisément ce qu'elle exclut qu'on vient chercher.
fn filtre_historique(nature: &str, lo: Date, hi: Date) -> String {
    format!(
        "nature_libelle = \"{nature}\" \
         and dateparution >= date'{lo}' and dateparution < date'{hi}'"
    )
}

/// Une fenêtre d'historique, en enregistrements bruts.
async fn exporter(
    nature: &str,
    select: &str,
    lo: Date,
    hi: Date,
) -> Result<Vec<serde_json::Value>> {
    let client = super::client();
    let body = send_text(
        client.get(EXPORTS).query(&[
            ("where", filtre_historique(nature, lo, hi).as_str()),
            ("select", select),
            ("limit", "-1"),
        ]),
        "boamp export",
    )
    .await?;
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l)
                .map_err(|e| SourceError::Invalid(format!("boamp export: ligne illisible: {e}")))
        })
        .collect()
}

/// Les avis de marché parus dans une fenêtre, diffusés ou non. Première moitié
/// du backfill : les consultations doivent être en stock avant que leurs résultats
/// ne les cherchent.
pub async fn avis_historiques(
    lo: Date,
    hi: Date,
    plateformes: &HashSet<String>,
) -> Result<Vec<AnnonceBrute>> {
    let recs = exporter("Avis de marché", SELECT, lo, hi).await?;
    let rows = recs
        .iter()
        .map(|rec| row(rec, plateformes))
        .collect::<Result<Vec<_>>>()?;
    Ok(dedup_source_uid(rows))
}

/// Les avis de résultat parus dans une fenêtre. Seconde moitié du backfill.
pub async fn resultats_historiques(lo: Date, hi: Date) -> Result<Vec<ResultatMarche>> {
    let recs = exporter("Résultat de marché", SELECT_RESULTATS, lo, hi).await?;
    recs.iter().map(row_resultat).collect()
}

fn row_resultat(rec: &serde_json::Value) -> Result<ResultatMarche> {
    let texte = |k: &str| {
        rec.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let premier_tableau = |k: &str| {
        rec.get(k)
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let idweb = texte("idweb")
        .ok_or_else(|| SourceError::Invalid(format!("boamp: résultat sans idweb: {rec}")))?;
    let date_parution = texte("dateparution")
        .and_then(|s| parse_date_fr(&s))
        .ok_or_else(|| SourceError::Invalid(format!("boamp: résultat {idweb} sans parution")))?;
    let donnees = texte("donnees").unwrap_or_default();
    // Blob illisible = résultat sans attribution : la ligne vaut par son événement.
    let attribution = serde_json::from_str::<serde_json::Value>(&donnees)
        .map(|v| parse_attribution(&v))
        .unwrap_or_default();
    Ok(ResultatMarche {
        parent_idweb: premier_tableau("annonce_lie").or_else(|| attribution.avis_initial.clone()),
        // La forme s'applique ici, à la construction — l'aval écrit tel quel.
        acheteur: texte("nomacheteur").map(|a| nettoyer_acheteur(&a)),
        siren_acheteur: siren(&donnees),
        objet: embellir_titre(&texte("objet").unwrap_or_default()),
        nature: premier_tableau("type_marche").and_then(|n| NatureMarche::parse(&n)),
        cpv: cpv(&donnees),
        departement: premier_tableau("code_departement").and_then(|d| normalise_departement(&d)),
        date_parution,
        url_avis: texte("url_avis")
            .unwrap_or_else(|| format!("https://www.boamp.fr/pages/avis/?q=idweb:{idweb}")),
        attribution,
        idweb,
    })
}

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s"'<>\\]+"#).unwrap());
static HOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://([^/\s]+)").unwrap());

fn urls(donnees: &str) -> Vec<String> {
    URL.find_iter(donnees)
        .map(|m| {
            m.as_str()
                .replace("&amp;", "&")
                .trim_end_matches(['.', ',', ';', ')'])
                .to_string()
        })
        .collect()
}

/// Les hôtes cités par chaque avis actif, un jeu d'hôtes canoniques par avis.
/// Alimente le recensement `profil_acheteur` (module `discover`), qui en tire
/// à la fois la pondération en avis et le taux de couverture — lequel se
/// mesure **par avis**, pas par hôte (un avis cite aussi le site
/// institutionnel de son acheteur).
pub async fn hotes_par_avis() -> Result<Vec<HashSet<String>>> {
    let mut par_avis = Vec::new();
    parcourir("Avis de marché", "idweb,donnees", |rec| {
        let donnees = rec.get("donnees").and_then(|v| v.as_str()).unwrap_or("");
        let mut hotes = HashSet::new();
        for url in urls(donnees) {
            if let Some(c) = HOTE.captures(&url) {
                let h = super::discover::canon_host(&c[1]);
                if !h.is_empty() {
                    hotes.insert(h);
                }
            }
        }
        par_avis.push(hotes);
        Ok(())
    })
    .await?;
    Ok(par_avis)
}

/// SIRET de l'acheteur : première `cbc:CompanyID` d'un eForms (l'acheteur est
/// la première Organization), sinon le champ `siret` des formats FNSimple.
static SIRET_EFORMS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##""cbc:CompanyID"\s*:\s*(?:\{[^{}]*?"#text"\s*:\s*)?"([0-9 ]{14,20})""##).unwrap()
});
static SIRET_FNS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)"siret"\s*:\s*"?(\d{14})"#).unwrap());

/// CPV principal, dans les trois formats que porte `donnees` : eForms
/// (`cac:MainCommodityClassification`), FNSimple européen (`CPV.PRINCIPAL`) et
/// FNSimple national (`codeCPV.objetPrincipal.classPrincipale`). Le champ n'a pas
/// de colonne dans le jeu ODS — seul `descripteur_code`, taxonomie propre au
/// BOAMP, y est exposé.
static CPV_EFORMS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r##""cac:MainCommodityClassification"\s*:\s*\[?\s*\{\s*"cbc:ItemClassificationCode"\s*:\s*(?:\{[^{}]*?"#text"\s*:\s*)?"(\d{8})""##,
    )
    .unwrap()
});
static CPV_FNS_UE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##""CPV"\s*:\s*\{\s*"PRINCIPAL"\s*:\s*(?:\{[^{}]*?"#text"\s*:\s*)?"?(\d{8})"##)
        .unwrap()
});
static CPV_FNS_FR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##""classPrincipale"\s*:\s*(?:\{[^{}]*?"#text"\s*:\s*)?"?(\d{8})"##).unwrap()
});

fn cpv(donnees: &str) -> Option<String> {
    [&CPV_EFORMS, &CPV_FNS_UE, &CPV_FNS_FR]
        .iter()
        .find_map(|re| re.captures(donnees))
        .map(|c| c[1].to_string())
}

fn siren(donnees: &str) -> Option<String> {
    let brut = SIRET_EFORMS
        .captures(donnees)
        .map(|c| c[1].replace(' ', ""))
        .filter(|s| s.len() == 14)
        .or_else(|| SIRET_FNS.captures(donnees).map(|c| c[1].to_string()))?;
    Some(brut[..9].to_string())
}

/// Référence du marché **chez l'acheteur** : la seule chaîne que l'avis et la
/// plateforme écrivent à l'identique, donc la clé de jointure exacte que le couple
/// (acheteur, référence) attend.
///
/// Trois formes selon le schéma de `donnees` : eForms
/// `cac:ProcurementProject/cbc:ID` (« 2621S », que l'API Atexo rend en
/// `reference`), `cbc:ContractFolderID`, et FNSimple `identifiantInterne`
/// (« PLOUVAssurances20272030 »). `ContractFolderID` porte tantôt la référence de
/// l'acheteur, tantôt l'UUID technique du dossier BOAMP : celui-là n'identifie rien
/// hors du BOAMP et se jette.
static REF_PROJET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r##""cac:ProcurementProject"\s*:\s*\{\s*"cbc:ID"\s*:\s*(?:\{[^{}]*?"#text"\s*:\s*)?"([^"]{1,80})""##,
    )
    .unwrap()
});
static REF_FOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##""cbc:ContractFolderID"\s*:\s*(?:\{[^{}]*?"#text"\s*:\s*)?"([^"]{1,80})""##)
        .unwrap()
});
static REF_FNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r##"(?i)"(?:identifiantInterne|REFERENCE_MARCHE)"\s*:\s*(?:\{[^{}]*?"#text"\s*:\s*)?"([^"]{1,80})""##,
    )
    .unwrap()
});
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap()
});

fn reference(donnees: &str) -> Option<String> {
    [&REF_PROJET, &REF_FOLDER, &REF_FNS]
        .iter()
        .filter_map(|re| re.captures(donnees))
        .map(|c| c[1].trim().to_string())
        .find(|r| r.chars().count() >= 4 && !UUID.is_match(r))
}

/// Plateforme de dépôt déclarée par l'avis, par la sémantique de ses champs —
/// les trois formats de `donnees`. Priorité : endpoint de dépôt (eForms
/// `TenderRecipientParty.EndpointID`, DTD `URL_PARTICIPATION`), puis accès au
/// DCE (`CallForTendersDocumentReference…URI`, `URL_DOCUMENT`, FNSimple
/// `urlDocConsul`), puis profil d'acheteur (`BuyerProfileURI`,
/// `URL_PROFIL_ACHETEUR`, FNSimple `urlProfilAch`/`urlProfilAcheteur`). Le
/// site institutionnel de l'acheteur (`WebsiteURI`, `IDENTITE.URL`, FNSimple
/// `url`) n'est jamais retenu. Les acheteurs déclarent parfois l'hôte nu
/// (`www.demat-ampa.fr`) : il reçoit son schéma.
///
/// Dernier ressort : une URL portée par **n'importe quel autre champ** —
/// adresse complémentaire, conditions de remise, prose libre (`rensgComplt`) —
/// vaut dépôt si et seulement si son hôte est une plateforme avérée :
/// recensée et classée par `annonces-discover` (`plateformes`), ou apex d'un
/// moteur connu. La sémantique de ces champs ne garantit rien ; c'est le
/// recensement qui valide, jamais une devinette.
fn url_depot(donnees: &str, plateformes: &HashSet<String>) -> Option<String> {
    let racine: serde_json::Value = serde_json::from_str(donnees).ok()?;
    let mut meilleure: Option<(u8, String)> = None;
    let mut chemin = Vec::new();
    visite_depot(&racine, &mut chemin, plateformes, &mut meilleure);
    meilleure.map(|(_, u)| u)
}

fn visite_depot<'a>(
    v: &'a serde_json::Value,
    chemin: &mut Vec<&'a str>,
    plateformes: &HashSet<String>,
    meilleure: &mut Option<(u8, String)>,
) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, val) in m {
                chemin.push(k);
                visite_depot(val, chemin, plateformes, meilleure);
                chemin.pop();
            }
        }
        serde_json::Value::Array(a) => {
            for val in a {
                visite_depot(val, chemin, plateformes, meilleure);
            }
        }
        serde_json::Value::String(s) => {
            let feuille = chemin.last().copied().unwrap_or("");
            let sous = |parent: &str| chemin.contains(&parent);
            let priorite = match feuille {
                "cbc:EndpointID" if sous("cac:TenderRecipientParty") => Some(0),
                "URL_PARTICIPATION" => Some(0),
                "cbc:URI" if sous("cac:CallForTendersDocumentReference") => Some(1),
                "URL_DOCUMENT" | "urlDocConsul" => Some(1),
                "cbc:BuyerProfileURI"
                | "URL_PROFIL_ACHETEUR"
                | "urlProfilAch"
                | "urlProfilAcheteur" => Some(2),
                _ => None,
            };
            let candidate = match priorite {
                Some(p) => url_declaree(s).map(|u| (p, u)),
                None => url_plateforme(s, plateformes).map(|u| (3, u)),
            };
            if let Some((p, url)) = candidate {
                if meilleure.as_ref().is_none_or(|(mp, _)| p < *mp) {
                    *meilleure = Some((p, url));
                }
            }
        }
        _ => {}
    }
}

/// Apex des moteurs multi-tenants connus : un hôte qui s'y rattache est une
/// plateforme, même sous une variante (`www.`, tenant) absente du recensement.
const MOTEURS_APEX: &[&str] = &[
    "marches-publics.info",
    "e-marchespublics.com",
    "achatpublic.com",
    "marches-securises.fr",
    "atline.fr",
    "synapse-entreprises.com",
    "xmarches.fr",
    "safetender.com",
    "klekoon.com",
];

/// La première URL d'une valeur libre dont l'hôte est une plateforme avérée —
/// champ URL entier (hôte nu compris) ou URL noyée dans de la prose.
fn url_plateforme(s: &str, plateformes: &HashSet<String>) -> Option<String> {
    let candidates = url_declaree(s).into_iter().chain(URL.find_iter(s).map(|m| {
        m.as_str()
            .trim_end_matches(['.', ',', ';', ')'])
            .to_string()
    }));
    for url in candidates {
        let hote = url
            .split("://")
            .nth(1)
            .unwrap_or(&url)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        let hote = hote
            .trim_start_matches("www.")
            .trim_end_matches('.')
            .to_string();
        if plateformes.contains(&hote)
            || MOTEURS_APEX
                .iter()
                .any(|a| hote == *a || hote.ends_with(&format!(".{a}")))
        {
            return Some(url.replace("&amp;", "&"));
        }
    }
    None
}

/// La valeur d'un champ d'URL déclaré, normalisée : schéma tel quel, hôte nu
/// `www.…` complété en https. Tout le reste (mail, texte libre) est écarté.
fn url_declaree(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with("http") {
        Some(s.replace("&amp;", "&"))
    } else if s.starts_with("www.") && !s.contains([' ', '@']) {
        Some(format!("https://{s}"))
    } else {
        None
    }
}

fn row(rec: &serde_json::Value, plateformes: &HashSet<String>) -> Result<AnnonceBrute> {
    let texte = |k: &str| {
        rec.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let idweb = texte("idweb")
        .ok_or_else(|| SourceError::Invalid(format!("boamp: enregistrement sans idweb: {rec}")))?;
    let donnees = texte("donnees").unwrap_or_default();
    let premier_tableau = |k: &str| {
        rec.get(k)
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let echeance = texte("datelimitereponse")
        .and_then(|s| parse_ts_iso(&s).or_else(|| parse_date_fr(&s).map(|d| echeance_ts(d, None))));
    // Un rectificatif amende un avis précédent, que `annonce_lie` nomme : c'est le
    // seul lien de re-publication que la source affirme, et il vaut clé de jointure
    // exacte. Un avis INITIAL porte parfois le même champ pour un simple voisinage
    // de marché (autres lots, marché antérieur) — celui-là ne fonde aucune identité,
    // on l'ignore.
    let mut cles_jointure = cles_jointure_depuis_urls(urls(&donnees).iter().map(String::as_str));
    if texte("etat").as_deref() == Some("RECTIFICATIF") {
        if let Some(lies) = rec.get("annonce_lie").and_then(|v| v.as_array()) {
            cles_jointure.extend(
                lies.iter()
                    .filter_map(|v| v.as_str())
                    .map(|id| format!("boamp:{id}")),
            );
        }
    }
    // La plateforme de dépôt que l'avis déclare. C'est le repli de la fiche à canal
    // BOAMP seul.
    let url_depot = url_depot(&donnees, plateformes);
    Ok(AnnonceBrute {
        moteur: "boamp",
        source_uid: format!("boamp:{idweb}"),
        cles_jointure,
        reference: reference(&donnees),
        acheteur: texte("nomacheteur").map(|a| nettoyer_acheteur(&a)),
        siren: siren(&donnees),
        objet: texte("objet").unwrap_or_default(),
        nature: premier_tableau("type_marche").and_then(|n| NatureMarche::parse(&n)),
        cpv: cpv(&donnees),
        departement: premier_tableau("code_departement").and_then(|d| normalise_departement(&d)),
        date_publication: texte("dateparution").and_then(|s| parse_date_fr(&s)),
        echeance,
        url_avis: texte("url_avis"),
        url_rc: None,
        url_dce: None,
        url_depot,
        procedure: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eforms_donne_siren_et_cles_de_jointure() {
        let donnees = r##"{"EFORMS":{"ContractNotice":{"efac:Organizations":{"efac:Organization":[
            {"efac:Company":{"cbc:WebsiteURI":"http://www.dives-sur-mer.fr",
             "cac:PartyLegalEntity":{"cbc:CompanyID":"21140225000016"}}},
            {"efac:Company":{"cac:PartyLegalEntity":{"cbc:CompanyID":"17140005400010"}}}]},
            "cac:ContractingParty":{"cbc:BuyerProfileURI":"https://www.marches-publics.gouv.fr/entreprise/consultation/2940332?orgAcronyme=x7c"}}}}"##;
        assert_eq!(siren(donnees).as_deref(), Some("211402250"));
        let rec = serde_json::json!({
            "idweb": "26-75464",
            "nomacheteur": "Commune de Dives-sur-Mer",
            "objet": "Restauration de l'église",
            "dateparution": "2026-08-01",
            "datelimitereponse": "2026-09-12T17:00:00+02:00",
            "type_marche": ["TRAVAUX"],
            "code_departement": ["14"],
            "url_avis": "https://www.boamp.fr/pages/avis/?q=idweb:26-75464",
            "donnees": donnees,
        });
        let r = row(&rec, &HashSet::new()).expect("enregistrement complet");
        assert_eq!(r.source_uid, "boamp:26-75464");
        assert_eq!(r.siren.as_deref(), Some("211402250"));
        assert_eq!(r.nature, Some(NatureMarche::Travaux));
        assert_eq!(r.departement.as_deref(), Some("14"));
        assert_eq!(
            r.cles_jointure,
            vec!["atexo:marches-publics.gouv.fr:2940332".to_string()]
        );
        assert_eq!(
            r.echeance.map(|t| t.to_string()).as_deref(),
            Some("2026-09-12T15:00:00Z")
        );
        assert_eq!(
            r.url_depot.as_deref(),
            Some("https://www.marches-publics.gouv.fr/entreprise/consultation/2940332?orgAcronyme=x7c")
        );
    }

    #[test]
    fn la_reference_de_l_acheteur_est_la_cle_partagee() {
        // eForms, avis 26-78242 : Atexo rend `reference: "2621S"`.
        let eforms = r##"{"EFORMS":{"ContractNotice":{"cbc:ContractFolderID":"34ef43f8-c653-4eea-81e1-9f1c2e102233",
            "cac:ProcurementProject":{"cbc:ID":"2621S","cbc:Name":{"@languageID":"FRA","#text":"Assurance"}}}}}"##;
        assert_eq!(reference(eforms).as_deref(), Some("2621S"));
        // FNSimple, avis 26-78479 : megalis rend `reference:
        // "PLOUVAssurances20272030"`.
        let fns = r#"{"FNSimple":{"initial":{"communication":{"identifiantInterne":"PLOUVAssurances20272030"}}}}"#;
        assert_eq!(reference(fns).as_deref(), Some("PLOUVAssurances20272030"));
        // Le `ContractFolderID` porte tantôt la référence, tantôt l'UUID du dossier
        // BOAMP, qui n'identifie rien hors du BOAMP.
        let folder = r##"{"EFORMS":{"ContractNotice":{"cbc:ContractFolderID":"26MORL0001PA"}}}"##;
        assert_eq!(reference(folder).as_deref(), Some("26MORL0001PA"));
        let uuid = r##"{"EFORMS":{"ContractNotice":{"cbc:ContractFolderID":"34ef43f8-c653-4eea-81e1-9f1c2e102233"}}}"##;
        assert_eq!(reference(uuid), None);
    }

    #[test]
    fn le_depot_suit_la_semantique_des_champs() {
        let sans = HashSet::new();
        let eforms = r##"{"EFORMS":{"ContractNotice":{
            "efac:Organizations":{"efac:Organization":{"efac:Company":{"cbc:WebsiteURI":"https://saint-avold.fr/"}}},
            "cac:ContractingParty":{"cbc:BuyerProfileURI":"https://profil.example.fr"},
            "cac:ProcurementProjectLot":[{"cac:TenderingTerms":{
                "cac:CallForTendersDocumentReference":{"cac:Attachment":{"cac:ExternalReference":{"cbc:URI":"https://dce.example.fr"}}},
                "cac:TenderRecipientParty":{"cbc:EndpointID":"https://www.klekoon.com"}}}]}}}"##;
        assert_eq!(
            url_depot(eforms, &sans).as_deref(),
            Some("https://www.klekoon.com")
        );
        let dtd = r#"{"IDENTITE":{"URL":"https://ville.example.fr",
            "URL_PROFIL_ACHETEUR":"https://profil.example.fr",
            "URL_PARTICIPATION":"https://depot.example.fr"}}"#;
        assert_eq!(
            url_depot(dtd, &sans).as_deref(),
            Some("https://depot.example.fr")
        );
        let fns = r#"{"FNSimple":{"initial":{"communication":{"urlProfilAch":"https://ville.e-marchespublics.com/pack/annonce_1.html"}}}}"#;
        assert_eq!(
            url_depot(fns, &sans).as_deref(),
            Some("https://ville.e-marchespublics.com/pack/annonce_1.html")
        );
        let fns_national = r#"{"avisMarcheFns":{"url":"http://www.ville-sannois.fr/",
            "urlProfilAcheteur":"https://www.marches-securises.fr",
            "urlDocConsul":"https://www.marches-publics.info/mpiaws/index.cfm?fuseaction=dematent.login"}}"#;
        assert_eq!(
            url_depot(fns_national, &sans).as_deref(),
            Some("https://www.marches-publics.info/mpiaws/index.cfm?fuseaction=dematent.login")
        );
        let nu =
            r#"{"FNSimple":{"initial":{"communication":{"urlProfilAch":"www.demat-ampa.fr"}}}}"#;
        assert_eq!(
            url_depot(nu, &sans).as_deref(),
            Some("https://www.demat-ampa.fr")
        );
        assert_eq!(
            url_depot(r#"{"IDENTITE":{"URL":"https://ville.example.fr"}}"#, &sans),
            None
        );
    }

    #[test]
    fn le_dernier_ressort_exige_une_plateforme_averee() {
        let sans = HashSet::new();
        let recensees: HashSet<String> = ["marches-publics.gouv.fr".to_string()].into();
        let prose = r#"{"MAPA":{"initial":{"renseignements":{"rensgComplt":"Le dossier de consultation est téléchargeable sur le site https://www.marches-publics.gouv.fr. Remise papier refusée."}}}}"#;
        assert_eq!(
            url_depot(prose, &recensees).as_deref(),
            Some("https://www.marches-publics.gouv.fr")
        );
        assert_eq!(url_depot(prose, &sans), None);
        let adresse = r#"{"MAPA":{"initial":{"adressesComplt":{"admOuTech":{"adm":{"coord":{"url":"https://www.marches-publics.info"}}}},
            "communication":{"urlProfilAch":"https://profil.example.fr"}}}}"#;
        assert_eq!(
            url_depot(adresse, &sans).as_deref(),
            Some("https://profil.example.fr")
        );
        let adresse_seule = r#"{"MAPA":{"initial":{"adressesComplt":{"envoi":{"conditionsRemise":"https://centreofficielles.e-marchespublics.com"}}}}}"#;
        assert_eq!(
            url_depot(adresse_seule, &sans).as_deref(),
            Some("https://centreofficielles.e-marchespublics.com")
        );
        let institutionnel =
            r#"{"MAPA":{"organisme":{"coord":{"url":"https://www.mairie-jouetsurlaubois.fr"}}}}"#;
        assert_eq!(url_depot(institutionnel, &sans), None);
    }

    #[test]
    fn le_rectificatif_joint_l_avis_qu_il_amende() {
        let rec = |etat: &str| {
            serde_json::json!({
                "idweb": "25-122365", "objet": "Marché", "dateparution": "2026-07-01",
                "etat": etat, "annonce_lie": ["25-95779"],
                "url_avis": "https://www.boamp.fr/pages/avis/?q=idweb:25-122365",
            })
        };
        assert_eq!(
            row(&rec("RECTIFICATIF"), &HashSet::new())
                .unwrap()
                .cles_jointure,
            vec!["boamp:25-95779".to_string()]
        );
        assert!(row(&rec("INITIAL"), &HashSet::new())
            .unwrap()
            .cles_jointure
            .is_empty());
    }

    #[test]
    fn cpv_principal_dans_les_trois_formats() {
        let eforms = r##"{"cac:ProcurementProject":{"cac:MainCommodityClassification":{"cbc:ItemClassificationCode":{"@listName":"cpv","#text":"60100000"}},
            "cac:AdditionalCommodityClassification":[{"cbc:ItemClassificationCode":{"@listName":"cpv","#text":"34120000"}}]}}"##;
        assert_eq!(cpv(eforms).as_deref(), Some("60100000"));
        let fns_ue = r#"{"OBJET":{"CPV":{"PRINCIPAL":"79951000"},"DIV_EN_LOTS":{"NON":""}}}"#;
        assert_eq!(cpv(fns_ue).as_deref(), Some("79951000"));
        let fns_fr =
            r#"{"objetMarche":{"codeCPV":{"objetPrincipal":{"classPrincipale":"45262660"}}}}"#;
        assert_eq!(cpv(fns_fr).as_deref(), Some("45262660"));
        assert_eq!(cpv(r#"{"codeCPV":{"objetPrincipal":""}}"#), None);
    }

    #[test]
    fn fnsimple_siret_plat_et_date_nue() {
        let donnees = r#"{"AVIS":{"IDENTITE":{"SIRET":"21140225000016","DENOMINATION":"X"}}}"#;
        assert_eq!(siren(donnees).as_deref(), Some("211402250"));
        let rec = serde_json::json!({
            "idweb": "22-1",
            "objet": "Marché",
            "dateparution": "2022-03-06",
            "datelimitereponse": "2026-09-12",
            "donnees": donnees,
        });
        let r = row(&rec, &HashSet::new()).expect("enregistrement minimal");
        assert_eq!(
            r.echeance.map(|t| t.to_string()).as_deref(),
            Some("2026-09-12T21:59:59Z")
        );
        assert!(r.cles_jointure.is_empty());
    }

    #[test]
    fn avis_sans_blob_reste_ingerable() {
        let rec = serde_json::json!({"idweb": "26-9", "objet": "O", "dateparution": "2026-08-01"});
        let r = row(&rec, &HashSet::new()).expect("idweb suffit");
        assert!(r.siren.is_none());
    }

    #[test]
    fn resultat_donne_parent_et_lots() {
        let donnees = r##"{"ATTRIBUTION":{"DECISION":{"TITULAIRE":{"DENOMINATION":"Parcs et sports","VILLE":"Chassieu"},"RENSEIGNEMENT":{"NB_OFFRE_RECU":4,"MONTANT":{"@DEVISE":"EUR","#text":31518}}}}}"##;
        let rec = serde_json::json!({
            "idweb": "26-78342",
            "nomacheteur": "Angers Loire Metropole (49)",
            "objet": "Maintenance préventive et corrective",
            "dateparution": "2026-08-07",
            "annonce_lie": ["26-7034"],
            "type_marche": ["SERVICES"],
            "code_departement": ["49"],
            "url_avis": "https://www.boamp.fr/pages/avis/?q=idweb:26-78342",
            "donnees": donnees,
        });
        let r = row_resultat(&rec).expect("résultat complet");
        assert_eq!(r.idweb, "26-78342");
        assert_eq!(r.parent_idweb.as_deref(), Some("26-7034"));
        assert_eq!(r.nature, Some(NatureMarche::Services));
        assert_eq!(r.departement.as_deref(), Some("49"));
        assert_eq!(r.attribution.lots.len(), 1);
        assert_eq!(r.attribution.lots[0].titulaire, "Parcs et sports");
        assert_eq!(r.attribution.lots[0].montant, Some(31_518.0));
        let rec = serde_json::json!({"idweb": "18-9", "objet": "O", "dateparution": "2018-01-05"});
        let r = row_resultat(&rec).expect("idweb suffit");
        assert_eq!(r.url_avis, "https://www.boamp.fr/pages/avis/?q=idweb:18-9");
        assert!(r.attribution.lots.is_empty());
    }

    #[test]
    fn le_mapa_rattache_par_son_avis_initial() {
        let donnees = r##"{"MAPA":{"attribution":{"avisInitial":{"idWeb":"26-8202"},
            "descriptionReduite":{"objet":"Collecte de déchets de soins"},
            "attribution":{"resultat":{"attribue":{
              "titulaire":{"PersonneMorale":"SUD EST ASSAINISSEMENT","adr":{"ville":"CAGNES SUR MER"}},
              "montant":{"@devise":"EUR","valeur":"20000"}}}}}}}"##;
        let rec = serde_json::json!({
            "idweb": "26-56907", "objet": "Collecte", "dateparution": "2026-07-30",
            "donnees": donnees,
        });
        let r = row_resultat(&rec).expect("résultat MAPA");
        assert_eq!(r.parent_idweb.as_deref(), Some("26-8202"));
        assert_eq!(r.attribution.lots.len(), 1);
        assert_eq!(r.attribution.lots[0].titulaire, "SUD EST ASSAINISSEMENT");
        assert_eq!(r.attribution.lots[0].montant, Some(20_000.0));
        assert_eq!(
            r.attribution.lots[0].ville.as_deref(),
            Some("CAGNES SUR MER")
        );
    }
}
