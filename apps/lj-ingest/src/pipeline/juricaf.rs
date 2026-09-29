//! Sonde JuriCAF : charge des pages « arrêt » déjà téléchargées sous
//! `<state_dir>/ingest/juricaf-probe/html/` par le chemin d'upsert standard
//! (identité, chunk, embed) — repérage du fond francophone avant
//! l'intégration négociée avec l'AHJUCAF. Le référentiel `jurisdiction`
//! minte les cours étrangères au passage (`sn_cour_supreme`…), avec des codes
//! dérivés de l'URN:LEX gravé dans chaque page.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;

use lj_parse::parse_juricaf;
use lj_store::repository::{ExtractedFields, JurisdictionRow};

use crate::config::Settings;

use super::batch::drain_batch;
use super::embed::build_embedder_opt;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, WriteMode};

/// Charge toutes les pages `*.html` du dossier de sonde (défaut :
/// `<state_dir>/ingest/juricaf-probe/html`). Idempotent : `source_uid`
/// `juricaf/{slug}` + checksum des octets bruts.
pub async fn juricaf_probe(dir: Option<std::path::PathBuf>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = dir.unwrap_or_else(|| settings.state_dir.join("ingest/juricaf-probe/html"));
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;

    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow!("lecture {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    paths.sort();

    let mut counts = IngestCounts::default();
    let mut candidates = Vec::new();
    for path in &paths {
        let slug = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("nom de fichier illisible: {}", path.display()))?
            .to_string();
        let raw = std::fs::read(path)?;
        let parsed = match parse_juricaf(&slug, &raw) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(slug = %slug, error = %e, "juricaf parse impossible — skip");
                counts.skipped += 1;
                continue;
            }
        };
        let mut extracted = ExtractedFields {
            date_lecture: parsed
                .decision
                .date_lecture
                .as_deref()
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
            docket_numbers: parsed.decision.numero_dossiers.clone().unwrap_or_default(),
            ..Default::default()
        };
        extracted.jurisdiction = Some(JurisdictionRow {
            code: parsed.jurisdiction_code.clone(),
            source_code: parsed.jurisdiction_code.clone(),
            jurisdiction_type: parsed
                .decision
                .jurisdiction_type
                .clone()
                .expect("parse_juricaf pose toujours le type"),
            city: None,
            label: parsed.jurisdiction_label.clone(),
        });
        candidates.push(Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: content_checksum(&raw),
            prebuilt_source_fields: Some(parsed.source_fields),
            prebuilt_extracted: Some(extracted),
            decision: parsed.decision,
            raw_payload: Vec::new(),
            payload_format: "html".to_string(),
            write_mode: WriteMode::Full,
            dila_fond: None,
            passages: true,
        });
    }

    tracing::info!(
        n = candidates.len(),
        skipped = counts.skipped,
        dir = %dir.display(),
        "juricaf probe: pages parsed"
    );
    if !candidates.is_empty() {
        drain_batch(
            &conn,
            embedder.as_ref(),
            candidates,
            require_embeddings,
            super::IngestMode::All,
            &mut counts,
        )
        .await?;
    }
    tracing::info!(
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        errors = counts.errors,
        chunks = counts.chunks_created,
        "juricaf probe done"
    );
    Ok(())
}
