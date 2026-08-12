//! Client Mistral : chat completions et OCR document, sur un pool de clés.
//!
//! Le débit n'est pas un réglage. Chaque appel passe par le registre
//! ([`crate::ratelimit`]), qui choisit une clé capable de payer et attend sinon.
//! Un appelant à échéance coupe l'attente par son `timeout_at`.
//!
//! Un client = un modèle = un seau, donc un registre par instance. Deux clients
//! sur deux **alias** du même modèle en tiendraient deux pour un seul seau ;
//! `GET /v1/models` déclare le graphe.

use anyhow::{Context, Result};
use base64::Engine;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::{SpanBackendWithUrl, TracingMiddleware};
use serde_json::json;

use crate::ratelimit::{now_unix, Ledger, Quota};

const MISTRAL_URL: &str = "https://api.mistral.ai/v1/chat/completions";
const OCR_URL: &str = "https://api.mistral.ai/v1/ocr";
/// L'OCR fixe son modèle ; celui du client reste le modèle chat.
const OCR_MODEL: &str = "mistral-ocr-latest";
const MAX_RETRIES: u32 = 5;
/// Back-off des pannes de service (5xx, transport) : ~2 s doublées à chaque cran.
const RETRY_BASE_S: f64 = 2.0;
const RETRYABLE_STATUSES: &[u16] = &[429, 500, 502, 503, 504];

/// Pas de retry interne — l'appelant décide (OCR : [`ocr_with_retry`]).
pub struct MistralClient {
    http: ClientWithMiddleware,
    keys: Vec<String>,
    /// Même index que `keys`.
    fingerprints: Vec<String>,
    model: String,
    idx: std::sync::atomic::AtomicUsize,
    /// Indices des clés refusées (401), sautées par le round-robin.
    dead: std::sync::Mutex<std::collections::HashSet<usize>>,
    /// Refus pas encore remontés, drainés par
    /// [`drain_spent_fingerprints`](Self::drain_spent_fingerprints).
    unreported: std::sync::Mutex<Vec<usize>>,
    ledger: Ledger,
}

impl MistralClient {
    /// `keys` non vide.
    pub fn new(keys: Vec<String>, model: String) -> Result<Self> {
        Self::build(keys, model)
    }

    fn build(keys: Vec<String>, model: String) -> Result<Self> {
        if keys.is_empty() {
            anyhow::bail!("MistralClient: keys must be non-empty");
        }
        let http = ClientBuilder::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .context("mistral: client HTTP")?,
        )
        .with(TracingMiddleware::<SpanBackendWithUrl>::new())
        .build();
        // Deux instances ne démarrent pas sur la même clé.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        let ledger = Ledger::new(keys.len());
        Ok(Self {
            http,
            idx: std::sync::atomic::AtomicUsize::new(seed % keys.len()),
            fingerprints: keys.iter().map(|k| key_fingerprint(k)).collect(),
            keys,
            model,
            dead: std::sync::Mutex::new(std::collections::HashSet::new()),
            unreported: std::sync::Mutex::new(Vec::new()),
            ledger,
        })
    }

    /// Départ round-robin, refusées retirées ; le registre prend la première qui
    /// peut payer. Pool entièrement refusé ⇒ ordre complet, une clé peut
    /// redevenir valide.
    fn key_order(&self) -> Vec<usize> {
        use std::sync::atomic::Ordering::Relaxed;
        let n = self.keys.len();
        let start = self.idx.fetch_add(1, Relaxed) % n;
        let all = || (0..n).map(|k| (start + k) % n);
        let dead = self.dead.lock().expect("mutex dead non empoisonné");
        let live: Vec<usize> = all().filter(|i| !dead.contains(i)).collect();
        if live.is_empty() {
            all().collect()
        } else {
            live
        }
    }

    fn mark_dead(&self, key_idx: usize) {
        if !self
            .dead
            .lock()
            .expect("mutex dead non empoisonné")
            .insert(key_idx)
        {
            return;
        }
        self.unreported
            .lock()
            .expect("mutex unreported non empoisonné")
            .push(key_idx);
        tracing::warn!(
            fingerprint = %self.fingerprints[key_idx],
            "mistral_key_dead_401"
        );
    }

    /// Aligne les clés écartées sur la liste de l'appelant, qui fait autorité.
    ///
    /// Remplace, n'ajoute pas : un 401 constaté en cours de route est reversé au
    /// magasin durable avant la fin de la requête
    /// ([`Self::drain_spent_fingerprints`]). Unir ferait de ce jeu un cliquet
    /// qu'aucune remise en service ne desserre tant que le processus vit.
    pub fn set_dead_fingerprints(&self, fingerprints: &[String]) {
        let mut dead = self.dead.lock().expect("mutex dead non empoisonné");
        *dead = self
            .fingerprints
            .iter()
            .enumerate()
            .filter(|(_, fp)| fingerprints.contains(fp))
            .map(|(i, _)| i)
            .collect();
    }

    pub fn live_key_count(&self) -> usize {
        self.keys.len() - self.dead.lock().expect("mutex dead non empoisonné").len()
    }

    /// Refus constatés depuis le dernier appel. Draine : un client de longue vie
    /// ne doit pas faire réécrire la même ligne à chaque requête.
    pub fn drain_spent_fingerprints(&self) -> Vec<String> {
        let mut unreported = self
            .unreported
            .lock()
            .expect("mutex unreported non empoisonné");
        unreported
            .drain(..)
            .map(|i| self.fingerprints[i].clone())
            .collect()
    }

    /// Borne de rotation : une passe complète du pool.
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Contenu brut du message ; le parsing incombe à l'appelant.
    /// `prompt_cache_key` amortit le prompt système sur des appels répétés.
    pub async fn chat(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
        prompt_cache_key: Option<&str>,
    ) -> Result<String, MistralError> {
        self.complete(
            self.chat_payload(system, user, max_tokens, prompt_cache_key),
            system.len() + user.len(),
        )
        .await
    }

    fn chat_payload(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
        prompt_cache_key: Option<&str>,
    ) -> serde_json::Value {
        let mut payload = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0,
            "stream": false,
        });
        if let Some(max_tokens) = max_tokens {
            payload["max_tokens"] = json!(max_tokens);
        }
        if let Some(key) = prompt_cache_key {
            payload["prompt_cache_key"] = json!(key);
        }
        payload
    }

    async fn complete(
        &self,
        payload: serde_json::Value,
        prompt_chars: usize,
    ) -> Result<String, MistralError> {
        let body = self.post_raw(MISTRAL_URL, payload, prompt_chars).await?;
        Ok(body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string())
    }

    /// PDF → markdown, pages concaténées. Non-déterministe : à cacher en amont
    /// (clé = checksum du PDF).
    pub async fn ocr(&self, pdf_bytes: &[u8]) -> Result<String, MistralError> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(pdf_bytes);
        let payload = json!({
            "model": OCR_MODEL,
            "document": {
                "type": "document_url",
                "document_url": format!("data:application/pdf;base64,{b64}"),
            },
            "table_format": "markdown",
            "include_image_base64": false,
            "extract_header": true,
            "extract_footer": true,
        });
        // Coût nul : l'OCR se facture à la page, la taille du base64 ne prédit
        // rien. Seul le compteur de requêtes gate ; l'en-tête corrige ensuite.
        let body = self.post_raw(OCR_URL, payload, 0).await?;
        let markdown = body["pages"]
            .as_array()
            .map(|pages| {
                pages
                    .iter()
                    .map(|p| p["markdown"].as_str().unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(markdown)
    }

    /// `prompt_chars` = taille du texte facturable, pour l'estimation de coût.
    async fn post_raw(
        &self,
        url: &str,
        payload: serde_json::Value,
        prompt_chars: usize,
    ) -> Result<serde_json::Value, MistralError> {
        // Un appelant à échéance coupe ce sommeil par son `timeout_at` ; la
        // réservation est alors rendue par `Drop`.
        let reservation = loop {
            match self
                .ledger
                .reserve(&self.key_order(), prompt_chars, now_unix())
            {
                Ok(reservation) => break reservation,
                Err(wait) => tokio::time::sleep(wait).await,
            }
        };
        let key_idx = reservation.key_idx();
        let response = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.keys[key_idx]))
            .json(&payload)
            .send()
            .await
            .map_err(MistralError::Middleware)?;
        // Un refus n'annonce que le compteur épuisé : c'est justement celui qui
        // doit arrêter d'émettre.
        let quota = Quota::from_headers(response.headers());
        reservation.settle(quota, now_unix());
        let status = response.status().as_u16();
        if !response.status().is_success() {
            // 401 : clé refusée, sautée ensuite.
            if status == 401 {
                self.mark_dead(key_idx);
            } else if status == 429 {
                // Quelle ressource a manqué ne se déduit pas du volume émis :
                // seule la réponse le dit.
                tracing::warn!(
                    key = %self.fingerprints[key_idx],
                    req_left = ?quota.req.map(|c| c.left),
                    tok_left = ?quota.tok.map(|c| c.left),
                    "mistral_rate_limited"
                );
            }
            return Err(MistralError::Status(status));
        }
        response.json().await.map_err(MistralError::Http)
    }
}

/// Empreinte xxh3-64 hex d'une clé API.
pub fn key_fingerprint(key: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(key.as_bytes()))
}

#[derive(Debug)]
pub enum MistralError {
    /// Réponse HTTP non-2xx (code de statut).
    Status(u16),
    /// Erreur de transport (réseau, timeout, parse JSON).
    Http(reqwest::Error),
    /// Erreur de la chaine middleware reqwest (envoi de la requete).
    Middleware(reqwest_middleware::Error),
}

impl std::fmt::Display for MistralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MistralError::Status(code) => write!(f, "mistral status {code}"),
            MistralError::Http(e) => write!(f, "mistral transport: {e}"),
            MistralError::Middleware(e) => write!(f, "mistral transport: {e}"),
        }
    }
}

impl std::error::Error for MistralError {}

pub fn is_retryable_status(code: u16) -> bool {
    RETRYABLE_STATUSES.contains(&code)
}

/// Back-off avec jitter ±25 %. `rand01` ∈ [0, 1) injecté (testable).
pub fn backoff_delay_s(attempt: u32, rand01: f64) -> f64 {
    let backoff = RETRY_BASE_S * 2f64.powi(attempt as i32);
    backoff * (0.75 + rand01 * 0.5)
}

/// Tirage non cryptographique dérivé de l'horloge.
pub fn rand01() -> f64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64)
        / 1_000_000_000.0
}

/// Trois régimes d'échec : **401** retente aussitôt sur la clé suivante,
/// **429** retente aussitôt car l'admission fera l'attente,
/// **5xx / transport** attend un back-off. Tout autre 4xx remonte.
pub async fn ocr_with_retry(
    client: &MistralClient,
    pdf_bytes: &[u8],
    label: &str,
) -> Result<String, MistralError> {
    let mut dead_keys = 0usize;
    let mut throttled = 0u32;
    let mut transient = 0u32;
    loop {
        match client.ocr(pdf_bytes).await {
            Ok(out) => return Ok(out),
            Err(MistralError::Status(401)) => {
                dead_keys += 1;
                if dead_keys >= client.key_count() {
                    return Err(MistralError::Status(401)); // pool entièrement refusé
                }
                tracing::warn!(label, dead_keys, "mistral_ocr_key_dead_rotate");
            }
            Err(MistralError::Status(429)) => {
                if throttled >= MAX_RETRIES {
                    return Err(MistralError::Status(429));
                }
                throttled += 1;
                tracing::warn!(label, attempt = throttled, "mistral_ocr_throttled_retry");
            }
            Err(MistralError::Status(code)) if !is_retryable_status(code) => {
                return Err(MistralError::Status(code));
            }
            Err(err) => {
                if transient >= MAX_RETRIES {
                    return Err(err);
                }
                let delay = backoff_delay_s(transient, rand01());
                transient += 1;
                tracing::warn!(
                    label,
                    attempt = transient,
                    max = MAX_RETRIES,
                    backoff = delay,
                    error = %err,
                    "mistral_ocr_retry"
                );
                tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_jitters() {
        // base 2 s : 1ʳᵉ tentative ∈ [1,5 ; 2,5] s selon jitter.
        assert!(backoff_delay_s(0, 0.0) >= 1.5 && backoff_delay_s(0, 1.0) <= 2.5);
        assert!(backoff_delay_s(2, 0.5) > backoff_delay_s(0, 0.5));
    }

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(404));
    }

    fn keys() -> Vec<String> {
        vec!["a".into(), "b".into(), "c".into()]
    }

    #[test]
    fn round_robin_advances_each_call() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        assert_eq!(c.key_order(), vec![0, 1, 2]);
        assert_eq!(c.key_order(), vec![1, 2, 0]);
        assert_eq!(c.key_order(), vec![2, 0, 1]);
        assert_eq!(c.key_order(), vec![0, 1, 2]);
    }

    #[test]
    fn round_robin_skips_dead_keys() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        c.mark_dead(1); // « b » morte (401)
        assert_eq!(c.key_order(), vec![0, 2]);
        assert_eq!(c.key_order(), vec![2, 0]);
        assert_eq!(c.drain_spent_fingerprints(), vec![key_fingerprint("b")]);
        c.mark_dead(1);
        assert!(c.drain_spent_fingerprints().is_empty());
    }

    #[test]
    fn les_cles_desactivees_en_base_sont_sautees_sans_etre_re_rapportees() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        c.set_dead_fingerprints(&[key_fingerprint("a")]);
        assert_eq!(c.key_order(), vec![1, 2]);
        // La base la connaît déjà : rien à persister.
        assert!(c.drain_spent_fingerprints().is_empty());
    }

    #[test]
    fn une_cle_reactivee_en_base_revient_dans_la_rotation() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        c.set_dead_fingerprints(&[key_fingerprint("a")]);
        assert_eq!(c.live_key_count(), 2);

        c.idx.store(0, Relaxed);
        c.set_dead_fingerprints(&[]);
        assert_eq!(c.live_key_count(), 3);
        assert_eq!(c.key_order(), vec![0, 1, 2]);
    }

    #[test]
    fn round_robin_all_dead_still_rotates() {
        use std::sync::atomic::Ordering::Relaxed;
        let c = MistralClient::new(keys(), "m".into()).unwrap();
        c.idx.store(0, Relaxed);
        for i in 0..3 {
            c.mark_dead(i);
        }
        // Tout le pool refusé : ordre complet, l'appelant borne ses retries.
        assert_eq!(c.key_order(), vec![0, 1, 2]);
        assert_eq!(c.key_order(), vec![1, 2, 0]);
    }

    #[test]
    fn fingerprint_is_stable_hex16() {
        assert_eq!(key_fingerprint("a").len(), 16);
        assert_eq!(key_fingerprint("a"), key_fingerprint("a"));
        assert_ne!(key_fingerprint("a"), key_fingerprint("b"));
    }

    #[test]
    fn chat_payload_optional_fields() {
        let c = MistralClient::new(vec!["k".into()], "mistral-small-2506".into()).unwrap();
        // Sans options : ni max_tokens ni prompt_cache_key ; température 0, stream off.
        let p = c.chat_payload("sys", "usr", None, None);
        assert_eq!(p["model"], "mistral-small-2506");
        assert_eq!(p["temperature"], 0);
        assert_eq!(p["stream"], false);
        assert!(p.get("max_tokens").is_none());
        assert!(p.get("prompt_cache_key").is_none());
        assert_eq!(p["messages"][0]["role"], "system");
        assert_eq!(p["messages"][0]["content"], "sys");
        assert_eq!(p["messages"][1]["content"], "usr");
        // Avec options (ex. ingest résumé + rerank listwise).
        let p = c.chat_payload("sys", "usr", Some(400), Some("rr-lws-v1"));
        assert_eq!(p["max_tokens"], 400);
        assert_eq!(p["prompt_cache_key"], "rr-lws-v1");
    }
}
