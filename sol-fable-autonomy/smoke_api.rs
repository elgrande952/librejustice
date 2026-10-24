//! Bounded integration harness for the unmodified LibreJustice API library.
//! Local disposable test only. This is not the production SSR server.
use std::sync::Arc;
use lj_api::{config::Settings, state::AppState};
use lj_store::{db, migrator::{apply_migrations, MigrationIntent}};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    anyhow::ensure!(settings.bind_host == "127.0.0.1", "test must bind loopback");
    anyhow::ensure!(settings.embed_backend == "dummy", "test must use dummy embedder");
    anyhow::ensure!(settings.db_url.contains("127.0.0.1:55432/lj_smoke"), "test database only");
    let pool = db::build_pool(&settings.db_url, 8)?;
    let conn = pool.get().await?;
    let applied = apply_migrations(&conn, MigrationIntent::Apply).await?;
    println!("MIGRATIONS_APPLIED={}", applied.len());
    apply_migrations(&conn, MigrationIntent::Verify).await?;
    let legal_count: i64 = conn.query_one("SELECT count(*) FROM legal_text", &[]).await?.get(0);
    anyhow::ensure!(legal_count == 0, "no artificial canonical legal text may be seeded");
    let backfill = std::fs::read_to_string(std::env::var("LJ_SMOKE_BACKFILL_SQL")?)?;
    for _ in 0..2 {
        conn.batch_execute(&backfill).await?;
    }
    let filiations: i64 = conn.query_one("SELECT count(*) FROM text_filiation", &[]).await?.get(0);
    anyhow::ensure!(filiations == 0, "backfill must skip missing canonical parents");
    println!("EMPTY_CORPUS_BACKFILL_PASSES=2; LEGAL_TEXT_ROWS=0; FILIATION_ROWS=0");
    let existing: i64 = conn.query_one("SELECT count(*) FROM decisions", &[]).await?.get(0);
    anyhow::ensure!(existing == 0, "requires empty disposable corpus");
    // Synthetic fixture: never represents a court ruling or a legal authority.
    conn.batch_execute(r#"
        WITH fixture AS (
            INSERT INTO decisions (jurisdiction_type, public_id, updated_at, canonical_ref,
                search_title, date_lecture, docket_numbers)
            VALUES ('CC', 'SfSmoke00001', NOW(), 'synthetic:sol-fable-smoke-v1',
                'DOCUMENT SYNTHETIQUE - test technique Sol-Fable', DATE '2026-01-01', ARRAY['SYNTHETIQUE-0001'])
            RETURNING id, jurisdiction_type, search_title, date_lecture, docket_numbers
        )
        INSERT INTO decision_text (decision_id, lang, text_status, lead, full_text,
            jurisdiction_type, search_title, date_lecture, docket_numbers)
        SELECT id, 'fr', 'automatic', TRUE,
            'DOCUMENT SYNTHETIQUE DE TEST. Le marqueur solfablefixtureverifiable valide uniquement la recherche lexicale. Ce document ne constitue pas une décision de justice.',
            jurisdiction_type, search_title, date_lecture, docket_numbers FROM fixture;
    "#).await?;
    let indexed: i64 = conn.query_one("SELECT count(*) FROM decision_text", &[]).await?.get(0);
    anyhow::ensure!(indexed == 1, "exactly one synthetic text must exist");
    drop(conn);
    let state = AppState::build(Arc::new(settings.clone()), pool);
    let app = lj_api::routes::build_router(state, true);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 18300)).await?;
    println!("SYNTHETIC_FIXTURES=1; API_LISTEN=127.0.0.1:18300; EMBEDDINGS=dummy_test_only");
    axum::serve(listener, app).await?;
    Ok(())
}
