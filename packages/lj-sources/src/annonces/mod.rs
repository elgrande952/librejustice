//! Connecteurs du fond « marchés publics » du pot Annonces.
//!
//! Un module par canal : BOAMP Opendatasoft (la colonne structurée — eForms,
//! SIRET, CPV) et les moteurs de profils d'acheteurs (liens profonds + MAPA
//! absents du BOAMP). Chaque connecteur ré-énumère le stock actif complet — pas
//! de delta incrémental — et rend des [`AnnonceBrute`] que
//! `lj_core::annonces::canonicaliser` fond en lignes canoniques.

use std::time::Duration;

use lj_core::annonces::AnnonceBrute;

use crate::error::{Result, SourceError};

pub mod achatpublic;
pub mod aji;
pub mod atexo;
pub mod atline;
pub mod aws;
pub mod boamp;
pub mod dematis;
pub mod discover;
pub mod klekoon;
pub mod modula;
pub mod presse;
pub mod safetender;
pub mod synapse;
pub mod ted;
pub mod xmarches;

/// Énumération d'un hôte d'un moteur multi-tenant : total déclaré + lignes.
pub(crate) type HoteRun = Result<(u64, Vec<AnnonceBrute>)>;

/// Verdict d'un hôte recensé tenté. Le recensement en tient l'historique, qui
/// seul sépare l'entrée morte de la panne du jour.
#[derive(Debug)]
pub struct HoteVerdict {
    pub host: String,
    pub joignable: bool,
}

/// Résultat d'une ré-énumération complète d'un canal.
#[derive(Debug)]
pub struct MoteurRun {
    /// Total déclaré par le moteur quand il est exploitable pour le sanity
    /// check collecté/annoncé — `None` quand le moteur n'en publie pas de
    /// fiable (Dematis compte les avis clos, Synapse n'en publie aucun…).
    pub annonce: Option<u64>,
    /// Sous-unités perdues après retries qui ne sont pas des hôtes recensés
    /// (cellule, tranche de pages, branding) : run partiel, on upserte ce qui
    /// est vu. Les hôtes se comptent dans [`Self::hotes`].
    pub echecs: u32,
    /// Un verdict par hôte recensé tenté, pour Atexo et Safetender. Vide pour
    /// les moteurs à point d'entrée unique.
    pub hotes: Vec<HoteVerdict>,
    pub rows: Vec<AnnonceBrute>,
}

/// Ce qu'une page de détail d'avis apprend en plus de sa ligne de listing :
/// les deux seuls champs que les scrapers ne portent pas en liste, et que le
/// BOAMP donne déjà par son flux.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DetailAnnonce {
    /// Code CPV principal (`45233140` ou `45233140-2`) — la division seule
    /// est dérivée à l'écriture. Klekoon ne publie que la division (`45`).
    pub cpv: Option<String>,
    /// Libellé de la procédure, tel que l'avis le donne.
    pub procedure: Option<String>,
    /// Acheteur, quand la fiche le nomme et que la liste ne le portait pas.
    ///
    /// Seul Atexo en sert : son API rend l'organisme en URI d'un référentiel
    /// sous JWT, sa fiche l'écrit en clair. Ne remplace jamais un libellé déjà
    /// posé — la liste, quand elle le donne, fait foi.
    pub acheteur: Option<String>,
}

/// Lit la page de détail d'un avis sur le moteur qui la sert. Moteur inconnu =
/// erreur franche : la file d'enrichissement retient la ligne, elle ne l'oublie
/// pas silencieusement. `egress` n'est consommé que par klekoon, dont le TLS
/// impose un pont — les autres moteurs l'ignorent.
pub async fn detail(moteur: &str, url_avis: &str, egress: &str) -> Result<DetailAnnonce> {
    match moteur {
        "atexo" => atexo::detail(url_avis).await,
        "safetender" => safetender::detail(url_avis).await,
        "aws" => aws::detail(url_avis).await,
        "dematis" => dematis::detail(url_avis).await,
        "achatpublic" => achatpublic::detail(url_avis).await,
        "atline" => atline::detail(url_avis).await,
        "synapse" => synapse::detail(url_avis).await,
        "xmarches" => xmarches::detail(url_avis).await,
        "aji" => aji::detail(url_avis).await,
        "klekoon" => klekoon::detail(egress, url_avis).await,
        // Le produit Modula ne publie de CPV nulle part (listing, fiche DCE,
        // bloc CPF) et la procédure est déjà en liste : fiche muette par
        // construction, sans requête.
        "modula" => Ok(DetailAnnonce::default()),
        // Canaux de publication, pas plateformes : TED sert CPV et procédure
        // dans sa recherche, le dump de presse ne mène qu'à la page d'avis
        // qu'on a déjà lue. Rien de plus à aller chercher.
        "ted" | "presse" => Ok(DetailAnnonce::default()),
        autre => Err(SourceError::Invalid(format!(
            "détail : moteur inconnu « {autre} »"
        ))),
    }
}

/// UA navigateur : plusieurs moteurs servent une page dégradée (ou un 403) aux
/// UA inconnus.
pub(crate) const UA_NAVIGATEUR: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149 Safari/537.36";

/// Client d'un moteur : jar de cookies (recherches à état de session),
/// redirections suivies (les marques blanches Atexo redirigent vers `www.`).
pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA_NAVIGATEUR)
        .cookie_store(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        .expect("reqwest client build")
}

/// Client de **sondage** (recensement des profils) : un hôte cité par un avis
/// peut être mort, parqué ou lent. Ici on classe des milliers d'hôtes, dont la
/// majorité ne répondra jamais : timeouts courts et un seul essai
/// (`send_text_once`).
pub(crate) fn client_sonde() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA_NAVIGATEUR)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        // Un profil sous nom de vanité (`mp74.fr`, `marches-publics.parishabitat.fr`)
        // sert le certificat de son moteur : 25 hôtes du recensement ne se
        // classaient pas pour cette seule raison.
        .danger_accept_invalid_certs(true)
        .build()
        .expect("reqwest client build")
}

/// Corps texte, un seul essai — pour les sondages de masse où un échec est une
/// information (l'hôte ne sert pas de profil d'acheteur), pas un incident.
pub(crate) async fn send_text_once(rb: reqwest::RequestBuilder) -> Option<(String, String)> {
    let r = rb.send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let url = r.url().to_string();
    r.text().await.ok().map(|t| (url, t))
}

/// Corps texte avec 3 retries (backoff ×2) sur 429/5xx et erreurs de
/// transport ; tout autre statut est une erreur franche.
pub(crate) async fn send_text(rb: reqwest::RequestBuilder, what: &str) -> Result<String> {
    const ESSAIS: u32 = 3;
    let mut derniere = String::new();
    for essai in 0..ESSAIS {
        if essai > 0 {
            tokio::time::sleep(Duration::from_millis(500 << essai)).await;
        }
        let rb = rb
            .try_clone()
            .ok_or_else(|| SourceError::Invalid(format!("{what}: requête non clonable")))?;
        match rb.send().await {
            Ok(r) if r.status().is_success() => match r.text().await {
                Ok(t) => return Ok(t),
                Err(e) => derniere = format!("lecture du corps: {e}"),
            },
            Ok(r) if r.status().as_u16() == 429 || r.status().is_server_error() => {
                derniere = format!("HTTP {}", r.status());
            }
            Ok(r) => {
                return Err(SourceError::Invalid(format!("{what}: HTTP {}", r.status())));
            }
            Err(e) => derniere = e.to_string(),
        }
    }
    Err(SourceError::Invalid(format!(
        "{what}: {derniere} après {ESSAIS} essais"
    )))
}

/// Aplatit une carte HTML en une seule ligne de texte : les motifs de champs
/// (« Date limite : … ») se lisent au fil du texte, pas dans l'arbre.
pub(crate) fn txt(html: &str) -> String {
    static TAG_ECHAPPE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)</?(?:b|i|u|em|strong|br|p|div|span|li|ul|ol|h[1-6]|table|tbody|tr|td|th|a|font|sup|sub)\b[^<>]*/?>",
        )
        .expect("regex balise échappée valide")
    });
    let plat = crate::html_strip::strip_html(html).replace('\n', " ");
    // Certains listings échappent le HTML de l'objet (`&lt;br&gt;`) : le
    // décodage d'entités du strip matérialise ces balises après leur retrait.
    // Second passage borné aux balises HTML avérées — `<<gens du voyage>>`
    // est du texte, pas une balise.
    if !TAG_ECHAPPE.is_match(&plat) {
        return plat;
    }
    TAG_ECHAPPE
        .replace_all(&plat, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Coupe la fin de la balise ouvrante sur laquelle un bloc a été découpé.
/// Découper sur `<table class="consultation ` laisse `en_cours" id="x">` en
/// tête du bloc : sans `<` devant, le strip HTML ne le voit pas comme une
/// balise et ce résidu passe pour du texte.
pub(crate) fn apres_balise(bloc: &str) -> &str {
    match bloc.find('>') {
        Some(i) => &bloc[i + 1..],
        None => bloc,
    }
}

/// Dédoublonne par `source_uid` (une consultation revient sur plusieurs
/// cellules ou pages d'un même moteur), ordre stable.
pub fn dedup_source_uid(rows: Vec<AnnonceBrute>) -> Vec<AnnonceBrute> {
    let mut vus = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|r| vus.insert(r.source_uid.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::txt;

    #[test]
    fn le_html_echappe_est_retire_mais_les_chevrons_de_texte_restent() {
        assert_eq!(
            txt("R&eacute;habilitation &lt;br&gt; de &lt;b&gt;trois&lt;/b&gt; maisons"),
            "Réhabilitation de trois maisons"
        );
        assert_eq!(
            txt("Gestion des terrains &lt;&lt;gens du voyage&gt;&gt; de la CDA"),
            "Gestion des terrains <<gens du voyage>> de la CDA"
        );
    }
}
