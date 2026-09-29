//! Réparation rétroactive des faux merges **intra-source** judilibre.
//!
//! Une dédup sur `canonical_ref` a fusionné des décisions distinctes partageant
//! `numbers`+cour+date (lot d'audience, RG réutilisé, arrêt sur déférée). Le
//! texte du perdant a été détruit : on le relit du cache local judilibre, sans
//! appel API. Le cache `compact` retire les tombstonés, donc une provenance
//! supprimée n'est jamais re-matérialisée.
//!
//! Par perdante, inverse d'une fusion :
//! 1. `create_split_decision` détache la provenance vers une décision squelette,
//!    l'autorité `rn=1` restant sur l'origine ;
//! 2. le payload relu du cache la matérialise (re-chunk + re-embed **local
//!    strict**). Les deux **dans une seule transaction**.
//!
//! `--dry-run` (défaut) : compte seulement, aucune écriture, aucun GPU.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use lj_sources::downloader::read_jsonl_gz_lines;
use lj_store::repository::DecisionRepository;

use super::batch::drain_batch_in_txn;
use super::embed::build_local_strict;
use super::prepare::classify_judilibre;
use super::resplit::{provenance_canonical_ref, provenance_jurisdiction_type};
use super::{generate_public_id, IngestCounts, IngestMode};
use crate::config::Settings;

/// `YYYYMM` d'une `decision_date` ISO (`YYYY-MM-DD`) ; `None` si malformée
/// (même découpage que `yyyymm_of` du downloader — bucket d'archivage cache).
fn yyyymm(decision_date: &str) -> Option<String> {
    let b = decision_date.as_bytes();
    if b.len() >= 7 && b[4] == b'-' {
        Some(format!("{}{}", &decision_date[0..4], &decision_date[5..7]))
    } else {
        None
    }
}

/// Une perdante planifiée : provenance à détacher + sa localisation cache.
struct Plan {
    decision_id: i64,
    source_uid: String,
    object_id: String,
    canonical_ref: String,
    jur_type: String,
}

#[derive(Default)]
struct Stats {
    losers_total: usize,
    skipped_no_jur_date: usize,
    skipped_no_canonical: usize,
    cache_miss: usize,
    ready: usize,
    split: usize,
    failed: usize,
}

/// Relit un fichier cache `.jsonl.gz` une fois et renvoie les payloads des
/// `needed` (par object_id top-level). Cache append-only → **last-in-file wins**
/// (version la plus récente). Fichier absent ⇒ map vide.
fn load_payloads(path: &Path, needed: &HashSet<&str>) -> Result<HashMap<String, Vec<u8>>> {
    let mut out: HashMap<String, Vec<u8>> = HashMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let lines = read_jsonl_gz_lines(path).with_context(|| format!("lecture cache {path:?}"))?;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
            if needed.contains(id) {
                out.insert(id.to_string(), line);
            }
        }
    }
    Ok(out)
}

/// Défait les faux merges intra-source judilibre. `execute = false` (dry-run) :
/// plan + comptage, aucune écriture. `limit` : borne le nombre de splits écrits.
pub async fn unmerge_same_source(execute: bool, limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let cache_root = settings.cache_dir().join("judilibre");

    let embedder = if execute {
        Some(build_local_strict(&settings).await?)
    } else {
        None
    };

    let losers = repo.fetch_same_source_judilibre_losers().await?;
    let mut stats = Stats {
        losers_total: losers.len(),
        ..Default::default()
    };
    tracing::info!(
        losers = losers.len(),
        execute,
        "unmerge-same-source: judilibre losers to re-create"
    );

    // Regroupe par fichier cache (jur, yyyymm) → lecture une seule fois par fichier.
    let mut by_file: BTreeMap<(String, String), Vec<Plan>> = BTreeMap::new();
    for (decision_id, source_uid, sf) in losers {
        let Some(object_id) = source_uid.strip_prefix("judilibre/").map(str::to_string) else {
            continue;
        };
        let jur = sf
            .get("jurisdiction")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let date = sf
            .get("decision_date")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ym = match (jur.is_empty(), yyyymm(date)) {
            (false, Some(ym)) => ym,
            _ => {
                stats.skipped_no_jur_date += 1;
                continue;
            }
        };
        let Some(canonical_ref) = provenance_canonical_ref(&sf, &source_uid) else {
            stats.skipped_no_canonical += 1;
            continue;
        };
        let jur_type =
            provenance_jurisdiction_type(&sf, &source_uid).unwrap_or_else(|| "tj".to_string());
        by_file
            .entry((jur.to_string(), ym))
            .or_default()
            .push(Plan {
                decision_id,
                source_uid,
                object_id,
                canonical_ref,
                jur_type,
            });
    }

    let mut processed = 0usize;
    'outer: for ((jur, ym), plans) in &by_file {
        let path = cache_root.join(jur).join(format!("{ym}.jsonl.gz"));
        let needed: HashSet<&str> = plans.iter().map(|p| p.object_id.as_str()).collect();
        let payloads = load_payloads(&path, &needed)?;

        for p in plans {
            let Some(line) = payloads.get(&p.object_id) else {
                stats.cache_miss += 1;
                tracing::warn!(object_id = %p.object_id, path = ?path, "unmerge: payload missing from the cache, left merged");
                continue;
            };
            stats.ready += 1;
            if !execute {
                continue;
            }
            let embedder = embedder.as_ref().expect("embedder présent en execute");

            conn.batch_execute("BEGIN").await?;
            let res: Result<()> = async {
                let public_id = generate_public_id();
                let new_id = repo
                    .create_split_decision(
                        p.decision_id,
                        std::slice::from_ref(&p.source_uid),
                        &p.jur_type,
                        &public_id,
                        &p.canonical_ref,
                    )
                    .await?;
                let cand = classify_judilibre(line.clone())?
                    .ok_or_else(|| anyhow!("payload cache non reconnu: {}", p.object_id))?;
                let mut counts = IngestCounts::default();
                drain_batch_in_txn(
                    &conn,
                    Some(embedder),
                    vec![cand],
                    true,
                    IngestMode::All,
                    &mut counts,
                )
                .await?;
                tracing::debug!(new_id, object_id = %p.object_id, "unmerge: split decision materialised");
                Ok(())
            }
            .await;

            match res {
                Ok(()) => {
                    conn.batch_execute("COMMIT").await?;
                    stats.split += 1;
                }
                Err(e) => {
                    let _ = conn.batch_execute("ROLLBACK").await;
                    stats.failed += 1;
                    tracing::warn!(object_id = %p.object_id, error = %e, "unmerge: split failed, left merged");
                }
            }

            processed += 1;
            if processed.is_multiple_of(500) {
                tracing::info!(
                    processed,
                    split = stats.split,
                    failed = stats.failed,
                    cache_miss = stats.cache_miss,
                    "unmerge-same-source progress"
                );
            }
            if limit.is_some_and(|n| processed >= n) {
                tracing::info!(limit = ?limit, "unmerge: --limit reached, stopping");
                break 'outer;
            }
        }
    }

    let mode = if execute {
        "EXECUTE (write)"
    } else {
        "DRY-RUN (read-only)"
    };
    println!("\n=== unmerge-same-source [{mode}] ===");
    println!(
        "perdantes judilibre (rn>1)            : {}",
        stats.losers_total
    );
    println!(
        "  without jur/date (skip)             : {}",
        stats.skipped_no_jur_date
    );
    println!(
        "  without canonical_ref (skip)        : {}",
        stats.skipped_no_canonical
    );
    println!(
        "  missing from the cache (left)       : {}",
        stats.cache_miss
    );
    println!("  rematerialisable (cache-hit)        : {}", stats.ready);
    if execute {
        println!("  split decisions created             : {}", stats.split);
        println!("  failures (left merged)              : {}", stats.failed);
    }
    Ok(())
}
