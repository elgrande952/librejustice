//! Mails transactionnels d'auth, par l'API HTTP Resend ou par le relais.
//!
//! Le domaine est vérifié chez Resend, SPF/DKIM/DMARC alignés : l'émetteur change,
//! le DNS non. Les liens atterrissent sur le front.

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::{AsyncTransport, Message};
use lj_mail::{rendre_html, Gabarit};
use serde_json::json;

use crate::error::{ApiError, Result};
use crate::state::AppState;

pub const LOG_TARGET: &str = "lj_api::mail";

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

/// Envoie le mail de confirmation d'inscription.
pub async fn send_confirmation(state: &AppState, to: &str, lien: &str, voie: Voie) -> Result<()> {
    rendre(state, to, lien, lj_mail::confirmation(), voie).await
}

/// Envoie le lien qui ouvre la session sans mot de passe.
pub async fn send_connexion(state: &AppState, to: &str, lien: &str, voie: Voie) -> Result<()> {
    rendre(state, to, lien, lj_mail::connexion(to), voie).await
}

/// Envoie le lien qui définit le mot de passe. Sert au compte qui l'a oublié
/// comme à celui qui n'en a jamais eu (inscrit par Google) : « définit » vaut
/// dans les deux cas, « réinitialise » ne vaudrait que dans le premier.
pub async fn send_recovery(state: &AppState, to: &str, lien: &str, voie: Voie) -> Result<()> {
    rendre(state, to, lien, lj_mail::recuperation(to), voie).await
}

/// Envoie à l'adresse actuelle la demande de confirmation du changement.
pub async fn send_email_change_ancienne(
    state: &AppState,
    to: &str,
    nouvelle: &str,
    lien: &str,
    voie: Voie,
) -> Result<()> {
    rendre(
        state,
        to,
        lien,
        lj_mail::changement_ancienne(nouvelle),
        voie,
    )
    .await
}

/// Envoie à la nouvelle adresse le lien qui prouve sa possession.
pub async fn send_email_change_nouvelle(
    state: &AppState,
    to: &str,
    lien: &str,
    voie: Voie,
) -> Result<()> {
    rendre(state, to, lien, lj_mail::changement_nouvelle(), voie).await
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
        tracing::error!(%erreur, "relay send refused");
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
        let quota = res
            .headers()
            .get("x-resend-daily-quota")
            .and_then(|v| v.to_str().ok());
        tracing::info!(target: LOG_TARGET, daily_quota = quota, "resend: mail sent");
        return Ok(());
    }
    let status = res.status();
    let corps = res.text().await.unwrap_or_default();
    // Le corps de Resend nomme la cause (domaine non vérifié, quota du jour,
    // adresse refusée) : sans lui, un échec d'envoi est indistinguable du suivant.
    tracing::error!(status = status.as_u16(), corps = %corps, "mail send refused");
    Err(ApiError::BadGateway("envoi_refuse".into()))
}
