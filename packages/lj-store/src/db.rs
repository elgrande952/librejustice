//! Pool de connexions Postgres (deadpool-postgres) + helpers.

use crate::error::{Result, StoreError};
pub use deadpool_postgres::Pool;
use deadpool_postgres::{Manager, ManagerConfig, Object, RecyclingMethod, Runtime};
use std::str::FromStr;
use std::time::Duration;
use tokio_postgres::NoTls;

pub type Connection = Object;

/// Délai max d'attente d'une connexion au pool avant échec (`pool.get()`).
const POOL_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Seuil au-delà duquel une attente de connexion laisse une ligne. Au repos le
/// pool sert en microsecondes : une attente visible est déjà une file.
const POOL_WAIT_LOG: Duration = Duration::from_millis(250);

/// Options de session posées côté serveur via le paramètre de démarrage libpq
/// `options`. Une connexion tient le `search_path` reçu à son ouverture : un
/// schéma entre donc ici un déploiement avant la migration qui le crée, et le
/// nommer avant qu'il existe est sans effet.
const SESSION_OPTIONS: &str = "-c search_path=public,paradedb,comptes \
                               -c statement_timeout=30s -c jit=off \
                               -c client_min_messages=error";

/// Injecte `password` dans une DSN qui n'en porte pas — `tokio-postgres` ne lit
/// pas `PGPASSWORD`, contrairement à libpq, et le mot de passe vit dans sa propre
/// variable pour ne pas être recopié dans chaque URL.
///
/// Place canonique de ce calcul : les binaires en portaient chacun une copie.
pub fn dsn_with_password(dsn: &str, password: Option<&str>) -> String {
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return dsn.to_string();
    };
    let Some((scheme, apres)) = dsn.split_once("://") else {
        return dsn.to_string();
    };
    let (netloc, reste) = match apres.find(['/', '?', '#']) {
        Some(i) => (&apres[..i], &apres[i..]),
        None => (apres, ""),
    };
    let (userinfo, hostport) = match netloc.rsplit_once('@') {
        Some((info, hp)) => (info, hp),
        None => return dsn.to_string(),
    };
    if userinfo.contains(':') {
        return dsn.to_string();
    }
    format!("{scheme}://{userinfo}:{password}@{hostport}{reste}")
}

/// Binary name, used as `application_name`: without it `pg_stat_activity`
/// reads `[unknown]` and maintenance cannot tell its connections from others'.
fn binary_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "lj".to_string())
}

/// Construit un pool depuis une DSN Postgres (forme libpq
/// `postgresql://user:pass@host:port/db?application_name=...`).
pub fn build_pool(dsn: &str, pool_max: usize) -> Result<Pool> {
    let mut pg_config = tokio_postgres::Config::from_str(dsn)
        .map_err(|e| StoreError::Pool(format!("DSN invalide: {e}")))?;

    let options = match pg_config.get_options() {
        Some(existing) => format!("{existing} {SESSION_OPTIONS}"),
        None => SESSION_OPTIONS.to_string(),
    };
    pg_config.options(&options);
    if pg_config.get_application_name().is_none() {
        pg_config.application_name(binary_name());
    }

    let mgr_config = ManagerConfig {
        recycling_method: RecyclingMethod::Custom("BEGIN; ROLLBACK;".to_string()),
    };
    let manager = Manager::from_config(pg_config, NoTls, mgr_config);

    Pool::builder(manager)
        .max_size(pool_max)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(POOL_WAIT_TIMEOUT))
        .build()
        .map_err(|e| StoreError::Pool(format!("construction du pool: {e}")))
}

/// `pool.get()` qui rend l'attente mesurée et la journalise au-delà de
/// [`POOL_WAIT_LOG`]. Sans elle, [`POOL_WAIT_TIMEOUT`] laisse une requête
/// attendre jusqu'à 30 s sans qu'aucune trace distingue la file d'un handler lent.
pub async fn get_timed(pool: &Pool) -> Result<(Connection, Duration)> {
    let started = std::time::Instant::now();
    let conn = pool
        .get()
        .await
        .map_err(|e| StoreError::Pool(format!("{e}")))?;
    let attente = started.elapsed();
    if attente >= POOL_WAIT_LOG {
        let ms = attente.as_millis();
        tracing::warn!(
            pool_wait_ms = ms as u64,
            "waited {ms} ms for a pool connection"
        );
    }
    Ok((conn, attente))
}

/// No-op : le crate `pgvector` accepte `vector` par nom de type, aucun
/// enregistrement runtime n'est requis.
pub async fn register_extension_types(_conn: &Connection) -> Result<()> {
    Ok(())
}
