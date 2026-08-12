//! Sonde inversée des pendants : à l'arrivée (ou réécriture des dockets) d'un
//! lot de décisions CIBLES, générer les clés candidates
//! (`lj_extract::identity::pending_target_keys`, inverse des reformatages des
//! SQL de famille) et attirer les citations/liens pendants qui les visaient —
//! le sens que la résolution at-write côté source ne couvre pas (la décision
//! citée qui arrive après ses citantes). Une clé manquée reste pendante,
//! rattrapée par `reconcile`.

use anyhow::Result;
use lj_store::repository::DecisionRepository;

/// Attire les pendants visant les décisions `ids` (fraîchement écrites).
/// Tourne dans la transaction de l'appelant. Renvoie le nombre de pendants
/// résolus.
pub(super) async fn attract_pending_for(repo: &DecisionRepository<'_>, ids: &[i64]) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut keys = Vec::new();
    let mut refs = Vec::new();
    for (jt, code, dockets, canonical_ref) in repo.decision_target_identities(ids).await? {
        keys.extend(lj_extract::identity::pending_target_keys(
            &jt,
            code.as_deref(),
            &dockets,
        ));
        refs.extend(canonical_ref);
    }
    let resolved = repo.attract_pending_citations(&keys, &refs).await?;
    if resolved > 0 {
        tracing::info!(
            resolved,
            targets = ids.len(),
            "sonde inversée : pendants attirés"
        );
    }
    Ok(resolved)
}
