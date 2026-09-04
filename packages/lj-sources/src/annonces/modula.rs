use std::sync::LazyLock;
use std::time::Duration;

use futures::StreamExt;
use regex::Regex;

use lj_core::annonces::{echeance_ts, parse_date_fr, parse_heure, AnnonceBrute};

use super::{apres_balise, dedup_source_uid, txt, MoteurRun};
use crate::error::{Result, SourceError};

/// Les portails, en hôte canonique (sans `www.`) : c'est la forme que porte le
/// `source_uid`, donc la clé que le BOAMP donne en citant le DCE anonyme.
const PORTAILS: [&str; 6] = [
    "proxilegales.fr",
    "achatspublicscorse.com",
    "marches-demat.com",
    "marchespublics.pro",
    "antilles-legales.fr",
    "laprovencemarchespublics.com",
];

const PAR_PAGE: u64 = 200;
/// Garde-fou de pagination : 8 000 annonces par portail, deux ordres de grandeur
/// au-dessus du plus gros listing filtré.
const MAX_PAGES: u64 = 40;

fn listing_url(host: &str, page: u64) -> String {
    format!(
        "https://www.{host}/publisher_portail/public/annonce/afficherAnnonces.jsp\
         ?type_avis=tout&tri=marche.date_dern_modif%20desc&filtreType=marche.ALL_REF\
         &sSEOperatorValue=AND&bLaunchSearch=true&sIsAnnonceDemat=demat\
         &se_iMaxElementPerPage={PAR_PAGE}&se_bDisplayMapAnnounce=false&numPage={page}"
    )
}

/// La page anonyme du dossier de consultation — celle que l'arrêté du 14 avril
/// 2017 oblige à servir sans compte, et la seule URL stable du produit (le PDF
/// de l'avis est signé par un jeton de rendu). C'est mot pour mot la forme sous
/// laquelle les avis BOAMP citent le profil.
fn url_dce(host: &str, id: &str) -> String {
    format!(
        "https://www.{host}/publisher_portail/public/annonce/\
         consulterDCEHorsIdentification.jsp?iIdAffaire={id}"
    )
}

/// Décodage **ISO-8859-1**, l'encodage que le produit impose
/// (`Content-Type: text/html;charset=ISO-8859-1` ; `Accept-Charset: utf-8` n'y
/// change rien). Le client du workspace est bâti sans le décodeur de charset de
/// reqwest : `send_text` rendrait un `U+FFFD` par octet accentué. Les 256 octets
/// du latin-1 sont exactement les points de code U+0000..U+00FF, donc le mapping
/// direct **est** le décodage (même frontière que `ariane.rs`).
fn latin1(octets: &[u8]) -> String {
    octets.iter().map(|&o| o as char).collect()
}

/// Charge une page de listing. Mêmes reprises que `send_text` — 3 essais,
/// backoff ×2 sur 429/5xx — mais sur les octets, que `latin1` décode.
async fn charge(client: &reqwest::Client, url: String, quoi: &str) -> Result<String> {
    const ESSAIS: u32 = 3;
    let mut derniere = String::new();
    for essai in 0..ESSAIS {
        if essai > 0 {
            tokio::time::sleep(Duration::from_millis(500 << essai)).await;
        }
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) => return Ok(latin1(&b)),
                Err(e) => derniere = format!("lecture du corps: {e}"),
            },
            Ok(r) if r.status().as_u16() == 429 || r.status().is_server_error() => {
                derniere = format!("HTTP {}", r.status());
            }
            Ok(r) => return Err(SourceError::Invalid(format!("{quoi}: HTTP {}", r.status()))),
            Err(e) => derniere = e.to_string(),
        }
    }
    Err(SourceError::Invalid(format!(
        "{quoi}: {derniere} après {ESSAIS} essais"
    )))
}

/// Valeur d'un champ du bloc, du libellé en gras jusqu'à la fin de cellule
/// (`</td>`) ou de ligne (`</tr>`) : l'objet vit dans la cellule voisine de son
/// libellé, les autres champs dans la même.
fn champ(bloc: &str, motif: &Regex) -> Option<String> {
    motif
        .captures(bloc)
        .map(|c| txt(&c[1]))
        .filter(|v| !v.is_empty())
}

/// Acheteur et référence, portés ensemble par l'en-tête du bloc :
/// « MAIRIE DE SAINT-OMER - 2026/22-25 (AAPC) ». On retire le type d'avis en
/// queue, puis on coupe au **premier** séparateur : quand un en-tête en porte
/// plusieurs, c'est la référence qui est composée (« MAIRIE DE HOULLE - 21 - 017
/// - HOU »), jamais l'acheteur.
fn entete(bloc: &str) -> (Option<String>, Option<String>) {
    static ENTETE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)class="post-title-alt"[^>]*>(.*?)</span>"#).expect("regex en-tête valide")
    });
    // Le type d'avis publié : AAPC (appel public à concurrence), AATR
    // (attribution)…
    static TYPE_AVIS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\([A-Z]{3,6}\)\s*$").expect("regex type d'avis valide"));
    // Identifiant interne de l'éditeur, collé au nom de l'acheteur —
    // « MAIRIE DE ROUVROY (00180330) » : six ou huit chiffres selon le portail,
    // jamais les neuf d'un SIREN. Le seuil à cinq laisse passer un millésime
    // entre parenthèses, qui appartient au nom.
    static ID_EDITEUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\(\d{5,}\)\s*$").expect("regex id éditeur valide"));

    let Some(brut) = ENTETE.captures(bloc).map(|c| txt(&c[1])) else {
        return (None, None);
    };
    let titre = TYPE_AVIS.replace(&brut, "");
    let (acheteur, reference) = match titre.split_once(" - ") {
        Some((a, r)) => (a, Some(r.trim().to_string())),
        None => (titre.as_ref(), None),
    };
    let acheteur = ID_EDITEUR.replace(acheteur.trim(), "").trim().to_string();
    (
        (!acheteur.is_empty()).then_some(acheteur),
        reference.filter(|r| !r.is_empty()),
    )
}

/// Date française du produit — « 7 Aout 2026 à 11:30 heures ».
fn date_et_heure(valeur: &str) -> Option<(jiff::civil::Date, Option<jiff::civil::Time>)> {
    static DATE_HEURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(\d{1,2}\s+[\p{L}.]+\s+\d{4})(?:\s*à\s*(\d{1,2}[h:]\d{2}))?")
            .expect("regex date française valide")
    });
    let c = DATE_HEURE.captures(valeur)?;
    let date = parse_date_fr(&c[1])?;
    Some((date, c.get(2).and_then(|h| parse_heure(h.as_str()))))
}

/// Les consultations **natives** d'une page de listing. Les deux natures de
/// relais et les avis d'attribution n'ont pas de DCE anonyme à servir : ils
/// tombent sur ce seul test.
fn parse_page(html: &str, host: &str) -> Vec<AnnonceBrute> {
    static DCE_ANONYME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"popupTelechargementDCEAnonyme\.jsp[^"']*?[?&]idMarche=(\d+)"#)
            .expect("regex DCE anonyme valide")
    });
    static OBJET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<strong>\s*Objet\s*:\s*</strong>(.*?)</tr>").expect("regex objet valide")
    });
    static PROCEDURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<strong>\s*Type de procédure\s*:\s*</strong>(.*?)</td>")
            .expect("regex procédure valide")
    });
    static PUBLICATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<strong>\s*Date de publication\s*:\s*</strong>(.*?)</td>")
            .expect("regex publication valide")
    });
    // Une procédure restreinte borne la remise des candidatures, pas des plis.
    static ECHEANCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?s)<strong>\s*Date limite de reception des (?:plis|candidatures)\s*:\s*</strong>(.*?)</td>",
        )
        .expect("regex échéance valide")
    });

    let mut out = Vec::new();
    for bloc in html.split(r#"<div class="post""#).skip(1).map(apres_balise) {
        let Some(id) = DCE_ANONYME.captures(bloc).map(|c| c[1].to_string()) else {
            continue;
        };
        let (acheteur, reference) = entete(bloc);
        let url = url_dce(host, &id);
        out.push(AnnonceBrute {
            moteur: "modula",
            source_uid: format!("modula:{host}:{id}"),
            cles_jointure: Vec::new(),
            reference,
            acheteur,
            siren: None,
            objet: champ(bloc, &OBJET).unwrap_or_default(),
            nature: None,
            cpv: None,
            departement: None,
            date_publication: champ(bloc, &PUBLICATION)
                .and_then(|v| date_et_heure(&v))
                .map(|(d, _)| d),
            echeance: champ(bloc, &ECHEANCE)
                .and_then(|v| date_et_heure(&v))
                .map(|(d, h)| echeance_ts(d, h)),
            url_avis: Some(url.clone()),
            url_rc: None,
            url_dce: Some(url),
            url_depot: None,
            procedure: champ(bloc, &PROCEDURE),
        });
    }
    out
}

/// Un portail, page par page. Une page sans aucun bloc d'annonce est la fin de
/// la liste : le produit ne publie son total qu'en français, dans une phrase.
async fn enumerate_portail(client: &reqwest::Client, host: &str) -> Result<Vec<AnnonceBrute>> {
    let mut rows = Vec::new();
    for page in 0..MAX_PAGES {
        let html = charge(
            client,
            listing_url(host, page),
            &format!("modula {host} page {page}"),
        )
        .await?;
        if !html.contains(r#"<div class="post""#) {
            break;
        }
        rows.extend(parse_page(&html, host));
    }
    Ok(rows)
}

/// Ré-énumère les six portails. Un portail en échec est un échec compté, pas
/// la perte du run : ce sont six sites indépendants derrière une même
/// application.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let resultats: Vec<(&str, Result<Vec<AnnonceBrute>>)> =
        futures::stream::iter(PORTAILS.iter().map(|host| {
            let client = &client;
            async move { (*host, enumerate_portail(client, host).await) }
        }))
        .buffer_unordered(PORTAILS.len())
        .collect()
        .await;
    let mut run = MoteurRun {
        // Le total du produit compte les annonces légales, les avis
        // d'attribution et les relais des portails voisins — rien de
        // comparable au collecté.
        annonce: None,
        echecs: 0,
        hotes: Vec::new(),
        rows: Vec::new(),
    };
    for (host, res) in resultats {
        match res {
            Ok(rows) => {
                tracing::debug!(host, natifs = rows.len(), "modula : portail énuméré");
                run.rows.extend(rows);
            }
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(host, error = %e, "modula : portail en échec");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bloc natif réel : boutons de dépôt, DCE anonyme, dates en toutes lettres.
    const BLOC_NATIF: &str = r#"<div class="post" style="display:inline-block;width:100%;">
	<div class="post-title post-title-annonce">
		<table class="fullWidth"><tr><td>
			<span class="post-title-alt"><strong>MAIRIE DE SAINT-OMER - 2026/22-25</strong>&nbsp;(AAPC)</strong></span>
		</td></tr></table>
	</div>
	<style>#mps { height:17px; }</style>
	<div class="post-block">
	    <table cellpadding="0" cellspacing="0">
	        <tr>
	            <td class="right" style="vertical-align:top;"><strong>Objet : </strong></td>
	            <td style="text-align:left;">
	            	<div style="display:inline-block; vertical-align: bottom">
	            PRESTATION DE SERVICES DE CONSERVATION RESTAURATION DES OEUVRES DU SERVICE DES MUSEES ET DU PATRIMOINE DE SAINT-OMER - PROGRAMME 2027-2030
	             	</div>
	             </td>
	        </tr>
	        <tr>
	        	<td colspan="2" style="padding-top:4px;"><strong>Type de procédure : </strong>Procédure adaptée ouverte</td>
	        </tr>
	    </table>
	</div>
	<div class="post-block" style="float:left;">
		<table>
			<tr><td><strong>Date de publication :</strong><br>7 Aout 2026 à 11:30 heures</td></tr>
			<tr><td><strong>Date limite de reception des plis :</strong><br>16 Octobre 2026 à 12:00 heures</td></tr>
			<tr><td style="text-align:left"><div><strong>Temps restant :</strong><span class="rouge">
				<div class="enligne" id="joursTimer_36083">68</div><div class="enligne"> jours</div>
			</span></div></td></tr>
		</table>
	</div>
	<div style="float:right;" align=right>
		<div class="post-button" onClick="javascript:OuvrirPopup('/publisher_portail/PublicationPdfGeneric;jsessionid=F5190CD0?iIdMarche=36083&iIdPublicationType=1&id=AAPC_36083&sts=8a10097963c9d91b',800, 600,'menubar=no')">Visualiser l'avis</div>
		<div class="post-button" onClick='javascript:window.name="mainWindow";OuvrirPopup("/publisher_portail/public/annonce/popupTelechargementDCEAnonyme.jsp;jsessionid=F5190CD0?idMarche=36083",800,315,"menubar=no")'>Télécharger le DCE</div>
		<div class="post-button repondre" onClick='Redirect("/publisher_portail/public/inscription/inscriptionOrganisation.jsp")'>Répondre à la consultation</div>
	</div>
</div>"#;

    /// Bloc de relais réseau réel : la consultation est native sur un autre portail,
    /// et le seul bouton renvoie à la recherche du portail source.
    const BLOC_RELAIS_RESEAU: &str = r#"<div class="post" style="display:inline-block;width:100%;">
	<div class="post-title post-title-annonce">
		<table class="fullWidth"><tr><td>
			<span class="post-title-alt"><strong>CENTRE HOSPITALIER UNIVERSITAIRE ROUEN (96096498) - 21293721</strong>&nbsp;(AAPC)</strong></span>
		</td></tr></table>
	</div>
	<div class="post-block" style="float:left; width:95%">
		<table style="display:inline-block; width:60%">
			<tr><td><strong>Objet : </strong>CHU de Rouen - APPEL A MANIFESTATION D&#039;INTERET POUR LA MISE EN PLACE D&#039;UN ESPACE LIBRAIRIE TEMPORAIRE </td></tr>
			<tr><td><strong>Date de publication :</strong><br>8 Aout 2026 à 00:00 heures</td></tr>
			<tr><td><table><tr>
				<td style="vertical-align: middle;">Publié sur le site :  </td>
				<td><img src="/include/images/rossel/logo_import.png;jsessionid=D692F9E9" alt="n.c." style="width:70px;" /></td>
			</tr></table></td></tr>
		</table>
		<table style="display:inline-grid; text-align:right; width:16%" align="right">
			<tr><td>
				<button class="post-button" onclick="alert(sMessageRedirection);window.open('https://www.proxilegales.fr/publisher_portail/public/annonce/afficherAnnonces.jsp?filtreType=marche.ALL_REF&filtre=21293721&raisonSociale=CENTRE+HOSPITALIER+UNIVERSITAIRE+ROUEN+%2896096498%29&bLaunchSearch=true&se_iMaxElementPerPage=5')" style="width:138px;">Accéder au marché sur le site source</button>
			</td></tr>
		</table>
	</div>
</div>"#;

    /// Bloc de relais presse réel : l'annonce légale republiée. Identifiant haché,
    /// aucun bouton de dépôt.
    const BLOC_RELAIS_PRESSE: &str = r#"<div class="post" style="display:inline-block;width:100%;">
	<div class="post-title post-title-annonce">
		<table class="fullWidth"><tr><td>
			<span class="post-title-alt"><strong>MAIRIE DE WIMILLE (96042163) - 21276834</strong>&nbsp;(AAPC)</strong></span>
		</td></tr></table>
	</div>
	<div class="post-block">
	    <table cellpadding="0" cellspacing="0">
	        <tr>
	            <td class="right"><strong>Objet : </strong></td>
	            <td style="text-align:left;">
	            	<div style="display:inline-block; vertical-align: bottom">
	            Accord cadre travaux d&#039;entretien et petites r&eacute;parations des toitures
	             	</div>
	             </td>
	        </tr>
	        <tr>
	        	<td colspan="2"><strong>Type de procédure : </strong>Procedure Adaptee Ouverte (sup&eacute;rieur &agrave; 90 K&euro;)</td>
	        </tr>
	    </table>
	</div>
	<div class="post-block" style="float:left;">
		<table>
			<tr><td><strong>Date de publication :</strong><br>31 Juillet 2026 à 00:00 heures</td></tr>
			<tr><td><strong>Date limite :</strong><br>21 Septembre 2026 à 12:00 heures</td></tr>
			<tr><td style="text-align:left"><table><tr><td><strong>Publié sur :</strong></td>
				<td><img src="/templatesClients/rossel/images/logo_annonces.png;jsessionid=F5190CD0?v=2.0" width="70px"></td>
			</tr></table></td></tr>
		</table>
	</div>
	<div style="float:right;" align=right>
		<div class="post-button" onClick='javascript:window.name="mainWindow";OuvrirPopup("/publisher_portail/public/annonce/displayPetiteAnnonceDetail.jsp;jsessionid=F5190CD0?iIdMarche=1d4ed465be393a66",800,600,"menubar=no")'>Visualiser l'annonce</div>
	</div>
</div>"#;

    #[test]
    fn seule_la_consultation_native_est_ingeree() {
        let page = format!("{BLOC_RELAIS_PRESSE}{BLOC_NATIF}{BLOC_RELAIS_RESEAU}");
        let rows = parse_page(&page, "proxilegales.fr");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_uid, "modula:proxilegales.fr:36083");
    }

    #[test]
    fn un_relais_reseau_ne_rend_rien() {
        assert!(parse_page(BLOC_RELAIS_RESEAU, "marches-demat.com").is_empty());
    }

    #[test]
    fn un_relais_presse_ne_rend_rien() {
        assert!(parse_page(BLOC_RELAIS_PRESSE, "proxilegales.fr").is_empty());
    }

    #[test]
    fn un_avis_d_attribution_ne_rend_rien() {
        let aatr = BLOC_NATIF
            .replace("popupTelechargementDCEAnonyme.jsp", "DownloadFilePublisher")
            .replace("(AAPC)", "(AATR)");
        assert!(parse_page(&aatr, "proxilegales.fr").is_empty());
    }

    #[test]
    fn le_bloc_natif_rend_tous_ses_champs() {
        let rows = parse_page(BLOC_NATIF, "proxilegales.fr");
        let r = &rows[0];
        assert_eq!(r.moteur, "modula");
        assert_eq!(r.acheteur.as_deref(), Some("MAIRIE DE SAINT-OMER"));
        assert_eq!(r.reference.as_deref(), Some("2026/22-25"));
        assert_eq!(
            r.objet,
            "PRESTATION DE SERVICES DE CONSERVATION RESTAURATION DES OEUVRES DU SERVICE \
             DES MUSEES ET DU PATRIMOINE DE SAINT-OMER - PROGRAMME 2027-2030"
        );
        assert_eq!(r.procedure.as_deref(), Some("Procédure adaptée ouverte"));
        assert_eq!(
            r.date_publication.map(|d| d.to_string()).as_deref(),
            Some("2026-08-07")
        );
        // 16 octobre 2026 à 12:00, Europe/Paris.
        assert_eq!(
            r.echeance.map(|t| t.to_string()).as_deref(),
            Some("2026-10-16T10:00:00Z")
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some(
                "https://www.proxilegales.fr/publisher_portail/public/annonce/\
                 consulterDCEHorsIdentification.jsp?iIdAffaire=36083"
            )
        );
        assert_eq!(r.url_dce, r.url_avis);
    }

    #[test]
    fn l_identifiant_est_local_au_portail() {
        let a = parse_page(BLOC_NATIF, "proxilegales.fr");
        let b = parse_page(BLOC_NATIF, "marchespublics.pro");
        assert_ne!(a[0].source_uid, b[0].source_uid);
    }

    #[test]
    fn l_entete_coupe_au_premier_separateur() {
        let entete_de = |t: &str| entete(&format!(r#"<span class="post-title-alt">{t}</span>"#));
        assert_eq!(
            entete_de("<strong>MAIRIE DE HOULLE - 21 - 017 - HOU</strong>&nbsp;(AAPC)"),
            (
                Some("MAIRIE DE HOULLE".to_string()),
                Some("21 - 017 - HOU".to_string())
            )
        );
        assert_eq!(
            entete_de(
                "<strong>MAIRIE DE ROUVROY (00180330) - M62724_2026_004</strong>&nbsp;(AAPC)"
            ),
            (
                Some("MAIRIE DE ROUVROY".to_string()),
                Some("M62724_2026_004".to_string())
            )
        );
        assert_eq!(
            entete_de("<strong>SM3A - SM3A - 2026.010</strong>&nbsp;(AAPC)"),
            (
                Some("SM3A".to_string()),
                Some("SM3A - 2026.010".to_string())
            )
        );
    }

    #[test]
    fn le_listing_se_lit_en_latin1() {
        assert_eq!(
            latin1(b"Type de proc\xe9dure : 16 Octobre 2026 \xe0 12:00 heures"),
            "Type de procédure : 16 Octobre 2026 à 12:00 heures"
        );
    }

    #[test]
    fn l_echeance_des_candidatures_est_lue_comme_celle_des_plis() {
        let restreinte = BLOC_NATIF.replace(
            "Date limite de reception des plis",
            "Date limite de reception des candidatures",
        );
        let rows = parse_page(&restreinte, "proxilegales.fr");
        assert_eq!(
            rows[0].echeance.map(|t| t.to_string()).as_deref(),
            Some("2026-10-16T10:00:00Z")
        );
    }
}
