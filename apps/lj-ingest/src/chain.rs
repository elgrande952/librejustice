//! Chaînes cron : listes ordonnées d'étapes exécutées indépendamment.
//!
//! L'ordre est conservé, l'abandon ne l'est pas : chaque étape tourne sous son
//! propre watchdog, les échecs sont collectés, et la chaîne sort non-zéro après
//! un `error!` récapitulatif (clé d'alerte Grafana : `chain_failed`).
//!
//! Le délai du watchdog est **porté par l'étape**, pas par la chaîne : un
//! plafond unique se cale sur la plus lente, et laisse les autres pendues des
//! heures sur un amont muet.

use std::time::Instant;

use anyhow::{bail, Result};
use clap::ValueEnum;

use crate::{pipeline, Command, DbCommand};

/// Plancher du délai d'une étape. En dessous, une journée simplement lente
/// (amont chargé, file plus grosse que d'habitude) se ferait couper.
const PLANCHER_SECS: u64 = 900;

/// Une étape de chaîne et son délai de watchdog.
struct Step {
    command: Command,
    watchdog_secs: u64,
}

/// Étape et son délai, calé sur ~3× le maximum observé au cron et jamais sous
/// [`PLANCHER_SECS`] : le watchdog vise l'amont pendu, pas la journée lente.
/// Une étape coupée n'arrête pas la chaîne et repart au run suivant, donc un
/// délai serré coûte un run d'étape, quand un délai large coûte la nuit.
fn step(command: Command, watchdog_secs: u64) -> Step {
    Step {
        command,
        watchdog_secs,
    }
}

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
fn quotidienne() -> Vec<Step> {
    vec![
        step(Command::Sync, 1_800),
        step(
            Command::Ingest {
                with_embeddings: true,
                mode: pipeline::IngestMode::MissingHash,
            },
            3_600,
        ),
        step(
            Command::SyncDila {
                fond: pipeline::Fond::Jade,
            },
            PLANCHER_SECS,
        ),
        step(
            Command::SyncDila {
                fond: pipeline::Fond::Constit,
            },
            PLANCHER_SECS,
        ),
        step(
            Command::SyncDila {
                fond: pipeline::Fond::Cnil,
            },
            PLANCHER_SECS,
        ),
        // Après JADE, qui vient de verser les décisions CE du jour : le
        // recensement les compare aux trois fonds ArianeWeb. Trois requêtes quand
        // rien n'a bougé, une année de pages quand une année a bougé.
        step(
            Command::SyncAriane {
                years: Vec::new(),
                census_only: false,
                limit: None,
            },
            PLANCHER_SECS,
        ),
        step(Command::SyncCedh, 3_600),
        step(Command::SyncCjue, PLANCHER_SECS),
        step(Command::SyncCnda, PLANCHER_SECS),
        step(Command::SyncLegi, PLANCHER_SECS),
        step(Command::SyncKali, PLANCHER_SECS),
        step(Command::SyncJorf, PLANCHER_SECS),
        step(
            Command::SyncEuLaw {
                limit: None,
                only: None,
                force: false,
                full: false,
            },
            1_800,
        ),
        step(Command::SyncCirculaires, PLANCHER_SECS),
        step(Command::SyncCirculairesBodies, PLANCHER_SECS),
        // Fond marchés du pot Annonces : ré-énumération complète des canaux puis
        // cycle de vie. Indépendant du reste de la chaîne.
        step(Command::Annonces, 1_800),
        // Cycle de vie du re-groupage du jour : orphelines résolues en
        // redirections. La forme, elle, s'applique à l'écriture ; son filet rétroactif
        // est hebdomadaire (`--formes`). Idempotent.
        step(
            Command::AnnoncesRegulariser { formes: false },
            PLANCHER_SECS,
        ),
        // Dépôts à racine nue, sans clé de jointure : la jumelle — par
        // référence, par titre + date d'échéance, ou par titre + acheteur dans
        // l'instance déclarée — rattrape ce que la moisson ne peut pas voir.
        // Avant l'enrichissement : la ligne fusionnée hérite du CPV BOAMP et sort de
        // la file.
        step(Command::AnnoncesJumeler { applique: true }, PLANCHER_SECS),
        // Le résidu sans jumelle en stock : la recherche de la plateforme, par
        // la référence que l'avis publie, atteint les consultations que la
        // liste énumérable de l'instance ne montre pas (strate hors API chez
        // Atexo et Atline). Après le jumelage, qui vient de vider le stock
        // appariable — il ne reste que ce qu'aucune clé ne peut voir.
        step(
            Command::AnnoncesResoudre {
                applique: true,
                limite: 500,
            },
            2_400,
        ),
        // Ce que les listings ne portent pas, tiré page à page. Après la moisson,
        // qui vient de renouveler la file.
        step(
            Command::AnnoncesEnrichir {
                limite: 2_000,
                budget_min: 15,
                sans_acheteur: false,
            },
            PLANCHER_SECS,
        ),
        step(Command::StampFreshness, PLANCHER_SECS),
        step(
            Command::GenerateSummaries {
                limit: None,
                concurrency: None,
                refresh_stale: false,
            },
            5_400,
        ),
        step(Command::Db(DbCommand::Analyze), PLANCHER_SECS),
        step(Command::Db(DbCommand::GcMcp), PLANCHER_SECS),
        // Après toutes les étapes qui écrivent des décisions, et avant le
        // sitemap, qui tire de ces compteurs ses nombres de pages.
        step(Command::Precompute, PLANCHER_SECS),
        step(Command::Sitemap { dry_run: false }, PLANCHER_SECS),
        step(
            Command::Indexnow {
                since_hours: 24.0,
                max_urls: 10_000,
            },
            PLANCHER_SECS,
        ),
        // En dernier : les invariants mesurent l'état que la chaîne vient de
        // produire. Ils ne la font pas échouer.
        step(Command::Invariants, PLANCHER_SECS),
    ]
}

/// Maintenance hebdomadaire, après la quotidienne du dimanche.
fn hebdomadaire() -> Vec<Step> {
    vec![
        step(Command::Db(DbCommand::ReindexSearch), 9_000),
        // The only door an EU act that no decision cites comes through. Before
        // sync-eu-law, which gives the catalogue entries it opens their body in
        // the same run.
        step(
            Command::SyncEuCatalog {
                depuis: None,
                jusqu_a: None,
                limit: None,
            },
            PLANCHER_SECS,
        ),
        // What the daily JO ingest keeps depositing: an act named by DILA, its
        // body at EUR-Lex. Left alone the two identities drift apart again.
        step(Command::SyncEuJorf { limit: None }, PLANCHER_SECS),
        // Filet du delta eu-law : balayage intégral des notices CELLAR,
        // auto-réparant (dérive du manifeste, échec delta, versions futures
        // resemées). Avant extract-text-refs, qui rejoue les renvois sur les corps que
        // ce balayage vient de rafraîchir.
        step(
            Command::SyncEuLaw {
                limit: None,
                only: None,
                force: false,
                full: true,
            },
            5_400,
        ),
        // Le parc de profils d'acheteurs bouge en mois : un recensement
        // hebdomadaire suffit, la moisson quotidienne lit le recensement en base.
        step(Command::AnnoncesDiscover, PLANCHER_SECS),
        // Filet de forme du pot Annonces : l'ingest écrit propre à la
        // construction, ce passage porte une règle nouvelle au stock clos que
        // la moisson ne revisite jamais (titres, acheteurs, lots).
        step(Command::AnnoncesRegulariser { formes: true }, PLANCHER_SECS),
        step(
            Command::Registries {
                source: pipeline::RegistrySource::Avocats,
                with_history: false,
            },
            PLANCHER_SECS,
        ),
        step(Command::PartiesRelink { full: false }, PLANCHER_SECS),
        // 1 450 s mesurés le 23/08/2026, donc un facteur 4 de garde : c'est le
        // délai le plus large au regard de sa mesure, et la somme des watchdogs
        // de la chaîne doit tenir sous la période du cron.
        step(
            Command::ReextractFields {
                dry_run: false,
                full: true,
                jurisdiction_type: Vec::new(),
                field: Vec::new(),
                citing_ref_uid: None,
                text_query: None,
                workers: None,
            },
            6_300,
        ),
        step(
            Command::UsageTerms {
                chunks: 32,
                min_citations: 10,
                workers: 6,
            },
            3_600,
        ),
        step(Command::BuildSuggest, 3_600),
        // 75 fiches, 81 ancres, et le Conseil d'État en ajoute quelques-unes par
        // an : hebdomadaire suffit. Avant les renvois, qui recomposent le titre
        // indexé des décisions qu'elle vient de nommer.
        step(Command::SyncCeList, PLANCHER_SECS),
        // Les renvois se reprojettent en entier (110 992 bundles ArianeWeb) et
        // reposent les noms d'usage ; le minage des surnoms de lois lit à côté
        // ~9 000 corps portant « dite loi ».
        step(Command::Renvois, PLANCHER_SECS),
        step(Command::AliasCandidats { revue: None }, PLANCHER_SECS),
        step(Command::ExtractTextRefs, 2_400),
        // Filet de dérive des acteurs, après la ré-extraction qui est l'écrivain
        // incrémental de `decision_party` : son compteur de réécritures mesure ce que
        // le régime at-write a laissé fuir. Un compteur non nul dans Grafana = fuite à
        // diagnostiquer.
        step(Command::PartiesReconcile { workers: None }, 3_600),
        // Affiliations tirées des appositions : une fonction du corpus, donc à
        // recalculer quand le corpus a bougé. Après la ré-extraction (qui rafraîchit
        // `firm_ord`) et le relink (qui résout les deux bouts).
        step(Command::DeriveAffiliations { apply: true }, PLANCHER_SECS),
        // Filet de dérive du régime « dérivé à l'écriture » : ferme la marche, il
        // doit voir l'état final du run.
        step(Command::Reconcile, 3_600),
    ]
}

/// Étapes de la chaîne, dans l'ordre.
fn steps(kind: ChainKind) -> Vec<Step> {
    let mut steps = quotidienne();
    if matches!(kind, ChainKind::Hebdo) {
        steps.extend(hebdomadaire());
    }
    steps
}

/// Déroule la chaîne. Chaque étape passe par
/// [`crate::run_with_watchdog`] — qui logue déjà son propre début/fin/échec —
/// sous **son** délai, et un échec n'interrompt pas les suivantes. Sort `Err`
/// si au moins une étape a échoué, pour que le cron marque le run en erreur.
///
/// `LIBREJUSTICE_INGEST_WATCHDOG_SECS` ne s'applique pas ici : il plafonne une
/// invocation directe, la chaîne porte ses propres délais.
pub(crate) async fn run(kind: ChainKind) -> Result<()> {
    let chain = kind.label();
    let steps = steps(kind);
    let total = steps.len();
    let started = Instant::now();
    tracing::info!(chain, total, "chain : début");

    let mut failed: Vec<&'static str> = Vec::new();
    for Step {
        command,
        watchdog_secs,
    } in steps
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Période du cron : deux runs consécutifs sont à 24 h.
    const PERIODE_CRON_SECS: u64 = 86_400;

    /// Toutes les étapes coupées au watchdog doivent tenir sous la période du
    /// cron, sinon deux chaînes se marchent dessus sur la même base. C'est la
    /// borne qu'un plafond unique ne donne pas : 39 étapes à 4 h valent 156 h.
    #[test]
    fn le_pire_cas_tient_sous_la_periode_du_cron() {
        for kind in [ChainKind::Quotidienne, ChainKind::Hebdo] {
            let pire: u64 = steps(kind).iter().map(|s| s.watchdog_secs).sum();
            assert!(
                pire < PERIODE_CRON_SECS,
                "chaîne {} : pire cas {pire} s, période {PERIODE_CRON_SECS} s",
                kind.label()
            );
        }
    }

    /// Aucun délai sous le plancher : une journée simplement lente ne doit pas
    /// suffire à faire couper une étape.
    #[test]
    fn aucun_delai_sous_le_plancher() {
        for kind in [ChainKind::Quotidienne, ChainKind::Hebdo] {
            for s in steps(kind) {
                assert!(
                    s.watchdog_secs >= PLANCHER_SECS,
                    "étape {} : délai {} s sous le plancher {PLANCHER_SECS} s",
                    s.command.label(),
                    s.watchdog_secs
                );
            }
        }
    }
}
