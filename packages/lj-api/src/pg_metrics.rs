//! Scraper de métriques Postgres → OTLP.
//!
//! Le row « PostgreSQL » du cockpit Grafana interroge le jeu de métriques du
//! `postgresqlreceiver` OTel (taille des tables/index, taille de la base,
//! vacuum, âge du WAL). On émet **les mêmes noms et unités** que ce receiver
//! pour que, après conversion OTLP → Prometheus côté Grafana Cloud (points →
//! underscores, suffixe d'unité ajouté), on retrouve
//! `postgresql_db_size_bytes`, `postgresql_table_size_bytes`,
//! `postgresql_index_size_bytes`, `postgresql_table_vacuum_count` et
//! `postgresql_wal_age_seconds`.
//!
//! Modèle d'export : des observable gauges OTel dont le callback (synchrone)
//! lit un snapshot rafraîchi par une boucle async toutes les 60 s — 1 DPM, la
//! limite incluse au free tier Grafana Cloud (`included_dpm_per_series`).
//!
//! Chaque famille de métriques se scrape **indépendamment**, dans sa propre
//! transaction bornée par [`SCRAPE_STATEMENT_TIMEOUT`] : les requêtes de taille
//! (`pg_database_size`, `pg_relation_size`) appellent `stat()` par segment de
//! fichier, donc leur latence suit l'I/O de la machine et non le volume de
//! données. Une famille lente n'aveugle donc pas les autres, le scraper ne
//! retient pas une connexion du pool pendant la saturation, et une famille qui
//! échoue garde sa valeur précédente.
//!
//! Point clé :
//! `postgresql.database.name`, `postgresql.table.name`, `postgresql.index.name`
//! sont posés comme **attributs de datapoint** sur chaque mesure — ils
//! deviennent des labels Prometheus. Posés comme attributs de `Resource`, ils
//! finiraient noyés dans la série `target_info`, sans dimension par
//! table/index.

use deadpool_postgres::Pool;
use opentelemetry::{global, KeyValue};
use std::sync::Mutex;
use std::time::Duration;
use tracing::{info, warn};

/// Période de scrape (1 DPM, free tier).
const SCRAPE_INTERVAL: Duration = Duration::from_secs(60);

/// Borne d'une requête de scrape, très en dessous du `statement_timeout` du
/// pool : la connexion est rendue vite quand l'I/O est saturé. La métrique
/// perdue est un point ; la connexion retenue, une requête utilisateur en moins.
const SCRAPE_STATEMENT_TIMEOUT: &str = "5s";

/// Une mesure de taille (table ou index), portant ses labels de datapoint.
struct Sized {
    database: String,
    table: String,
    /// `Some` pour un index, `None` pour une table.
    index: Option<String>,
    bytes: i64,
}

/// Compteur de vacuum manuel par table.
struct Vacuum {
    database: String,
    table: String,
    count: i64,
}

/// Snapshot des métriques Postgres, rafraîchi par la boucle async et lu par les
/// callbacks des observable gauges.
struct PgStatsSnapshot {
    db_size: Vec<(String, i64)>,
    tables: Vec<Sized>,
    indexes: Vec<Sized>,
    vacuums: Vec<Vacuum>,
    /// Âge du WAL courant en secondes (un point par base, partagé instance-wide).
    wal_age_seconds: Option<f64>,
}

impl PgStatsSnapshot {
    const EMPTY: Self = Self {
        db_size: Vec::new(),
        tables: Vec::new(),
        indexes: Vec::new(),
        vacuums: Vec::new(),
        wal_age_seconds: None,
    };
}

/// État global partagé entre la boucle de scrape et les callbacks OTel. Chaque
/// champ vit sa propre vie : le scrape n'écrase que les familles qu'il a
/// réussi à lire.
static SNAPSHOT: Mutex<PgStatsSnapshot> = Mutex::new(PgStatsSnapshot::EMPTY);

/// Démarre le scraper : enregistre les observable gauges OTel, puis boucle de
/// rafraîchissement du snapshot toutes les 60 s. Conçu pour `tokio::spawn`.
///
/// Le `Meter` vient du `MeterProvider` global posé par
/// `telemetry::init_telemetry` ; sans provider global, les instruments sont des
/// no-op silencieux.
pub async fn run(pool: Pool) {
    register_gauges();
    info!(
        interval_secs = SCRAPE_INTERVAL.as_secs(),
        "pg_metrics : scraper démarré"
    );

    let mut ticker = tokio::time::interval(SCRAPE_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(err) = scrape(&pool).await {
            warn!(error = %err, "pg_metrics : scrape échoué (retry au prochain tick)");
        }
    }
}

/// Enregistre les observable gauges. Chaque callback projette le dernier
/// snapshot en mesures, une par couple (table/index/db) avec ses labels.
fn register_gauges() {
    let meter = global::meter("librejustice-pg");

    // postgresql.db_size (By) → postgresql_db_size_bytes
    let _db_size = meter
        .i64_observable_gauge("postgresql.db_size")
        .with_description("The database disk usage.")
        .with_unit("By")
        .with_callback(|observer| {
            let snap = SNAPSHOT.lock().unwrap();
            for (database, bytes) in &snap.db_size {
                observer.observe(
                    *bytes,
                    &[KeyValue::new("postgresql.database.name", database.clone())],
                );
            }
        })
        .build();

    // postgresql.table.size (By) → postgresql_table_size_bytes
    let _table_size = meter
        .i64_observable_gauge("postgresql.table.size")
        .with_description("Disk space used by a table.")
        .with_unit("By")
        .with_callback(|observer| {
            let snap = SNAPSHOT.lock().unwrap();
            for t in &snap.tables {
                observer.observe(
                    t.bytes,
                    &[
                        KeyValue::new("postgresql.database.name", t.database.clone()),
                        KeyValue::new("postgresql.table.name", t.table.clone()),
                    ],
                );
            }
        })
        .build();

    // postgresql.index.size (By) → postgresql_index_size_bytes
    let _index_size = meter
        .i64_observable_gauge("postgresql.index.size")
        .with_description("The size of the index on disk.")
        .with_unit("By")
        .with_callback(|observer| {
            let snap = SNAPSHOT.lock().unwrap();
            for i in &snap.indexes {
                let index = i.index.as_deref().unwrap_or_default();
                observer.observe(
                    i.bytes,
                    &[
                        KeyValue::new("postgresql.database.name", i.database.clone()),
                        KeyValue::new("postgresql.table.name", i.table.clone()),
                        KeyValue::new("postgresql.index.name", index.to_string()),
                    ],
                );
            }
        })
        .build();

    // postgresql.table.vacuum.count → postgresql_table_vacuum_count
    let _vacuum = meter
        .i64_observable_gauge("postgresql.table.vacuum.count")
        .with_description("Number of times a table has manually been vacuumed.")
        .with_callback(|observer| {
            let snap = SNAPSHOT.lock().unwrap();
            for v in &snap.vacuums {
                observer.observe(
                    v.count,
                    &[
                        KeyValue::new("postgresql.database.name", v.database.clone()),
                        KeyValue::new("postgresql.table.name", v.table.clone()),
                    ],
                );
            }
        })
        .build();

    // postgresql.wal.age (s) → postgresql_wal_age_seconds
    let _wal_age = meter
        .f64_observable_gauge("postgresql.wal.age")
        .with_description("Age of the oldest WAL file.")
        .with_unit("s")
        .with_callback(|observer| {
            if let Some(age) = SNAPSHOT.lock().unwrap().wal_age_seconds {
                observer.observe(age, &[]);
            }
        })
        .build();
}
/// Rafraîchit le snapshot, une famille à la fois : un échec laisse la famille
/// à sa valeur précédente sans empêcher les suivantes. L'erreur remontée est
/// celle du checkout de connexion — les échecs par famille sont loggués sur
/// place avec leur nom.
async fn scrape(pool: &Pool) -> anyhow::Result<()> {
    let conn = pool.get().await?;

    // Taille de chaque base non-template (parité `postgresql.db_size`).
    if let Some(rows) = read(
        &conn,
        "db_size",
        "SELECT datname, pg_database_size(datname) \
         FROM pg_database WHERE datistemplate = false",
    )
    .await
    {
        SNAPSHOT.lock().unwrap().db_size = rows
            .iter()
            .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1)))
            .collect();
    }

    // Taille des tables utilisateur (`pg_relation_size`, comme le receiver).
    if let Some(rows) = read(
        &conn,
        "table_size",
        "SELECT current_database(), \
                schemaname || '.' || relname, \
                pg_relation_size(relid), \
                NULL \
         FROM pg_stat_user_tables",
    )
    .await
    {
        SNAPSHOT.lock().unwrap().tables = rows.iter().map(sized).collect();
    }

    // Taille des index utilisateur (label `postgresql.table.name` sans schéma,
    // comportement par défaut du receiver hors feature gate `separateSchemaAttr`).
    if let Some(rows) = read(
        &conn,
        "index_size",
        "SELECT current_database(), relname, \
                pg_relation_size(indexrelid), indexrelname \
         FROM pg_stat_user_indexes",
    )
    .await
    {
        SNAPSHOT.lock().unwrap().indexes = rows.iter().map(sized).collect();
    }

    // Nombre de vacuum manuels par table (`postgresql.table.vacuum.count`).
    if let Some(rows) = read(
        &conn,
        "vacuum_count",
        "SELECT current_database(), \
                schemaname || '.' || relname, \
                vacuum_count \
         FROM pg_stat_user_tables",
    )
    .await
    {
        SNAPSHOT.lock().unwrap().vacuums = rows
            .iter()
            .map(|r| Vacuum {
                database: r.get(0),
                table: r.get(1),
                count: r.get(2),
            })
            .collect();
    }

    // Âge du WAL : secondes écoulées depuis le dernier checkpoint réussi
    // (proxy de `postgresql.wal.age`, qui mesure l'âge du plus vieux segment).
    if let Some(rows) = read(
        &conn,
        "wal_age",
        "SELECT EXTRACT(EPOCH FROM (now() - checkpoint_time))::float8 \
         FROM pg_control_checkpoint()",
    )
    .await
    {
        SNAPSHOT.lock().unwrap().wal_age_seconds =
            rows.first().and_then(|r| r.get::<_, Option<f64>>(0));
    }

    Ok(())
}

/// Lit une famille dans sa propre transaction bornée par
/// [`SCRAPE_STATEMENT_TIMEOUT`]. `None` = échec loggué, l'appelant garde la
/// valeur précédente. Le `ROLLBACK` inconditionnel rend la connexion utilisable
/// pour la famille suivante, y compris après une annulation par
/// `statement_timeout` (qui rejette tout jusqu'à la fin de la transaction).
async fn read(
    conn: &deadpool_postgres::Object,
    family: &'static str,
    sql: &str,
) -> Option<Vec<tokio_postgres::Row>> {
    let read = async {
        conn.batch_execute("BEGIN").await?;
        conn.batch_execute(&format!(
            "SET LOCAL statement_timeout = '{SCRAPE_STATEMENT_TIMEOUT}'"
        ))
        .await?;
        conn.query(sql, &[]).await
    }
    .await;
    let _ = conn.batch_execute("ROLLBACK").await;
    match read {
        Ok(rows) => Some(rows),
        Err(err) => {
            warn!(
                family,
                error = %err,
                "pg_metrics : famille non lue (valeur précédente conservée)"
            );
            None
        }
    }
}

/// Projette une ligne `(database, table, bytes, index)` en [`Sized`] — forme
/// commune aux tailles de table (4ᵉ colonne `NULL`) et d'index.
fn sized(r: &tokio_postgres::Row) -> Sized {
    Sized {
        database: r.get(0),
        table: r.get(1),
        bytes: r.get(2),
        index: r.get(3),
    }
}
