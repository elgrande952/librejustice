//! Service HTTP du `robots.txt`, rendu depuis la table de `lj-core`.
//!
//! Le fichier n'existe pas dans le bundle : il est rendu ici pour que la politique
//! n'ait qu'une écriture (`lj_crawlers`), partagée avec les expressions WAF
//! que pousse `lj-ingest crawler-policy sync`.

use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(lj_crawlers::ROBOTS_PATH, get(robots_txt))
}

async fn robots_txt(State(state): State<AppState>) -> Response {
    let sitemap = format!(
        "{}{}",
        state.settings.public_base_url.trim_end_matches('/'),
        lj_crawlers::SITEMAP_PATH
    );
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        lj_crawlers::robots_txt(&sitemap),
    )
        .into_response()
}
