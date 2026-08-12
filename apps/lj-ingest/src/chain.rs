//! Chaînes cron : listes ordonnées d'étapes exécutées indépendamment.
//!
//! L'ordre est conservé, l'abandon ne l'est pas : chaque étape tourne sous son
//! propre watchdog, les échecs sont collectés, et la chaîne sort non-zéro après
//! un `error!` récapitulatif (clé d'alerte Grafana : `chain_failed`).

use std::time::Instant;

use anyhow::{bail, Result};
use clap::ValueEnum;

use crate::{pipeline, Command, DbCommand};

/// Chaîne planifiée. `Hebdo` = la quotidienne suivie de la maintenance
/// hebdomadaire, dans un seul processus.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum ChainKind {
    /// Lundi → samedi : sources du jour, fraîcheur, résumés, publication.
    Quotidienne,
    /// Dimanche : la quotidienne puis la maintenance hebdomadaire.
    Hebdo,
}

impl ChainKind {
    /// Libellé stable pour les breadcrumbs / l'alerte (`chain="quotidienne"`).
    fn label(self) -> &'static str {
        match self {
            ChainKind::Quotidienne => "quotidienne",
            ChainKind::Hebdo => "hebdo",
        }
    }
}

/// Sources du jour + publication. Aucune étape ne dépend du **succès** de la
/// précédente : l'ordre porte une fraîcheur (les résumés couvrent ce que
/// l'ingest vient d'écrire, IndexNow soumet ce que `generate-summaries` vient
/// de bumper), pas une précondition.
fn quotidienne() -> Vec<Command> {
    vec![
        Command::Sync,
        Command::Ingest {
            with_embeddings: true,
            mode: pipeline::IngestMode::MissingHash,
        },
        Command::SyncDila {
            fond: pipeline::Fond::Jade,
        },
        Command::SyncDila {
            fond: pipeline::Fond::Constit,
        },
        Command::SyncDila {
            fond: pipeline::Fond::Cnil,
        },
        Command::SyncCedh,
        Command::SyncCjue,
        Command::SyncCnda,
        Command::SyncLegi,
        Command::SyncKali,
        Command::SyncJorf,
        Command::SyncEuLaw {
            limit: None,
            only: None,
            force: false,
            full: false,
        },
        Command::SyncCirculaires,
        Command::SyncCirculairesBodies,
        // Fond marchés du pot Annonces : ré-énumération complète des canaux puis
        // cycle de vie. Indépendant du reste de la chaîne.
        Command::Annonces,
        // Cycle de vie du re-groupage du jour : orphelines résolues en
        // redirections. La forme, elle, s'applique à l'écriture ; son filet rétroactif
        // est hebdomadaire (`--formes`). Idempotent.
        Command::AnnoncesRegulariser { formes: false },
        // Dépôts à racine nue, sans clé de jointure : la jumelle titre +
        // acheteur pliés dans l'instance rattrape ce que la moisson ne peut pas voir.
        // Avant l'enrichissement : la ligne fusionnée hérite du CPV BOAMP et sort de
        // la file.
        Command::AnnoncesJumeler { applique: true },
        // Ce que les listings ne portent pas, tiré page à page. Après la moisson,
        // qui vient de renouveler la file.
        Command::AnnoncesEnrichir {
            limite: 2_000,
            budget_min: 15,
            sans_acheteur: false,
        },
        Command::StampFreshness,
        Command::GenerateSummaries {
            limit: None,
            concurrency: None,
            refresh_stale: false,
        },
        Command::Db(DbCommand::Analyze),
        Command::Db(DbCommand::GcMcp),
        Command::Sitemap { dry_run: false },
        Command::Indexnow {
            since_hours: 24.0,
            max_urls: 10_000,
        },
    ]
}

/// Maintenance hebdomadaire, après la quotidienne du dimanche.
fn hebdomadaire() -> Vec<Command> {
    vec![
        Command::Db(DbCommand::ReindexSearch),
        // Filet du delta eu-law : balayage intégral des notices CELLAR,
        // auto-réparant (dérive du manifeste, échec delta, versions futures
        // resemées). Avant extract-text-refs, qui rejoue les renvois sur les corps que
        // ce balayage vient de rafraîchir.
        Command::SyncEuLaw {
            limit: None,
            only: None,
            force: false,
            full: true,
        },
        // Le parc de profils d'acheteurs bouge en mois : un recensement
        // hebdomadaire suffit, la moisson quotidienne lit le recensement en base.
        Command::AnnoncesDiscover,
        // Filet de forme du pot Annonces : l'ingest écrit propre à la
        // construction, ce passage porte une règle nouvelle au stock clos que
        // la moisson ne revisite jamais (titres, acheteurs, lots).
        Command::AnnoncesRegulariser { formes: true },
        Command::Registries {
            source: pipeline::RegistrySource::Avocats,
            with_history: false,
        },
        Command::PartiesRelink { full: false },
        Command::ReextractFields {
            overwrite: false,
            dry_run: false,
            full: true,
            jurisdiction_type: Vec::new(),
            field: Vec::new(),
            citing_ref_uid: None,
            workers: None,
        },
        Command::UsageTerms {
            chunks: 32,
            min_citations: 10,
            workers: 6,
        },
        Command::BuildSuggest,
        Command::ExtractTextRefs,
        // Filet de dérive des acteurs, après la ré-extraction qui est l'écrivain
        // incrémental de `decision_party` : son compteur de réécritures mesure ce que
        // le régime at-write a laissé fuir. Un compteur non nul dans Grafana = fuite à
        // diagnostiquer.
        Command::PartiesReconcile { workers: None },
        // Affiliations tirées des appositions : une fonction du corpus, donc à
        // recalculer quand le corpus a bougé. Après la ré-extraction (qui rafraîchit
        // `firm_ord`) et le relink (qui résout les deux bouts).
        Command::DeriveAffiliations { apply: true },
        // Filet de dérive du régime « dérivé à l'écriture » : ferme la marche, il
        // doit voir l'état final du run.
        Command::Reconcile,
    ]
}

/// Étapes de la chaîne, dans l'ordre.
fn steps(kind: ChainKind) -> Vec<Command> {
    let mut steps = quotidienne();
    if matches!(kind, ChainKind::Hebdo) {
        steps.extend(hebdomadaire());
    }
    steps
}

/// Déroule la chaîne. Chaque étape passe par
/// [`crate::run_with_watchdog`] — qui logue déjà son propre début/fin/échec —
/// et un échec n'interrompt pas les suivantes. Sort `Err` si au moins une étape
/// a échoué, pour que le cron marque le run en erreur.
pub(crate) async fn run(kind: ChainKind, watchdog_secs: u64) -> Result<()> {
    let chain = kind.label();
    let steps = steps(kind);
    let total = steps.len();
    let started = Instant::now();
    tracing::info!(chain, total, "chain : début");

    let mut failed: Vec<&'static str> = Vec::new();
    for command in steps {
        let step = command.label();
        if crate::run_with_watchdog(command, watchdog_secs)
            .await
            .is_err()
        {
            failed.push(step);
        }
    }

    let elapsed_s = started.elapsed().as_secs();
    if failed.is_empty() {
        tracing::info!(chain, total, elapsed_s, "chain : succès");
        return Ok(());
    }
    // Signal d'alarme : un seul événement à interroger côté Grafana
    // (`chain_failed > 0`), qui porte la liste des étapes tombées.
    let steps_failed = failed.join(",");
    tracing::error!(
        chain,
        total,
        chain_failed = failed.len(),
        steps_failed,
        elapsed_s,
        "chain : étapes en échec"
    );
    bail!(
        "chain {chain} : {}/{total} étapes en échec ({steps_failed})",
        failed.len()
    )
}
