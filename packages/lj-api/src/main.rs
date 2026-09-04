//! Binaire `librejustice-api` (axum).

use anyhow::Result;
use lj_api::{
    cache_metrics, config::Settings, host_metrics, pg_metrics, routes, state::AppState, telemetry,
};
use lj_store::db;
use std::sync::Arc;

// Runtime mono-thread : le rendu SSR Leptos crée des valeurs `!Send`
// (`StoredValue::new_local`, `Rc<navigate>`) dans l'Owner réactif ; sur un runtime
// multi-thread, le drop de l'Owner peut tomber sur un autre worker →
// `SendWrapper::invalid_drop` → panic qui tue le stream Suspense. Mono-thread,
// création et drop sont sur le même thread. Le chemin chaud est de l'I/O async
// (Postgres + embedding HTTP) et tout le CPU lourd (parse XML, highlight, exports
// PDF/DOCX) est offloadé en `spawn_blocking` (pool dédié).
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let settings = Settings::from_env()?;
    // Guard a garder vivant : son `Drop` flush les batch processors OTLP a
    // l'arret (sinon les derniers spans/logs bufferises ne partent pas).
    let _telemetry_guard = telemetry::init_telemetry(&settings)?;

    let pool = db::build_pool(&settings.db_url, settings.pool_max)?;
    let state = AppState::build(Arc::new(settings.clone()), pool);

    tokio::spawn(cache_metrics::run(state.clone()));
    if settings.scrapes_shared_resources() {
        tokio::spawn(pg_metrics::run(state.pool.clone()));
        tokio::spawn(host_metrics::run());
    } else {
        tracing::info!(
            env = settings
                .deployment_environment
                .as_deref()
                .unwrap_or("(absent)"),
            "scrapers de ressources partagées inactifs"
        );
    }

    let app = routes::create_app(state, true);
    let addr = format!("{}:{}", settings.bind_host, settings.bind_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("librejustice-api listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
