//! Connectors of the public-procurement fond of the notice pot.
//!
//! One module per channel: the BOAMP on Opendatasoft (the structured column:
//! eForms, SIRET, CPV) and the buyer-profile engines (deep links, and the MAPA
//! the BOAMP lacks). Each connector re-lists the whole active stock, with no
//! incremental delta, and returns [`AnnonceBrute`] rows that
//! `lj_annonces::canonicaliser` merges into canonical rows.

use std::time::Duration;

use lj_annonces::{AnnonceBrute, ContenuAnnonce};
use wreq_util::Profile;

use lj_sources::error::{Result, SourceError};

pub mod achatpublic;
pub mod aji;
pub mod atexo;
pub mod atline;
pub mod aws;
pub mod aws_tenants;
pub mod boamp;
pub mod centraledesmarches;
pub mod decp;
pub mod dematis;
pub mod discover;
mod fiche;
mod formulaire;
pub mod klekoon;
pub mod modula;
pub mod nicematin;
pub mod presse;
pub mod provincenord;
pub mod safetender;
pub mod sonde;
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

/// What a notice page adds to its listing row: the fields the scrapers do not
/// carry in a list, and that the BOAMP already gives through its feed.
#[derive(Debug, Default, Clone, PartialEq)]
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
    /// Place of performance, when the notice page names it and the listing does
    /// not. Without it the consultation answers no département filter.
    pub zones: Vec<lj_annonces::zones::Zone>,
    pub contenu: ContenuAnnonce,
}

/// Lit la page de détail d'un avis sur le moteur qui la sert. Moteur inconnu =
/// erreur franche : la file d'enrichissement retient la ligne, elle ne l'oublie
/// pas silencieusement.
pub async fn detail(moteur: &str, url_avis: &str) -> Result<DetailAnnonce> {
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
        "klekoon" => klekoon::detail(url_avis).await,
        "centraledesmarches" => centraledesmarches::detail(url_avis).await,
        "modula" => modula::detail(url_avis).await,
        // Canaux de publication, pas plateformes : TED sert CPV et procédure
        // dans sa recherche, le dump de presse ne mène qu'à la page d'avis
        // qu'on a déjà lue. Rien de plus à aller chercher.
        "ted" | "presse" => Ok(DetailAnnonce::default()),
        // The consultation view of Province Nord repeats the listing row and
        // adds no object; the press ad is read whole at enumeration.
        "provincenord" | "nicematin" => Ok(DetailAnnonce::default()),
        autre => Err(SourceError::Invalid(format!(
            "détail : moteur inconnu « {autre} »"
        ))),
    }
}

pub(crate) const NAVIGATEUR: Profile = Profile::Chrome149;

static SORTIE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub fn poser_sortie(proxy: Option<String>) {
    let _ = SORTIE.set(proxy.filter(|p| !p.is_empty()));
}

pub(crate) fn avec_sortie(b: wreq::ClientBuilder) -> wreq::ClientBuilder {
    match SORTIE.get().and_then(Option::as_deref) {
        Some(url) => b.proxy(wreq::Proxy::all(url).expect("proxy de sortie valide")),
        None => b,
    }
}

/// Client d'un moteur : jar de cookies (recherches à état de session),
/// redirections suivies (les marques blanches Atexo redirigent vers `www.`).
pub(crate) fn client() -> wreq::Client {
    avec_sortie(wreq::Client::builder())
        .emulation(NAVIGATEUR)
        .cookie_store(true)
        // wreq follows nothing by default where reqwest followed ten hops, and
        // an unfollowed redirect reads as an empty body with no error.
        .redirect(wreq::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        .expect("wreq client build")
}

/// Client de **sondage** (recensement des profils) : un hôte cité par un avis
/// peut être mort, parqué ou lent. Ici on classe des milliers d'hôtes, dont la
/// majorité ne répondra jamais : timeouts courts et un seul essai
/// (`send_text_once`).
pub(crate) fn client_sonde() -> wreq::Client {
    avec_sortie(wreq::Client::builder())
        .emulation(NAVIGATEUR)
        .redirect(wreq::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        // Un profil sous nom de vanité (`mp74.fr`, `marches-publics.parishabitat.fr`)
        // sert le certificat de son moteur : 25 hôtes du recensement ne se
        // classaient pas pour cette seule raison.
        .tls_cert_verification(false)
        .tls_verify_hostname(false)
        .build()
        .expect("wreq client build")
}

/// Corps texte, un seul essai — pour les sondages de masse où un échec est une
/// information (l'hôte ne sert pas de profil d'acheteur), pas un incident.
pub(crate) async fn send_text_once(rb: wreq::RequestBuilder) -> Option<(String, String)> {
    let r = rb.send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let url = r.uri().to_string();
    r.text().await.ok().map(|t| (url, t))
}

/// Corps texte avec 3 retries (backoff ×2) sur 429/5xx et erreurs de
/// transport ; tout autre statut est une erreur franche.
pub(crate) async fn send_text(rb: wreq::RequestBuilder, what: &str) -> Result<String> {
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

/// Binary body, same retries as [`send_text`].
pub(crate) async fn send_bytes(rb: wreq::RequestBuilder, what: &str) -> Result<Vec<u8>> {
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
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) => return Ok(b.to_vec()),
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

static TAG_ECHAPPE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)</?(?:b|i|u|em|strong|br|p|div|span|li|ul|ol|h[1-6]|table|tbody|tr|td|th|a|font|sup|sub)\b[^<>]*/?>",
    )
    .expect("regex balise échappée valide")
});

/// A text field of a registry that escapes it as HTML, twice on the BOAMP and
/// TED (`&amp;quot;`). Its line breaks stay, the tags it escaped go.
pub fn desechapper(brut: &str) -> String {
    if !brut.contains('&') {
        return brut.to_string();
    }
    let texte = lj_sources::html_strip::decode_entities(brut);
    if !TAG_ECHAPPE.is_match(&texte) {
        return texte;
    }
    TAG_ECHAPPE
        .replace_all(&texte, |c: &regex::Captures| {
            match c[0].to_lowercase().starts_with("<br") {
                true => "\n",
                false => " ",
            }
        })
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Aplatit une carte HTML en une seule ligne de texte : les motifs de champs
/// (« Date limite : … ») se lisent au fil du texte, pas dans l'arbre.
pub(crate) fn txt(html: &str) -> String {
    // A notice written in Windows-1252 and served as UTF-8 turns its bullet into
    // U+0095, a control character no strip removes and no reader renders.
    let plat: String = lj_sources::html_strip::strip_html(html)
        .replace('\n', " ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
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
    use super::{desechapper, txt};

    #[test]
    fn a_field_escaped_twice_reads_as_text_and_keeps_its_line_breaks() {
        assert_eq!(
            desechapper("Magazine &amp;quot;Le Columérin&amp;quot; &amp;amp; Cie"),
            "Magazine \"Le Columérin\" & Cie"
        );
        assert_eq!(
            desechapper("Département de l&#039;Oise"),
            "Département de l'Oise"
        );
        assert_eq!(
            desechapper("Lot 1\ncomposites &amp;lt;br/&amp;gt;Les prestations"),
            "Lot 1\ncomposites\nLes prestations"
        );
        assert_eq!(
            desechapper("véhicules &amp;lt; 3,5 T\n\nsuite"),
            "véhicules < 3,5 T\n\nsuite"
        );
        assert_eq!(desechapper("R&D, sans entité"), "R&D, sans entité");
    }

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
