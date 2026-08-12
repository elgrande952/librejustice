//! `sync-eu-law` : le fond UE suit la **dernière version consolidée
//! applicable** de chaque acte, sourcée en Formex 4 via CELLAR.
//!
//! Registre des actes = les datasets `eu-*.json` de `<state_dir>/ingest/corpus/`
//! (uid `EU/…` ou fiche JORFTEXT unique). Pour chaque acte : notice branch →
//! dernière version consolidée applicable (date ≤ aujourd'hui, repli CELEX de
//! base) → si différente de la version **en base** (celex des `source_url` du
//! texte — l'autorité du skip, pas le fichier), fetch du Formex, parse pur
//! (`lj_core::formex`), régénération du dataset, puis rechargement par le
//! loader legal-corpus (purge par texte, TOC, finisseurs) — un seul chemin
//! d'écriture DB, aucun watermark : la comparaison disponible/ingéré est
//! auto-réparante, et le marqueur `consolidated_celex` du fichier n'est qu'un
//! cache de fetch (run interrompu entre écriture et chargement).
//!
//! Les actes sans Formex (pré-format ~1983, directives purement modificatives)
//! gardent leur dataset et leur stock : comptés `no_formex`, jamais réécrits
//! en aveugle. Les échecs réseau sont comptés par acte ; au-delà de 5 % la
//! passe sort en erreur après avoir chargé ce qui a réussi.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use futures::StreamExt;
use serde::Deserialize;

use lj_core::formex::parse_formex_articles;
use lj_sources::cjue::CjueClient;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Concurrence des fetchs CELLAR (notices + Formex).
const FETCH_CONCURRENCY: usize = 4;

/// Champs d'identité relus depuis le dataset existant — le sync ne fabrique
/// jamais une fiche, il rafraîchit le corps d'un acte déjà au registre.
#[derive(Deserialize)]
struct SeedDoc {
    text_uid: String,
    title: String,
    #[serde(default)]
    title_key: Option<String>,
    nature: String,
    jurisdiction: String,
    #[serde(default)]
    date_texte: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    /// Marqueur d'idempotence posé par ce sync ; absent, le dataset est régénéré
    /// au premier run.
    #[serde(default)]
    consolidated_celex: Option<String>,
}

/// Un acte du registre, prêt à sonder.
struct SeedAct {
    path: PathBuf,
    doc: SeedDoc,
    /// CELEX de base (`32006L0112`), dérivé de `source_url`.
    celex_base: String,
}

/// Issue d'un acte de la passe.
enum Outcome {
    /// Dataset régénéré (nouvelle version consolidée ou premier passage).
    Updated(PathBuf),
    /// Déjà à la dernière version applicable.
    Unchanged,
    /// Aucune manifestation Formex FR pour la cible : stock conservé.
    NoFormex,
    /// Erreur réseau/parse : stock conservé, compté.
    Failed,
}

/// CELEX porté par l'URL du dataset (`…/resource/celex/{celex}`), ramené à sa
/// forme de base : une CELEX consolidée `0{corps}-{AAAAMMJJ}` (bootstrap REACH)
/// redevient `3{corps}`.
fn celex_base_from_url(url: &str) -> Option<String> {
    let celex = url.rsplit("/celex/").next().filter(|c| !c.is_empty())?;
    if let Some((head, tail)) = celex.split_once('-') {
        if celex.starts_with('0') && tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) {
            return Some(format!("3{}", &head[1..]));
        }
    }
    Some(celex.to_string())
}

/// Partition des versions d'une notice : dernière **applicable** (date ≤
/// aujourd'hui) et versions **futures** pré-publiées — gardées en attente au
/// manifeste et re-sondées à leur échéance.
fn split_versions<'a>(
    versions: &'a [String],
    today_compact: &str,
) -> (Option<&'a String>, Vec<String>) {
    let applicable = versions.iter().rfind(|v| v.as_str() <= today_compact);
    let futures = versions
        .iter()
        .filter(|v| v.as_str() > today_compact)
        .cloned()
        .collect();
    (applicable, futures)
}

/// Sonde un acte et régénère son dataset si une version plus récente est
/// applicable. Le skip s'appuie sur l'état **DB** (`db_celex`) — le marqueur
/// fichier ne sert que de cache de fetch : un dataset déjà régénéré pour la
/// cible mais jamais chargé (crash entre écriture et chargement) est resservi
/// tel quel au loader. `force` re-fetche et recharge sans regarder ni l'un ni
/// l'autre. Rend aussi les versions futures lues dans la notice.
async fn refresh_act(
    client: &CjueClient,
    seed: &SeedAct,
    db_celex: Option<&str>,
    today_compact: &str,
    force: bool,
) -> Result<(Outcome, Vec<String>)> {
    let versions = client
        .consolidated_versions(&seed.celex_base)
        .await
        .map_err(|e| anyhow!("notice {}: {e}", seed.celex_base))?;
    let (applicable, futures) = split_versions(&versions, today_compact);
    let target = match applicable {
        Some(v) => format!("0{}-{v}", &seed.celex_base[1..]),
        None => seed.celex_base.clone(),
    };
    if !force {
        if db_celex == Some(target.as_str()) {
            return Ok((Outcome::Unchanged, futures));
        }
        if seed.doc.consolidated_celex.as_deref() == Some(target.as_str()) {
            // Fichier déjà à la cible, base en retard : rechargement sans refetch.
            return Ok((Outcome::Updated(seed.path.clone()), futures));
        }
    }
    let Some(xml) = client
        .formex_act_fr(&target)
        .await
        .map_err(|e| anyhow!("formex {target}: {e}"))?
    else {
        // Pas de repli consolidé → base : régresser au texte d'origine écraserait
        // un stock potentiellement plus riche (bootstrap XHTML).
        return Ok((Outcome::NoFormex, futures));
    };
    let articles = parse_formex_articles(&xml);
    if articles.is_empty() {
        tracing::warn!(celex = %target, text_uid = %seed.doc.text_uid, "Formex sans article, dataset conservé");
        return Ok((Outcome::NoFormex, futures));
    }

    // `effect_date` : date de la version consolidée servie ; un acte jamais
    // consolidé vaut depuis son adoption (`date_texte`, sinon borne ouverte).
    let effect_date = match applicable {
        Some(v) => Some(format!("{}-{}-{}", &v[0..4], &v[4..6], &v[6..8])),
        None => seed.doc.date_texte.clone(),
    };
    let today_iso = format!(
        "{}-{}-{}",
        &today_compact[0..4],
        &today_compact[4..6],
        &today_compact[6..8]
    );
    let arts_json: Vec<serde_json::Value> = articles
        .iter()
        .map(|a| {
            // Le sous-titre Formex (« Définitions ») ouvre le corps : LEGI n'a
            // pas de titre par article, le corps est le seul porteur.
            let texte = match &a.subtitle {
                Some(s) => format!("{s}\n{}", a.texte),
                None => a.texte.clone(),
            };
            serde_json::json!({
                "num": a.num,
                "title_path": a.title_path,
                "texte": texte,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "text_uid": seed.doc.text_uid,
        "source": "eur-lex",
        "jurisdiction": seed.doc.jurisdiction,
        "title": seed.doc.title,
        "title_key": seed.doc.title_key,
        "nature": seed.doc.nature,
        "translation": "officiel",
        // Un article supprimé par amendement reste dans le Formex consolidé
        // en coquille vide (`<ALINEA/>`) : corps vide ⇒ `ABROGE` (loader).
        "detect_abrogation": true,
        "source_url": format!("http://publications.europa.eu/resource/celex/{target}"),
        "source_asof": today_iso,
        "date_texte": seed.doc.date_texte,
        "effect_date": effect_date,
        "consolidated_celex": target,
        "articles": arts_json,
    });
    std::fs::write(&seed.path, serde_json::to_vec_pretty(&doc)?)
        .map_err(|e| anyhow!("write {}: {e}", seed.path.display()))?;
    tracing::info!(
        text_uid = %seed.doc.text_uid,
        celex = %target,
        articles = articles.len(),
        "dataset eu-law régénéré"
    );
    Ok((Outcome::Updated(seed.path.clone()), futures))
}

/// Registre des actes : les datasets `eu-*.json` du corpus dir.
fn seed_acts(dir: &Path, only: Option<&str>) -> Result<Vec<SeedAct>> {
    let mut seeds = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| anyhow!("lecture {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| anyhow!("readdir: {e}"))?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.starts_with("eu-") || !name.ends_with(".json") {
            continue;
        }
        if let Some(pat) = only {
            if !name.contains(pat) {
                continue;
            }
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("lecture {}: {e}", path.display()))?;
        let doc: SeedDoc = serde_json::from_str(&raw)
            .map_err(|e| anyhow!("dataset invalide {}: {e}", path.display()))?;
        let Some(celex_base) = doc
            .source_url
            .as_deref()
            .and_then(celex_base_from_url)
            .filter(|c| c.len() >= 2)
        else {
            tracing::warn!(dataset = %path.display(), "dataset eu-law sans CELEX exploitable, ignoré");
            continue;
        };
        seeds.push(SeedAct {
            path,
            doc,
            celex_base,
        });
    }
    seeds.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(seeds)
}

/// Passe `sync-eu-law` : régénère les datasets dont une version consolidée
/// plus récente est applicable, recharge ces seuls datasets. Deux cadences :
/// **delta** (défaut en régime — une requête SPARQL liste les consolidations
/// parues depuis le watermark, seuls les actes touchés ou à échéance de
/// version future sont sondés) et **balayage intégral** (`full`, filet hebdo
/// auto-réparant ; aussi le bootstrap quand le manifeste est vierge). Un run
/// partiel (`limit`/`only`, debug) balaie son sous-ensemble sans toucher le
/// manifeste.
pub async fn sync_eu_law(
    limit: Option<usize>,
    only: Option<&str>,
    force: bool,
    full: bool,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    let mut seeds = seed_acts(&dir, only)?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    // Fond UE en base : autorité du skip, ET complément du registre disque —
    // `state_dir` est local à la machine, un acte chargé ailleurs (bootstrap sur
    // la machine de dev) n'a pas son dataset ici.
    let registry = repo
        .eu_law_registry()
        .await
        .map_err(|e| anyhow!("eu_law_registry: {e}"))?;
    let db_celex: std::collections::HashMap<String, String> = registry
        .iter()
        .map(|(uid, celex, ..)| (uid.clone(), celex.clone()))
        .collect();
    let on_disk: HashSet<String> = seeds.iter().map(|s| s.doc.text_uid.clone()).collect();
    for (uid, celex, title, nature, jurisdiction, date_texte) in registry {
        if on_disk.contains(&uid) {
            continue;
        }
        let filename = format!("eu-{}.json", uid.replace('/', "-").to_lowercase());
        if let Some(pat) = only {
            if !filename.contains(pat) {
                continue;
            }
        }
        let url = format!("http://publications.europa.eu/resource/celex/{celex}");
        let Some(celex_base) = celex_base_from_url(&url).filter(|c| c.len() >= 2) else {
            tracing::warn!(text_uid = %uid, celex = %celex, "acte en base sans CELEX exploitable, ignoré");
            continue;
        };
        seeds.push(SeedAct {
            path: dir.join(filename),
            doc: SeedDoc {
                text_uid: uid,
                title,
                title_key: None,
                nature,
                jurisdiction,
                date_texte: date_texte.map(|d| d.to_string()),
                source_url: Some(url),
                consolidated_celex: None,
            },
            celex_base,
        });
    }
    seeds.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(n) = limit {
        seeds.truncate(n);
    }
    if seeds.is_empty() {
        tracing::warn!(dir = %dir.display(), "sync-eu-law : aucun acte au registre");
        return Ok(());
    }
    let today_compact = chrono::Utc::now().format("%Y%m%d").to_string();
    let today_iso = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Manifeste delta : un run partiel (debug) ne le touche pas.
    let partial = limit.is_some() || only.is_some();
    let manifest_path = lj_sources::cjue::eu_law_manifest_path(&settings.cache_dir());
    let mut manifest = lj_sources::cjue::EuLawManifest::load(&manifest_path)
        .map_err(|e| anyhow!("manifest eu-law: {e}"))?;
    let all_bases: HashSet<String> = seeds.iter().map(|s| s.celex_base.clone()).collect();

    let client = Arc::new(CjueClient::new());
    let mut mode = "full";
    if !full && !force && !partial {
        if let Some(since) = manifest.last_delta_date.clone() {
            let refs = client
                .consolidations_since(&since)
                .await
                .map_err(|e| anyhow!("delta consolidations: {e}"))?;
            if refs.len() >= lj_sources::cjue::CONSOLIDATIONS_LIMIT {
                // Watermark trop ancien pour une fenêtre fiable.
                tracing::warn!(since = %since, "delta consolidations plein : bascule en balayage intégral");
            } else {
                mode = "delta";
                let touched: HashSet<&str> = refs.iter().map(|r| r.body.as_str()).collect();
                let due: HashSet<String> = manifest
                    .pending
                    .iter()
                    .filter(|(_, vs)| vs.iter().any(|v| v.as_str() <= today_compact.as_str()))
                    .map(|(base, _)| base.clone())
                    .collect();
                seeds.retain(|s| {
                    touched.contains(&s.celex_base[1..]) || due.contains(&s.celex_base)
                });
                tracing::info!(
                    since = %since,
                    parues = refs.len(),
                    echues = due.len(),
                    matched = seeds.len(),
                    "sync-eu-law : delta consolidations"
                );
            }
        }
    }

    let total = seeds.len();
    if total == 0 {
        // Delta vide : rien de paru pour le registre, le watermark avance.
        manifest.last_delta_date = Some(today_iso);
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
        println!("sync-eu-law : delta vide, rien à recharger.");
        return Ok(());
    }
    tracing::info!(total, force, mode, "sync-eu-law : balayage CELLAR");

    /// Résultat d'une sonde : `futures` absent sur échec (le manifeste garde
    /// alors les versions en attente connues plutôt que de les effacer).
    struct Probe {
        celex_base: String,
        outcome: Outcome,
        futures: Option<Vec<String>>,
    }
    let probes: Vec<Probe> = futures::stream::iter(seeds.iter().map(|seed| {
        let client = Arc::clone(&client);
        let today = today_compact.clone();
        let db = db_celex.get(&seed.doc.text_uid).map(String::as_str);
        async move {
            match refresh_act(&client, seed, db, &today, force).await {
                Ok((outcome, futures)) => Probe {
                    celex_base: seed.celex_base.clone(),
                    outcome,
                    futures: Some(futures),
                },
                Err(e) => {
                    tracing::warn!(
                        text_uid = %seed.doc.text_uid,
                        celex = %seed.celex_base,
                        error = %e,
                        "sync-eu-law : acte en échec, stock conservé"
                    );
                    Probe {
                        celex_base: seed.celex_base.clone(),
                        outcome: Outcome::Failed,
                        futures: None,
                    }
                }
            }
        }
    }))
    .buffer_unordered(FETCH_CONCURRENCY)
    .collect()
    .await;

    let mut updated: Vec<PathBuf> = Vec::new();
    let (mut unchanged, mut no_formex, mut failed) = (0usize, 0usize, 0usize);
    for p in &probes {
        match &p.outcome {
            Outcome::Updated(path) => updated.push(path.clone()),
            Outcome::Unchanged => unchanged += 1,
            Outcome::NoFormex => no_formex += 1,
            Outcome::Failed => failed += 1,
        }
    }
    updated.sort();

    if !partial {
        // Versions futures en attente : remplacées par ce que la notice du jour
        // annonce pour chaque acte sondé, purgées des actes sortis du registre.
        manifest.pending.retain(|base, _| all_bases.contains(base));
        for p in &probes {
            if let Some(futures) = &p.futures {
                if futures.is_empty() {
                    manifest.pending.remove(&p.celex_base);
                } else {
                    manifest
                        .pending
                        .insert(p.celex_base.clone(), futures.clone());
                }
            }
        }
    }

    if !updated.is_empty() {
        let mut publie_targets: HashSet<String> = HashSet::new();
        for path in &updated {
            // Un dataset = une transaction : pas de texte à moitié rechargé
            // (la purge et les upserts atterrissent ensemble).
            conn.batch_execute("BEGIN")
                .await
                .map_err(|e| anyhow!("BEGIN: {e}"))?;
            match super::legal_corpus::load_dataset_file(&repo, path, &mut publie_targets).await {
                Ok(()) => conn
                    .batch_execute("COMMIT")
                    .await
                    .map_err(|e| anyhow!("COMMIT: {e}"))?,
                Err(e) => {
                    let _ = conn.batch_execute("ROLLBACK").await;
                    return Err(e);
                }
            }
        }
        super::legal_corpus::run_corpus_finishers(&repo, publie_targets).await?;
    }

    tracing::info!(
        total,
        mode,
        updated = updated.len(),
        unchanged,
        no_formex,
        failed,
        "sync-eu-law terminé"
    );
    println!(
        "sync-eu-law ({mode}) : {} rechargés, {unchanged} à jour, {no_formex} sans Formex, \
         {failed} échecs (sur {total}).",
        updated.len()
    );
    if !partial {
        // Watermark : en delta il n'avance que sur run sans échec (un acte raté
        // reste dans la fenêtre du lendemain) ; en intégral il avance dès que le
        // balayage est sain (≤ 5 % d'échecs, le filet hebdo rattrape le reste).
        let advance = match mode {
            "delta" => failed == 0,
            _ => failed * 20 <= total,
        };
        if advance {
            manifest.last_delta_date = Some(today_iso);
        }
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
    }
    // Trop d'échecs = CELLAR malade ou recette cassée : sortie franche, après
    // avoir chargé ce qui a réussi (idempotent, le prochain run reprend).
    if failed * 20 > total {
        bail!("sync-eu-law : {failed} échecs sur {total} actes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{celex_base_from_url, split_versions};

    /// Spec : la dernière applicable sert la cible du jour, les futures
    /// pré-publiées partent en attente au manifeste.
    #[test]
    fn split_versions_separe_applicable_et_futures() {
        let versions = vec![
            "20240101".to_string(),
            "20260501".to_string(),
            "20260918".to_string(),
        ];
        let (applicable, futures) = split_versions(&versions, "20260808");
        assert_eq!(applicable.map(String::as_str), Some("20260501"));
        assert_eq!(futures, vec!["20260918".to_string()]);
        // Jamais consolidé à date : pas d'applicable, tout en attente.
        let (applicable, futures) = split_versions(&versions, "20230101");
        assert_eq!(applicable, None);
        assert_eq!(futures.len(), 3);
    }

    #[test]
    fn celex_base_couvre_les_formes_du_registre() {
        assert_eq!(
            celex_base_from_url("http://publications.europa.eu/resource/celex/32006L0112")
                .as_deref(),
            Some("32006L0112")
        );
        // Bootstrap REACH : URL consolidée datée → CELEX de base.
        assert_eq!(
            celex_base_from_url("http://publications.europa.eu/resource/celex/02006R1907-20221217")
                .as_deref(),
            Some("32006R1907")
        );
        // CAAS (secteur 4, parenthèses) : inchangé.
        assert_eq!(
            celex_base_from_url("http://publications.europa.eu/resource/celex/42000A0922(02)")
                .as_deref(),
            Some("42000A0922(02)")
        );
    }
}
