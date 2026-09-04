//! Sources européennes : CEDH (HUDOC) et CJUE (EUR-Lex). Corpus bilingue
//! FR-prioritaire : on retient la version FR d'une affaire si elle existe,
//! sinon EN.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use futures::{StreamExt, TryStreamExt};

use lj_core::decision::Decision;
use lj_core::parsing::{build_source_fields, parse_cedh, parse_cjue};
use lj_sources::cedh::CedhResult;
use lj_store::db::Connection;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

use super::batch::drain_batch;
use super::embed::build_embedder_opt;
use super::{generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode, BATCH_SIZE};

/// Clé d'affaire HUDOC regroupant les versions linguistiques d'un même arrêt :
/// ECLI si présent, sinon numéro(s) de requête (`appno`), sinon `itemid` (isolé).
/// HUDOC publie un document par langue ; deux versions d'une affaire partagent
/// ECLI et `appno`.
fn cedh_case_key(row: &CedhResult) -> String {
    let ecli = row.columns["ecli"].as_str().unwrap_or("").trim();
    if !ecli.is_empty() {
        return format!("ecli:{ecli}");
    }
    let appno = row.columns["appno"].as_str().unwrap_or("").trim();
    if !appno.is_empty() {
        return format!("appno:{appno}");
    }
    format!("itemid:{}", row.itemid)
}

/// Regroupe les enregistrements HUDOC par affaire (`cedh_case_key`), chaque groupe
/// ordonné **FR d'abord** (repli EN), en préservant l'ordre de première
/// apparition des affaires. Un arrêt a un enregistrement **par langue avec un
/// doctype distinct** (`HFJUD` = arrêt FR, `HEJUD` = judgment EN) : on garde les
/// deux versions pour **cascader au fetch** — le FR est souvent un stub 204 quand
/// la langue officielle de l'affaire est l'anglais, le texte réel vivant alors
/// dans le `HEJUD`.
fn group_cases(rows: Vec<CedhResult>) -> Vec<Vec<CedhResult>> {
    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Vec<CedhResult>> = Vec::new();
    for row in rows {
        let key = cedh_case_key(&row);
        let i = *idx.entry(key).or_insert_with(|| {
            out.push(Vec::new());
            out.len() - 1
        });
        out[i].push(row);
    }
    // FRE (false → 0) avant les autres langues : tri stable, ordre interne conservé.
    for group in &mut out {
        group.sort_by_key(|r| u8::from(r.language != "FRE"));
    }
    out
}

/// Cascade FR→EN **au fetch** pour une affaire (patron [`lj_sources::cjue::CjueClient::resource_text`]) :
/// tente le corps de chaque version linguistique (FR d'abord, cf. [`group_cases`])
/// et renvoie la **première** dont le corps est non vide, avec son texte strippé.
/// `Ok(None)` si aucune langue n'a de corps (204 partout → SKIP différé).
async fn cedh_case_body<'a>(
    client: &lj_sources::cedh::CedhClient,
    case: &'a [CedhResult],
) -> Result<Option<(&'a CedhResult, String)>> {
    for row in case {
        // Throttle de courtoisie porté par le client, au réseau seul.
        let body = client
            .body_text(&row.itemid)
            .await
            .map_err(|e| anyhow!("cedh body {}: {e}", row.itemid))?;
        if !body.trim().is_empty() {
            return Ok(Some((row, body)));
        }
    }
    Ok(None)
}

/// Concurrence des re-sondes FR des affaires EN-only : le débit agrégé reste
/// borné par le throttle par appel du client.
const FR_PROBE_CONCURRENCY: usize = 8;

/// Concurrence des pages de liste HUDOC d'une année.
const LIST_CONCURRENCY: usize = 4;

/// Stratégie de filtrage des affaires déjà en base, partagée par [`run_cedh`] et
/// [`run_cjue`] (corpus bilingue FR-prioritaire).
#[derive(Clone, Copy)]
enum EuropeSkip {
    /// Bootstrap / re-bootstrap : saute toute affaire déjà ingérée (on ne tire
    /// que la traîne neuve, courtoisie de la source).
    AllPresent,
    /// Sync incrémental : saute les affaires déjà servies en **français** ;
    /// re-fetch les nouvelles et les EN-only (upgrade FR différé). Un arrêt déjà en
    /// FR est définitif.
    FrPresentOnly,
}

/// Sous-ensemble des `source_uid` présents ET déjà servis en FR — le sync CJUE
/// saute ceux-là (cf. [`DecisionRepository::find_fr_source_uids`]).
async fn present_fr_source_uids(conn: &Connection, uids: &[String]) -> Result<HashSet<String>> {
    DecisionRepository::new(conn)
        .find_fr_source_uids(uids)
        .await
        .map_err(|e| anyhow!("find_fr_source_uids: {e}"))
}

async fn present_source_uids(conn: &Connection, uids: &[String]) -> Result<HashSet<String>> {
    let repo = DecisionRepository::new(conn);
    let states = repo
        .find_ingest_states(uids)
        .await
        .map_err(|e| anyhow!("find_ingest_states: {e}"))?;
    Ok(states.into_keys().collect())
}

/// Première année balayée au bootstrap CEDH/CJUE. La CEDH rend des arrêts depuis
/// 1960 (première affaire 1961) ; EUR-Lex secteur 6 depuis 1954.
const EUROPE_START_YEAR: i32 = 1960;

/// Année courante (UTC), borne haute des balayages CEDH/CJUE.
fn current_year() -> i32 {
    chrono::Utc::now()
        .date_naive()
        .format("%Y")
        .to_string()
        .parse()
        .unwrap_or(EUROPE_START_YEAR)
}

/// Construit le `source_fields` HTML : les métadonnées verbatim
/// (colonnes HUDOC / prédicats CDM, qui ne portent pas de texte) plus les
/// sections rebasées sur `full_text` ([`build_source_fields`] retire `text`/
/// `zones` absents et n'ajoute que `sections`).
fn html_source_fields(metadata: &serde_json::Value, decision: &Decision) -> serde_json::Value {
    build_source_fields(metadata, &decision.sections)
}

/// Octets bruts hashés pour l'idempotence d'un document HTML CEDH/CJUE :
/// métadonnées sérialisées (colonnes/prédicats) suivies du corps brut. Un
/// changement de l'un ou l'autre invalide le checksum.
fn html_checksum(metadata: &serde_json::Value, body_text: &str) -> String {
    let mut payload = serde_json::to_vec(metadata).unwrap_or_default();
    payload.extend_from_slice(body_text.as_bytes());
    super::content_checksum(&payload)
}

/// Normalise un code langue source (HUDOC 'FRE'/'ENG', EUR-Lex 'fra'/'eng') vers
/// ISO-639-2/T ('fra'/'eng'), la valeur portée par la colonne `decision_sources.lang`.
/// Toute autre valeur → `None` (langue non gérée).
fn iso639_2t(raw: &str) -> Option<&'static str> {
    match raw {
        "FRE" | "fra" => Some("fra"),
        "ENG" | "eng" => Some("eng"),
        _ => None,
    }
}

/// Construit le candidat CEDH/CJUE depuis `(decision parsée, métadonnées, corps,
/// langue de la source)`. La langue est **dite ici par l'ingester** dans
/// `source_fields["lang"]` → matérialisée en colonne `lang` par
/// `upsert_decision_source`.
fn html_candidate(
    decision: Decision,
    metadata: &serde_json::Value,
    body_text: &str,
    lang: Option<&str>,
) -> Candidate {
    let mut source_fields = html_source_fields(metadata, &decision);
    if let Some(l) = lang {
        source_fields["lang"] = serde_json::json!(l);
    }
    Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        content_checksum: html_checksum(metadata, body_text),
        prebuilt_source_fields: Some(source_fields),
        prebuilt_extracted: None,
        decision,
        raw_payload: Vec::new(),
        payload_format: "html".to_string(),
        write_mode: WriteMode::Full,
        dila_fond: None,
    }
}

/// Pool DB + migrations + repo prêt pour un ingest CEDH/CJUE (sans embeddings :
/// backfill séparé, comme l'ingest DILA).
pub(super) async fn europe_pool() -> Result<deadpool_postgres::Pool> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    Ok(pool)
}

/// Ingère les arrêts CEDH (HUDOC) d'une plage d'années en base (corpus bilingue
/// FR-prioritaire).
///
/// Par année, de la plus ancienne à la plus récente :
/// 1. **Liste seule**, sans corps : ne retient que les arrêts FR (`HFJUD`) et EN
///    (`HEJUD`) ; notes et communiqués non ingérés.
/// 2. **Regroupement par affaire** ([`group_cases`]), FR d'abord — les deux
///    versions restent pour cascader au fetch.
/// 3. **Filtrage des présents** : `AllPresent` retire toute affaire déjà en
///    base ; `FrPresentOnly` ne retire que celles servies en français et
///    re-sonde les EN-only, dont le stub `HFJUD` passe de 204 à 200 le jour où
///    la traduction paraît. Les autres sont closes sans re-fetch ni parse.
/// 4. **Corps + ingestion**. **Corps vide ⇒ SKIP** : c'est le lag de conversion
///    DOCX→HTML, repris au run suivant. Idempotent par `content_checksum`,
///    manifeste par année.
async fn run_cedh(start_year: i32, end_year: i32, skip: EuropeSkip) -> Result<()> {
    use lj_sources::cedh::{parse_results, CedhClient, CedhManifest, PAGE_SIZE};

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    // Cache disque des corps (warmable hors-DB via `cache_cedh`) : relecture locale
    // sans réseau ni throttle.
    let client = CedhClient::new().with_body_cache(settings.cache_dir());
    let manifest_path = lj_sources::cedh::manifest_path(&settings.cache_dir());
    let mut manifest = CedhManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut empty_bodies = 0usize;
    let mut en_fallback = 0usize;
    for year in start_year..=end_year {
        // (1) Liste seule : la première page donne `resultcount`, les suivantes
        // partent en concurrence, ordre conservé. On ne garde que les arrêts FR
        // (`HFJUD`) **et** EN (`HEJUD`) : les deux versions d'une affaire remontent
        // pour cascader au fetch (cf. group_cases).
        let is_judgment = |r: &CedhResult| r.doctype == "HFJUD" || r.doctype == "HEJUD";
        let first = client
            .results_page(year, 0)
            .await
            .map_err(|e| anyhow!("cedh results {year} start=0: {e}"))?;
        let (resultcount, first_rows) = parse_results(&first);
        let mut rows: Vec<CedhResult> = first_rows.into_iter().filter(is_judgment).collect();
        // Fenêtre globale 10 000 par année (cap HUDOC).
        let year_end = (resultcount as usize).min(10_000);
        let pages: Vec<Vec<CedhResult>> =
            futures::stream::iter((PAGE_SIZE..year_end).step_by(PAGE_SIZE).map(|start| {
                let client = &client;
                async move {
                    let page = client
                        .results_page(year, start)
                        .await
                        .map_err(|e| anyhow!("cedh results {year} start={start}: {e}"))?;
                    Ok::<_, anyhow::Error>(parse_results(&page).1)
                }
            }))
            .buffered(LIST_CONCURRENCY)
            .try_collect()
            .await?;
        for page_rows in pages {
            rows.extend(page_rows.into_iter().filter(is_judgment));
        }

        // (2) Regroupement par affaire (FR d'abord, repli EN au fetch).
        let mut cases = group_cases(rows);

        // (3) Filtrage des présents selon la stratégie. Une affaire (2 itemids
        // FR/EN) est « présente » si l'un de ses itemids l'est. `AllPresent` saute
        // tout ingéré ; `FrPresentOnly` ne saute que le déjà-FR (définitif) et sépare
        // le reste : déjà en base (EN-only) → re-sonde FR concurrente (4a), absente →
        // cascade complète (4b).
        let mut upgrade_cases: Vec<Vec<CedhResult>> = Vec::new();
        if !cases.is_empty() {
            let uids: Vec<String> = cases
                .iter()
                .flatten()
                .map(|r| format!("cedh/{}", r.itemid))
                .collect();
            match skip {
                EuropeSkip::AllPresent => {
                    let skip_uids = present_source_uids(&conn, &uids).await?;
                    cases.retain(|case| {
                        !case
                            .iter()
                            .any(|r| skip_uids.contains(&format!("cedh/{}", r.itemid)))
                    });
                }
                EuropeSkip::FrPresentOnly => {
                    let fr_uids = present_fr_source_uids(&conn, &uids).await?;
                    let present_uids = present_source_uids(&conn, &uids).await?;
                    let mut fresh: Vec<Vec<CedhResult>> = Vec::new();
                    for case in std::mem::take(&mut cases) {
                        let has = |set: &HashSet<String>| {
                            case.iter()
                                .any(|r| set.contains(&format!("cedh/{}", r.itemid)))
                        };
                        if has(&fr_uids) {
                            continue; // déjà servie en FR : définitif.
                        }
                        if has(&present_uids) {
                            upgrade_cases.push(case);
                        } else {
                            fresh.push(case);
                        }
                    }
                    cases = fresh;
                }
            }
        }

        // (4a) Re-sonde FR concurrente des affaires EN-only déjà en base : seul le
        // document FR est sondé — corps vide = rien de neuf, affaire close pour la
        // nuit (ni re-fetch EN, ni parse, ni drain) ; corps servi = la traduction est
        // parue, l'affaire rejoint la cascade, son corps FR étant déjà au cache
        // disque.
        let fr_probed = upgrade_cases.len();
        let mut fr_won = 0usize;
        if !upgrade_cases.is_empty() {
            let winners: Vec<Option<Vec<CedhResult>>> =
                futures::stream::iter(upgrade_cases.into_iter().map(|case| {
                    let client = &client;
                    async move {
                        for row in case.iter().filter(|r| r.language == "FRE") {
                            let body = client
                                .body_text(&row.itemid)
                                .await
                                .map_err(|e| anyhow!("cedh body {}: {e}", row.itemid))?;
                            if !body.trim().is_empty() {
                                return Ok::<_, anyhow::Error>(Some(case));
                            }
                        }
                        Ok(None)
                    }
                }))
                .buffer_unordered(FR_PROBE_CONCURRENCY)
                .try_collect()
                .await?;
            for case in winners.into_iter().flatten() {
                fr_won += 1;
                cases.push(case);
            }
        }

        // (4b) Cascade FR→EN par affaire + ingestion par batch (affaires
        // nouvelles + gagnantes de la re-sonde).
        let mut counts = IngestCounts::default();
        let mut candidates: Vec<Candidate> = Vec::new();
        for case in &cases {
            let Some((row, body)) = cedh_case_body(&client, case).await? else {
                // Aucune langue de l'affaire n'a de corps : re-fetch différé.
                empty_bodies += 1;
                counts.empty_skipped += 1;
                continue;
            };
            if row.language != "FRE" {
                en_fallback += 1;
            }
            let decision = parse_cedh(&body, &row.columns, &row.itemid)
                .map_err(|e| anyhow!("parse_cedh {}: {e}", row.itemid))?;
            candidates.push(html_candidate(
                decision,
                &row.columns,
                &body,
                iso639_2t(&row.language),
            ));
            if candidates.len() >= BATCH_SIZE {
                let batch = std::mem::take(&mut candidates);
                drain_batch(
                    &conn,
                    embedder.as_ref(),
                    batch,
                    require_embeddings,
                    IngestMode::MissingHash,
                    &mut counts,
                )
                .await?;
            }
        }
        if !candidates.is_empty() {
            drain_batch(
                &conn,
                embedder.as_ref(),
                candidates,
                require_embeddings,
                IngestMode::MissingHash,
                &mut counts,
            )
            .await?;
        }

        manifest.mark_year(year);
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
        tracing::info!(
            year,
            created = counts.created,
            updated = counts.updated,
            skipped = counts.skipped,
            empty = counts.empty_skipped,
            fr_probed,
            fr_won,
            "ingest_cedh_year"
        );
        total.merge(&counts);
    }

    tracing::info!(
        start_year,
        end_year,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        empty_bodies,
        en_fallback,
        "ingest_cedh_total"
    );
    Ok(())
}

/// Ingère les arrêts/ordonnances CJUE (EUR-Lex) d'une plage d'années (corpus
/// bilingue FR-prioritaire).
///
/// Pour chaque année : pagine la liste SPARQL dédupliquée par CELEX (`sparql_page`
/// → `parse_sparql`, OFFSET cappé 10 000). En `AllPresent` (re-bootstrap), retire
/// d'emblée les CELEX déjà en base ; en `FrPresentOnly` (sync), seuls les déjà-FR.
/// Pour chaque CELEX restant : fetch le texte du resource **FR-prioritaire avec
/// repli EN** (`resource_text`, cascade d'`Accept` × langue). **Ni FR ni EN /
/// corps vide ⇒ SKIP** (affaire indisponible dans nos deux langues). Sinon
/// `parse_cjue` → `drain_batch` (`payload_format = "html"`,
/// `source_uid = cjue/{CELEX}`) ; la langue obtenue est tracée en `source_fields`
/// (`resource_obtained_language`). Conclusions AG (`OPIN_AG`) regroupées par CELEX,
/// non ingérées comme décisions concurrentes. Idempotent par `content_checksum`,
/// throttle de courtoisie, manifeste par année.
///
/// `source_fields` porte les **prédicats CDM** par CELEX
/// (`fetch_work_predicates`), la bannière « objet » du corps et l'ECLI de la
/// liste. Fetch en échec ⇒ repli sur `{case-law_ecli, work_date_document}`.
async fn run_cjue(start_year: i32, end_year: i32, skip: EuropeSkip) -> Result<()> {
    use lj_sources::cjue::{
        extract_objet, parse_sparql, CjueClient, CjueManifest, PAGE_SIZE, THROTTLE,
    };

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    let client = CjueClient::new();
    let manifest_path = lj_sources::cjue::manifest_path(&settings.cache_dir());
    let mut manifest = CjueManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut no_lang_skipped = 0usize;
    let mut en_fallback = 0usize;
    for year in start_year..=end_year {
        let mut offset = 0usize;
        let mut counts = IngestCounts::default();
        loop {
            let page = client
                .sparql_page(year, offset)
                .await
                .map_err(|e| anyhow!("cjue sparql {year} offset={offset}: {e}"))?;
            let mut rows = parse_sparql(&page);
            if rows.is_empty() {
                break;
            }
            // On saute les CELEX déjà ingérés selon la stratégie. Le `len` post-filtrage
            // ne sert pas à la pagination — l'OFFSET avance toujours de `PAGE_SIZE` sur la
            // liste brute.
            let uids: Vec<String> = rows.iter().map(|r| format!("cjue/{}", r.celex)).collect();
            let skip_uids = match skip {
                EuropeSkip::AllPresent => present_source_uids(&conn, &uids).await?,
                EuropeSkip::FrPresentOnly => present_fr_source_uids(&conn, &uids).await?,
            };
            rows.retain(|r| !skip_uids.contains(&format!("cjue/{}", r.celex)));

            let mut candidates: Vec<Candidate> = Vec::new();
            for row in &rows {
                tokio::time::sleep(THROTTLE).await;
                let Some((body, lang)) = client
                    .resource_text(&row.celex)
                    .await
                    .map_err(|e| anyhow!("cjue resource {}: {e}", row.celex))?
                else {
                    // Ni FR ni EN servi (404/406/3xx) : affaire hors de nos deux langues.
                    no_lang_skipped += 1;
                    counts.empty_skipped += 1;
                    continue;
                };
                if body.trim().is_empty() {
                    no_lang_skipped += 1;
                    counts.empty_skipped += 1;
                    continue;
                }
                if lang != "fra" {
                    en_fallback += 1;
                }
                // Prédicats CDM riches par CELEX. Échec du fetch → repli minimum
                // tracé.
                tokio::time::sleep(THROTTLE).await;
                let mut predicates = match client.fetch_work_predicates(&row.celex).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            celex = %row.celex,
                            error = %e,
                            "cjue prédicats CDM indisponibles, repli minimum ecli+date"
                        );
                        serde_json::json!({})
                    }
                };
                // ECLI verbatim (jamais dérivé du CELEX) + date :
                // toujours présents, source = liste SPARQL (le DESCRIBE peut les
                // omettre ou les multiplier). Bannière « objet » = abstract FR
                // officiel extrait du corps.
                let map = predicates.as_object_mut().expect("map_predicates → objet");
                map.insert("case-law_ecli".into(), serde_json::json!(row.ecli));
                map.insert("work_date_document".into(), serde_json::json!(row.date));
                // Langue de la rendition réellement servie (FR-prioritaire, repli EN) —
                // traçabilité du corpus bilingue (badge UI).
                map.insert("resource_obtained_language".into(), serde_json::json!(lang));
                if let Some(objet) = extract_objet(&body) {
                    map.insert("objet".into(), serde_json::json!(objet));
                }
                let decision = parse_cjue(&body, &predicates, &row.celex)
                    .map_err(|e| anyhow!("parse_cjue {}: {e}", row.celex))?;
                candidates.push(html_candidate(
                    decision,
                    &predicates,
                    &body,
                    iso639_2t(lang),
                ));
                if candidates.len() >= BATCH_SIZE {
                    let batch = std::mem::take(&mut candidates);
                    drain_batch(
                        &conn,
                        embedder.as_ref(),
                        batch,
                        require_embeddings,
                        IngestMode::MissingHash,
                        &mut counts,
                    )
                    .await?;
                }
            }
            if !candidates.is_empty() {
                drain_batch(
                    &conn,
                    embedder.as_ref(),
                    candidates,
                    require_embeddings,
                    IngestMode::MissingHash,
                    &mut counts,
                )
                .await?;
            }

            offset += PAGE_SIZE;
            // OFFSET cappé 10 000 (EUR-Lex) : au-delà, la page suivante est vide.
            if offset >= 10_000 {
                break;
            }
        }

        manifest.mark_year(year);
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
        tracing::info!(
            year,
            created = counts.created,
            updated = counts.updated,
            skipped = counts.skipped,
            empty = counts.empty_skipped,
            "ingest_cjue_year"
        );
        total.merge(&counts);
    }

    tracing::info!(
        start_year,
        end_year,
        created = total.created,
        updated = total.updated,
        skipped = total.skipped,
        no_lang_skipped,
        en_fallback,
        "ingest_cjue_total"
    );
    Ok(())
}

/// Bootstrap CEDH : balaye les années [[`from_year`] (défaut [`EUROPE_START_YEAR`]),
/// courante] en `AllPresent` : toute affaire déjà ingérée est sautée, seuls les
/// corps neufs (traîne EN-only) sont fetchés. Idempotent (`content_checksum`).
/// `from_year` cible un backfill sans re-lister le fonds ancien.
pub async fn ingest_cedh(from_year: Option<i32>) -> Result<()> {
    run_cedh(
        from_year.unwrap_or(EUROPE_START_YEAR),
        current_year(),
        EuropeSkip::AllPresent,
    )
    .await
}

/// Réchauffe **uniquement** le cache disque des corps CEDH (`<cache_dir>/cedh/bodies/`)
/// pour les années [[`from_year`] (défaut [`EUROPE_START_YEAR`]), courante] — **sans
/// toucher la base** (ni pool, ni migrations). Découple le fetch réseau (throttlé,
/// une fois) de l'ingest : une fois le cache chaud, `ingest-cedh` relit tout en local.
/// Fetch borné en concurrence.
pub async fn cache_cedh(from_year: Option<i32>) -> Result<()> {
    use lj_sources::cedh::{parse_results, CedhClient, PAGE_SIZE};
    use std::sync::Arc;

    const CACHE_CONCURRENCY: usize = 8;

    let settings = Settings::from_env()?;
    let client = Arc::new(CedhClient::new().with_body_cache(settings.cache_dir()));
    let start_year = from_year.unwrap_or(EUROPE_START_YEAR);
    let end_year = current_year();

    let mut total_fetched = 0usize;
    let mut total_empty = 0usize;
    for year in start_year..=end_year {
        // (1) Liste seule (métadonnées), ne garder que les arrêts, comme `run_cedh`.
        let mut rows: Vec<CedhResult> = Vec::new();
        let mut start = 0usize;
        loop {
            let page = client
                .results_page(year, start)
                .await
                .map_err(|e| anyhow!("cedh results {year} start={start}: {e}"))?;
            let (resultcount, page_rows) = parse_results(&page);
            if page_rows.is_empty() {
                break;
            }
            rows.extend(
                page_rows
                    .into_iter()
                    .filter(|r| r.doctype == "HFJUD" || r.doctype == "HEJUD"),
            );
            start += PAGE_SIZE;
            if start as u64 >= resultcount || start >= 10_000 {
                break;
            }
        }
        // (2) Regroupement par affaire (FR d'abord) → on réchauffe exactement ce que
        // l'ingest lira : cascade FR→EN, on s'arrête au 1er corps non vide par affaire.
        let cases = group_cases(rows);

        // (3) Fetch borné en concurrence (une affaire par tâche) : chaque `body_text`
        // écrit le cache disque. La tâche cascade FR→EN et s'arrête au 1er corps.
        let mut year_fetched = 0usize;
        let mut year_empty = 0usize;
        for batch in cases.chunks(CACHE_CONCURRENCY) {
            let mut set = tokio::task::JoinSet::new();
            for case in batch {
                let client = Arc::clone(&client);
                let itemids: Vec<String> = case.iter().map(|r| r.itemid.clone()).collect();
                set.spawn(async move {
                    for id in &itemids {
                        match client.body_text(id).await {
                            Ok(b) if !b.trim().is_empty() => {
                                return Ok::<bool, anyhow::Error>(true)
                            }
                            Ok(_) => continue,
                            Err(e) => return Err(anyhow!("cedh body {id}: {e}")),
                        }
                    }
                    Ok(false)
                });
            }
            while let Some(joined) = set.join_next().await {
                if joined.map_err(|e| anyhow!("join cache {e}"))?? {
                    year_fetched += 1;
                } else {
                    year_empty += 1;
                }
            }
        }
        tracing::info!(
            year,
            fetched = year_fetched,
            empty = year_empty,
            "cache_cedh_year"
        );
        total_fetched += year_fetched;
        total_empty += year_empty;
    }
    tracing::info!(
        start_year,
        end_year,
        total_fetched,
        total_empty,
        "cache_cedh_total"
    );
    Ok(())
}

/// Bootstrap CJUE : balaye toutes les années [[`EUROPE_START_YEAR`], courante]
/// (saute toute affaire déjà ingérée, cf. [`ingest_cedh`]).
pub async fn ingest_cjue() -> Result<()> {
    run_cjue(EUROPE_START_YEAR, current_year(), EuropeSkip::AllPresent).await
}

/// Sync incrémental CEDH : fenêtre glissante `[N-1, N]` en `FrPresentOnly`. Ne
/// re-fetch que les affaires neuves et celles encore servies en **EN-only** :
/// leur stub `HFJUD` (204 au run précédent) est re-sondé et bascule le
/// `full_text` en français dès que la traduction paraît. La fenêtre N-1 rattrape
/// l'arrêt publié EN-only fin d'année N-1 dont le FR n'arrive qu'en N. Les
/// affaires déjà FR sont sautées (définitives). Le remplissage initial d'une base
/// vierge reste le bootstrap [`ingest_cedh`] (historique complet).
pub async fn sync_cedh() -> Result<()> {
    let year = current_year();
    run_cedh(year - 1, year, EuropeSkip::FrPresentOnly).await
}

/// Sync incrémental CJUE : balaye l'année courante **et la précédente** (fenêtre
/// glissante `[N-1, N]`), et ne re-fetch que les affaires neuves + celles encore
/// servies en EN-only (upgrade FR différé). La fenêtre rattrape l'upgrade FR d'un
/// arrêt publié EN-only en fin d'année N-1 dont la traduction n'arrive qu'en N.
/// Les arrêts déjà en FR sont sautés (cf. [`EuropeSkip::FrPresentOnly`]).
pub async fn sync_cjue() -> Result<()> {
    let year = current_year();
    run_cjue(year - 1, year, EuropeSkip::FrPresentOnly).await
}
