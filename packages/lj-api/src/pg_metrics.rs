use deadpool_postgres::Pool;
use opentelemetry::{global, KeyValue};
use std::sync::Mutex;
use std::time::Duration;
use tracing::{info, warn};

const SCRAPE_INTERVAL: Duration = Duration::from_secs(60);

const SCRAPE_STATEMENT_TIMEOUT: &str = "5s";

struct Sized {
    database: String,
    table: String,
    index: Option<String>,
    bytes: i64,
}

struct Vacuum {
    database: String,
    table: String,
    count: i64,
}

struct PgStatsSnapshot {
    db_size: Vec<(String, i64)>,
    tables: Vec<Sized>,
    indexes: Vec<Sized>,
    vacuums: Vec<Vacuum>,
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

static SNAPSHOT: Mutex<PgStatsSnapshot> = Mutex::new(PgStatsSnapshot::EMPTY);

pub async fn run(pool: Pool) {
    register_gauges();
    info!(
        interval_secs = SCRAPE_INTERVAL.as_secs(),
        "pg_metrics: scraper started"
    );

    let mut ticker = tokio::time::interval(SCRAPE_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(err) = scrape(&pool).await {
            warn!(error = %err, "pg_metrics: scrape failed (retry at next tick)");
        }
    }
}

fn register_gauges() {
    let meter = global::meter("librejustice-pg");

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
async fn scrape(pool: &Pool) -> anyhow::Result<()> {
    let conn = pool.get().await?;

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
                "pg_metrics: family not read (previous value kept)"
            );
            None
        }
    }
}

fn sized(r: &tokio_postgres::Row) -> Sized {
    Sized {
        database: r.get(0),
        table: r.get(1),
        bytes: r.get(2),
        index: r.get(3),
    }
}
