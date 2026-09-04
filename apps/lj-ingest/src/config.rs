//! Configuration ingest.
//!
//! Settings centralisé, prefix `LIBREJUSTICE_` : tout passe par
//! [`Settings::from_env`].

use std::path::PathBuf;

use anyhow::Result;
use lj_sources::state_paths::StatePaths;

/// Lecture canonique de l'env : variable absente ou vide → `None`.
fn env_opt(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// Variable string avec valeur par défaut.
fn env_or(key: &str, default: &str) -> String {
    env_opt(key).unwrap_or_else(|| default.to_string())
}

/// Découpe une liste séparée par des virgules en éléments non vides, trimés.
///
/// `None`/vide → `[]`.
fn split_csv(value: Option<String>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|raw| {
            raw.split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Parse un budget neurons (`LIBREJUSTICE_INGEST_NEURON_BUDGET`).
///
/// Accepte un entier brut ou un suffixe d'échelle `k`/`m` (`"9k"` → 9000,
/// `"1.5m"` → 1_500_000), insensible à la casse. Vide/absent → `None`. Valeur
/// non parsable = erreur franche.
fn parse_neuron_budget(raw: Option<String>) -> Result<Option<usize>> {
    let Some(raw) = raw else { return Ok(None) };
    let raw = raw.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Ok(None);
    }
    let (num, scale) = match raw.strip_suffix('k') {
        Some(n) => (n, 1_000.0),
        None => match raw.strip_suffix('m') {
            Some(n) => (n, 1_000_000.0),
            None => (raw.as_str(), 1.0),
        },
    };
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("LIBREJUSTICE_INGEST_NEURON_BUDGET invalide : {raw:?}"))?;
    Ok(Some((value * scale) as usize))
}

/// Délai du watchdog en secondes (`LIBREJUSTICE_INGEST_WATCHDOG_SECS`).
///
/// Absent/vide → défaut 4 h (14400 s). `0` désactive le watchdog. Valeur non
/// parsable = erreur franche.
fn parse_watchdog_secs(raw: Option<String>) -> Result<u64> {
    let Some(raw) = raw else { return Ok(14_400) };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(14_400);
    }
    raw.parse()
        .map_err(|_| anyhow::anyhow!("LIBREJUSTICE_INGEST_WATCHDOG_SECS invalide : {raw:?}"))
}

/// Paramètres runtime du pipeline d'ingestion.
///
/// Source : variables d'environnement préfixées `LIBREJUSTICE_`.
#[derive(Debug, Clone)]
pub struct Settings {
    pub state_dir: PathBuf,
    pub db_url: String,
    pub piste_client_id: Option<String>,
    pub piste_client_secret: Option<String>,
    pub embed_backend: String,
    pub embed_url: String,
    pub embed_api_key: Option<String>,
    pub cloudflare_account_id: Option<String>,
    pub cloudflare_backend_token: Option<String>,
    /// Budget neurons Workers AI par run (cap dur côté embedder Cloudflare).
    /// Accepte les suffixes `k`/`m` (ex. `"9k"` → 9000). `None` = pas de cap.
    pub ingest_neuron_budget: Option<usize>,
    pub mistral_api_keys: Vec<String>,
    pub mistral_model: String,
    pub indexnow_key: Option<String>,
    pub grafana_otlp_endpoint: Option<String>,
    pub grafana_otlp_user: Option<String>,
    pub grafana_cloud_api_key: Option<String>,
    pub otel_service_name: String,
    pub deployment_environment: Option<String>,
    pub host_name: Option<String>,
    pub container_name: Option<String>,
    /// Plafond dur de durée d'une invocation `lj-ingest` directe (watchdog).
    /// Au-delà, le process logge une erreur et abandonne avec un code non nul.
    /// `0` = watchdog désactivé. Une chaîne cron n'en dépend pas : chaque étape
    /// porte son propre délai (`chain.rs`).
    pub watchdog_secs: u64,
    /// URL de base du pont d'egress, seul chemin du connecteur klekoon (TLS 1.2
    /// legacy que rustls refuse). Défaut = pont local `http://127.0.0.1:8090`
    /// (hôte/dev) ; en prod, le conteneur `cron` reçoit l'adresse interne par
    /// son environnement.
    pub egress_url: String,
}

impl Settings {
    /// Lit l'environnement (prefix `LIBREJUSTICE_`) et valide.
    pub fn from_env() -> Result<Self> {
        let state_dir = StatePaths::from_env().root().to_path_buf();

        let pg_password = env_opt("LIBREJUSTICE_PG_PASSWORD");
        let db_url_raw = env_or(
            "LIBREJUSTICE_DB_URL",
            "postgresql://librejustice@127.0.0.1:5432/librejustice",
        );
        let db_url = inject_pg_password(&db_url_raw, pg_password.as_deref())?;

        Ok(Self {
            state_dir,
            db_url,
            piste_client_id: env_opt("LIBREJUSTICE_PISTE_CLIENT_ID"),
            piste_client_secret: env_opt("LIBREJUSTICE_PISTE_CLIENT_SECRET"),
            embed_backend: env_or("LIBREJUSTICE_EMBED_BACKEND", "auto"),
            embed_url: env_or(
                "LIBREJUSTICE_EMBED_URL",
                "http://127.0.0.1:8400/v1/embeddings",
            ),
            embed_api_key: env_opt("LIBREJUSTICE_EMBED_API_KEY"),
            cloudflare_account_id: env_opt("LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID"),
            cloudflare_backend_token: env_opt("LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN"),
            ingest_neuron_budget: parse_neuron_budget(env_opt(
                "LIBREJUSTICE_INGEST_NEURON_BUDGET",
            ))?,
            mistral_api_keys: split_csv(env_opt("LIBREJUSTICE_MISTRAL_API_KEYS")),
            mistral_model: env_or("LIBREJUSTICE_MISTRAL_MODEL", "mistral-small-2603"),
            indexnow_key: env_opt("LIBREJUSTICE_INDEXNOW_KEY"),
            grafana_otlp_endpoint: env_opt("LIBREJUSTICE_GRAFANA_OTLP_ENDPOINT"),
            grafana_otlp_user: env_opt("LIBREJUSTICE_GRAFANA_OTLP_USER"),
            grafana_cloud_api_key: env_opt("LIBREJUSTICE_GRAFANA_CLOUD_API_KEY"),
            otel_service_name: env_or("LIBREJUSTICE_OTEL_SERVICE_NAME", "librejustice-ingest"),
            deployment_environment: env_opt("LIBREJUSTICE_DEPLOYMENT_ENVIRONMENT"),
            host_name: env_opt("LIBREJUSTICE_HOST_NAME"),
            container_name: env_opt("LIBREJUSTICE_CONTAINER_NAME"),
            watchdog_secs: parse_watchdog_secs(env_opt("LIBREJUSTICE_INGEST_WATCHDOG_SECS"))?,
            egress_url: env_or("LIBREJUSTICE_EGRESS_URL", "http://127.0.0.1:8090"),
        })
    }

    /// Chemins sous `state_dir` (`lj_sources::state_paths::StatePaths`).
    pub fn paths(&self) -> StatePaths {
        StatePaths::new(self.state_dir.clone())
    }

    /// `ingest/cache` — fonds auto-téléchargés (working data du pipeline).
    pub fn cache_dir(&self) -> PathBuf {
        self.paths().cache()
    }

    /// `ingest/corpus` — datasets légaux curés (métadonnées `legal_text` +
    /// chemin du markdown OCR), chargés par `load-legal-corpus`.
    pub fn legal_corpus_dir(&self) -> PathBuf {
        self.paths().corpus()
    }
}

/// Injecte `pg_password` dans `db_url` si l'URL n'en porte pas déjà un.
///
/// La forme reconstruite est `scheme://user:pwd@host[:port]/...`.
fn inject_pg_password(db_url: &str, pg_password: Option<&str>) -> Result<String> {
    let Some(password) = pg_password.filter(|p| !p.is_empty()) else {
        return Ok(db_url.to_string());
    };

    let Some((scheme, after_scheme)) = db_url.split_once("://") else {
        return Ok(db_url.to_string());
    };
    let (netloc, rest) = match after_scheme.find(['/', '?', '#']) {
        Some(idx) => (&after_scheme[..idx], &after_scheme[idx..]),
        None => (after_scheme, ""),
    };

    let (userinfo, hostport) = match netloc.rsplit_once('@') {
        Some((info, hp)) => (Some(info), hp),
        None => (None, netloc),
    };

    if userinfo.is_some_and(|info| info.contains(':')) {
        return Ok(db_url.to_string());
    }

    let username = userinfo.unwrap_or("");
    let new_netloc = format!("{username}:{password}@{hostport}");
    Ok(format!("{scheme}://{new_netloc}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_password_when_absent() {
        let url = inject_pg_password(
            "postgresql://librejustice@127.0.0.1:5432/librejustice",
            Some("s3cret"),
        )
        .unwrap();
        assert_eq!(
            url,
            "postgresql://librejustice:s3cret@127.0.0.1:5432/librejustice"
        );
    }

    #[test]
    fn keep_existing_password() {
        let url = inject_pg_password(
            "postgresql://librejustice:already@127.0.0.1:5432/librejustice",
            Some("s3cret"),
        )
        .unwrap();
        assert_eq!(
            url,
            "postgresql://librejustice:already@127.0.0.1:5432/librejustice"
        );
    }

    #[test]
    fn no_password_no_change() {
        let url =
            inject_pg_password("postgresql://librejustice@127.0.0.1/librejustice", None).unwrap();
        assert_eq!(url, "postgresql://librejustice@127.0.0.1/librejustice");
    }

    #[test]
    fn inject_without_port() {
        let url =
            inject_pg_password("postgresql://lj@db.internal/librejustice", Some("pw")).unwrap();
        assert_eq!(url, "postgresql://lj:pw@db.internal/librejustice");
    }

    #[test]
    fn split_csv_trims_and_filters() {
        assert_eq!(
            split_csv(Some(" a , b ,, c ".to_string())),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(split_csv(None).is_empty());
        assert!(split_csv(Some("   ".to_string())).is_empty());
    }

    #[test]
    fn parse_neuron_budget_scales_and_validates() {
        assert_eq!(parse_neuron_budget(None).unwrap(), None);
        assert_eq!(parse_neuron_budget(Some("  ".into())).unwrap(), None);
        assert_eq!(parse_neuron_budget(Some("9k".into())).unwrap(), Some(9_000));
        assert_eq!(
            parse_neuron_budget(Some("9000".into())).unwrap(),
            Some(9_000)
        );
        assert_eq!(
            parse_neuron_budget(Some("1.5M".into())).unwrap(),
            Some(1_500_000)
        );
        assert!(parse_neuron_budget(Some("plein".into())).is_err());
    }

    #[test]
    fn computed_dirs() {
        let settings = Settings {
            state_dir: PathBuf::from("/data/lj"),
            db_url: "postgresql://x".into(),
            piste_client_id: None,
            piste_client_secret: None,
            embed_backend: "openai-http".into(),
            embed_url: "http://x".into(),
            embed_api_key: None,
            cloudflare_account_id: None,
            cloudflare_backend_token: None,
            ingest_neuron_budget: None,
            mistral_api_keys: vec![],
            mistral_model: "m".into(),
            indexnow_key: None,
            grafana_otlp_endpoint: None,
            grafana_otlp_user: None,
            grafana_cloud_api_key: None,
            otel_service_name: "librejustice-ingest".into(),
            deployment_environment: None,
            host_name: None,
            container_name: None,
            watchdog_secs: 14_400,
            egress_url: "http://127.0.0.1:8090".into(),
        };
        assert_eq!(settings.cache_dir(), PathBuf::from("/data/lj/ingest/cache"));
    }
}
