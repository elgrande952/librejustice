//! Filet unique de dérive : la commande `lj-ingest reconcile` enchaîne TOUS les
//! filets du régime « dérivé à l'écriture » — rôles de textes + dénorm articles,
//! arrays légaux dénormalisés, liens/citations pendants (chronologie,
//! décision→décision, texte→décision). Chaque brique est idempotente ; en régime
//! les écrivains at-write ne laissent rien fuir et chaque passe est ~no-op. Un
//! compteur > 0 = fuite d'un écrivain à diagnostiquer.
//!
//! Le volet parties (relink + annuaire) vit dans `parties-relink`, collé au reload
//! hebdo du registre CNB dont il dépend. Tout nouveau filet de dérive DOIT
//! s'enregistrer ici.

use anyhow::{anyhow, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Commande `reconcile` : rejoue tous les filets de dérive. Idempotent.
pub async fn reconcile_all() -> Result<()> {
    // Rôles des textes + dénorm articles (pleine table, dérive attendue ~0).
    super::backfill_text_roles().await?;
    // Arrays légaux dénormalisés (garde IS DISTINCT FROM, warn si dérive > 0).
    crate::citations_maintenance::resync_arrays().await?;

    // Liens/citations pendants : rejoue les résolveurs pleine-table. En régime
    // ils sont résolus à l'écriture (côté source dans la transaction du lot,
    // côté cible par la sonde inversée `pipeline::attract`).
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let resolved = repo.resolve_pending_decision_links().await?;
    tracing::info!(resolved, "liens de chronologie pendants résolus");
    let resolved = repo.resolve_pending_case_citations().await?;
    tracing::info!(resolved, "citations de jurisprudence pendantes résolues");
    let resolved = repo.resolve_pending_text_case_citations().await?;
    tracing::info!(resolved, "citations texte→jurisprudence pendantes résolues");

    // Liens conseils devenus ambigus à date : le delta du reload CNB ne voit que le
    // voisinage des identités ajoutées, ce filet plein rattrape l'ambiguïté de stock.
    // Dé-liage puis re-résolution des SEULES clés libérées — nul en régime,
    // proportionnel à la dérive sinon. Une clé restée ambiguë demeure pendante ; une
    // clé dont le dé-liage révèle le bon porteur se relie ici.
    let freed = repo.cnb_unlink_ambiguous().await?;
    let relinked = repo.resolve_pending_counsel_by_keys(&freed).await?;
    tracing::info!(
        keys = freed.len(),
        relinked,
        "liens conseils ambigus à date dé-liés puis re-résolus"
    );

    // Commentaires de norme curés : le dataset sous `state_dir` est la source de
    // vérité, rejoué ici pour que le retrait d'une entrée et la correction d'une ancre
    // morte se propagent sans intervention. L'ancre étant le slug, la dérive attendue
    // est nulle ; `dangling > 0` = dataset à corriger.
    let report = super::reconcile_article_commentaires().await?;
    tracing::info!(
        entries = report.entries,
        upserted = report.upserted,
        tombstoned = report.tombstoned,
        dangling = report.dangling,
        "commentaires de norme curés réconciliés"
    );
    Ok(())
}
