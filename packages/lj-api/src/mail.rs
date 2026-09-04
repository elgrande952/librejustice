//! Mails transactionnels d'auth, par l'API HTTP Resend ou par le relais.
//!
//! Le domaine est vérifié chez Resend, SPF/DKIM/DMARC alignés : l'émetteur change,
//! le DNS non. Les liens atterrissent sur le front. Un seul gabarit : la charte
//! n'a qu'un endroit où vivre, chaque mail n'apporte que son texte.

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::{AsyncTransport, Message};
use serde_json::json;

use crate::error::{ApiError, Result};
use crate::state::AppState;

const LAYOUT: &str = include_str!("../templates/mail/layout.html");
const NOTICE: &str = include_str!("../templates/mail/notice.html");

/// Le relais porte les envois internes : ils n'usent pas la réputation
/// d'émission dont dépendent les vrais destinataires, qu'un Outlook personnel
/// juge durement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voie {
    Resend,
    Relais,
}

impl Voie {
    pub fn pour(interne: bool) -> Self {
        if interne {
            Self::Relais
        } else {
            Self::Resend
        }
    }
}

/// Ce qu'un mail apporte au gabarit commun, sujet compris. Le lien voyage à
/// part : lui seul est construit côté serveur.
pub struct Gabarit {
    pub sujet: String,
    pub corps: String,
    pub notice: String,
    pub bouton: String,
    /// Première phrase du pied. Un mail d'auth ne se répond pas, un courrier
    /// d'information se désabonne : la charte ne tranche pas pour les deux.
    pub pied: String,
}

/// Dans la notice et non dans le gabarit : celui-ci rend aussi du courrier qui
/// ne porte aucun lien à durée de vie.
const VALIDITE: &str = "Lien valable 1 heure.";
/// `noreply@` has no mailbox: without the contact address here, an auth mail
/// would offer no way to reach anyone.
const PIED_AUTH: &str = "Message automatique, ne pas répondre. Pour nous joindre, \
     <a href=\"mailto:contact@librejustice.fr\" \
     style=\"color:#892122;text-decoration:underline;\">contact@librejustice.fr</a>.";

fn confirmation() -> Gabarit {
    Gabarit {
        sujet: "Votre inscription à LibreJustice".into(),
        corps: "Un compte LibreJustice a été créé avec cette adresse.".into(),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette inscription, \
             ignorez cet e-mail."
        ),
        bouton: "Confirmer l'adresse".into(),
        pied: PIED_AUTH.into(),
    }
}

fn connexion(compte: &str) -> Gabarit {
    Gabarit {
        sujet: "Votre connexion à LibreJustice".into(),
        corps: format!(
            "Ce lien ouvre la session du compte {}.",
            adresse_html(compte)
        ),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette demande, ignorez cet e-mail. \
             Personne ne s'est connecté."
        ),
        bouton: "Me connecter".into(),
        pied: PIED_AUTH.into(),
    }
}

fn recuperation(compte: &str) -> Gabarit {
    Gabarit {
        sujet: "Votre mot de passe LibreJustice".into(),
        corps: format!(
            "Ce lien définit le mot de passe du compte {}.",
            adresse_html(compte)
        ),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette demande, ignorez cet e-mail. \
             Votre compte n'a pas été modifié."
        ),
        bouton: "Choisir un mot de passe".into(),
        pied: PIED_AUTH.into(),
    }
}

fn changement_ancienne(nouvelle: &str) -> Gabarit {
    Gabarit {
        sujet: "Changement d'adresse de votre compte LibreJustice".into(),
        corps: format!(
            "Votre compte passerait à l'adresse {}. \
             Un second lien partira ensuite vers cette adresse.",
            adresse_html(nouvelle)
        ),
        notice: format!(
            "{VALIDITE} Si vous n'êtes pas à l'origine de cette demande, ignorez cet e-mail. \
             Votre adresse n'a pas été modifiée."
        ),
        bouton: "Confirmer la demande".into(),
        pied: PIED_AUTH.into(),
    }
}

fn changement_nouvelle() -> Gabarit {
    Gabarit {
        sujet: "Votre nouvelle adresse LibreJustice".into(),
        corps: "Un compte LibreJustice change d'adresse pour celle-ci. \
                Ce lien termine le changement."
            .into(),
        notice: format!(
            "{VALIDITE} Si ce compte n'est pas le vôtre, ignorez cet e-mail. \
             Rien ne sera modifié."
        ),
        bouton: "Confirmer l'adresse".into(),
        pied: PIED_AUTH.into(),
    }
}

/// Un mail rendu sans être envoyé, pour relecture.
pub struct Apercu {
    pub nom: &'static str,
    pub sujet: String,
    pub html: String,
}

/// Tous les mails d'authentification, rendus avec une adresse et un lien
/// d'exemple. Le rendu vient d'ici, sans quoi relire la charte ne prouverait
/// rien sur ce qui part.
pub fn apercus(exemple: &str, lien: &str) -> Vec<Apercu> {
    [
        ("Confirmation d'inscription", confirmation()),
        ("Lien de connexion", connexion(exemple)),
        ("Mot de passe", recuperation(exemple)),
        (
            "Changement d'adresse, ancienne",
            changement_ancienne(exemple),
        ),
        ("Changement d'adresse, nouvelle", changement_nouvelle()),
    ]
    .into_iter()
    .map(|(nom, gabarit)| Apercu {
        nom,
        sujet: gabarit.sujet.clone(),
        html: rendre_html(&gabarit, lien),
    })
    .collect()
}

/// Envoie le mail de confirmation d'inscription.
pub async fn send_confirmation(state: &AppState, to: &str, lien: &str, voie: Voie) -> Result<()> {
    rendre(state, to, lien, confirmation(), voie).await
}

/// Envoie le lien qui ouvre la session sans mot de passe.
pub async fn send_connexion(state: &AppState, to: &str, lien: &str, voie: Voie) -> Result<()> {
    rendre(state, to, lien, connexion(to), voie).await
}

/// Envoie le lien qui définit le mot de passe. Sert au compte qui l'a oublié
/// comme à celui qui n'en a jamais eu (inscrit par Google) : « définit » vaut
/// dans les deux cas, « réinitialise » ne vaudrait que dans le premier.
pub async fn send_recovery(state: &AppState, to: &str, lien: &str, voie: Voie) -> Result<()> {
    rendre(state, to, lien, recuperation(to), voie).await
}

/// Envoie à l'adresse actuelle la demande de confirmation du changement.
pub async fn send_email_change_ancienne(
    state: &AppState,
    to: &str,
    nouvelle: &str,
    lien: &str,
    voie: Voie,
) -> Result<()> {
    rendre(state, to, lien, changement_ancienne(nouvelle), voie).await
}

/// Envoie à la nouvelle adresse le lien qui prouve sa possession.
pub async fn send_email_change_nouvelle(
    state: &AppState,
    to: &str,
    lien: &str,
    voie: Voie,
) -> Result<()> {
    rendre(state, to, lien, changement_nouvelle(), voie).await
}

/// Laissée nue, Gmail transforme l'adresse en `mailto:` bleu souligné qui
/// concurrence le bouton (constaté le 22/08 sur un envoi réel). L'ancre posée
/// d'avance tient la charte : un client ne relie pas ce qui est déjà un lien.
fn adresse_html(brute: &str) -> String {
    let e = echapper(brute);
    format!(
        "<a href=\"mailto:{e}\" \
         style=\"color:#051223;text-decoration:none;font-weight:700;\">{e}</a>"
    )
}

/// L'adresse est reprise dans le corps du mail : elle vient de la requête, donc
/// elle s'échappe avant d'entrer dans du HTML.
fn echapper(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Rend le gabarit dans la charte commune, sans rien envoyer.
pub fn rendre_html(gabarit: &Gabarit, lien: &str) -> String {
    // Vide, le bloc de notice rendrait son filet gauche et sa marge.
    let notice = if gabarit.notice.is_empty() {
        String::new()
    } else {
        NOTICE.replace("{{NOTICE}}", &gabarit.notice)
    };
    // `{{CORPS}}` en dernier : lui seul porte du texte venu de la requête, donc
    // lui seul pourrait y réinjecter un marqueur.
    LAYOUT
        .replace("{{SUJET}}", &gabarit.sujet)
        .replace("{{LIEN}}", lien)
        .replace("{{BOUTON}}", &gabarit.bouton)
        .replace("{{NOTICE_BLOC}}", &notice)
        .replace("{{PIED}}", &gabarit.pied)
        .replace("{{CORPS}}", &gabarit.corps)
}

async fn rendre(
    state: &AppState,
    to: &str,
    lien: &str,
    gabarit: Gabarit,
    voie: Voie,
) -> Result<()> {
    let html = rendre_html(&gabarit, lien);
    match voie {
        Voie::Resend => resend(state, to, &gabarit.sujet, &html).await,
        Voie::Relais => relais(state, to, &gabarit.sujet, &html).await,
    }
}

/// Le relais n'annonce pas `STARTTLS` : le lien qui y mène est déjà chiffré.
async fn relais(state: &AppState, to: &str, sujet: &str, html: &str) -> Result<()> {
    let Some(mailer) = state.mailer.clone() else {
        return Err(ApiError::Unavailable("relais_non_configure".into()));
    };
    let adresse = |valeur: &str| {
        valeur
            .parse::<Mailbox>()
            .map_err(|e| ApiError::Internal(format!("adresse illisible: {e}")))
    };
    let message = Message::builder()
        .from(adresse(&state.settings.mail_from)?)
        .to(adresse(to)?)
        .subject(sujet)
        .header(ContentType::TEXT_HTML)
        .body(html.to_string())
        .map_err(|e| ApiError::Internal(format!("message irrecevable: {e}")))?;
    if let Err(erreur) = mailer.send(message).await {
        tracing::error!(%erreur, "envoi par le relais refusé");
        return Err(ApiError::BadGateway("envoi_refuse".into()));
    }
    Ok(())
}

async fn resend(state: &AppState, to: &str, sujet: &str, html: &str) -> Result<()> {
    if state.settings.resend_api_key.is_empty() {
        return Err(ApiError::Unavailable("mail_non_configure".into()));
    }
    let res = reqwest::Client::new()
        .post("https://api.resend.com/emails")
        .bearer_auth(&state.settings.resend_api_key)
        .json(&json!({
            "from": state.settings.mail_from,
            "to": [to],
            "subject": sujet,
            "html": html,
        }))
        .send()
        .await
        .map_err(|e| ApiError::BadGateway(format!("resend injoignable: {e}")))?;

    if res.status().is_success() {
        return Ok(());
    }
    let status = res.status();
    let corps = res.text().await.unwrap_or_default();
    // Le corps de Resend nomme la cause (domaine non vérifié, quota du jour,
    // adresse refusée) : sans lui, un échec d'envoi est indistinguable du suivant.
    tracing::error!(status = status.as_u16(), corps = %corps, "envoi de mail refusé");
    Err(ApiError::BadGateway("envoi_refuse".into()))
}
