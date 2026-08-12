//! Chargement de données de la page décision (Resources).

use leptos::prelude::*;
use lj_dtos::{DecisionDetail, DecisionPartyDto, SimilarDecisionHit};
use serde::{Deserialize, Serialize};

use crate::api::{ApiClient, ApiError};

/// Erreur de page sérialisable (le `Resource` la stream SSR → hydrate ; `ApiError`
/// n'est pas `Serialize`, on porte status + message).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageError {
    pub status: u16,
    pub message: String,
}

impl From<ApiError> for PageError {
    fn from(err: ApiError) -> Self {
        Self {
            status: err.status,
            message: err.message,
        }
    }
}

/// Voisins résolus, deux volets (principe / espèce). Ne porte JAMAIS d'erreur
/// en rejet — l'erreur est repliée en `error: Option<String>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarResult {
    pub principe: Vec<SimilarDecisionHit>,
    pub espece: Vec<SimilarDecisionHit>,
    pub abstained: bool,
    pub error: Option<String>,
}

fn client() -> ApiClient {
    ApiClient::from_context()
}

/// Charge le détail (bloquant SSR pour le SEO). Le résumé est garanti en base et
/// porté par le détail, donc présent dans le HTML SSR (description / synthèse)
/// sans génération à la volée.
pub async fn fetch_detail(id: String) -> Result<DecisionDetail, PageError> {
    if id.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Identifiant invalide".to_string(),
        });
    }
    let detail = client().fetch_decision(&id).await?;
    Ok(detail)
}

/// Charge les voisins (non bloquant, streamé via `<Suspense>`). Ne rejette
/// jamais : erreur repliée en `error`.
pub async fn fetch_similar(id: String) -> SimilarResult {
    if id.trim().is_empty() {
        return SimilarResult {
            principe: Vec::new(),
            espece: Vec::new(),
            abstained: false,
            error: None,
        };
    }
    match client().fetch_similar_decisions(&id).await {
        Ok(response) => SimilarResult {
            principe: response.principe,
            espece: response.espece,
            abstained: response.abstained,
            error: None,
        },
        Err(err) => SimilarResult {
            principe: Vec::new(),
            espece: Vec::new(),
            abstained: false,
            error: Some(err.message),
        },
    }
}

/// Acteurs (parties + conseils) d'une décision, encart « Parties ». Non
/// bloquant, streamé ; ne rejette jamais — erreur repliée en `error`. Encart
/// masqué si la liste est vide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartiesResult {
    pub parties: Vec<DecisionPartyDto>,
    pub error: Option<String>,
}

/// Charge les parties (streamé via `<Suspense>`). Erreur repliée en `error`.
pub async fn fetch_parties(id: String) -> PartiesResult {
    if id.trim().is_empty() {
        return PartiesResult {
            parties: Vec::new(),
            error: None,
        };
    }
    match client().fetch_decision_parties(&id).await {
        Ok(response) => PartiesResult {
            parties: response.parties,
            error: None,
        },
        Err(err) => PartiesResult {
            parties: Vec::new(),
            error: Some(err.message),
        },
    }
}

/// Adapte un futur de fetch en futur `Send`, requis par `Resource::new[_blocking]`.
///
/// Côté SSR (reqwest tokio, auth no-op) le futur est déjà `Send` → identité.
/// Côté wasm (single-thread) reqwest/`fetch` et le shim auth (`JsFuture`) sont
/// `!Send` ; `SendWrapper` les rend `Send` sans risque (un seul thread).
#[cfg(feature = "ssr")]
pub fn sendable<F>(fut: F) -> F
where
    F: std::future::Future + Send,
{
    fut
}

#[cfg(feature = "hydrate")]
pub fn sendable<F>(fut: F) -> send_wrapper::SendWrapper<F>
where
    F: std::future::Future,
{
    send_wrapper::SendWrapper::new(fut)
}

/// Récupère le segment `id` de la route `/decision/:id`. Vide si absent.
pub fn decision_id() -> Signal<String> {
    let params = leptos_router::hooks::use_params_map();
    Signal::derive(move || params.read().get("id").unwrap_or_default())
}
