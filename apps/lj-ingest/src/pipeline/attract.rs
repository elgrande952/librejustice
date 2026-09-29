//! Reverse probe of the pending rows: when a batch of TARGET decisions
//! arrives (or has its dockets rewritten), probe the pending addresses with
//! the numbers those decisions carry
//! (`lj_extract::identity::pending_target_dockets`) and attract the citations
//! and links that aimed at them, the direction the at-write resolution on the
//! source side does not cover (a cited decision arriving after the ones that
//! cite it). A missed address stays pending, caught up by `reconcile`.

use anyhow::Result;
use lj_store::repository::DecisionRepository;

/// Attire les pendants visant les décisions `ids` (fraîchement écrites).
/// Tourne dans la transaction de l'appelant. Renvoie le nombre de pendants
/// résolus.
pub(super) async fn attract_pending_for(repo: &DecisionRepository<'_>, ids: &[i64]) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut dockets = Vec::new();
    for (code, numeros) in repo.decision_target_identities(ids).await? {
        dockets.extend(lj_extract::identity::pending_target_dockets(
            code.as_deref(),
            &numeros,
        ));
    }
    dockets.sort();
    dockets.dedup();
    let resolved = repo.attract_pending_citations(&dockets).await?;
    if resolved > 0 {
        tracing::info!(
            resolved,
            targets = ids.len(),
            "reverse probe: pendings attracted"
        );
    }
    Ok(resolved)
}
