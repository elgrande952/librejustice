//! Orchestration d'un batch : write (I/O DB, tokio) + drain (triage → process).

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use anyhow::{anyhow, Result};
use rayon::prelude::*;

use lj_llm::backend::Embedder;
use lj_store::db::Connection;
use lj_store::repository::{
    BulkDecisionWrite, DecisionRepository, ExistingDecisionState, UpsertStatus,
};

use crate::chunking::DEFAULT_CHUNK_TOKENS;

use super::embed::{embed_prepared, to_bulk_write, EmbeddedDecision};
use super::prepare::{chunk_write, extract_write, triage_candidates};
use super::{
    Candidate, ExtractCtx, IngestCounts, IngestMode, PreparedDecision, WriteMode, BATCH_SIZE,
};

/// Écrit un batch en DB. Une transaction par batch.
async fn write_writes(conn: &Connection, writes: Vec<BulkDecisionWrite>) -> Result<IngestCounts> {
    let mut counts = IngestCounts::default();
    let repo = DecisionRepository::new(conn);
    conn.batch_execute("BEGIN").await?;

    let result = write_writes_inner(&repo, writes, &mut counts).await;
    match &result {
        Ok(()) => {
            conn.batch_execute("COMMIT").await?;
        }
        Err(_) => {
            let _ = conn.batch_execute("ROLLBACK").await;
        }
    }
    result.map(|()| counts)
}

async fn write_writes_inner(
    repo: &DecisionRepository<'_>,
    writes: Vec<BulkDecisionWrite>,
    counts: &mut IngestCounts,
) -> Result<()> {
    // Décisions écrites (créées/mises à jour) : cibles potentielles de
    // pendants plus anciens — sonde inversée en fin de lot.
    let mut fresh_ids: Vec<i64> = Vec::new();
    for write in writes {
        if write.write_mode == WriteMode::SourceXmlOnly.as_str() {
            let decision_id = write
                .decision_id
                .ok_or_else(|| anyhow!("backfill source_xml sans decision_id"))?;
            // Backfill du `public_id` (+ format) à contenu inchangé :
            // `source_uid`/`source_fields` vivent sur la provenance, qui existe déjà
            // ici. On complète seulement l'identifiant manquant.
            repo.set_public_id(decision_id, &write.public_id).await?;
            repo.set_payload_format(decision_id, &write.payload_format)
                .await?;
            counts.updated += 1;
            continue;
        }

        let decision_id = match write.decision_id {
            None => {
                let result = repo
                    .upsert(
                        &write.decision,
                        &write.content_checksum,
                        &write.public_id,
                        write.extracted.as_ref(),
                        write.canonical_ref.as_deref(),
                        &write.source_fields,
                        write.embed_version,
                        &write.payload_format,
                    )
                    .await?;
                match result.status {
                    UpsertStatus::Created => counts.created += 1,
                    UpsertStatus::Updated => counts.updated += 1,
                    UpsertStatus::Skipped => {
                        counts.skipped += 1;
                        continue;
                    }
                }
                result.id
            }
            Some(id) => {
                repo.update_existing(
                    id,
                    &write.decision,
                    &write.content_checksum,
                    &write.public_id,
                    write.extracted.as_ref(),
                    write.canonical_ref.as_deref(),
                    &write.source_fields,
                    write.embed_version,
                    &write.payload_format,
                )
                .await?;
                counts.updated += 1;
                id
            }
        };
        counts.chunks_created += repo
            .replace_chunks(decision_id, &write.decision, &write.chunks)
            .await?;
        repo.set_payload_format(decision_id, &write.payload_format)
            .await?;
        fresh_ids.push(decision_id);
    }
    // Sonde inversée : les décisions du lot attirent les citations/liens
    // pendants qui les visaient. Dernier statement avant le COMMIT de
    // l'appelant : fenêtre minimale du verrou advisory de l'attract.
    super::attract::attract_pending_for(repo, &fresh_ids).await?;
    Ok(())
}

/// Phase **1** d'un lot : lecture d'état (DB), triage, chunk (CPU).
/// N'écrit rien, n'embed rien, **n'extrait rien** — elle rend les décisions
/// chunkées, prêtes à être vectorisées. Elle ne dépend pas du contexte
/// d'extraction.
async fn stage_prepare(
    conn: &Connection,
    candidates: Vec<Candidate>,
    require_embeddings: bool,
    mode: IngestMode,
    chunk_tokens: usize,
) -> Result<(Vec<PreparedDecision>, IngestCounts, Duration)> {
    let mut counts = IngestCounts::default();
    let started = Instant::now();
    // Les tours de vidange du pipeline passent un lot vide : rien à demander.
    if candidates.is_empty() {
        return Ok((Vec::new(), counts, Duration::ZERO));
    }
    let uids: Vec<String> = candidates
        .iter()
        .map(|c| c.decision.source_uid.clone())
        .collect();
    let existing = fetch_states(conn, &uids).await?;
    let (survivors, skipped, deduped) =
        triage_candidates(candidates, &existing, require_embeddings, mode);
    counts.skipped += skipped;
    counts.dedup_in_batch += deduped;
    if survivors.is_empty() {
        return Ok((Vec::new(), counts, started.elapsed()));
    }

    // Chunk CPU parallèle, déporté sur un thread bloquant : `into_par_iter`
    // monopolise son appelant jusqu'au bout.
    let prepared: Vec<Result<Option<PreparedDecision>>> = tokio::task::spawn_blocking(move || {
        survivors
            .into_par_iter()
            .map(|cand| chunk_write(cand, chunk_tokens))
            .collect()
    })
    .await?;

    let mut writes = Vec::with_capacity(prepared.len());
    for item in prepared {
        match item? {
            Some(p) => writes.push(p),
            None => counts.empty_skipped += 1,
        }
    }
    Ok((writes, counts, started.elapsed()))
}

/// Phase **2** d'un lot : vectorisation. Poste dominant et non parallélisable
/// (le GPU sature).
async fn stage_embed<E: Embedder>(
    embedder: Option<&E>,
    writes: Vec<PreparedDecision>,
    chunk_tokens: usize,
) -> Result<(Vec<EmbeddedDecision>, Duration)> {
    if writes.is_empty() {
        return Ok((Vec::new(), Duration::ZERO));
    }
    let started = Instant::now();
    let embedded = embed_prepared(embedder, writes, chunk_tokens).await?;
    Ok((embedded, started.elapsed()))
}

/// Phase **3** d'un lot : extraction structurée (CPU) + assemblage des
/// écritures.
///
/// Placée **après** l'embed : elle seule dépend du contexte d'extraction
/// (linker + vocab), dont l'hydratation est le coût fixe du run — en aval du
/// GPU, il se paie pendant que l'embedder travaille.
async fn stage_extract(
    ctx: &'static ExtractCtx,
    items: Vec<EmbeddedDecision>,
) -> Result<(Vec<BulkDecisionWrite>, Duration)> {
    if items.is_empty() {
        return Ok((Vec::new(), Duration::ZERO));
    }
    let started = Instant::now();
    let extracted: Vec<Result<BulkDecisionWrite>> = tokio::task::spawn_blocking(move || {
        items
            .into_par_iter()
            .map(|(prepared, embeds)| {
                extract_write(prepared, ctx).map(|p| to_bulk_write(p, embeds))
            })
            .collect()
    })
    .await?;
    let mut bulk = Vec::with_capacity(extracted.len());
    for item in extracted {
        bulk.push(item?);
    }
    Ok((bulk, started.elapsed()))
}

/// Précheck DB groupé des `source_uid` d'un batch.
async fn fetch_states(
    conn: &Connection,
    uids: &[String],
) -> Result<HashMap<String, ExistingDecisionState>> {
    let repo = DecisionRepository::new(conn);
    Ok(repo.find_ingest_states(uids).await?)
}

/// Triage + traitement d'un batch de candidats (lecture states → triage →
/// prepare → embed → écriture). Chemin séquentiel : une transaction par batch,
/// aucun recouvrement. Reste le chemin des sources à faible volume et des
/// commandes de maintenance.
pub(super) async fn drain_batch<E: Embedder>(
    conn: &Connection,
    embedder: Option<&E>,
    candidates: Vec<Candidate>,
    require_embeddings: bool,
    mode: IngestMode,
    counts: &mut IngestCounts,
) -> Result<()> {
    let (writes, staged, _) = stage_prepare(
        conn,
        candidates,
        require_embeddings,
        mode,
        DEFAULT_CHUNK_TOKENS,
    )
    .await?;
    counts.merge(&staged);
    let (embedded, _) = stage_embed(embedder, writes, DEFAULT_CHUNK_TOKENS).await?;
    if embedded.is_empty() {
        return Ok(());
    }
    let ctx = super::extract_ctx(conn).await?;
    let (bulk, _) = stage_extract(ctx, embedded).await?;
    let written = write_writes(conn, bulk).await?;
    counts.merge(&written);
    Ok(())
}

/// Variante de [`drain_batch`] écrivant **sous la transaction de l'appelant**
/// (pas de `BEGIN`/`COMMIT` propre) : le re-split enveloppe chaque cluster dans
/// sa propre transaction et y enchaîne create_split + re-ingest.
pub(super) async fn drain_batch_in_txn<E: Embedder>(
    conn: &Connection,
    embedder: Option<&E>,
    candidates: Vec<Candidate>,
    require_embeddings: bool,
    mode: IngestMode,
    counts: &mut IngestCounts,
) -> Result<()> {
    let (writes, staged, _) = stage_prepare(
        conn,
        candidates,
        require_embeddings,
        mode,
        DEFAULT_CHUNK_TOKENS,
    )
    .await?;
    counts.merge(&staged);
    let (embedded, _) = stage_embed(embedder, writes, DEFAULT_CHUNK_TOKENS).await?;
    if embedded.is_empty() {
        return Ok(());
    }
    let ctx = super::extract_ctx(conn).await?;
    let (bulk, _) = stage_extract(ctx, embedded).await?;
    let repo = DecisionRepository::new(conn);
    write_writes_inner(&repo, bulk, counts).await
}

/// Temps cumulé par nature de travail, sur la durée d'un [`run_stream`].
#[derive(Default)]
struct Phases {
    /// Lecture d'état + triage + chunk (DB en lecture, puis CPU).
    prepare: Duration,
    /// Appels au backend d'embedding.
    embed: Duration,
    /// Extraction structurée + assemblage des écritures (CPU).
    extract: Duration,
    /// Transactions d'écriture.
    write: Duration,
}

/// Profondeur des canaux entre étages, en lots.
const STAGE_QUEUE: usize = 1;

/// Profondeur du canal embed → extract, en lots.
///
/// L'étage d'extraction ne consomme rien tant que son contexte s'hydrate : ce
/// canal absorbe les lots que l'embedder produit pendant ce temps, sinon la
/// contre-pression remonte et le GPU s'arrête.
const EXTRACT_QUEUE: usize = 4;

/// Ingest **en flux**, pour toute source capable d'émettre des candidats.
///
/// Les étages sont des boucles indépendantes reliées par des canaux bornés :
/// chacun consomme dès qu'il a de quoi, et le run coûte le **max des sommes** —
/// c'est-à-dire l'embed seul, poste dominant qui sature à débit constant quel
/// que soit le nombre de requêtes en vol.
///
/// ```text
/// produce ─→ [candidats] ─→ prepare ─→ [préparés] ─→ embed ─→ [embarqués] ─→ extract ─→ [bulk] ─→ write
/// ```
///
/// `produce` reçoit l'émetteur et rend les compteurs que lui seul connaît
/// (erreurs de parse, membres vides) : seul point d'adaptation par source, tout
/// le reste est commun.
///
/// **Les écritures restent strictement séquentielles et dans l'ordre.** Un seul
/// étage écrit, sur une seule connexion : la résolution d'identité de l'upsert
/// (`source_uid` → ECLI → `canonical_ref`) est un lire-puis-écrire non atomique
/// que des transactions concurrentes casseraient.
///
/// Le décalage introduit est borné par les canaux : `prepare` lit l'état d'avant
/// le commit des lots encore en vol, donc un `source_uid` présent deux fois dans
/// cette fenêtre n'est plus filtré au triage. L'upsert le rattrape dans sa propre
/// transaction et rend `Skipped`/`Updated`.
pub(super) async fn run_stream<E, P, Fut>(
    read_conn: &Connection,
    write_conn: &Connection,
    embedder: Option<&E>,
    require_embeddings: bool,
    mode: IngestMode,
    produce: P,
) -> Result<IngestCounts>
where
    E: Embedder,
    P: FnOnce(mpsc::Sender<Candidate>) -> Fut,
    Fut: std::future::Future<Output = Result<IngestCounts>>,
{
    let (cand_tx, cand_rx) = mpsc::channel::<Candidate>(BATCH_SIZE * (STAGE_QUEUE + 1));
    let (prep_tx, prep_rx) = mpsc::channel::<Vec<PreparedDecision>>(STAGE_QUEUE);
    let (emb_tx, emb_rx) = mpsc::channel::<Vec<EmbeddedDecision>>(EXTRACT_QUEUE);
    let (bulk_tx, bulk_rx) = mpsc::channel::<Vec<BulkDecisionWrite>>(STAGE_QUEUE);

    let started = Instant::now();
    let mut phases = Phases::default();

    let (source_counts, prepare_out, embed_out, extract_out, write_out) = tokio::try_join!(
        async {
            let counts = produce(cand_tx).await?;
            Result::<_>::Ok(counts)
        },
        prepare_loop(read_conn, cand_rx, prep_tx, require_embeddings, mode),
        embed_loop(embedder, prep_rx, emb_tx),
        extract_loop(write_conn, emb_rx, bulk_tx),
        write_loop(write_conn, bulk_rx),
    )?;

    let mut counts = source_counts;
    counts.merge(&prepare_out.0);
    counts.merge(&write_out.0);
    phases.prepare = prepare_out.1;
    phases.embed = embed_out;
    phases.extract = extract_out;
    phases.write = write_out.1;

    tracing::info!(
        wall_s = started.elapsed().as_secs_f64().round() as u64,
        prepare_s = phases.prepare.as_secs_f64().round() as u64,
        embed_s = phases.embed.as_secs_f64().round() as u64,
        extract_s = phases.extract.as_secs_f64().round() as u64,
        write_s = phases.write.as_secs_f64().round() as u64,
        "pipeline_phases"
    );
    Ok(counts)
}

/// Étage 1 : agrège les candidats en lots et les prépare.
async fn prepare_loop(
    conn: &Connection,
    mut rx: mpsc::Receiver<Candidate>,
    tx: mpsc::Sender<Vec<PreparedDecision>>,
    require_embeddings: bool,
    mode: IngestMode,
) -> Result<(IngestCounts, Duration)> {
    let mut counts = IngestCounts::default();
    let mut elapsed = Duration::ZERO;
    let mut buffer: Vec<Candidate> = Vec::with_capacity(BATCH_SIZE);
    let mut closed = false;
    loop {
        // `recv_many` rend dès qu'**un** candidat est disponible : cette boucle de
        // remplissage groupe le lot avant `fetch_states` et l'embed.
        while !closed && buffer.len() < BATCH_SIZE {
            let room = BATCH_SIZE - buffer.len();
            if rx.recv_many(&mut buffer, room).await == 0 {
                closed = true;
            }
        }
        if buffer.is_empty() {
            break;
        }
        let batch = std::mem::take(&mut buffer);
        let (writes, batch_counts, took) =
            stage_prepare(conn, batch, require_embeddings, mode, DEFAULT_CHUNK_TOKENS).await?;
        counts.merge(&batch_counts);
        elapsed += took;
        // Récepteur tombé = un étage aval a échoué ; `try_join!` porte l'erreur.
        if !writes.is_empty() && tx.send(writes).await.is_err() {
            break;
        }
    }
    Ok((counts, elapsed))
}

/// Étage 2 : vectorise. Poste dominant du pipeline.
async fn embed_loop<E: Embedder>(
    embedder: Option<&E>,
    mut rx: mpsc::Receiver<Vec<PreparedDecision>>,
    tx: mpsc::Sender<Vec<EmbeddedDecision>>,
) -> Result<Duration> {
    let mut elapsed = Duration::ZERO;
    while let Some(writes) = rx.recv().await {
        let (embedded, took) = stage_embed(embedder, writes, DEFAULT_CHUNK_TOKENS).await?;
        elapsed += took;
        if !embedded.is_empty() && tx.send(embedded).await.is_err() {
            break;
        }
    }
    Ok(elapsed)
}

/// Étage 3 : extraction structurée + assemblage des écritures.
///
/// Hydrate le contexte d'extraction **avant** de consommer : le coût fixe du run
/// se paie pendant que l'embedder travaille. La connexion est celle de
/// l'écriture, oisive tant que rien n'est extrait ; l'hydratation (lecture seule,
/// une fois par process) est finie avant la première transaction.
async fn extract_loop(
    conn: &Connection,
    mut rx: mpsc::Receiver<Vec<EmbeddedDecision>>,
    tx: mpsc::Sender<Vec<BulkDecisionWrite>>,
) -> Result<Duration> {
    let ctx = super::extract_ctx(conn).await?;
    let mut elapsed = Duration::ZERO;
    while let Some(items) = rx.recv().await {
        let (bulk, took) = stage_extract(ctx, items).await?;
        elapsed += took;
        if !bulk.is_empty() && tx.send(bulk).await.is_err() {
            break;
        }
    }
    Ok(elapsed)
}

/// Étage 4 : une transaction par lot, séquentielles et dans l'ordre.
async fn write_loop(
    conn: &Connection,
    mut rx: mpsc::Receiver<Vec<BulkDecisionWrite>>,
) -> Result<(IngestCounts, Duration)> {
    let mut counts = IngestCounts::default();
    let mut elapsed = Duration::ZERO;
    while let Some(bulk) = rx.recv().await {
        let started = Instant::now();
        let written = write_writes(conn, bulk).await?;
        elapsed += started.elapsed();
        counts.merge(&written);
    }
    Ok((counts, elapsed))
}
