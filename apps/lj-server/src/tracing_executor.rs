//! Exécuteur `any_spawner` qui rattache chaque tâche Leptos au span courant.
//!
//! Une tâche tokio n'hérite d'aucun contexte de son parent : sans cela, chaque
//! `Resource` / `Suspense` ouvre une trace neuve au lieu de se rattacher à la
//! requête.

use any_spawner::{CustomExecutor, Executor, PinnedFuture, PinnedLocalFuture};
use anyhow::{anyhow, Result};
use tracing::Instrument;

struct TracingExecutor;

impl CustomExecutor for TracingExecutor {
    fn spawn(&self, fut: PinnedFuture<()>) {
        tokio::spawn(fut.instrument(tracing::Span::current()));
    }

    fn spawn_local(&self, fut: PinnedLocalFuture<()>) {
        tokio::task::spawn_local(fut.instrument(tracing::Span::current()));
    }

    fn poll_local(&self) {}
}

/// Pose l'exécuteur. À appeler une seule fois, avant `generate_route_list` et
/// avant de servir : `leptos_axum` inscrit sinon l'exécuteur tokio nu.
pub fn install() -> Result<()> {
    Executor::init_custom_executor(TracingExecutor).map_err(|e| {
        anyhow!(
            "exécuteur any_spawner déjà inscrit ({e:?}) : \
             `install()` doit précéder tout appel à leptos_axum"
        )
    })
}

#[cfg(test)]
mod tests {
    /// `install()` écrit un `OnceLock` de process : ce test est le seul du
    /// binaire à toucher l'exécuteur.
    #[test]
    fn une_tache_leptos_herite_du_span_courant() {
        use tracing_subscriber::util::SubscriberInitExt as _;

        super::install().unwrap();
        // Sans abonné, un span n'a pas d'identifiant et la comparaison est vide.
        let _abonne = tracing_subscriber::registry().set_default();

        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let racine = tracing::info_span!("racine");
                let attendu = racine.id();
                assert!(attendu.is_some(), "la racine doit avoir un id");
                let (tx, rx) = tokio::sync::oneshot::channel();
                racine.in_scope(|| {
                    super::Executor::spawn(async move {
                        let _ = tx.send(tracing::Span::current().id());
                    });
                });
                assert_eq!(rx.await.unwrap(), attendu, "span perdu au spawn");
            });
    }
}
