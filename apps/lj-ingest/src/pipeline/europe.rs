//! European sources: the CEDH (HUDOC) and the CJUE (EUR-Lex). Each language
//! version of a judgment is a provenance of its own, with its own text.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use futures::{StreamExt, TryStreamExt};

use lj_core::decision::Decision;
use lj_parse::{build_source_fields, parse_cedh, parse_cjue};
use lj_sources::cedh::CedhResult;
use lj_store::db::Connection;
use lj_store::repository::DecisionRepository;
use lj_vocab::languages::{eu_of_iso_639_1, iso_639_1_of_eu};

use crate::config::Settings;

use super::batch::drain_batch;
use super::embed::build_embedder_opt;
use super::{generate_public_id, Candidate, IngestCounts, IngestMode, WriteMode, BATCH_SIZE};

/// Concurrence des pages de liste HUDOC d'une année.
const LIST_CONCURRENCY: usize = 4;

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

/// The Publications Office code of a HUDOC language, the value
/// `decision_sources.lang` carries.
fn hudoc_lang(raw: &str) -> Result<&'static str> {
    match raw {
        "FRE" => Ok("fra"),
        "ENG" => Ok("eng"),
        other => Err(anyhow!("HUDOC language {other}")),
    }
}

/// The candidate of one language version, `lang` its Publications Office code in
/// lowercase, written into `source_fields["lang"]`. `lead` when this version
/// becomes the text its decision serves, the only one cut into passages.
fn html_candidate(
    decision: Decision,
    metadata: &serde_json::Value,
    body_text: &str,
    lang: &str,
    lead: bool,
) -> Candidate {
    let mut source_fields = html_source_fields(metadata, &decision);
    source_fields["lang"] = serde_json::json!(lang);
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
        passages: lead,
    }
}

/// Pool DB + migrations + repo prêt pour un ingest CEDH/CJUE (sans embeddings :
/// backfill séparé, comme l'ingest DILA).
pub(super) async fn europe_pool() -> Result<deadpool_postgres::Pool> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    Ok(pool)
}

/// Lists the HUDOC judgments of a year, French (`HFJUD`) and English (`HEJUD`)
/// versions alike.
async fn cedh_judgments(
    client: &lj_sources::cedh::CedhClient,
    year: i32,
) -> Result<Vec<CedhResult>> {
    use lj_sources::cedh::{parse_results, PAGE_SIZE};
    let is_judgment = |r: &CedhResult| r.doctype == "HFJUD" || r.doctype == "HEJUD";
    let first = client
        .results_page(year, 0)
        .await
        .map_err(|e| anyhow!("cedh results {year} start=0: {e}"))?;
    let (resultcount, first_rows) = parse_results(&first);
    let mut rows: Vec<CedhResult> = first_rows.into_iter().filter(is_judgment).collect();
    // HUDOC caps a query at 10,000 results.
    let year_end = (resultcount as usize).min(10_000);
    let pages: Vec<Vec<CedhResult>> = futures::stream::iter(
        (PAGE_SIZE..year_end)
            .step_by(PAGE_SIZE)
            .map(|start| async move {
                let page = client
                    .results_page(year, start)
                    .await
                    .map_err(|e| anyhow!("cedh results {year} start={start}: {e}"))?;
                Ok::<_, anyhow::Error>(parse_results(&page).1)
            }),
    )
    .buffered(LIST_CONCURRENCY)
    .try_collect()
    .await?;
    for page_rows in pages {
        rows.extend(page_rows.into_iter().filter(is_judgment));
    }
    Ok(rows)
}

/// Ingests the CEDH judgments of a range of years, every language version
/// HUDOC serves. A version already held is skipped; an empty body (the
/// DOCX→HTML conversion lag, or the French stub of a judgment not yet
/// translated) is fetched again at the next run.
async fn run_cedh(start_year: i32, end_year: i32) -> Result<()> {
    use lj_sources::cedh::{CedhClient, CedhManifest};

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;

    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    let client = CedhClient::new().with_body_cache(settings.cache_dir());
    let manifest_path = lj_sources::cedh::manifest_path(&settings.cache_dir());
    let mut manifest = CedhManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    for year in start_year..=end_year {
        let mut rows = cedh_judgments(&client, year).await?;
        let uids: Vec<String> = rows.iter().map(|r| format!("cedh/{}", r.itemid)).collect();
        let held = present_source_uids(&conn, &uids).await?;
        let ecli = |r: &CedhResult| r.columns["ecli"].as_str().unwrap_or("").to_string();
        let mut french: HashSet<String> = rows
            .iter()
            .filter(|r| r.language == "FRE" && held.contains(&format!("cedh/{}", r.itemid)))
            .map(ecli)
            .collect();
        rows.retain(|r| !held.contains(&format!("cedh/{}", r.itemid)));
        // French first: a judgment's first version creates the decision.
        rows.sort_by_key(|r| u8::from(r.language != "FRE"));

        let mut counts = IngestCounts::default();
        let mut candidates: Vec<Candidate> = Vec::new();
        for row in &rows {
            let body = client
                .body_text(&row.itemid)
                .await
                .map_err(|e| anyhow!("cedh body {}: {e}", row.itemid))?;
            if body.trim().is_empty() {
                counts.empty_skipped += 1;
                continue;
            }
            let decision = parse_cedh(&body, &row.columns, &row.itemid)
                .map_err(|e| anyhow!("parse_cedh {}: {e}", row.itemid))?;
            let lead = row.language == "FRE" || !french.contains(&ecli(row));
            if row.language == "FRE" {
                french.insert(ecli(row));
            }
            candidates.push(html_candidate(
                decision,
                &row.columns,
                &body,
                hudoc_lang(&row.language)?,
                lead,
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
        empty = total.empty_skipped,
        "ingest_cedh_total"
    );
    Ok(())
}

/// The ISO 639-1 languages of procedure of a CJUE judgment, read from its CDM
/// predicates (`…/language/DEU`).
fn cjue_procedure_languages(predicates: &serde_json::Value) -> Vec<&'static str> {
    let codes: Vec<&str> = match &predicates["case-law_uses_procedure_language"] {
        serde_json::Value::String(s) => vec![s.as_str()],
        serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => Vec::new(),
    };
    codes
        .into_iter()
        .filter_map(|c| iso_639_1_of_eu(c.rsplit('/').next().unwrap_or(c)))
        .collect()
}

/// The languages a CJUE judgment is wanted in: French, English and its
/// languages of procedure.
fn cjue_wanted_languages(predicates: &serde_json::Value) -> Vec<&'static str> {
    let mut wanted = vec!["fr", "en"];
    for iso in cjue_procedure_languages(predicates) {
        if !wanted.contains(&iso) {
            wanted.push(iso);
        }
    }
    wanted
}

/// The CDM predicates a provenance stores, without what the ingest adds per
/// language.
fn stored_predicates(source_fields: &serde_json::Value) -> serde_json::Value {
    let mut p = source_fields.clone();
    if let Some(map) = p.as_object_mut() {
        for key in ["sections", "lang", "resource_obtained_language", "objet"] {
            map.remove(key);
        }
    }
    p
}

/// Ingests the CJUE judgments and orders (EUR-Lex) of a range of years, in
/// French, in English and in their language of procedure, one provenance
/// `cjue/{CELEX}/{lang}` per language.
///
/// Per year, the SPARQL list deduplicated by CELEX (`sparql_page` →
/// `parse_sparql`, OFFSET capped at 10,000). A CELEX whose wanted languages are
/// all held costs no request. The predicates come from a provenance already
/// held, else from `fetch_work_predicates`, which names the language of
/// procedure. A language EUR-Lex does not serve is asked again at the next run.
async fn run_cjue(start_year: i32, end_year: i32) -> Result<()> {
    use lj_sources::cjue::{
        extract_objet, parse_sparql, CjueClient, CjueManifest, PAGE_SIZE, THROTTLE,
    };

    let settings = Settings::from_env()?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let (embedder, require_embeddings) = build_embedder_opt(&settings).await?;
    let client = CjueClient::new();
    let manifest_path = lj_sources::cjue::manifest_path(&settings.cache_dir());
    let mut manifest = CjueManifest::load(&manifest_path).map_err(|e| anyhow!("manifest: {e}"))?;

    let mut total = IngestCounts::default();
    let mut unserved = 0usize;
    for year in start_year..=end_year {
        let mut offset = 0usize;
        let mut counts = IngestCounts::default();
        loop {
            let page = client
                .sparql_page(year, offset)
                .await
                .map_err(|e| anyhow!("cjue sparql {year} offset={offset}: {e}"))?;
            let rows = parse_sparql(&page);
            if rows.is_empty() {
                break;
            }
            let celexes: Vec<String> = rows.iter().map(|r| r.celex.clone()).collect();
            let held = repo
                .cjue_languages(&celexes)
                .await
                .map_err(|e| anyhow!("cjue_languages: {e}"))?;

            let mut candidates: Vec<Candidate> = Vec::new();
            for row in &rows {
                let (present, stored) = match held.get(&row.celex) {
                    Some((langs, fields)) => (langs.clone(), Some(stored_predicates(fields))),
                    None => (Vec::new(), None),
                };
                if let Some(p) = &stored {
                    if cjue_wanted_languages(p)
                        .iter()
                        .all(|l| present.iter().any(|h| h == l))
                    {
                        continue;
                    }
                }
                let mut predicates = match stored {
                    Some(p) => p,
                    None => {
                        tokio::time::sleep(THROTTLE).await;
                        match client.fetch_work_predicates(&row.celex).await {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!(
                                    celex = %row.celex,
                                    error = %e,
                                    "cjue CDM predicates unavailable, minimum ecli+date fallback"
                                );
                                serde_json::json!({})
                            }
                        }
                    }
                };
                // The SPARQL list carries the ECLI verbatim (never derived from the
                // CELEX) and the date, which the DESCRIBE may omit or repeat.
                let map = predicates.as_object_mut().expect("map_predicates → objet");
                map.insert("case-law_ecli".into(), serde_json::json!(row.ecli));
                map.insert("work_date_document".into(), serde_json::json!(row.date));
                if let Some(original) = cjue_procedure_languages(&predicates).first() {
                    predicates["original_lang"] =
                        serde_json::json!(eu_of_iso_639_1(original).expect("official language"));
                }
                let mut fetched: Vec<(&'static str, String)> = Vec::new();
                for iso in cjue_wanted_languages(&predicates) {
                    if present.iter().any(|h| h == iso) {
                        continue;
                    }
                    let eu = eu_of_iso_639_1(iso).expect("wanted languages are official");
                    tokio::time::sleep(THROTTLE).await;
                    // EUR-Lex answers some renditions 500 for good (62019CJ0937 eng).
                    let body = match client.resource_text_in(&row.celex, eu).await {
                        Ok(body) => body,
                        Err(error) => {
                            tracing::warn!(celex = %row.celex, lang = eu, %error, "cjue resource failed, language skipped");
                            None
                        }
                    };
                    match body.filter(|b| !b.trim().is_empty()) {
                        Some(body) => fetched.push((iso, body)),
                        None => {
                            unserved += 1;
                            counts.empty_skipped += 1;
                        }
                    }
                }
                // The authority's order: French, English, then the first code.
                let mut served: Vec<&str> = present.iter().map(String::as_str).collect();
                served.extend(fetched.iter().map(|(iso, _)| *iso));
                served.sort_by_key(|l| (*l != "fr", *l != "en", *l));
                for (iso, body) in fetched {
                    let eu = eu_of_iso_639_1(iso).expect("wanted languages are official");
                    let mut fields = predicates.clone();
                    fields["resource_obtained_language"] = serde_json::json!(eu);
                    if iso == "fr" {
                        if let Some(objet) = extract_objet(&body) {
                            fields["objet"] = serde_json::json!(objet);
                        }
                    }
                    let decision = parse_cjue(&body, &fields, &row.celex, iso)
                        .map_err(|e| anyhow!("parse_cjue {}: {e}", row.celex))?;
                    let lead = served.first() == Some(&iso);
                    candidates.push(html_candidate(decision, &fields, &body, eu, lead));
                }
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
        unserved,
        "ingest_cjue_total"
    );
    Ok(())
}

/// Bootstrap CEDH : balaye les années [[`from_year`] (défaut [`EUROPE_START_YEAR`]),
/// courante]. `from_year` cible un backfill sans re-lister le fonds ancien.
pub async fn ingest_cedh(from_year: Option<i32>) -> Result<()> {
    run_cedh(from_year.unwrap_or(EUROPE_START_YEAR), current_year()).await
}

/// Warms the disk cache of the CEDH bodies (`<cache_dir>/cedh/bodies/`) for
/// the years [[`from_year`] (default [`EUROPE_START_YEAR`]), current], without
/// touching the base: `ingest-cedh` then reads them locally.
pub async fn cache_cedh(from_year: Option<i32>) -> Result<()> {
    use lj_sources::cedh::CedhClient;

    const CACHE_CONCURRENCY: usize = 8;

    let settings = Settings::from_env()?;
    let client = CedhClient::new().with_body_cache(settings.cache_dir());
    let start_year = from_year.unwrap_or(EUROPE_START_YEAR);
    let end_year = current_year();

    let mut total_fetched = 0usize;
    let mut total_empty = 0usize;
    for year in start_year..=end_year {
        let rows = cedh_judgments(&client, year).await?;
        let served: Vec<bool> = futures::stream::iter(rows.iter().map(|r| {
            let client = &client;
            async move {
                let body = client
                    .body_text(&r.itemid)
                    .await
                    .map_err(|e| anyhow!("cedh body {}: {e}", r.itemid))?;
                Ok::<_, anyhow::Error>(!body.trim().is_empty())
            }
        }))
        .buffer_unordered(CACHE_CONCURRENCY)
        .try_collect()
        .await?;
        let year_fetched = served.iter().filter(|s| **s).count();
        let year_empty = served.len() - year_fetched;
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

/// Bootstrap CJUE : balaye toutes les années [[`EUROPE_START_YEAR`], courante].
pub async fn ingest_cjue() -> Result<()> {
    run_cjue(EUROPE_START_YEAR, current_year()).await
}

/// Daily CEDH sync over the sliding window `[N-1, N]`: new judgments, and the
/// language versions HUDOC has published since the last run.
pub async fn sync_cedh() -> Result<()> {
    let year = current_year();
    run_cedh(year - 1, year).await
}

/// Daily CJUE sync over the sliding window `[N-1, N]`: new judgments, and the
/// languages EUR-Lex has published since the last run.
pub async fn sync_cjue() -> Result<()> {
    let year = current_year();
    run_cjue(year - 1, year).await
}

/// Page of the joined-cases catch-up, under the CELLAR cap of 10,000 sorted
/// rows. A CELEX year weighs tens of links (78 in 1985), so one page answers it
/// whole and the loop is only there to guard the outlier.
const JOINED_CASES_PAGE: usize = 2_000;

/// First CELEX year swept for joined cases. Sector 6 opens in 1954, and the
/// corpus reaches back to 1958 even though the ingest sweeps from 1960: the year
/// of the CELEX is that of the case, which the judgment may postdate by years.
const JOINED_CASES_START_YEAR: i32 = 1954;

/// Poses the joined cases over the CJUE corpus that predates the predicate:
/// sweeps `case-law_joins_case_court` for the whole of CELLAR and writes it into
/// the `source_fields` of the provenances we hold, in the shape the ingest gives
/// it. Writes no docket number: `reextract-fields --jurisdiction-type CJUE
/// --field docket_numbers` reads them back through `parse_cjue`, and its
/// inverted probe then attracts the citations left pending.
pub async fn backfill_cjue_joined_cases(dry_run: bool) -> Result<()> {
    use lj_sources::cjue::{parse_joined_cases, predicate_value, CjueClient, THROTTLE};

    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let client = CjueClient::new();

    let mut works = 0usize;
    let mut links = 0usize;
    let mut written = 0u64;
    for year in JOINED_CASES_START_YEAR..=current_year() {
        let mut offset = 0usize;
        loop {
            let page = client
                .joined_cases_page(year, offset, JOINED_CASES_PAGE)
                .await
                .map_err(|e| anyhow!("cjue affaires jointes {year} offset={offset}: {e}"))?;
            let rows = parse_joined_cases(&page);
            if rows.is_empty() {
                break;
            }
            works += rows.len();
            links += rows.iter().map(|(_, j)| j.len()).sum::<usize>();

            let (uids, values): (Vec<String>, Vec<String>) = rows
                .into_iter()
                .flat_map(|(celex, joints)| {
                    let value = predicate_value(joints).to_string();
                    lj_vocab::languages::EU_OFFICIAL_LANGUAGES
                        .iter()
                        .map(move |l| (format!("cjue/{celex}/{l}"), value.clone()))
                })
                .unzip();
            if !dry_run {
                written += repo.set_source_fields_joined_cases(&uids, &values).await?;
            }

            offset += JOINED_CASES_PAGE;
            tokio::time::sleep(THROTTLE).await;
        }
    }

    tracing::info!(works, links, written, dry_run, "backfill_cjue_joined_cases");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cjue_judgment_is_wanted_in_french_english_and_its_languages_of_procedure() {
        let one = serde_json::json!({
            "case-law_uses_procedure_language": "http://publications.europa.eu/resource/authority/language/DEU"
        });
        assert_eq!(cjue_wanted_languages(&one), ["fr", "en", "de"]);
        let french = serde_json::json!({"case-law_uses_procedure_language": "FRA"});
        assert_eq!(cjue_wanted_languages(&french), ["fr", "en"]);
        let joined = serde_json::json!({"case-law_uses_procedure_language": ["ITA", "NLD"]});
        assert_eq!(cjue_procedure_languages(&joined), ["it", "nl"]);
        assert_eq!(cjue_wanted_languages(&serde_json::json!({})), ["fr", "en"]);
    }
}
