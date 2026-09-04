//! TED — la colonne européenne du fond marchés.
//!
//! API v3 de l'Office des publications, publique et sans clé, servie en eForms.
//! Stock actif = acheteur français et date limite de réception non passée
//! (7 078 avis le 15/08/2026, en ~30 pages de 250). Trois champs portent le
//! profil d'acheteur — `document-url-lot` (le DCE), `submission-url-lot` (le
//! dépôt), `buyer-profile` — remplis à 92 / 95 / 58 % : quand l'URL porte
//! l'identifiant du dossier, elle donne la clé de jointure vers la ligne du
//! moteur, sinon elle donne le tenant.
//!
//! Ce que TED ajoute au BOAMP : un avis publié au JOUE dont l'acheteur a omis
//! le BOAMP, et les reports d'échéance, qu'un rectificatif TED date alors que la
//! ligne BOAMP reste figée.

use jiff::civil::Date;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{json, Value};

use lj_core::annonces::{
    cles_jointure_depuis_urls, departement_from_cp, embellir_titre, nettoyer_acheteur,
    parse_date_fr, parse_ts_iso, AnnonceBrute, NatureMarche,
};

use super::MoteurRun;
use crate::error::{Result, SourceError};

const API: &str = "https://api.ted.europa.eu/v3/notices/search";
/// Plafond servi par l'API sur `limit`.
const PAR_PAGE: usize = 250;
/// Garde-fou : le stock FR ouvert tient en ~30 pages. Au-delà, la requête a
/// changé de sens.
const PAGES_MAX: usize = 200;

/// Acheteur français, date limite non passée. Le filtre serveur évite de
/// paginer tout l'historique — un avis clos n'apprend plus rien à la moisson.
const REQUETE: &str =
    "(organisation-country-buyer IN (FRA)) AND (deadline-receipt-request>=today())";

const CHAMPS: [&str; 15] = [
    "publication-number",
    "notice-title",
    "publication-date",
    "deadline-receipt-request",
    "buyer-name",
    "buyer-profile",
    "document-url-lot",
    "document-url-part",
    "submission-url-lot",
    "internal-identifier-proc",
    "contract-nature",
    "procedure-type",
    "classification-cpv",
    "place-of-performance-post-code-proc",
    "organisation-identifier-buyer",
];

/// Les champs qui portent une URL de profil d'acheteur, dans l'ordre de
/// confiance : le lien profond du dossier avant la porte du tenant.
const CHAMPS_URL: [&str; 4] = [
    "document-url-lot",
    "document-url-part",
    "submission-url-lot",
    "buyer-profile",
];

#[derive(Deserialize)]
struct Reponse {
    #[serde(default)]
    notices: Vec<Value>,
    #[serde(rename = "totalNoticeCount", default)]
    total: u64,
}

async fn page(client: &reqwest::Client, n: usize, champs: &[&str]) -> Result<Reponse> {
    let corps = json!({
        "query": REQUETE,
        "fields": champs,
        "limit": PAR_PAGE,
        "page": n,
    });
    let body = super::send_text(client.post(API).json(&corps), "ted search").await?;
    serde_json::from_str(&body)
        .map_err(|e| SourceError::Invalid(format!("ted: réponse illisible: {e}")))
}

/// Ré-énumère les avis FR encore ouverts.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let plafond = plafond_annee();
    let mut rows = Vec::new();
    let mut annonce = 0;
    for n in 1..=PAGES_MAX {
        let r = page(&client, n, &CHAMPS).await?;
        annonce = r.total;
        if r.notices.is_empty() {
            break;
        }
        rows.extend(r.notices.iter().filter_map(|n| depuis_notice(n, plafond)));
        if rows.len() as u64 >= annonce {
            break;
        }
    }
    Ok(MoteurRun {
        annonce: Some(annonce),
        echecs: 0,
        hotes: Vec::new(),
        rows: super::dedup_source_uid(rows),
    })
}

/// Les hôtes que cite chaque avis ouvert — second témoin du recensement, à côté
/// du BOAMP, qui n'en fait pas le même métier : les entités adjudicatrices des
/// secteurs spéciaux (SNCF, RATP, EDF, RTE, La Poste…) publient au JOUE et sur
/// leur propre profil sans passer par le BOAMP. Sur les 718 hôtes cités le
/// 15/08/2026, 152 étaient absents d'un recensement fondé sur le seul BOAMP.
///
/// Quatre champs suffisent : la pagination y est deux fois plus légère que
/// celle de [`enumerate`].
pub async fn hotes_par_avis() -> Result<Vec<std::collections::HashSet<String>>> {
    let client = super::client();
    let mut par_avis: Vec<std::collections::HashSet<String>> = Vec::new();
    for n in 1..=PAGES_MAX {
        let r = page(&client, n, &CHAMPS_URL).await?;
        if r.notices.is_empty() {
            break;
        }
        par_avis.extend(r.notices.iter().map(|notice| {
            urls(notice)
                .iter()
                .filter_map(|u| super::discover::hote_de(u))
                .map(|h| super::discover::canon_host(&h))
                .collect()
        }));
        if par_avis.len() as u64 >= r.total {
            break;
        }
    }
    Ok(par_avis)
}

/// Un champ multilingue TED (`{"fra": ["…"]}`) : le français d'abord, sinon la
/// première langue servie.
fn texte(v: Option<&Value>) -> Option<String> {
    let premier = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.first().and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    match v? {
        Value::Object(o) => o.get("fra").or_else(|| o.values().next()).and_then(premier),
        autre => premier(autre),
    }
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

/// Premier élément d'un champ répété par lot (`["services", "services"]`).
fn premier(v: Option<&Value>) -> Option<&str> {
    match v? {
        Value::String(s) => Some(s.as_str()),
        Value::Array(a) => a.first().and_then(Value::as_str),
        _ => None,
    }
}

/// Horizon au-delà duquel une date limite est une coquille de saisie : le stock
/// FR du 15/08/2026 en porte 31 au-delà de 2035, dont une en 2924. Un système
/// d'acquisition dynamique reste ouvert des années — dix ans laissent passer
/// les vrais, pas les fautes de frappe.
const HORIZON_ANS: i16 = 10;

/// La date limite la plus tardive parmi les lots : la consultation reste
/// ouverte tant qu'un de ses lots l'est. Une date aberrante est écartée plutôt
/// que ramenée à une borne — la ligne existe, elle n'est simplement pas dans la
/// fenêtre ouverte.
fn echeance(v: Option<&Value>, plafond: i16) -> Option<Timestamp> {
    let tardive = match v? {
        Value::String(s) => parse_ts_iso(s),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .filter_map(parse_ts_iso)
            .max(),
        _ => None,
    }?;
    let annee = tardive.to_zoned(jiff::tz::TimeZone::UTC).date().year();
    (annee <= plafond).then_some(tardive)
}

/// L'année au-delà de laquelle une échéance n'est plus crédible.
fn plafond_annee() -> i16 {
    Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::get("Europe/Paris").expect("tzdb embarquée"))
        .date()
        .year()
        + HORIZON_ANS
}

/// TED normalise le titre en « France – <libellé CPV> – <objet> » (7 073 des
/// 7 078 avis FR ouverts le 15/08/2026). Le préfixe n'est pas l'objet du
/// marché : gardé, il empêche la ligne de se fondre dans sa jumelle, dont le
/// titre est celui que l'acheteur a écrit. Le tiret long est le séparateur de
/// TED, jamais celui d'un objet de marché — un objet garde ses tirets courts.
fn objet_nu(titre: &str) -> &str {
    static PREFIXE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^France\s*[–—]\s*[^–—]+?\s*[–—]\s*").expect("préfixe TED valide")
    });
    match PREFIXE.find(titre) {
        Some(m) => &titre[m.end()..],
        None => titre,
    }
}

/// La nature du marché, en vocabulaire eForms.
fn nature(v: Option<&Value>) -> Option<NatureMarche> {
    match premier(v)? {
        "works" => Some(NatureMarche::Travaux),
        "supplies" => Some(NatureMarche::Fournitures),
        "services" => Some(NatureMarche::Services),
        _ => Some(NatureMarche::Autres),
    }
}

/// Le SIREN de l'acheteur : `organisation-identifier-buyer` porte le SIRET,
/// parfois suivi d'identifiants internes qui n'en sont pas.
fn siren(v: Option<&Value>) -> Option<String> {
    let candidats: Vec<&str> = match v? {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => return None,
    };
    candidats
        .into_iter()
        .map(|s| s.replace(' ', ""))
        .find(|s| s.len() == 14 && s.chars().all(|c| c.is_ascii_digit()))
        .map(|s| s[..9].to_string())
}

fn urls(n: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for champ in CHAMPS_URL {
        match n.get(champ) {
            Some(Value::String(s)) => out.push(s.clone()),
            Some(Value::Array(a)) => {
                out.extend(a.iter().filter_map(Value::as_str).map(str::to_string));
            }
            _ => {}
        }
    }
    // Ordre de confiance conservé, doublons ôtés : un même lien revient sur
    // chaque lot.
    let mut vus = std::collections::HashSet::new();
    out.retain(|u| vus.insert(u.clone()));
    out
}

/// L'avis sur le site TED, en français : l'URL de référence de la ligne.
fn url_avis(numero: &str) -> String {
    format!("https://ted.europa.eu/fr/notice/-/detail/{numero}")
}

fn depuis_notice(n: &Value, plafond: i16) -> Option<AnnonceBrute> {
    let numero = n.get("publication-number")?.as_str()?;
    let objet = texte(n.get("notice-title")).map(|t| embellir_titre(objet_nu(&t)))?;
    let urls = urls(n);
    let date_publication: Option<Date> = texte(n.get("publication-date"))
        .as_deref()
        .and_then(parse_date_fr);
    Some(AnnonceBrute {
        moteur: "ted",
        source_uid: format!("ted:{numero}"),
        cles_jointure: cles_jointure_depuis_urls(urls.iter().map(String::as_str)),
        reference: texte(n.get("internal-identifier-proc")),
        acheteur: texte(n.get("buyer-name")).map(|a| nettoyer_acheteur(&a)),
        siren: siren(n.get("organisation-identifier-buyer")),
        objet,
        nature: nature(n.get("contract-nature")),
        cpv: premier(n.get("classification-cpv")).map(str::to_string),
        departement: premier(n.get("place-of-performance-post-code-proc"))
            .and_then(departement_from_cp),
        date_publication,
        echeance: echeance(n.get("deadline-receipt-request"), plafond),
        url_avis: Some(url_avis(numero)),
        url_rc: None,
        url_dce: urls.first().cloned(),
        // Le tenant, quand l'avis le nomme : c'est lui que le recensement
        // classe et que le jumelage lit.
        url_depot: texte(n.get("buyer-profile")),
        procedure: texte(n.get("procedure-type")),
    })
}

#[cfg(test)]
mod tests {
    use super::{depuis_notice, objet_nu};
    use lj_core::annonces::NatureMarche;

    /// Plafond fixe : le test ne dépend pas de l'année où il tourne.
    const PLAFOND: i16 = 2036;

    /// TED préfixe l'objet du pays et du libellé CPV — 7 073 avis sur 7 078 le
    /// 15/08/2026. Le préfixe part, les tirets courts de l'objet restent : sans
    /// ça, aucune ligne TED ne se fond dans sa jumelle de plateforme.
    #[test]
    fn le_prefixe_pays_et_cpv_de_ted_ne_fait_pas_partie_de_l_objet() {
        assert_eq!(
            objet_nu("France – Véhicules à moteur – Achat de véhicules - lot 2"),
            "Achat de véhicules - lot 2"
        );
        assert_eq!(
            objet_nu("France – Services d'architecture – Réfection – tranche ferme"),
            "Réfection – tranche ferme"
        );
        // Un titre qui ne suit pas le gabarit n'est pas amputé.
        assert_eq!(objet_nu("Fourniture de repas"), "Fourniture de repas");
    }

    /// Le stock FR porte 31 échéances au-delà de 2035, dont une en 2924 : une
    /// coquille de saisie ne doit pas tenir une ligne ouverte pour mille ans.
    #[test]
    fn une_echeance_aberrante_est_ecartee_pas_ramenee() {
        let n = serde_json::json!({
            "publication-number": "1-2026",
            "notice-title": "France – Transport – AT 01 transport scolaire",
            "deadline-receipt-request": ["2924-03-18T12:00:00+02:00"],
            "buyer-profile": ["https://marchespublics.auvergnerhonealpes.eu/"]
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert_eq!(r.echeance, None);
        assert_eq!(r.objet, "AT 01 transport scolaire");
    }

    /// Forme réelle servie le 15/08/2026 : champs multilingues, valeurs
    /// répétées par lot, URLs en tableau. L'URL PLACE porte l'identifiant du
    /// dossier — donc la clé de jointure vers la ligne Atexo.
    #[test]
    fn un_avis_ted_rend_sa_cle_de_jointure_et_son_echeance_la_plus_tardive() {
        let n = serde_json::json!({
            "publication-number": "49554-2024",
            "notice-title": {"fra": ["France – Produits pharmaceutiques – Fourniture de produits de contraste"]},
            "publication-date": "2024-01-24Z",
            "deadline-receipt-request": [
                "2026-09-14T12:00:00+02:00",
                "2026-10-02T12:00:00+02:00"
            ],
            "buyer-name": {"fra": ["CHU DE BORDEAUX (33)"]},
            "buyer-profile": ["https://www.marches-publics.gouv.fr/?page=Entreprise.EntrepriseAdvancedSearch&AllCons&id=2461241&orgAcronyme=x7c"],
            "document-url-lot": ["https://www.marches-publics.gouv.fr/?page=Entreprise.EntrepriseAdvancedSearch&AllCons&id=2461241&orgAcronyme=x7c"],
            "internal-identifier-proc": "24S0012",
            "contract-nature": ["supplies", "supplies"],
            "procedure-type": "open",
            "classification-cpv": ["33696000", "33696000"],
            "place-of-performance-post-code-proc": ["33000"],
            "organisation-identifier-buyer": ["26330011200017", "1776461"]
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert_eq!(r.source_uid, "ted:49554-2024");
        assert_eq!(
            r.cles_jointure,
            vec!["atexo:marches-publics.gouv.fr:2461241".to_string()]
        );
        assert_eq!(r.acheteur.as_deref(), Some("CHU DE BORDEAUX"));
        assert_eq!(r.siren.as_deref(), Some("263300112"));
        assert_eq!(r.reference.as_deref(), Some("24S0012"));
        assert_eq!(r.nature, Some(NatureMarche::Fournitures));
        assert_eq!(r.cpv.as_deref(), Some("33696000"));
        assert_eq!(r.departement.as_deref(), Some("33"));
        assert_eq!(
            r.echeance.map(|t| t.to_string()),
            Some("2026-10-02T10:00:00Z".to_string())
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some("https://ted.europa.eu/fr/notice/-/detail/49554-2024")
        );
    }

    /// Un avis sans objet ne fait pas une ligne : rien à afficher, rien à
    /// apparier. La réconciliation l'écarterait de toute façon.
    #[test]
    fn un_avis_sans_objet_est_ignore() {
        let n = serde_json::json!({"publication-number": "1-2026"});
        assert!(depuis_notice(&n, PLAFOND).is_none());
    }
}
