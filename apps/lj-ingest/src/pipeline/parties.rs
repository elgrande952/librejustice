//! Filet de dérive + résolution des acteurs par décision.
//! L'écriture nominale est au fil de l'eau (`update_extracted_fields` /
//! ingest, patron `case_citation`) ; `parties-reconcile` re-dérive la
//! relation depuis les colonnes NER plates de `decisions` — mêmes lignes que
//! le fil de l'eau (spans-évidences, nature, `resolve_key` via
//! `lj_extract::parties`) — et n'écrit que ce qui a changé. Unique
//! rattrapage de la relation.

use crate::config::Settings;
use anyhow::{anyhow, Result};
use lj_store::repository::{DecisionPartyRow, DecisionRepository};

/// Décisions lues par lot (keyset id).
const READ_BATCH: i64 = 1_000;

/// (colonne, qualité, côté) — l'ordre définit `ord` intra-décision, identique
/// aux cellules du fil de l'eau (`lj_ingest::extract`). La colonne
/// `intervenors` est gatée : pas d'émission relation en prod.
const COLUMNS: &[(&str, &str, Option<&str>)] = &[
    ("applicant_companies", "party", Some("applicant")),
    ("defendant_companies", "party", Some("defendant")),
    ("applicant_law_firms", "law_firm", Some("applicant")),
    ("defendant_law_firms", "law_firm", Some("defendant")),
    ("applicant_counsel_names", "counsel_name", Some("applicant")),
    ("defendant_counsel_names", "counsel_name", Some("defendant")),
];

/// Filet de dérive des acteurs : re-dérive la relation depuis les colonnes
/// plates de `decisions` et **n'écrit que ce qui a changé** —
/// `replace_decision_parties_bulk` porte déjà le skip-diff par empreinte (un
/// set inchangé garde ses `entity_uid`), la garde jamais-dégrader du gold et
/// la résolution at-write scopée au lot.
///
/// Forcer une re-dérivation après un changement de `actor_rows` ne demande
/// rien de plus : la dérivation entre dans l'empreinte, donc les lignes
/// concernées divergent et sont réécrites.
///
/// Le périmètre est plus large que celui de `reextract-fields --full`, qui
/// exclut `full_text IS NULL` et les révisions manuelles : ici toute décision
/// portant des valeurs NER **ou** des lignes acteur est vue.
pub async fn reconcile_parties(workers: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let workers = workers.map(|w| w.max(1)).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(4))
            .unwrap_or(4)
            .clamp(2, 8)
    });
    let pool = lj_store::db::build_pool(&settings.db_url, workers + 1)
        .map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;

    let non_empty = COLUMNS
        .iter()
        .map(|(c, _, _)| format!("COALESCE(cardinality({c}), 0)"))
        .collect::<Vec<_>>()
        .join(" + ");
    // Union avec les décisions AYANT des lignes acteur : celles dont toutes les
    // valeurs NER ont disparu doivent rendre un set vide, que
    // `replace_decision_parties_bulk` matérialise en DELETE.
    let ids: Vec<i64> = conn
        .query(
            &format!(
                "SELECT id FROM decisions WHERE {non_empty} > 0 \
                 UNION SELECT DISTINCT decision_id FROM decision_party \
                 ORDER BY id"
            ),
            &[],
        )
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    drop(conn);
    let total = ids.len();
    tracing::info!(total, workers, "parties-reconcile : worklist construite");
    if ids.is_empty() {
        return Ok(());
    }

    let started = std::time::Instant::now();
    let rewritten = std::sync::atomic::AtomicUsize::new(0);
    let seen = std::sync::atomic::AtomicUsize::new(0);
    let chunk = total.div_ceil(workers);
    let futs: Vec<_> = ids
        .chunks(chunk)
        .map(|slice| reconcile_worker(&pool, slice, &rewritten, &seen))
        .collect();
    futures::future::try_join_all(futs).await?;

    let rewritten = rewritten.load(std::sync::atomic::Ordering::Relaxed);
    tracing::info!(
        total,
        rewritten,
        elapsed_s = started.elapsed().as_secs(),
        "parties-reconcile : dérive résorbée"
    );
    println!(
        "parties-reconcile : {total} décisions examinées, {rewritten} réécrites \
         ({} s)",
        started.elapsed().as_secs()
    );
    Ok(())
}

/// Une tranche de la worklist : lecture par lots, dérivation en Rust, écriture
/// sur changement seulement. Tranches d'ids DISJOINTES — deux workers ne
/// touchent jamais la même décision.
async fn reconcile_worker(
    pool: &lj_store::db::Pool,
    slice: &[i64],
    rewritten: &std::sync::atomic::AtomicUsize,
    seen: &std::sync::atomic::AtomicUsize,
) -> Result<()> {
    use std::sync::atomic::Ordering;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0; SET synchronous_commit = off")
        .await?;
    let repo = DecisionRepository::new(&conn);
    let cols = COLUMNS
        .iter()
        .map(|(c, _, _)| *c)
        .collect::<Vec<_>>()
        .join(", ");
    let query =
        format!("SELECT id, full_text, {cols} FROM decisions WHERE id = ANY($1) ORDER BY id");

    for batch in slice.chunks(READ_BATCH as usize) {
        let batch = batch.to_vec();
        let fetched = conn.query(&query, &[&batch]).await?;
        let items: Vec<(i64, Option<Vec<DecisionPartyRow>>)> = fetched
            .iter()
            .map(|row| {
                let id: i64 = row.get(0);
                let full_text: String = row.get::<_, Option<String>>(1).unwrap_or_default();
                let lists: Vec<Vec<String>> = (0..COLUMNS.len())
                    .map(|i| row.get::<_, Option<Vec<String>>>(i + 2).unwrap_or_default())
                    .collect();
                let cells: Vec<lj_extract::parties::Cell<'_>> = COLUMNS
                    .iter()
                    .zip(&lists)
                    .map(|((_, quality, side), values)| (*quality, *side, values.as_slice()))
                    .collect();
                (id, Some(actor_rows(&full_text, &cells)))
            })
            .collect();

        conn.batch_execute("BEGIN").await?;
        let changed = match repo.replace_decision_parties_bulk(&items).await {
            Ok(changed) => changed,
            Err(e) => {
                conn.batch_execute("ROLLBACK").await?;
                return Err(anyhow::Error::new(e).context("replace_decision_parties_bulk"));
            }
        };
        conn.batch_execute("COMMIT").await?;
        rewritten.fetch_add(changed.len(), Ordering::Relaxed);
        let done = seen.fetch_add(items.len(), Ordering::Relaxed) + items.len();
        if done % 200_000 < READ_BATCH as usize {
            tracing::info!(
                decisions = done,
                rewritten = rewritten.load(Ordering::Relaxed),
                "parties-reconcile : progression"
            );
        }
    }
    Ok(())
}

/// Lignes acteur d'une décision — même dérivation que le fil de l'eau, un
/// seul endroit qui la porte.
fn actor_rows(full_text: &str, cells: &[lj_extract::parties::Cell<'_>]) -> Vec<DecisionPartyRow> {
    lj_extract::parties::actor_rows(full_text, cells)
        .into_iter()
        .map(|r| DecisionPartyRow {
            quality: r.quality.to_string(),
            side: r.side.map(str::to_string),
            value: r.value,
            resolve_key: r.resolve_key,
            nature: r.nature.map(|n| n.as_str().to_string()),
            barreau: r.barreau,
            role: r.role.map(str::to_string),
            firm_ord: r.firm_ord,
            char_starts: r.char_starts,
            char_ends: r.char_ends,
        })
        .collect()
}

/// Résolution des pendants. Défaut (hebdo) : passe « cabinets normalisés »
/// seule (la seule sans jumelle at-write) + recount annuaire. `--full` : toutes
/// les passes pleines (dépliage SIRENE/RNA 29 M, ~25 min), one-shot post-reload
/// de registre SIRENE/RNA.
pub async fn relink_parties(full: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    relink_with(&DecisionRepository::new(&conn), full).await
}

pub(crate) async fn relink_with(repo: &DecisionRepository<'_>, full: bool) -> Result<()> {
    let linked = if full {
        repo.resolve_pending_parties().await?
    } else {
        repo.resolve_pending_cabinets().await?
    };
    let (total, resolved, siren, rna, cnb, oacc) = repo.decision_party_stats().await?;
    tracing::info!(
        linked,
        total,
        resolved,
        siren,
        rna,
        cnb,
        oacc,
        "parties : résolution"
    );
    println!(
        "decision_party : {total} lignes, {resolved} résolues \
         ({siren} siren:, {rna} rna:, {cnb} cnb:, {oacc} oacc:) — {linked} liées par cette passe."
    );
    // Rafraîchit les compteurs annuaire d'`entity` depuis la relation
    // fraîchement résolue : `decision_count` et `annuaire_registre` reflètent
    // l'état des liens de cette passe.
    let annuaire = repo.refresh_annuaire().await?;
    tracing::info!(annuaire, "annuaire : compteurs rafraîchis");
    println!("annuaire : {annuaire} entités avec contentieux.");
    Ok(())
}
