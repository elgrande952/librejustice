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

/// Options de session posées côté serveur via le paramètre de démarrage libpq
/// `options`.
const SESSION_OPTIONS: &str = "-c statement_timeout=30s -c jit=off -c client_min_messages=error";

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

/// No-op : le crate `pgvector` accepte `vector` par nom de type, aucun
/// enregistrement runtime n'est requis.
pub async fn register_extension_types(_conn: &Connection) -> Result<()> {
    Ok(())
}
