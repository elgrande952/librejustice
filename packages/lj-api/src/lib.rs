//! `lj-api` — serveur API mono-serveur.
//!
//! Axum + tokio-postgres/deadpool + pgvector. Endpoint MCP (rmcp). Auth OAuth +
//! JWT. Télémétrie tracing + OTel. Les DTOs viennent de `lj-dtos` (source de
//! vérité partagée avec le front).

pub mod annonces;
pub mod annonces_bodacc;
pub mod annonces_joafe;
pub mod auth;
pub mod bookmarks;
pub mod cache;
pub mod config;
pub mod decision_views;
pub mod decisions;
pub mod embedder;
pub mod entities;
pub mod error;
pub mod jurisdiction_hubs;
pub mod legi;
pub mod mcp;
pub mod mcp_presenters;
pub mod me;
pub mod norm_hubs;
pub mod oauth;
pub mod pdf_export;
pub mod pg_metrics;
pub mod ratelimit;
pub mod redirect;
pub mod referential;
pub mod registre;
pub mod rerank;
pub mod robots;
pub mod routes;
pub mod search;
pub mod search_history;
pub mod signals;
pub mod sitemap;
pub mod snippets;
pub mod state;
pub mod stats;
pub mod telemetry;
pub mod titles;
pub mod usage;

#[cfg(test)]
mod dependency_boundary {
    #[test]
    fn no_lj_extract_in_serve_path() {
        let manifest = include_str!("../Cargo.toml");
        assert!(
            !manifest.contains("lj-extract"),
            "lj-api ne doit pas dépendre de lj-extract : \
             promouvoir la primitive partagée dans lj-core"
        );
    }
}
