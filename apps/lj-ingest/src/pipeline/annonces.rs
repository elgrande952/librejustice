//! Fond marchés du pot Annonces : moisson quotidienne des canaux,
//! réconciliation multi-canal, upsert.
//!
//! On enregistre le flux, on ne stampe aucun état : la fenêtre « ouverte » se
//! calcule à la lecture (`echeance > NOW()`). Un connecteur cassé ne coûte donc
//! que de la fraîcheur — ses annonces sortent de la fenêtre par leur échéance —
//! mais l'écart doit se voir : l'alarme `annonces_moteurs_degrades` sonne quand
//! un moteur rend zéro ligne ou moins de 90 % de ce qu'il déclare.
//!
//! Ré-énumération complète quotidienne, pas de delta incrémental à maintenir.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{anyhow, Result};
use jiff::tz::TimeZone;
use jiff::Timestamp;

use lj_core::annonces::{
    canonicaliser, cpv_division_uid, marche_uid_de_resultat, plie, AnnonceBrute, AnnonceCanonique,
    LotAttribue, ResultatMarche,
};
use lj_dtos::ANNONCE_FOND_MARCHE;
use lj_sources::annonces::{self, MoteurRun};
use lj_store::repository::{
    AnnonceAEnrichir, AnnonceChamps, AnnonceSourceWriteItem, AnnonceWriteItem, AnnonceWriteStats,
    DecisionRepository, DetailPose, ResultatPose,
};

use crate::config::Settings;

/// Ratio collecté/annoncé sous lequel un moteur est déclaré dégradé : 90 %
/// laisse la marge du churn normal sans laisser passer une casse de parseur.
const RATIO_MIN: f64 = 0.90;

fn degrade(run: &MoteurRun) -> bool {
    run.rows.is_empty()
        || run
            .annonce
            .is_some_and(|a| a > 0 && (run.rows.len() as f64) < a as f64 * RATIO_MIN)
}

/// Moisson quotidienne : tous les canaux, puis réconciliation et écriture.
pub async fn annonces() -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Upserts de masse par unnest : hors statement_timeout.
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // Les moteurs multi-tenants énumèrent les hôtes recensés par
    // `annonces-discover` ; sans recensement, ils ne rendent rien (et le
    // sanity check les déclare dégradés, sans fermeture).
    let hotes_atexo = repo
        .hotes_profil_acheteur("atexo")
        .await
        .map_err(|e| anyhow!("hotes atexo: {e}"))?;
    let hotes_safetender = repo
        .hotes_profil_acheteur("safetender")
        .await
        .map_err(|e| anyhow!("hotes safetender: {e}"))?;
    // Les plateformes avérées (tous moteurs), validateurs du dépôt de
    // dernier ressort du connecteur BOAMP.
    let plateformes: HashSet<String> = repo
        .hotes_plateformes_classees()
        .await
        .map_err(|e| anyhow!("hotes plateformes: {e}"))?
        .into_iter()
        .collect();
    tracing::info!(
        atexo = hotes_atexo.len(),
        safetender = hotes_safetender.len(),
        plateformes = plateformes.len(),
        "annonces : profils recensés"
    );

    // Les canaux en parallèle — le mur horaire est Klekoon, en série à 1 req/s.
    // Un canal en erreur franche vaut un run vide de ce canal, pas l'échec de la
    // moisson.
    let (
        boamp,
        atexo,
        safetender,
        aws,
        dematis,
        achatpublic,
        atline,
        synapse,
        xmarches,
        aji,
        klekoon,
        modula,
    ) = tokio::join!(
        annonces::boamp::enumerate(&plateformes),
        annonces::atexo::enumerate(&hotes_atexo),
        annonces::safetender::enumerate(&hotes_safetender),
        annonces::aws::enumerate(),
        annonces::dematis::enumerate(),
        annonces::achatpublic::enumerate(),
        annonces::atline::enumerate(),
        annonces::synapse::enumerate(),
        annonces::xmarches::enumerate(),
        annonces::aji::enumerate(),
        annonces::klekoon::enumerate(),
        annonces::modula::enumerate(),
    );

    let runs: Vec<(&'static str, MoteurRun)> = [
        ("boamp", boamp),
        ("atexo", atexo),
        ("safetender", safetender),
        ("aws", aws),
        ("dematis", dematis),
        ("achatpublic", achatpublic),
        ("atline", atline),
        ("synapse", synapse),
        ("xmarches", xmarches),
        ("aji", aji),
        ("klekoon", klekoon),
        ("modula", modula),
    ]
    .into_iter()
    .filter_map(|(moteur, res)| match res {
        Ok(run) => Some((moteur, run)),
        Err(e) => {
            tracing::error!(moteur, error = %e, "annonces : moteur en échec total");
            None
        }
    })
    .collect();

    let mut degrades: Vec<&'static str> = Vec::new();
    let mut brutes: Vec<AnnonceBrute> = Vec::new();
    for (moteur, run) in runs {
        let sain = !degrade(&run);
        tracing::info!(
            moteur,
            collecte = run.rows.len(),
            annonce = run.annonce,
            echecs = run.echecs,
            sain,
            "annonces : moteur"
        );
        if !sain {
            degrades.push(moteur);
        }
        brutes.extend(run.rows);
    }

    let aujourdhui = Timestamp::now()
        .to_zoned(TimeZone::get("Europe/Paris").expect("tzdb embarquée"))
        .date();
    let (canoniques, stats, absorbees) = reconcilier_et_ecrire(&repo, brutes, aujourdhui).await?;
    let relies = repo
        .relier_annonces_par_denomination()
        .await
        .map_err(|e| anyhow!("rattachement par dénomination: {e}"))?;

    // Fond `attribution` : la fenêtre active des avis de résultat. Un échec ne
    // coûte que de la fraîcheur d'enrichissement, pas la moisson.
    match annonces::boamp::resultats().await {
        Ok(resultats) => match ecrire_resultats(&repo, resultats).await {
            Ok((n, s, rattaches)) => tracing::info!(
                resultats = n,
                annonces = s.annonces,
                canaux = s.canaux,
                aretes = s.aretes,
                rattaches,
                "annonces : résultats de marché"
            ),
            Err(e) => tracing::error!(error = %e, "annonces : écriture des résultats en échec"),
        },
        Err(e) => tracing::error!(error = %e, "annonces : résultats de marché en échec"),
    }

    // Tripwire de volumétrie.
    for t in repo
        .tailles_tables_annonces()
        .await
        .map_err(|e| anyhow!("tailles: {e}"))?
    {
        tracing::info!(table = %t.table, bytes = t.bytes, "annonces : taille de table");
    }

    tracing::info!(
        canoniques,
        annonces = stats.annonces,
        canaux = stats.canaux,
        aretes = stats.aretes,
        absorbees,
        relies,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces"
    );
    if !degrades.is_empty() {
        // Clé d'alerte Grafana : `annonces_moteurs_degrades`. Non fatal — le
        // reste de la moisson est écrit, mais l'écart doit se voir.
        tracing::error!(
            annonces_moteurs_degrades = degrades.len(),
            moteurs = degrades.join(","),
            "annonces : moteurs dégradés, couverture partielle"
        );
    }
    Ok(())
}

/// Le dataset BOAMP ne porte pas d'avis paru avant.
const PLANCHER_BOAMP: jiff::civil::Date = jiff::civil::date(2010, 1, 1);

/// Backfill de l'historique BOAMP. Mois par mois, du plus ancien au plus
/// récent : les avis de marché de la fenêtre, puis ses résultats. L'ordre est la
/// décision — un résultat cherche sa consultation en stock, et les mois
/// antérieurs sont déjà écrits quand il arrive. Reprenable : tout est
/// idempotent, relancer sur une fenêtre déjà passée ne réécrit rien.
pub async fn annonces_backfill(
    depuis: Option<jiff::civil::Date>,
    jusqu: jiff::civil::Date,
) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let plateformes: HashSet<String> = repo
        .hotes_plateformes_classees()
        .await
        .map_err(|e| anyhow!("hotes plateformes: {e}"))?
        .into_iter()
        .collect();

    let repris = depuis.unwrap_or(PLANCHER_BOAMP);
    tracing::info!(depuis = %repris, jusqu = %jusqu, "backfill : début");

    let un_mois = jiff::Span::new().months(1);
    let mut lo = repris.first_of_month();
    let (mut avis_total, mut resultats_total, mut orphelins_total, mut rattaches_total) =
        (0u64, 0u64, 0u64, 0u64);
    while lo < jusqu {
        let hi = lo
            .checked_add(un_mois)
            .map_err(|e| anyhow!("mois suivant de {lo}: {e}"))?;
        let mois = std::time::Instant::now();

        let brutes = annonces::boamp::avis_historiques(lo, hi, &plateformes)
            .await
            .map_err(|e| anyhow!("avis historiques {lo}: {e}"))?;
        let vus = brutes.len();
        let (_, mois_stats, _) = reconcilier_et_ecrire(&repo, brutes, hi)
            .await
            .map_err(|e| anyhow!("avis {lo}: {e}"))?;
        let ecrits = mois_stats.annonces;

        let resultats = annonces::boamp::resultats_historiques(lo, hi)
            .await
            .map_err(|e| anyhow!("résultats historiques {lo}: {e}"))?;
        let (n, stats, rattaches) = ecrire_resultats(&repo, resultats).await?;

        avis_total += vus as u64;
        resultats_total += n as u64;
        orphelins_total += stats.annonces;
        rattaches_total += rattaches;
        tracing::info!(
            mois = %lo.strftime("%Y-%m").to_string(),
            avis = vus,
            avis_ecrits = ecrits,
            resultats = n,
            orphelins = stats.annonces,
            rattaches,
            elapsed_s = mois.elapsed().as_secs(),
            "backfill : mois"
        );
        lo = hi;
    }

    tracing::info!(
        avis = avis_total,
        resultats = resultats_total,
        orphelins = orphelins_total,
        rattaches = rattaches_total,
        elapsed_s = debut.elapsed().as_secs(),
        "backfill : fin"
    );
    Ok(())
}

fn date_chrono(d: jiff::civil::Date) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32)
        .expect("date civile valide par construction")
}

/// Réconcilie un batch contre le stock et l'écrit : charge les affectations
/// existantes des canaux du batch et de leurs clés de jointure, canonicalise,
/// upserte par lots (10 arrays de paramètres par statement), puis applique les
/// fusions découvertes — l'élue vient d'être écrite, l'absorbée se vide de ses
/// canaux de batch, il reste le hors-batch, la redirection et la suppression.
/// Rend (canoniques, stats, absorbées).
async fn reconcilier_et_ecrire(
    repo: &DecisionRepository<'_>,
    brutes: Vec<AnnonceBrute>,
    aujourdhui: jiff::civil::Date,
) -> Result<(usize, AnnonceWriteStats, u64)> {
    let cles: Vec<String> = brutes
        .iter()
        .flat_map(|b| std::iter::once(b.source_uid.clone()).chain(b.cles_jointure.iter().cloned()))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let existants: HashMap<String, String> = repo
        .annonces_par_source(&cles)
        .await
        .map_err(|e| anyhow!("affectations existantes: {e}"))?
        .into_iter()
        .collect();
    let reconciliation = canonicaliser(brutes, &existants, aujourdhui);
    let items: Vec<AnnonceWriteItem> = reconciliation.annonces.iter().map(ecrire).collect();
    let mut stats = AnnonceWriteStats::default();
    for lot in items.chunks(2_000) {
        let s = repo
            .upsert_annonces(lot)
            .await
            .map_err(|e| anyhow!("upsert_annonces: {e}"))?;
        stats.annonces += s.annonces;
        stats.canaux += s.canaux;
        stats.aretes += s.aretes;
    }
    let fusions: Vec<(String, String)> = reconciliation
        .fusions
        .into_iter()
        .map(|f| (f.ancien, f.nouveau))
        .collect();
    let absorbees = repo
        .fusionner_annonces(&fusions)
        .await
        .map_err(|e| anyhow!("fusions: {e}"))?;
    Ok((items.len(), stats, absorbees))
}

/// Régularisation du stock : chaque ligne orpheline (uid abandonné par un
/// re-groupage, plus aucun canal) est résolue par ses propres `details.liens` —
/// clés de jointure extraites des URLs et URLs d'avis elles-mêmes — vers la
/// ligne qui porte aujourd'hui ses canaux, puis absorbée (redirection +
/// suppression). Une orpheline sans trace ou à cibles multiples n'est pas
/// devinée : elle reste, comptée. Avec `formes`, réapplique aussi les règles de
/// forme à tout le stock — le filet hebdomadaire.
pub async fn annonces_regulariser(formes: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let orphelines = repo
        .annonces_orphelines(ANNONCE_FOND_MARCHE)
        .await
        .map_err(|e| anyhow!("orphelines: {e}"))?;
    let mut cles: Vec<String> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for o in &orphelines {
        cles.extend(lj_core::annonces::cles_jointure_depuis_urls(
            o.urls.iter().map(String::as_str),
        ));
        urls.extend(o.urls.iter().cloned());
    }
    cles.sort();
    cles.dedup();
    urls.sort();
    urls.dedup();
    let par_source: HashMap<String, String> = repo
        .annonces_par_source(&cles)
        .await
        .map_err(|e| anyhow!("par source: {e}"))?
        .into_iter()
        .collect();
    let par_url: HashMap<String, String> = repo
        .annonces_par_url_avis(&urls)
        .await
        .map_err(|e| anyhow!("par url: {e}"))?
        .into_iter()
        .collect();
    let candidats: Vec<String> = {
        let mut v: Vec<String> = par_source
            .values()
            .chain(par_url.values())
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let champs: HashMap<String, AnnonceChamps> = repo
        .annonces_champs(&candidats)
        .await
        .map_err(|e| anyhow!("champs des cibles: {e}"))?
        .into_iter()
        .map(|c| (c.uid.clone(), c))
        .collect();

    let total = orphelines.len();
    let mut fusions: Vec<(String, String)> = Vec::new();
    let mut compte = CompteRegularisation::default();
    for o in orphelines {
        let mut cibles: Vec<&String> =
            lj_core::annonces::cles_jointure_depuis_urls(o.urls.iter().map(String::as_str))
                .iter()
                .filter_map(|k| par_source.get(k))
                .chain(o.urls.iter().filter_map(|u| par_url.get(u)))
                .collect();
        cibles.sort();
        cibles.dedup();
        let cibles: Vec<&AnnonceChamps> =
            cibles.iter().filter_map(|uid| champs.get(*uid)).collect();
        match cible_de_regularisation(&o, &cibles) {
            Resolution::Unique(uid) => {
                compte.uniques += 1;
                fusions.push((o.uid, uid.to_string()));
            }
            Resolution::Jumelle(uid) => {
                compte.jumelles += 1;
                fusions.push((o.uid, uid.to_string()));
            }
            Resolution::Indiscernables(uid) => {
                compte.indiscernables += 1;
                fusions.push((o.uid, uid.to_string()));
            }
            Resolution::Aucune if cibles.is_empty() => compte.sans_trace += 1,
            Resolution::Aucune => compte.ambigues += 1,
        }
    }
    let absorbees = repo
        .fusionner_annonces(&fusions)
        .await
        .map_err(|e| anyhow!("fusions: {e}"))?;

    // Filet hebdomadaire de la forme : l'ingest écrit propre à la
    // construction, mais une règle nouvelle (`embellir_titre`,
    // `nettoyer_acheteur`) doit aussi atteindre le stock clos, que la moisson
    // ne revisite jamais.
    let (mut formes_corrigees, mut resultats_corriges) = (0u64, 0u64);
    if formes {
        let corrections: Vec<(String, String, String)> = repo
            .formes_annonces()
            .await
            .map_err(|e| anyhow!("formes: {e}"))?
            .into_iter()
            .filter_map(|(uid, titre, acteur)| {
                let titre_propre = lj_core::annonces::embellir_titre(&titre);
                let acteur_propre = lj_core::annonces::nettoyer_acheteur(&acteur);
                (titre_propre != titre || acteur_propre != acteur).then_some((
                    uid,
                    titre_propre,
                    acteur_propre,
                ))
            })
            .collect();
        for lot in corrections.chunks(2_000) {
            formes_corrigees += repo
                .corriger_formes(lot)
                .await
                .map_err(|e| anyhow!("corriger formes: {e}"))?;
        }

        // Même filet pour les lots d'attribution des blocs `details.resultat`.
        let resultats: Vec<(String, serde_json::Value)> = repo
            .resultats_annonces()
            .await
            .map_err(|e| anyhow!("résultats: {e}"))?
            .into_iter()
            .filter_map(|(uid, mut resultat)| {
                embellir_resultat(&mut resultat).then_some((uid, resultat))
            })
            .collect();
        for lot in resultats.chunks(2_000) {
            resultats_corriges += repo
                .corriger_resultats(lot)
                .await
                .map_err(|e| anyhow!("corriger résultats: {e}"))?;
        }
    }

    tracing::info!(
        orphelines = total,
        absorbees,
        uniques = compte.uniques,
        jumelles = compte.jumelles,
        indiscernables = compte.indiscernables,
        sans_trace = compte.sans_trace,
        ambigues = compte.ambigues,
        formes_corrigees,
        resultats_corriges,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_regulariser"
    );
    Ok(())
}

#[derive(Default)]
struct CompteRegularisation {
    uniques: usize,
    jumelles: usize,
    indiscernables: usize,
    sans_trace: usize,
    ambigues: usize,
}

/// L'issue de la régularisation d'une orpheline.
enum Resolution<'a> {
    /// Une seule ligne en stock est désignée par les liens de l'orpheline.
    Unique(&'a str),
    /// Parmi plusieurs cibles, une seule porte le même titre et le même
    /// acheteur (normalisés) — l'échéance départage s'il en reste plusieurs.
    Jumelle(&'a str),
    /// Plusieurs jumelles aux champs strictement identiques (lots jumeaux,
    /// republication multi-plateformes) : la ligne orpheline est une chimère
    /// d'époque dont le contenu vaut pour chacune — la plus petite par uid,
    /// déterministe, ne perd rien.
    Indiscernables(&'a str),
    /// On ne devine pas.
    Aucune,
}

/// Les lignes de `cibles` qui portent le même titre et le même acheteur que
/// `titre`/`acteur`, au pliage canonique du fond près
/// ([`lj_core::annonces::plie`] — casse, accents, apostrophe typographique,
/// blancs). Un acheteur vide d'un côté n'est pas une contradiction : une
/// observation manquante ne réfute rien. Deux graphies divergentes valent la
/// même quand les deux lignes portent la même entité acheteur, résolue sans
/// ambiguïté : « Ville de Maubeuge » et « COMMUNE DE MAUBEUGE » sont
/// `siren:215903923`.
fn jumelles<'a>(
    titre: &str,
    acteur: &str,
    acheteur_uid: Option<&str>,
    cibles: &[&'a AnnonceChamps],
) -> Vec<&'a AnnonceChamps> {
    let (titre, acteur) = (plie(titre), plie(acteur));
    cibles
        .iter()
        .copied()
        .filter(|c| plie(&c.titre) == titre && acheteur_compatible(&acteur, acheteur_uid, c))
        .collect()
}

/// Le garde acheteur du jumelage, isolé pour valoir aussi sur le repli
/// approchant : deux graphies égales, une observation manquante d'un côté, ou
/// la même entité résolue des deux côtés. `acteur` arrive plié.
fn acheteur_compatible(acteur: &str, acheteur_uid: Option<&str>, c: &AnnonceChamps) -> bool {
    let ca = plie(&c.acteur_libelle);
    ca == acteur
        || ca.is_empty()
        || acteur.is_empty()
        || (acheteur_uid.is_some() && c.acheteur_uid.as_deref() == acheteur_uid)
}

/// Choisit la cible d'une orpheline parmi les lignes que ses propres liens
/// désignent (URL d'avis ou clé de jointure).
fn cible_de_regularisation<'a>(
    orpheline: &lj_store::repository::AnnonceOrpheline,
    cibles: &[&'a AnnonceChamps],
) -> Resolution<'a> {
    if let [seule] = cibles {
        return Resolution::Unique(&seule.uid);
    }
    let jumelles = jumelles(&orpheline.titre, &orpheline.acteur_libelle, None, cibles);
    match jumelles.as_slice() {
        [] => Resolution::Aucune,
        [seule] => Resolution::Jumelle(&seule.uid),
        _ => {
            let strictes: Vec<&&AnnonceChamps> = jumelles
                .iter()
                .filter(|c| c.echeance == orpheline.echeance)
                .collect();
            match strictes.as_slice() {
                [] => {
                    // Aucune à l'échéance de l'orpheline (qui date d'une autre
                    // génération) : si les jumelles sont indiscernables entre
                    // elles, le choix est libre — sinon on ne devine pas.
                    let cle = |c: &AnnonceChamps| (plie(&c.acteur_libelle), c.echeance);
                    let premiere = cle(jumelles[0]);
                    if jumelles.iter().all(|c| cle(c) == premiere) {
                        Resolution::Indiscernables(
                            jumelles
                                .iter()
                                .map(|c| c.uid.as_str())
                                .min()
                                .expect("au moins deux jumelles"),
                        )
                    } else {
                        Resolution::Aucune
                    }
                }
                [seule] => Resolution::Jumelle(&seule.uid),
                plusieurs => Resolution::Indiscernables(
                    plusieurs
                        .iter()
                        .map(|c| c.uid.as_str())
                        .min()
                        .expect("au moins deux jumelles strictes"),
                ),
            }
        }
    }
}

/// Les moteurs multi-tenants : leur `source_uid` porte l'instance
/// (`atexo:<hôte>:<id>`), et deux instances n'ont aucun marché en commun. Le
/// canal cible d'un dépôt y est donc l'instance, pas le moteur.
const MULTI_TENANTS: [&str; 3] = ["atexo", "safetender", "modula"];

/// Le canal que désigne un dépôt : le moteur pour les plateformes nationales,
/// l'instance pour les multi-tenants — soit exactement le préfixe des
/// `source_uid` que son connecteur produit.
fn canal_de_depot(moteur: &str, hote: &str) -> String {
    if MULTI_TENANTS.contains(&moteur) {
        format!("{moteur}:{hote}")
    } else {
        moteur.to_string()
    }
}

/// Le canal d'un `source_uid` de plateforme, dans le même vocabulaire.
fn canal_de_source(source_uid: &str) -> String {
    let mut parts = source_uid.split(':');
    let moteur = parts.next().unwrap_or_default();
    if MULTI_TENANTS.contains(&moteur) {
        format!("{moteur}:{}", parts.next().unwrap_or_default())
    } else {
        moteur.to_string()
    }
}

/// L'hôte canonique d'une URL (`https://www.X.fr/entreprise` → `x.fr`).
fn hote_de(url: &str) -> Option<String> {
    let hote =
        annonces::discover::canon_host(url.split_once("://")?.1.split(['/', '?', '#']).next()?);
    (!hote.is_empty()).then_some(hote)
}

/// Pourquoi une candidate n'a pas donné de paire.
#[derive(Default)]
struct CompteJumelage {
    avec_cle: usize,
    hote_inconnu: usize,
    sans_jumelle: usize,
    ambigues: usize,
    disputees: usize,
    jumelles_disputees: usize,
}

/// L'issue du jumelage d'une candidate.
enum Jumelage<'a> {
    /// Une seule prétendante — d'emblée, ou après départage par l'échéance.
    Une(&'a str),
    /// Aucune ligne du canal cible ne porte ces titre et acheteur.
    Aucune,
    /// Plusieurs prétendantes que l'échéance ne sépare pas.
    Ambigue,
}

/// La jumelle plateforme d'une ligne que seul le BOAMP publie : même titre et
/// même acheteur pliés qu'elle — ou la même entité acheteur quand les deux
/// canaux l'écrivent autrement —, l'échéance en départage quand plusieurs
/// répondent. Le prédicat de la régularisation, sans son repli
/// « indiscernables » : un canal BOAMP appartient à UN marché, et entre deux
/// lignes que rien ne sépare, il n'y a pas de choix libre.
fn jumelle_de_depot<'a>(
    ligne: &lj_store::repository::AnnonceAJumeler,
    vivier: &[&'a AnnonceChamps],
) -> Jumelage<'a> {
    let jumelles = jumelles(
        &ligne.titre,
        &ligne.acteur_libelle,
        ligne.acheteur_uid.as_deref(),
        vivier,
    );
    match jumelles.as_slice() {
        [seule] => Jumelage::Une(&seule.uid),
        [] => Jumelage::Aucune,
        plusieurs => match plusieurs
            .iter()
            .filter(|c| c.echeance == ligne.echeance)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [seule] => Jumelage::Une(&seule.uid),
            _ => Jumelage::Ambigue,
        },
    }
}

/// Fusion par jumelles des lignes ouvertes que seul le BOAMP publie.
///
/// Quand l'avis BOAMP déclare une plateforme de dépôt sans identifiant de
/// dossier (`https://www.marches-securises.fr`), la moisson n'a aucune clé de
/// jointure : la plateforme est connue, le marché non. Reste la jumelle —
/// l'annonce que cette plateforme publie sous le même titre et le même acheteur.
/// La ligne plateforme est l'élue (elle porte les liens profonds du dossier),
/// la ligne BOAMP est absorbée.
///
/// Dry-run par défaut : sans `applique`, la passe lit, journalise chaque paire
/// et n'écrit rien.
pub async fn annonces_jumeler(applique: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // Le recensement `profil_acheteur` est le seul oracle hôte → moteur : il
    // couvre les marques blanches et les instances multi-tenants, qu'aucune
    // liste de domaines ne nomme. Les moteurs qu'on ne moissonne pas n'ont
    // aucune ligne en stock, donc aucune jumelle à offrir.
    let moteurs: HashMap<String, String> = repo
        .moteurs_par_hote()
        .await
        .map_err(|e| anyhow!("moteurs par hôte: {e}"))?
        .into_iter()
        .filter(|(_, m)| annonces::discover::MOTEURS_CONNECTES.contains(&m.as_str()))
        .collect();

    let canaux = repo
        .canaux_plateforme_ouverts(ANNONCE_FOND_MARCHE)
        .await
        .map_err(|e| anyhow!("canaux ouverts: {e}"))?;
    let mut vivier: HashMap<String, Vec<&AnnonceChamps>> = HashMap::new();
    for c in &canaux {
        vivier
            .entry(canal_de_source(&c.source_uid))
            .or_default()
            .push(&c.annonce);
    }
    // Une ligne à deux canaux du même moteur n'est qu'une prétendante.
    for lignes in vivier.values_mut() {
        lignes.sort_by(|a, b| a.uid.cmp(&b.uid));
        lignes.dedup_by(|a, b| a.uid == b.uid);
    }

    let candidates = repo
        .annonces_a_jumeler(ANNONCE_FOND_MARCHE)
        .await
        .map_err(|e| anyhow!("candidates au jumelage: {e}"))?;
    let total = candidates.len();
    let mut compte = CompteJumelage::default();
    let mut paires: Vec<(&lj_store::repository::AnnonceAJumeler, &str, String)> = Vec::new();
    for ligne in &candidates {
        // Une clé de jointure extraite est un join exact : la moisson la joue
        // seule, et si elle n'a rien rattaché c'est que la cible n'est pas en
        // stock — le titre ne doit pas passer par-dessus.
        if !lj_core::annonces::cles_jointure_depuis_urls(
            ligne
                .urls
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(ligne.depot.as_str())),
        )
        .is_empty()
        {
            compte.avec_cle += 1;
            continue;
        }
        let Some(canal) =
            hote_de(&ligne.depot).and_then(|h| moteurs.get(&h).map(|m| canal_de_depot(m, &h)))
        else {
            compte.hote_inconnu += 1;
            continue;
        };
        let Some(vivier) = vivier.get(&canal) else {
            compte.sans_jumelle += 1;
            continue;
        };
        match jumelle_de_depot(ligne, vivier) {
            Jumelage::Une(uid) => paires.push((ligne, uid, canal)),
            Jumelage::Aucune => compte.sans_jumelle += 1,
            Jumelage::Ambigue => compte.ambigues += 1,
        }
    }
    // Une jumelle revendiquée par deux candidates : au plus l'une a raison, et
    // rien ne dit laquelle. Le garde-fou de l'ambiguïté vaut dans les deux sens.
    let revendications: HashMap<&str, usize> = paires.iter().fold(HashMap::new(), |mut m, p| {
        *m.entry(p.1).or_default() += 1;
        m
    });
    compte.disputees = paires.iter().filter(|p| revendications[p.1] > 1).count();
    compte.jumelles_disputees = revendications.values().filter(|n| **n > 1).count();
    paires.retain(|p| revendications[p.1] == 1);

    let champs: HashMap<&str, &AnnonceChamps> = canaux
        .iter()
        .map(|c| (c.annonce.uid.as_str(), &c.annonce))
        .collect();
    let mut par_moteur: BTreeMap<&str, usize> = BTreeMap::new();
    for (ligne, uid, canal) in &paires {
        *par_moteur
            .entry(canal.split(':').next().unwrap_or(canal))
            .or_default() += 1;
        let jumelle = champs[uid];
        tracing::info!(
            boamp_uid = %ligne.uid,
            plateforme_uid = %uid,
            canal = %canal,
            boamp_titre = %ligne.titre,
            plateforme_titre = %jumelle.titre,
            boamp_acheteur = %ligne.acteur_libelle,
            plateforme_acheteur = %jumelle.acteur_libelle,
            acheteur_uid = ligne.acheteur_uid.as_deref().unwrap_or_default(),
            boamp_echeance = %ligne.echeance.map(|e| e.to_rfc3339()).unwrap_or_default(),
            plateforme_echeance = %jumelle.echeance.map(|e| e.to_rfc3339()).unwrap_or_default(),
            "annonces_jumeler : paire"
        );
    }

    // La ligne plateforme est l'élue : elle porte les liens profonds du
    // dossier (RC, DCE), que l'avis BOAMP n'a pas.
    let fusions: Vec<(String, String)> = paires
        .iter()
        .map(|(l, uid, _)| (l.uid.clone(), uid.to_string()))
        .collect();
    let absorbees = if applique {
        repo.fusionner_annonces(&fusions)
            .await
            .map_err(|e| anyhow!("fusions: {e}"))?
    } else {
        0
    };

    tracing::info!(
        applique,
        candidates = total,
        paires = fusions.len(),
        absorbees,
        moteurs = par_moteur
            .iter()
            .map(|(m, n)| format!("{m}={n}"))
            .collect::<Vec<_>>()
            .join(","),
        avec_cle = compte.avec_cle,
        hote_inconnu = compte.hote_inconnu,
        sans_jumelle = compte.sans_jumelle,
        ambigues = compte.ambigues,
        disputees = compte.disputees,
        jumelles_disputees = compte.jumelles_disputees,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_jumeler"
    );
    Ok(())
}

/// Remonte la chaîne des parents d'un avis de résultat dans le lot : un
/// rectificatif cite l'avis de résultat qu'il corrige, qui cite la
/// consultation. S'arrête au premier id hors du lot (le marché réel) ou au
/// premier id revu (chaîne cyclique).
fn remonte_parent(parent_de: &HashMap<&str, &str>, depuis: Option<&str>) -> Option<String> {
    let mut courant = depuis?;
    let mut vus = HashSet::from([courant]);
    while let Some(&p) = parent_de.get(courant) {
        if !vus.insert(p) {
            break;
        }
        courant = p;
    }
    Some(courant.to_string())
}

async fn ecrire_resultats(
    repo: &DecisionRepository<'_>,
    resultats: Vec<ResultatMarche>,
) -> Result<(usize, AnnonceWriteStats, u64)> {
    // `annonce_lie` est un `source_uid` BOAMP : la consultation est derrière.
    // Mais un rectificatif d'avis de résultat cite l'avis de résultat qu'il
    // corrige, pas la consultation : on remonte la chaîne dans le lot jusqu'au
    // marché réel. Sans ça le rectificatif fonde sa propre ligne et réserve le
    // canal propre de l'original — deux revendications du même source_uid, et le
    // statement canaux rejette le lot entier.
    let parent_de: HashMap<&str, &str> = resultats
        .iter()
        .filter_map(|r| r.parent_idweb.as_deref().map(|p| (r.idweb.as_str(), p)))
        .collect();
    let cles: Vec<String> = resultats
        .iter()
        .filter_map(|r| remonte_parent(&parent_de, r.parent_idweb.as_deref()))
        .map(|id| format!("boamp:{id}"))
        .collect();
    let parents: HashMap<String, String> = repo
        .annonces_par_source(&cles)
        .await
        .map_err(|e| anyhow!("parents des résultats: {e}"))?
        .into_iter()
        .collect();

    // Plusieurs avis d'un même marché absent du stock (rectificatif, lots
    // publiés séparément) frappent le même uid via `parent_idweb` : le plus
    // récent garde la ligne, les canaux et arêtes de tous sont conservés.
    let mut orphelins: HashMap<String, (jiff::civil::Date, AnnonceWriteItem)> = HashMap::new();
    // Le résultat le plus récent gagne la ligne du marché (un résultat republié
    // remplace le premier) — et un seul UPDATE par marché.
    let mut poses: HashMap<String, (jiff::civil::Date, ResultatPose)> = HashMap::new();
    for r in &resultats {
        let lots: Vec<serde_json::Value> = r.attribution.lots.iter().map(lot_json).collect();
        let mut resultat = serde_json::json!({
            "lots": lots,
            "texte": r.attribution.texte,
            "avis_url": r.url_avis,
        });
        // Une seule fonction de forme pour les lots, à l'écriture comme au
        // filet : ce qui sort d'ici est déjà ce que la fiche affiche.
        embellir_resultat(&mut resultat);
        let mut sirens: Vec<String> = r
            .attribution
            .lots
            .iter()
            .filter_map(|l| l.siren.clone())
            .collect();
        sirens.sort();
        sirens.dedup();
        let aretes: Vec<(String, String)> = sirens
            .into_iter()
            .map(|s| (s, "titulaire".to_string()))
            .collect();
        let source = AnnonceSourceWriteItem {
            source_uid: format!("boamp:{}", r.idweb),
            url_avis: Some(r.url_avis.clone()),
        };

        let parent_effectif = remonte_parent(&parent_de, r.parent_idweb.as_deref());
        match parent_effectif
            .as_ref()
            .and_then(|id| parents.get(&format!("boamp:{id}")))
        {
            Some(parent) => {
                let pose = ResultatPose {
                    annonce_uid: parent.clone(),
                    resultat,
                    source,
                    aretes_siren: aretes,
                };
                match poses.entry(parent.clone()) {
                    Entry::Vacant(v) => {
                        v.insert((r.date_parution, pose));
                    }
                    Entry::Occupied(mut o) if o.get().0 < r.date_parution => {
                        o.insert((r.date_parution, pose));
                    }
                    Entry::Occupied(_) => {}
                }
            }
            None => {
                let mut sources = vec![source];
                // La consultation est déclarée mais absente du stock : on
                // réserve son canal, pour qu'elle se raccroche à cette ligne au
                // lieu d'en créer une seconde quand le backfill l'amènera. Une
                // chaîne cyclique retombe sur l'avis lui-même : rien à réserver.
                if let Some(p) = parent_effectif.as_deref().filter(|p| *p != r.idweb) {
                    sources.push(AnnonceSourceWriteItem {
                        source_uid: format!("boamp:{p}"),
                        url_avis: None,
                    });
                }
                let item = AnnonceWriteItem {
                    uid: marche_uid_de_resultat(&r.idweb, parent_effectif.as_deref()),
                    fond: ANNONCE_FOND_MARCHE.to_string(),
                    famille_uid: r.nature.map(|n| n.facet_uid().to_string()),
                    // Déjà en forme : le connecteur forme à la construction.
                    titre: r.objet.chars().take(400).collect(),
                    acteur_libelle: r.acheteur.clone().unwrap_or_default(),
                    departement: r.departement.clone(),
                    cpv_uid: r.cpv.as_deref().and_then(cpv_division_uid),
                    date_publication: date_chrono(r.date_parution),
                    // Sans échéance connue, la ligne n'entre jamais dans la fenêtre
                    // vivante — ce qui est juste : le marché est attribué, il ne prend
                    // plus d'offres.
                    echeance: None,
                    url_action: r.url_avis.clone(),
                    details: serde_json::json!({
                        "cpv": r.cpv,
                        "liens": [{"moteur": "boamp", "avis": r.url_avis}],
                        "resultat": resultat,
                    }),
                    aretes_siren: aretes,
                    sources,
                };
                match orphelins.entry(item.uid.clone()) {
                    Entry::Vacant(v) => {
                        v.insert((r.date_parution, item));
                    }
                    Entry::Occupied(mut o) => {
                        let (date, en_place) = o.get_mut();
                        let perdant = if *date < r.date_parution {
                            *date = r.date_parution;
                            std::mem::replace(en_place, item)
                        } else {
                            item
                        };
                        for s in perdant.sources {
                            if !en_place
                                .sources
                                .iter()
                                .any(|g| g.source_uid == s.source_uid)
                            {
                                en_place.sources.push(s);
                            }
                        }
                        for a in perdant.aretes_siren {
                            if !en_place.aretes_siren.contains(&a) {
                                en_place.aretes_siren.push(a);
                            }
                        }
                    }
                }
            }
        }
    }

    let orphelins: Vec<AnnonceWriteItem> = orphelins.into_values().map(|(_, i)| i).collect();
    let mut stats = AnnonceWriteStats::default();
    for lot in orphelins.chunks(2_000) {
        let s = repo
            .upsert_annonces(lot)
            .await
            .map_err(|e| anyhow!("upsert résultats: {e}"))?;
        stats.annonces += s.annonces;
        stats.canaux += s.canaux;
        stats.aretes += s.aretes;
    }
    let liens: Vec<ResultatPose> = poses.into_values().map(|(_, p)| p).collect();
    let rattaches = repo
        .poser_resultats(&liens)
        .await
        .map_err(|e| anyhow!("poser les résultats: {e}"))?;
    Ok((resultats.len(), stats, rattaches))
}

fn lot_json(l: &LotAttribue) -> serde_json::Value {
    serde_json::json!({
        "titulaire": l.titulaire,
        "siren": l.siren,
        "ville": l.ville,
        "description": l.description,
        "montant": l.montant,
        "devise": l.devise,
        "nb_offres": l.nb_offres,
        "date_attribution": l.date_attribution.map(|d| d.to_string()),
    })
}

/// Re-normalise un bloc `details.resultat` en place — la même forme que
/// `lot_json` donne à l'écriture, appliquée au stock par la régularisation.
/// Rend vrai si le bloc a changé. Le `texte` ne bouge pas : ses sauts de
/// ligne sont la structure du paragraphe d'attribution, rendue telle quelle.
fn embellir_resultat(resultat: &mut serde_json::Value) -> bool {
    let Some(lots) = resultat.get_mut("lots").and_then(|l| l.as_array_mut()) else {
        return false;
    };
    let mut change = false;
    for lot in lots {
        if let Some(t) = lot.get("titulaire").and_then(|v| v.as_str()) {
            let propre = lj_core::annonces::nettoyer_acheteur(t);
            if propre != t {
                lot["titulaire"] = propre.into();
                change = true;
            }
        }
        if let Some(d) = lot.get("description").and_then(|v| v.as_str()) {
            let propre = lj_core::annonces::embellir_titre(d);
            if propre != d {
                lot["description"] = propre.into();
                change = true;
            }
        }
    }
    change
}

/// Projette une ligne canonique en écriture. `details` porte le spécifique du
/// fond — jamais le payload source brut : les liens par usage, la référence
/// acheteur, la procédure, le CPV complet et les canaux, tous normalisés.
fn ecrire(c: &AnnonceCanonique) -> AnnonceWriteItem {
    let liens: Vec<serde_json::Value> = c
        .sources
        .iter()
        .map(|s| {
            serde_json::json!({
                "moteur": s.moteur,
                "avis": s.url_avis,
                "rc": s.url_rc,
                "dce": s.url_dce,
            })
        })
        .collect();
    let details = serde_json::json!({
        "reference": c.reference,
        "procedure": c.procedure,
        "cpv": c.cpv,
        "liens": liens,
        "depot": c.url_depot,
    });
    AnnonceWriteItem {
        uid: c.uid.clone(),
        fond: ANNONCE_FOND_MARCHE.to_string(),
        famille_uid: c.famille_uid.map(str::to_string),
        titre: c.titre.chars().take(400).collect(),
        acteur_libelle: c.acheteur_libelle.clone(),
        departement: c.departement.clone(),
        cpv_uid: c.cpv_uid.clone(),
        date_publication: date_chrono(c.date_publication),
        echeance: c
            .echeance
            .and_then(|t| chrono::DateTime::from_timestamp_millis(t.as_millisecond())),
        url_action: c.url_action.clone(),
        details,
        aretes_siren: c
            .sirens
            .iter()
            .map(|s| (s.clone(), "acheteur".to_string()))
            .collect(),
        sources: c
            .sources
            .iter()
            .map(|s| AnnonceSourceWriteItem {
                source_uid: s.source_uid.clone(),
                url_avis: s.url_avis.clone(),
            })
            .collect(),
    }
}

/// Pause entre deux fiches d'un même moteur. La politesse se mesure par hôte :
/// les moteurs avancent en parallèle, chacun à ce débit.
const POLITESSE: std::time::Duration = std::time::Duration::from_millis(300);

/// Lots d'écriture — un aller-retour par tranche, pas par fiche.
const LOT_DETAIL: usize = 200;

/// Le bloc `details.detail` d'une page lue. Un champ absent n'entre pas : le
/// bloc vide vaut « visitée, rien à en tirer » et sort quand même la ligne de
/// la file.
fn bloc_detail(d: &annonces::DetailAnnonce) -> serde_json::Value {
    let mut bloc = serde_json::Map::new();
    if let Some(cpv) = &d.cpv {
        bloc.insert("cpv".into(), serde_json::json!(cpv));
    }
    if let Some(procedure) = &d.procedure {
        bloc.insert("procedure".into(), serde_json::json!(procedure));
    }
    serde_json::Value::Object(bloc)
}

/// Enrichissement par page de détail. La file est dérivée — les annonces
/// ouvertes sans bloc `detail` et sans canal BOAMP — et ne se vide que par la
/// pose du bloc : un fetch en échec n'écrit rien, la ligne repasse la nuit
/// suivante. Un moteur dont l'extracteur n'est pas écrit rend une erreur
/// franche, ses lignes restent en file, et le compteur d'échecs le dit.
///
/// Les moteurs avancent en parallèle mais chacun en série, et un hôte qui
/// rationne ses fiches impose sa cadence à sa branche : `budget_min` empêche
/// cette branche de tenir la chaîne quotidienne des heures durant. Passé le
/// budget elle rend la main, ses lignes comptent en `remises` et repassent la
/// nuit suivante. Un rattrapage manuel monte le plafond pour vider la file
/// d'une traite — c'est ce budget qui borne, jamais l'hôte.
pub async fn annonces_enrichir(limite: i64, budget_min: u64, sans_acheteur: bool) -> Result<()> {
    let debut = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(budget_min * 60);
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let file = repo
        .annonces_a_enrichir(
            ANNONCE_FOND_MARCHE,
            &lj_core::annonces::PRIORITE,
            limite,
            sans_acheteur,
        )
        .await
        .map_err(|e| anyhow!("file d'enrichissement: {e}"))?;
    let mut par_moteur: HashMap<String, Vec<AnnonceAEnrichir>> = HashMap::new();
    for ligne in file {
        par_moteur
            .entry(ligne.moteur.clone())
            .or_default()
            .push(ligne);
    }

    let lus =
        futures::future::join_all(par_moteur.into_iter().map(|(moteur, lignes)| async move {
            let debut_moteur = std::time::Instant::now();
            let (mut poses, mut echecs, mut derniere) = (Vec::new(), 0u32, String::new());
            let mut remises = 0usize;
            for (i, ligne) in lignes.iter().enumerate() {
                if debut_moteur.elapsed() >= budget {
                    remises = lignes.len() - i;
                    break;
                }
                if i > 0 {
                    tokio::time::sleep(POLITESSE).await;
                }
                match annonces::detail(&moteur, &ligne.url_avis).await {
                    Ok(d) => poses.push(DetailPose {
                        annonce_uid: ligne.uid.clone(),
                        cpv_uid: d.cpv.as_deref().and_then(cpv_division_uid),
                        detail: bloc_detail(&d),
                        acheteur: d.acheteur.clone(),
                    }),
                    Err(e) => {
                        echecs += 1;
                        derniere = e.to_string();
                    }
                }
            }
            (moteur, poses, echecs, derniere, remises)
        }))
        .await;

    let (mut total_ok, mut total_echecs, mut ecrits, mut total_remises) =
        (0usize, 0u32, 0u64, 0usize);
    for (moteur, poses, echecs, derniere, remises) in lus {
        tracing::info!(
            moteur = %moteur,
            ok = poses.len(),
            echecs,
            remises,
            erreur = %derniere,
            "annonces_enrichir : moteur"
        );
        total_ok += poses.len();
        total_echecs += echecs;
        total_remises += remises;
        for lot in poses.chunks(LOT_DETAIL) {
            ecrits += repo
                .poser_detail_annonces(lot)
                .await
                .map_err(|e| anyhow!("poser les détails ({moteur}): {e}"))?;
        }
    }

    tracing::info!(
        ok = total_ok,
        echecs = total_echecs,
        remises = total_remises,
        ecrits,
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_enrichir"
    );
    Ok(())
}

/// Recensement des profils d'acheteurs : les hôtes cités par les avis BOAMP
/// actifs, classés par moteur. Alimente `profil_acheteur`, dont dépendent les
/// moteurs multi-tenants (Atexo, Safetender). Hebdomadaire : le parc de
/// profils bouge en mois, pas en jours.
pub async fn annonces_discover() -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let recensement = annonces::discover::recenser_depuis_boamp()
        .await
        .map_err(|e| anyhow!("recensement: {e}"))?;
    let items: Vec<(String, Option<String>, i32)> = recensement
        .profils
        .iter()
        .map(|p| (p.host.clone(), p.moteur.map(str::to_string), p.avis_boamp))
        .collect();
    let ecrits = repo
        .upsert_profils_acheteurs(&items)
        .await
        .map_err(|e| anyhow!("upsert profils: {e}"))?;
    // La couverture du fond, mesurée par avis : sa dérive est le signal qu'un
    // moteur a changé d'interface ou qu'un nouvel éditeur prend des parts.
    let couverture = if recensement.avis > 0 {
        100.0 * recensement.avis_couverts as f64 / recensement.avis as f64
    } else {
        0.0
    };
    tracing::info!(
        hotes = recensement.profils.len(),
        ecrits,
        moteurs = recensement
            .profils
            .iter()
            .filter(|p| p.moteur.is_some())
            .count(),
        avis = recensement.avis,
        avis_couverts = recensement.avis_couverts,
        couverture_pct = format!("{couverture:.1}"),
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_discover"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_store::repository::{AnnonceAJumeler, AnnonceOrpheline};

    fn a_jumeler(titre: &str, acteur: &str, echeance: Option<&str>) -> AnnonceAJumeler {
        AnnonceAJumeler {
            uid: "marche:b".into(),
            titre: titre.into(),
            acteur_libelle: acteur.into(),
            echeance: echeance.map(|e| e.parse().expect("timestamp valide")),
            depot: "https://www.marches-securises.fr".into(),
            urls: Vec::new(),
            acheteur_uid: None,
        }
    }

    /// La même candidate, rattachée à une entité acheteur.
    fn a_jumeler_entite(
        titre: &str,
        acteur: &str,
        echeance: Option<&str>,
        entite: &str,
    ) -> AnnonceAJumeler {
        AnnonceAJumeler {
            acheteur_uid: Some(entite.into()),
            ..a_jumeler(titre, acteur, echeance)
        }
    }

    fn orpheline(titre: &str, acteur: &str, echeance: Option<&str>) -> AnnonceOrpheline {
        AnnonceOrpheline {
            uid: "marche:o".into(),
            titre: titre.into(),
            acteur_libelle: acteur.into(),
            echeance: echeance.map(|e| e.parse().expect("timestamp valide")),
            urls: Vec::new(),
        }
    }

    fn cible(uid: &str, titre: &str, acteur: &str, echeance: Option<&str>) -> AnnonceChamps {
        AnnonceChamps {
            uid: uid.into(),
            titre: titre.into(),
            acteur_libelle: acteur.into(),
            echeance: echeance.map(|e| e.parse().expect("timestamp valide")),
            acheteur_uid: None,
        }
    }

    /// La même cible, rattachée à une entité acheteur.
    fn cible_entite(
        uid: &str,
        titre: &str,
        acteur: &str,
        echeance: Option<&str>,
        entite: &str,
    ) -> AnnonceChamps {
        AnnonceChamps {
            acheteur_uid: Some(entite.into()),
            ..cible(uid, titre, acteur, echeance)
        }
    }

    #[test]
    fn la_chaine_des_parents_remonte_au_marche_reel() {
        let parent_de: HashMap<&str, &str> = [("26-2", "26-1"), ("26-3", "26-2")].into();
        assert_eq!(
            remonte_parent(&parent_de, Some("26-2")),
            Some("26-1".into())
        );
        assert_eq!(
            remonte_parent(&parent_de, Some("26-3")),
            Some("26-1".into())
        );
        assert_eq!(
            remonte_parent(&parent_de, Some("26-9")),
            Some("26-9".into())
        );
        assert_eq!(remonte_parent(&parent_de, None), None);
        let cycle: HashMap<&str, &str> = [("a", "b"), ("b", "a")].into();
        assert_eq!(remonte_parent(&cycle, Some("b")), Some("a".into()));
    }

    #[test]
    fn le_bloc_detail_ne_porte_que_ce_que_la_page_a_donne() {
        assert_eq!(
            bloc_detail(&annonces::DetailAnnonce::default()),
            serde_json::json!({})
        );
        assert_eq!(
            bloc_detail(&annonces::DetailAnnonce {
                cpv: Some("45233140-2".into()),
                procedure: None,
                acheteur: None,
            }),
            serde_json::json!({"cpv": "45233140-2"})
        );
        assert_eq!(
            bloc_detail(&annonces::DetailAnnonce {
                cpv: Some("45233140-2".into()),
                procedure: Some("Procédure adaptée".into()),
                acheteur: None,
            }),
            serde_json::json!({"cpv": "45233140-2", "procedure": "Procédure adaptée"})
        );
    }

    #[test]
    fn le_filet_donne_aux_lots_la_meme_forme_que_l_ecriture() {
        let mut resultat = serde_json::json!({
            "lots": [{
                "titulaire": "OLYMPIQUE MARQUAGE\n1001 avenue de la Batterie",
                "description": "FOURNITURE DE CHAUDIERES BOIS DECHIQUETE ET SES TRAVAUX ASSOCIES",
                "montant": 144235.5,
            }],
            "texte": "Attribué.\n\nVoies de recours :\nTribunal administratif de Nice.",
        });
        assert!(embellir_resultat(&mut resultat));
        let lot = &resultat["lots"][0];
        assert_eq!(lot["titulaire"], "OLYMPIQUE MARQUAGE");
        assert_eq!(
            lot["description"],
            "Fourniture de chaudieres bois dechiquete et ses travaux associes"
        );
        assert!(resultat["texte"].as_str().unwrap().contains('\n'));
        assert!(!embellir_resultat(&mut resultat));
    }

    #[test]
    fn une_seule_cible_gagne_sans_condition() {
        let o = orpheline("Consultation_TEST", "X", None);
        let c = cible("marche:a", "Tout autre titre", "Y", None);
        assert!(matches!(
            cible_de_regularisation(&o, &[&c]),
            Resolution::Unique("marche:a")
        ));
    }

    #[test]
    fn la_jumelle_se_reconnait_a_la_casse_et_aux_espaces_pres() {
        let o = orpheline(
            "Consultation_TEST_Sans_Signature",
            "Plateforme  X",
            Some("2050-12-31T00:00:00Z"),
        );
        let a = cible(
            "marche:a",
            "Consultation_Test_Sans_Signature",
            "plateforme x",
            Some("2050-12-31T00:00:00Z"),
        );
        let b = cible(
            "marche:b",
            "Une autre consultation",
            "plateforme x",
            Some("2050-12-31T00:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Jumelle("marche:a")
        ));
    }

    #[test]
    fn l_echeance_departage_deux_jumelles_de_titre() {
        let o = orpheline(
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:a",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-09T12:00:00Z"),
        );
        let b = cible(
            "marche:b",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Jumelle("marche:b")
        ));
    }

    #[test]
    fn des_jumelles_strictes_indiscernables_departagent_par_uid() {
        let o = orpheline(
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let a = cible(
            "marche:b",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let b = cible(
            "marche:a",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Indiscernables("marche:a")
        ));
    }

    #[test]
    fn des_jumelles_indiscernables_entre_elles_resolvent_malgre_l_echeance_d_une_autre_generation()
    {
        let o = orpheline(
            "Fourniture de contenants à déchets",
            "E.N.A.C.",
            Some("2026-08-07T16:00:00Z"),
        );
        let a = cible(
            "marche:b",
            "Fourniture de contenants à déchets",
            "",
            Some("2026-08-07T14:00:00Z"),
        );
        let b = cible(
            "marche:a",
            "Fourniture de contenants à déchets",
            "",
            Some("2026-08-07T14:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Indiscernables("marche:a")
        ));
    }

    #[test]
    fn des_jumelles_discernables_a_l_echeance_inconnue_ne_resolvent_pas() {
        let o = orpheline("Travaux de voirie", "Ville", Some("2026-08-01T12:00:00Z"));
        let a = cible(
            "marche:a",
            "Travaux de voirie",
            "Ville",
            Some("2026-08-10T12:00:00Z"),
        );
        let b = cible(
            "marche:b",
            "Travaux de voirie",
            "Ville",
            Some("2026-08-20T12:00:00Z"),
        );
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Aucune
        ));
    }

    #[test]
    fn sans_jumelle_on_ne_devine_pas() {
        let o = orpheline(
            "Dépôt de Munitions de NEUBOURG",
            "ESID",
            Some("2026-08-05T12:00:00Z"),
        );
        let a = cible(
            "marche:a",
            "Groupement de Munitions de NEUBOURG",
            "ESID",
            Some("2026-07-27T12:00:00Z"),
        );
        let b = cible("marche:b", "Autre chose", "ESID", None);
        assert!(matches!(
            cible_de_regularisation(&o, &[&a, &b]),
            Resolution::Aucune
        ));
    }

    // ── Jumelage des dépôts sans clé ────────────────────────────────────────

    #[test]
    fn la_jumelle_d_un_depot_se_reconnait_a_la_typographie_pres() {
        let ligne = a_jumeler(
            "Accord-cadre pour l’organisation du Festival",
            "Office de Tourisme de Saint Martin",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Accord-cadre pour l'organisation du Festival",
            "OFFICE DE TOURISME DE SAINT MARTIN",
            Some("2026-09-04T12:00:00Z"),
        );
        let b = cible(
            "marche:q",
            "Marché d'assurances",
            "Office de Tourisme de Saint Martin",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a, &b]),
            Jumelage::Une("marche:p")
        ));
    }

    #[test]
    fn l_echeance_departage_deux_jumelles_de_depot() {
        let ligne = a_jumeler(
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-09T12:00:00Z"),
        );
        let b = cible(
            "marche:q",
            "Fourniture de matériel",
            "Ville",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a, &b]),
            Jumelage::Une("marche:q")
        ));
    }

    #[test]
    fn deux_jumelles_indiscernables_ne_donnent_aucune_paire() {
        let ligne = a_jumeler(
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        let b = cible(
            "marche:q",
            "Prestations de plonge",
            "CCAS",
            Some("2026-08-20T12:00:00Z"),
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a, &b]),
            Jumelage::Ambigue
        ));
    }

    #[test]
    fn un_acheteur_different_refuse_la_paire() {
        let ligne = a_jumeler(
            "Prestations de services d'assurance",
            "CC Loire Layon Aubance",
            Some("2026-09-04T12:00:00Z"),
        );
        let a = cible(
            "marche:p",
            "Prestations de services d'Assurance",
            "MAIRIE DE ROBION",
            Some("2026-09-04T12:00:00Z"),
        );
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
    }

    #[test]
    fn un_acheteur_absent_de_la_plateforme_ne_refute_rien() {
        let ligne = a_jumeler("Travaux de voirie", "Ville de Lille", None);
        let a = cible("marche:p", "Travaux de voirie", "", None);
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a]),
            Jumelage::Une("marche:p")
        ));
    }

    #[test]
    fn deux_graphies_d_un_meme_siren_font_le_meme_acheteur() {
        let ligne = a_jumeler_entite(
            "Fourniture de repas",
            "Ville de Maubeuge",
            Some("2026-09-04T12:00:00Z"),
            "siren:215903923",
        );
        let a = cible_entite(
            "marche:p",
            "Fourniture de repas",
            "COMMUNE DE MAUBEUGE",
            Some("2026-09-04T12:00:00Z"),
            "siren:215903923",
        );
        assert!(matches!(
            jumelle_de_depot(&ligne, &[&a]),
            Jumelage::Une("marche:p")
        ));
    }

    #[test]
    fn deux_sirens_distincts_refusent_la_paire() {
        let ligne = a_jumeler_entite(
            "Prestations de services d'assurances",
            "BRESTPORT",
            Some("2026-09-04T12:00:00Z"),
            "siren:892370974",
        );
        let a = cible_entite(
            "marche:p",
            "Prestations de services d'assurances",
            "Commune de Callac",
            Some("2026-09-04T12:00:00Z"),
            "siren:212200257",
        );
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
    }

    #[test]
    fn une_entite_d_un_seul_cote_ne_prouve_rien() {
        let ligne = a_jumeler_entite(
            "Travaux de voirie",
            "Cté Cnes du Grand Pic St Loup",
            None,
            "siren:243400694",
        );
        let a = cible(
            "marche:p",
            "Travaux de voirie",
            "CC du Grand Pic Saint Loup",
            None,
        );
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
    }

    #[test]
    fn sans_jumelle_aucune_paire() {
        let ligne = a_jumeler("Travaux de voirie", "Ville de Lille", None);
        let a = cible("marche:p", "Travaux de toiture", "Ville de Lille", None);
        assert!(matches!(jumelle_de_depot(&ligne, &[&a]), Jumelage::Aucune));
        assert!(matches!(jumelle_de_depot(&ligne, &[]), Jumelage::Aucune));
    }

    #[test]
    fn le_canal_cible_est_l_instance_pour_un_multi_tenant() {
        assert_eq!(
            canal_de_depot("atexo", "marches.megalis.bretagne.bzh"),
            "atexo:marches.megalis.bretagne.bzh"
        );
        assert_eq!(
            canal_de_source("atexo:marches.megalis.bretagne.bzh:489"),
            "atexo:marches.megalis.bretagne.bzh"
        );
        assert_eq!(canal_de_depot("atline", "marches-securises.fr"), "atline");
        assert_eq!(canal_de_source("aws:pub:MPI-pub-2026219817"), "aws");
        assert_eq!(canal_de_source("atline:1538690665o1tv5rpoutni"), "atline");
    }

    #[test]
    fn l_hote_d_un_depot_est_canonique() {
        assert_eq!(
            hote_de("https://www.marches-securises.fr/"),
            Some("marches-securises.fr".into())
        );
        assert_eq!(
            hote_de("https://marches.maximilien.fr/entreprise"),
            Some("marches.maximilien.fr".into())
        );
        assert_eq!(
            hote_de("https://marches.local-trust.com/?page=Frame.ConsultationsOrganisme"),
            Some("marches.local-trust.com".into())
        );
        assert_eq!(hote_de("www.marches-securises.fr"), None);
    }
}
