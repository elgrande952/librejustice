//! Ré-extraction des champs structurés depuis les payloads stockés.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Result};

use lj_core::decision::Decision;
use lj_store::db::Pool;
use lj_store::repository::{DecisionRepository, ExtractedFields, REEXTRACTABLE_FIELDS};

use crate::config::Settings;

/// Taille d'un lot écrit en une transaction (fetch `id = ANY`, DELETE+INSERT bulk).
const BATCH: usize = 256;

/// Ré-extrait les champs structurés depuis les payloads stockés.
///
/// Pipeline concurrent : un seul scan construit la worklist des ids périmés,
/// partitionnée en `workers` tranches disjointes ; chaque worker (sa propre
/// connexion) fait `fetch → extract (spawn_blocking) → write` en recouvrement.
/// Aucun ré-chunking ni ré-embedding.
pub async fn reextract_fields(
    fields: Option<&[String]>,
    overwrite: bool,
    dry_run: bool,
    full: bool,
    jurisdiction_types: Option<&[String]>,
    citing_ref_uid: Option<&str>,
    workers: Option<usize>,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let selected = normalize_reextract_fields(fields)?;

    // Concurrence DB = nombre de workers (une connexion chacun). Override explicite
    // `--workers N` ; défaut auto : ~cœurs − 4, borné [2, 8].
    let workers = workers.map(|w| w.max(1)).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(4))
            .unwrap_or(4)
            .clamp(2, 8)
    });
    let pool = lj_store::db::build_pool(&settings.db_url, workers + 1)
        .map_err(|e| anyhow!("build_pool: {e}"))?;

    // Worklist : un seul scan (`stale_decision_ids_for_reextract`). Modes scopés
    // (`--juridiction-type` / `--citing-ref-uid`) : on draine leur keyset pour bâtir
    // la même worklist, puis on parallélise pareil.
    let ids = build_worklist(&pool, full, jurisdiction_types, citing_ref_uid).await?;
    let total = ids.len();
    tracing::info!(total, workers, "reextract : worklist construite");
    if ids.is_empty() {
        return Ok(());
    }

    let started = Instant::now();
    let processed = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    // Compteurs du mode sonde : par champ colonne, lignes divergentes + deux
    // exemples ; par couche satellite, items dont l'extracteur ne produit rien.
    let diffs: Arc<Mutex<DiffReport>> = Arc::new(Mutex::new(DiffReport::default()));

    let ticker = {
        let processed = processed.clone();
        let errors = errors.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            tick.tick().await; // immédiat — on saute le premier
            loop {
                tick.tick().await;
                let done = processed.load(Ordering::Relaxed);
                let rate = done as f64 / started.elapsed().as_secs_f64().max(1e-3);
                let eta_s = ((total - done.min(total)) as f64 / rate.max(1e-3)) as i64;
                tracing::info!(
                    processed = done,
                    total,
                    errors = errors.load(Ordering::Relaxed),
                    rate_per_s = rate as i64,
                    eta_s,
                    "reextract progress"
                );
            }
        })
    };

    // Tranches disjointes contiguës (équilibrées) → un worker par tranche. Les
    // workers tournent en concurrence coopérative sur une seule task
    // (`try_join_all`, pas `spawn`) : leurs futures ne sont pas `Send`, les params
    // `dyn ToSql` du repo ne l'étant pas. Le premier worker en erreur fait échouer
    // l'ensemble.
    let chunk_size = total.div_ceil(workers);
    let worker_futs: Vec<_> = ids
        .chunks(chunk_size)
        .map(|slice| {
            worker_loop(
                pool.clone(),
                slice.to_vec(),
                selected.clone(),
                overwrite,
                dry_run.then(|| diffs.clone()),
                &processed,
                &errors,
            )
        })
        .collect();
    let result = futures::future::try_join_all(worker_futs).await;
    ticker.abort();
    result.map_err(|e| e.context("reextract worker"))?;

    tracing::info!(
        processed = processed.load(Ordering::Relaxed),
        errors = errors.load(Ordering::Relaxed),
        "Re-extraction terminée"
    );

    if dry_run {
        diffs.lock().unwrap().report(total);
        return Ok(());
    }

    // Fin de run : tout est déjà résolu at-write dans les lots (citations côté
    // source, sonde inversée côté cible, parties scopées) — reste l'annuaire, dont
    // les compteurs dérivent des liens fraîchement posés. Run ciblé → recount scopé
    // à la worklist ; `--full` → refresh complet. Le sweep intégral des pendants
    // reste dans `reconcile`.
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let annuaire = if full {
        repo.refresh_annuaire().await?
    } else {
        repo.refresh_annuaire_scoped(&ids).await?
    };
    tracing::info!(annuaire, "annuaire rafraîchi en fin de reextract");
    Ok(())
}

/// Agrégat du mode sonde (`--dry-run`) : ce qu'un `--overwrite` réécrirait.
#[derive(Default)]
struct DiffReport {
    /// Champ colonne → (lignes divergentes, jusqu'à quatre exemples).
    columns: std::collections::BTreeMap<String, (i64, Vec<String>)>,
    /// Couche satellite → items dont l'extracteur ne produit RIEN. Sans
    /// `--overwrite` la couche existante survit ; avec, elle est purgée.
    satellites: std::collections::BTreeMap<&'static str, usize>,
}

impl DiffReport {
    fn absorb(
        &mut self,
        per_field: Vec<(String, i64, Vec<String>)>,
        selected: &[String],
        items: &[(i64, ExtractedFields)],
    ) {
        for (field, count, samples) in per_field {
            let slot = self.columns.entry(field).or_default();
            slot.0 += count;
            for s in samples {
                if slot.1.len() < 4 {
                    slot.1.push(s);
                }
            }
        }
        let muet =
            |vide: fn(&ExtractedFields) -> bool| items.iter().filter(|(_, e)| vide(e)).count();
        for (layer, muet) in [
            (
                "legal_references",
                muet(|e| e.citation_occurrences.is_none()),
            ),
            ("decision_links", muet(|e| e.decision_links.is_none())),
            ("case_citations", muet(|e| e.case_citations.is_none())),
            ("parties", muet(|e| e.parties.is_none())),
        ] {
            if selected.iter().any(|f| f == layer) {
                *self.satellites.entry(layer).or_default() += muet;
            }
        }
    }

    fn report(&self, total: usize) {
        for (field, (count, samples)) in &self.columns {
            if *count == 0 {
                continue;
            }
            tracing::info!(
                field,
                divergent = count,
                sur = total,
                exemples = ?samples,
                "dry-run : colonne que --overwrite réécrirait"
            );
        }
        let intactes: Vec<&str> = self
            .columns
            .iter()
            .filter(|(_, (c, _))| *c == 0)
            .map(|(f, _)| f.as_str())
            .collect();
        tracing::info!(?intactes, "dry-run : colonnes identiques à l'extracteur");
        for (layer, muet) in &self.satellites {
            tracing::info!(
                layer,
                extracteur_muet = muet,
                sur = total,
                "dry-run : couche satellite que --overwrite purgerait (sans lui, elle survit)"
            );
        }
    }
}

/// Construit la worklist d'ids selon le mode. Défaut (version-gated) : un scan.
/// Scopé : draine le keyset correspondant (jeux petits) pour réutiliser le même
/// pipeline parallèle en aval.
async fn build_worklist(
    pool: &Pool,
    full: bool,
    jurisdiction_types: Option<&[String]>,
    citing_ref_uid: Option<&str>,
) -> Result<Vec<i64>> {
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;
    let repo = DecisionRepository::new(&conn);
    if full {
        return Ok(repo.all_decision_ids_for_reextract().await?);
    }
    match (citing_ref_uid, jurisdiction_types) {
        (None, None) => Ok(repo.stale_decision_ids_for_reextract().await?),
        // Ciblé par texte cité : un seul scan GIN-piloté.
        (Some(uid), _) => Ok(repo
            .decision_ids_for_reextract_by_citing_ref_uid(uid)
            .await?),
        (None, Some(jts)) => {
            let mut out = Vec::new();
            let mut last_id = 0i64;
            loop {
                let ids = repo
                    .decision_ids_for_reextract_by_juridiction(last_id, BATCH as i64, jts)
                    .await?;
                let Some(&max) = ids.last() else { break };
                last_id = max;
                out.extend(ids);
            }
            Ok(out)
        }
    }
}

/// Boucle d'un worker : traite sa tranche d'ids par lots de [`BATCH`], chacun
/// `fetch → extract (CPU sur le pool blocking) → write transaction`, sur sa
/// propre connexion. La passe CPU tourne en `spawn_blocking` (ne bloque pas le
/// runtime) ; `workers` lots s'extraient donc en parallèle sur autant de cœurs.
async fn worker_loop(
    pool: Pool,
    slice: Vec<i64>,
    selected: Vec<String>,
    overwrite: bool,
    diffs: Option<Arc<Mutex<DiffReport>>>,
    processed: &AtomicUsize,
    errors: &AtomicUsize,
) -> Result<()> {
    let selected_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Réglages de session du run bulk (le pool est local à `reextract_fields`) :
    // - `session_replication_role = replica` coupe les triggers RI — les cibles du
    //   COPY `legal_citation` sont lues du MÊME catalogue, dans la même base ;
    // - `synchronous_commit = off` : pas d'attente de flush WAL au COMMIT de chaque
    //   lot, le reextract est idempotent et un crash rejoue le lot.
    conn.batch_execute(
        "SET statement_timeout = 0; \
         SET session_replication_role = replica; \
         SET synchronous_commit = off",
    )
    .await?;
    let repo = DecisionRepository::new(&conn);
    // Contexte d'extraction du run (linker + vocab compilé) — hydraté par le
    // premier worker, partagé par tous (`&'static`, traverse le spawn_blocking).
    let ctx = super::extract_ctx(&conn).await?;

    for batch in slice.chunks(BATCH) {
        let rows: Vec<(i64, String, serde_json::Value, String)> =
            repo.fetch_reextract_inputs_batch(batch).await?;
        if rows.is_empty() {
            continue;
        }

        // CPU (re-parse + re-extract) hors du thread async, parallélisé rayon (pool
        // global partagé entre workers).
        let extracted = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            rows.into_par_iter()
                .map(|(id, full_text, source_fields, source_uid)| {
                    reextract_one(id, &full_text, &source_fields, &source_uid, ctx)
                        .map(|fields| (id, fields))
                })
                .collect::<Vec<Result<(i64, ExtractedFields)>>>()
        })
        .await
        .map_err(|e| anyhow!("join extract: {e}"))?;

        let mut items: Vec<(i64, ExtractedFields)> = Vec::with_capacity(extracted.len());
        let mut batch_errors = 0usize;
        for item in extracted {
            match item {
                Ok(pair) => items.push(pair),
                Err(e) => {
                    tracing::error!(error = %e, "reextract échec");
                    batch_errors += 1;
                }
            }
        }
        if batch_errors > 0 {
            errors.fetch_add(batch_errors, Ordering::Relaxed);
        }
        if items.is_empty() {
            continue;
        }

        if let Some(diffs) = &diffs {
            let per_field = repo
                .diff_extracted_fields_bulk(&items, &selected_refs)
                .await?;
            diffs.lock().unwrap().absorb(per_field, &selected, &items);
            processed.fetch_add(items.len(), Ordering::Relaxed);
            continue;
        }

        conn.batch_execute("BEGIN").await?;
        let write = async {
            repo.update_extracted_fields_bulk(&items, Some(&selected_refs), overwrite)
                .await
                // `anyhow::Error::new` préserve la chaîne `StoreError → DbError`
                // (SQLSTATE + message).
                .map_err(|e| anyhow::Error::new(e).context("update_extracted_fields_bulk"))?;
            // Sonde inversée : des dockets réécrits peuvent faire de ces
            // décisions la cible de pendants plus anciens.
            if selected.iter().any(|f| f == "docket_numbers") {
                let ids: Vec<i64> = items.iter().map(|(id, _)| *id).collect();
                super::attract::attract_pending_for(&repo, &ids).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match write {
            Ok(()) => {
                conn.batch_execute("COMMIT").await?;
                processed.fetch_add(items.len(), Ordering::Relaxed);
            }
            Err(e) => {
                let _ = conn.batch_execute("ROLLBACK").await;
                return Err(e);
            }
        }
    }
    Ok(())
}

/// Re-parse + re-extract depuis la reconstruction `(full_text, source_fields)` —
/// chemin linéaire unique, quel que soit le format du payload d'origine (le texte
/// extrait vit en base, jamais re-parsé). `member_name` reconstruit par
/// `from_source_fields` (provenance) ; `classify_uid`/fond/numéro dérivés du
/// `source_uid` ; couche préconstruite relue de `source_fields`, comme
/// `prepare::extract_write` la superpose à l'ingest.
fn reextract_one(
    decision_id: i64,
    full_text: &str,
    source_fields: &serde_json::Value,
    source_uid: &str,
    ctx: &super::ExtractCtx,
) -> Result<ExtractedFields> {
    let decision = Decision::from_source_fields(full_text, source_fields, source_uid);
    let full = lj_ingest::extract::extracted_fields(
        &decision,
        &ctx.link,
        &ctx.vocab,
        &ctx.chrono,
        &ctx.jur_labels,
    )
    .map_err(|e| anyhow!("extract id={decision_id}: {e}"))?;
    // Même superposition qu'à l'ingest (`prepare::extract_write`).
    Ok(
        match lj_ingest::extract::prebuilt_from_source_fields(source_uid, source_fields) {
            Some(pre) => lj_ingest::extract::merge_prebuilt(pre, full),
            None => full,
        },
    )
}

/// Normalise la liste de champs ré-extractibles.
fn normalize_reextract_fields(fields: Option<&[String]>) -> Result<Vec<String>> {
    let Some(fields) = fields.filter(|f| !f.is_empty()) else {
        return Ok(REEXTRACTABLE_FIELDS.iter().map(|s| s.to_string()).collect());
    };
    // Dédup en préservant l'ordre.
    let mut seen = std::collections::HashSet::new();
    let mut selected: Vec<String> = Vec::new();
    for f in fields {
        let f = f.trim();
        if f.is_empty() || !seen.insert(f.to_string()) {
            continue;
        }
        selected.push(f.to_string());
    }
    let unknown: Vec<&String> = selected
        .iter()
        .filter(|f| !REEXTRACTABLE_FIELDS.contains(&f.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(anyhow!(
            "fields inconnus : {unknown:?}. Choix : {}",
            REEXTRACTABLE_FIELDS.join(", ")
        ));
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_fields_defaults_to_all() {
        let all = normalize_reextract_fields(None).unwrap();
        assert_eq!(all.len(), REEXTRACTABLE_FIELDS.len());
        assert!(all.contains(&"legal_references".to_string()));
    }

    #[test]
    fn normalize_fields_rejects_unknown() {
        let err = normalize_reextract_fields(Some(&["bogus".to_string()]));
        assert!(err.is_err());
    }

    #[test]
    fn normalize_fields_dedups_preserving_order() {
        let fields = vec![
            "date_lecture".to_string(),
            "date_lecture".to_string(),
            "solution_uid".to_string(),
        ];
        let out = normalize_reextract_fields(Some(&fields)).unwrap();
        assert_eq!(
            out,
            vec!["date_lecture".to_string(), "solution_uid".to_string()]
        );
    }
}
