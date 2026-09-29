use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("postgres: {}", pg_detail(.0))]
    Postgres(#[from] tokio_postgres::Error),
    #[error("pool: {0}")]
    Pool(String),
    #[error(
        "migration {version:04}: la base contient des versions absentes sur disque: {missing:?}"
    )]
    UnknownMigrations { version: i32, missing: Vec<i32> },
    #[error(
        "schéma en retard de {} migration(s) ({pending:?}) : lancez `lj-ingest migrate`, seule commande qui applique",
        pending.len()
    )]
    PendingMigrations { pending: Vec<i32> },
    #[error("invalid: {0}")]
    Invalid(String),
}

/// Rend une erreur Postgres diagnosticable, sur une ligne.
///
/// Le `Display` de `tokio_postgres::Error` n'imprime que la catégorie du `Kind` ;
/// SQLSTATE, message serveur, table et contrainte vivent dans `source()`.
///
/// Tout site qui journalise une erreur Postgres brute passe par ici.
pub fn pg_detail(err: &tokio_postgres::Error) -> String {
    if let Some(db) = err.as_db_error() {
        let mut out = format!("{} {} {}", db.severity(), db.code().code(), db.message());
        for (label, value) in [
            ("table", db.table()),
            ("column", db.column()),
            ("constraint", db.constraint()),
            ("detail", db.detail()),
            ("hint", db.hint()),
            ("where", db.where_()),
        ] {
            if let Some(v) = value {
                out.push_str(&format!(" [{label}={v}]"));
            }
        }
        return out.replace('\n', " ");
    }
    let mut out = err.to_string();
    let mut cause = std::error::Error::source(err);
    while let Some(e) = cause {
        out.push_str(&format!(": {e}"));
        cause = e.source();
    }
    out.replace('\n', " ")
}

/// Idem pour une erreur de checkout deadpool, dont la variante `Backend` porte
/// une erreur Postgres.
pub fn pool_detail(err: &deadpool_postgres::PoolError) -> String {
    match err {
        deadpool_postgres::PoolError::Backend(e) => format!("backend: {}", pg_detail(e)),
        other => other.to_string(),
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;
