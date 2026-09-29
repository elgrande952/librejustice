use std::time::Instant;

use anyhow::{bail, Result};
use clap::ValueEnum;

use crate::{pipeline, AssertionsCommand, Command, DbCommand};

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

/// Scheduled chain. `Weekly` = the daily one followed by the weekly
/// maintenance, in a single process.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum ChainKind {
    /// Monday to Saturday: the day's sources, freshness, summaries, publication.
    Daily,
    /// Sunday: the daily one, then the weekly maintenance.
    Weekly,
    /// Before the quotas reset: what the day has not spent.
    Drain,
}

impl ChainKind {
    /// Stable label for the breadcrumbs and the alert (`chain="daily"`).
    fn label(self) -> &'static str {
        match self {
            ChainKind::Daily => "daily",
            ChainKind::Weekly => "weekly",
            ChainKind::Drain => "drain",
        }
    }
}

/// Sources du jour + publication. Aucune étape ne dépend du **succès** de la
/// précédente : l'ordre porte une fraîcheur (les résumés couvrent ce que
/// l'ingest vient d'écrire, IndexNow soumet ce que `generate-summaries` vient
/// de bumper), pas une précondition.
fn daily() -> Vec<Step> {
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
        // 75 s on the passes that worked, 12 to 14/09/2026. The 3 600 it carried
        // dated from the pooled connection that froze for a full hour, fixed on
        // 31/08/2026 and never recalibrated since.
        step(Command::SyncCedh, PLANCHER_SECS),
        step(Command::SyncCjue, PLANCHER_SECS),
        step(Command::SyncCnda, PLANCHER_SECS),
        // Web doctrine, after the decision sources: a note attaches to a
        // decision already in the stock, which the day's harvest just wrote.
        step(Command::SyncAdde, PLANCHER_SECS),
        step(
            Command::SyncHal {
                scan: pipeline::HalScan::Daily,
            },
            PLANCHER_SECS,
        ),
        step(Command::SyncLegi, PLANCHER_SECS),
        step(Command::SyncKali, PLANCHER_SECS),
        step(Command::SyncJorf, PLANCHER_SECS),
        step(
            Command::SyncEuLaw {
                limit: None,
                only: None,
                force: false,
                full: false,
                backlog: false,
                concurrency: 4,
            },
            1_800,
        ),
        step(Command::SyncCirculaires, PLANCHER_SECS),
        step(Command::SyncCirculairesBodies, PLANCHER_SECS),
        // Fond marchés du pot Annonces, en une étape : énumération des canaux,
        // cycle de vie, fiches des lignes neuves, attributions. Indépendant du
        // reste de la chaîne, et seul à écrire dans ce fonds.
        step(
            Command::Annonces {
                centrale_jours: lj_sources_annonces::centraledesmarches::FENETRE_JOURS,
                relancer_muets: false,
            },
            // The internal budgets bound the phases, not this: 1 200 per engine
            // for the harvest, 1 800 for the notice pull. Measured 1 694 s.
            4_500,
        ),
        step(Command::StampFreshness, PLANCHER_SECS),
        step(
            Command::GenerateSummaries {
                limit: None,
                refresh_stale: false,
                cited_min: None,
                by_jurisdiction: false,
                reverse: false,
                models: Vec::new(),
                accounts: Vec::new(),
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

/// Weekly maintenance, after Sunday's daily chain.
fn weekly() -> Vec<Step> {
    vec![
        step(Command::Db(DbCommand::ReindexSearch), 9_000),
        // The net of the registry delta the daily sync-eu-law opens with: the
        // whole registry, a few minutes of SPARQL. Before sync-eu-law, which
        // gives the catalogue entries it opens their body in the same run.
        step(
            Command::SyncEuCatalog {
                full: true,
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
                backlog: false,
                concurrency: 4,
            },
            5_400,
        ),
        // 376 s on 13/09/2026, 913 s on 22/09 for 6 % more hosts: what weighs is
        // the share that does not answer, four probes in series each. The floor
        // cut the 20/09 pass 13 s from the end, and nothing is written before it.
        step(Command::AnnoncesDiscover, 2_700),
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
        // 1 450 s mesurés le 23/08/2026, et le facteur de garde tombe de 4 à 3
        // le 16/09/2026 : la somme des watchdogs de l'hebdo tenait à 300 s sous
        // la période du cron, et c'est ici qu'il y avait de quoi rendre.
        step(
            Command::ReextractFields {
                dry_run: false,
                full: true,
                jurisdiction_type: Vec::new(),
                jurisdiction_code: Vec::new(),
                field: Vec::new(),
                citing_ref_uid: None,
                text_query: None,
                // Runs at 3am, and the watchdog below was measured at 8 workers.
                workers: Some(8),
            },
            4_350,
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
        step(Command::PartiesReconcile { workers: None }, 3_600),
        // Affiliations tirées des appositions : une fonction du corpus, donc à
        // recalculer quand le corpus a bougé. Après la ré-extraction (qui rafraîchit
        // `firm_ord`) et le relink (qui résout les deux bouts).
        step(Command::DeriveAffiliations { apply: true }, PLANCHER_SECS),
        // Reads what the week's harvest failed to reach, so it runs after
        // `annonces-discover`, whose census is what tells an unconnected engine
        // from an unlisted tenant. Writes nothing.
        step(Command::AnnoncesCouverture { limite: 40 }, PLANCHER_SECS),
        // What the daily chain could not write as a collage, its court having
        // published nothing yet: ArianeWeb lands months after the decision, so the
        // summary exists before the words that beat it. ~35 rows a week, a queue
        // built in 3,4 s.
        step(
            Command::UpgradeSummariesCommentaries { limit: None },
            PLANCHER_SECS,
        ),
        // Net under the daily scan: 180 days instead of 30, ~10 requests, and
        // a `warn!` on what it repairs. A week repairing nothing says the daily
        // pass holds.
        step(
            Command::SyncHal {
                scan: pipeline::HalScan::Repair,
            },
            PLANCHER_SECS,
        ),
        // Filet de dérive du régime « dérivé à l'écriture » : ferme la marche, il
        // doit voir l'état final du run.
        step(Command::Reconcile, 3_600),
    ]
}

/// What the day's quotas still hold, spent before they reset.
///
/// Gemini's admitted rate is learnt by steps from a cold start, 8 requests a minute
/// over the first four, measured 20/09/2026, where its day holds 16 000 and expires
/// at 07:00 UTC. A run cut at two hours never leaves slow start and each restart pays
/// it again, hence the rest of the day for the rewrite.
///
/// It serves the cited decisions first: the doors together turn over thousands a day
/// against 3,6 million stale rows, and a cited one is read 3,8 times as often.
fn night_tail() -> Vec<Step> {
    vec![step(
        Command::GenerateSummaries {
            limit: None,
            refresh_stale: true,
            cited_min: Some(1),
            by_jurisdiction: false,
            reverse: false,
            models: Vec::new(),
            accounts: Vec::new(),
        },
        64_800,
    )]
}

/// Étapes de la chaîne, dans l'ordre.
fn steps(kind: ChainKind) -> Vec<Step> {
    if matches!(kind, ChainKind::Drain) {
        return night_tail();
    }
    let mut steps = daily();
    match kind {
        // After every writer of the referential: the states of the texts the
        // day touched, and of those never compiled. Not on Sunday, whose
        // budget is spent to the last 450 s; Monday's three-day window covers it.
        ChainKind::Daily => {
            let after = steps
                .iter()
                .position(|s| s.command.label() == "sync-circulaires-bodies")
                .expect("the referential writers precede the compiler")
                + 1;
            steps.insert(
                after,
                step(
                    Command::Assertions(AssertionsCommand::Compile {
                        full: false,
                        days: 3,
                        text_uid: None,
                        dry_run: false,
                    }),
                    PLANCHER_SECS,
                ),
            );
        }
        ChainKind::Weekly => {
            // The only pass that pays for the tenants silent seven nights: a
            // platform given back is found within a week, the other six do not
            // probe it.
            for s in &mut steps {
                if let Command::Annonces { relancer_muets, .. } = &mut s.command {
                    *relancer_muets = true;
                }
            }
            steps.extend(weekly());
        }
        ChainKind::Drain => unreachable!("rendue plus haut"),
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
    tracing::info!(chain, total, "chain: start");

    let mut failed: Vec<&'static str> = Vec::new();
    for Step {
        command,
        watchdog_secs,
    } in steps
    {
        let step = command.label();
        let depart = Instant::now();
        let ok = crate::run_with_watchdog(command, watchdog_secs)
            .await
            .is_ok();
        if !ok {
            failed.push(step);
        }
        // A step cut by the watchdog logs nothing of its own, and the delay to
        // compare a duration against is here rather than in the command.
        tracing::info!(
            chain,
            step,
            ok,
            watchdog_secs,
            elapsed_s = depart.elapsed().as_secs(),
            "chain: step"
        );
    }

    let elapsed_s = started.elapsed().as_secs();
    if failed.is_empty() {
        tracing::info!(chain, total, elapsed_s, "chain: success");
        return Ok(());
    }
    let steps_failed = failed.join(",");
    tracing::error!(
        chain,
        total,
        chain_failed = failed.len(),
        steps_failed,
        elapsed_s,
        "chain: steps failed"
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
        for kind in [ChainKind::Daily, ChainKind::Weekly, ChainKind::Drain] {
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
        for kind in [ChainKind::Daily, ChainKind::Weekly, ChainKind::Drain] {
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
