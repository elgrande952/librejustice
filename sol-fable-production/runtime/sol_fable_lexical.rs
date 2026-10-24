//! Independent, authenticated, read-only lexical API over the real LibreJustice core.
//! It neither constructs a model nor exposes upstream administration/OAuth/AI routes.
use std::{path::Path, sync::Arc};
use anyhow::{bail, ensure, Context};
use axum::{extract::{Path as AxumPath, Query, Request, State}, http::{header, StatusCode}, middleware::{self, Next}, response::{IntoResponse, Response}, routing::get, Json, Router};
use aws_lc_rs::{digest, hmac};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use lj_api::{config::Settings, state::AppState};
use lj_dtos::{SearchMode, SearchRequest};
use lj_store::{db, migrator::{apply_migrations, MigrationIntent}, repository::{DecisionRepository, UpsertStatus}};
use serde::Deserialize;
use serde_json::{json, Value};

const UPSTREAM: &str = "ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75";
const MAX_CORPUS_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
struct Runtime { api: AppState, auth: Arc<(hmac::Key, hmac::Tag)> }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery { q: String, limit: Option<u32>, offset: Option<u32>, mode: Option<String>, #[serde(rename="aiMode")] ai_mode: Option<bool> }

fn request(q: SearchQuery) -> anyhow::Result<SearchRequest> {
    ensure!(!q.q.trim().is_empty() && q.q.chars().count() <= lj_dtos::QUERY_MAX_CHARS, "query must contain 1..512 characters");
    ensure!(q.mode.as_deref().unwrap_or("lexical") == "lexical", "only explicit lexical search is supported; auto/semantic/reference modes are disabled");
    ensure!(!q.ai_mode.unwrap_or(false), "AI reranking is disabled");
    let limit = q.limit.unwrap_or(10);
    let offset = q.offset.unwrap_or(0);
    ensure!((1..=20).contains(&limit) && offset <= 1000, "limit must be 1..20 and offset <=1000");
    let req: SearchRequest = serde_json::from_value(json!({"query":q.q,"mode":"lexical","limit":limit,"offset":offset,"aiMode":false}))?;
    ensure!(req.mode == SearchMode::Lexical && !req.ai_mode, "lexical policy invariant");
    Ok(req)
}

async fn authorize(State(rt): State<Runtime>, req: Request, next: Next) -> Response {
    let supplied = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer "));
    let valid = supplied.map(|v| hmac::verify(&rt.auth.0, v.as_bytes(), rt.auth.1.as_ref()).is_ok()).unwrap_or(false);
    if !valid { return (StatusCode::UNAUTHORIZED, Json(json!({"error":"authentication_required"}))).into_response(); }
    let mut response = next.run(req).await;
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn health(State(rt): State<Runtime>) -> Response {
    let ok = match rt.api.pool.get().await { Ok(c) => c.simple_query("SELECT 1").await.is_ok(), Err(_) => false };
    (if ok {StatusCode::OK} else {StatusCode::SERVICE_UNAVAILABLE}, Json(json!({"status":if ok {"ok"} else {"database_unavailable"},"service":"LibreJustice Sol-Fable lexical","upstream":UPSTREAM,"mode":"lexical_only","embeddings":"disabled","reranking":"disabled"}))).into_response()
}

async fn search(State(rt): State<Runtime>, Query(q): Query<SearchQuery>) -> Response {
    let req = match request(q) {Ok(r)=>r,Err(e)=>return (StatusCode::UNPROCESSABLE_ENTITY,Json(json!({"error":e.to_string()}))).into_response()};
    match lj_api::search::search_nocache_counted(&rt.api, &req).await { Ok(r)=>Json(r).into_response(),Err(e)=>e.into_response() }
}

async fn decision(State(rt): State<Runtime>, AxumPath(id): AxumPath<String>) -> Response {
    if id.len()!=12 || !id.bytes().all(|c|c.is_ascii_alphanumeric() || c==b'_' || c==b'-') { return StatusCode::BAD_REQUEST.into_response(); }
    match lj_api::decisions::get_decision(&rt.api,&id).await { Ok(r)=>Json(r).into_response(),Err(e)=>e.into_response() }
}

async fn corpus(State(rt): State<Runtime>) -> Response {
    match rt.api.pool.get().await {
        Ok(c)=>match c.query_one("SELECT (SELECT count(*) FROM decisions WHERE deleted_at IS NULL), (SELECT count(*) FROM legal_text)",&[]).await {
            Ok(row)=>Json(json!({"decisions":row.get::<_,i64>(0),"legal_texts":row.get::<_,i64>(1),"complete":false,"scope":"Only explicitly imported official snapshots; no claim to complete legal coverage","embeddings":"disabled"})).into_response(),
            Err(_)=>StatusCode::SERVICE_UNAVAILABLE.into_response()
        },
        Err(_)=>StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot { source_url: String, retrieved_at: String, licence: String, payload: Value }

fn validate_snapshot(row: &Snapshot) -> anyhow::Result<()> {
    let id = row.payload.get("id").and_then(Value::as_str).context("missing official decision id")?;
    ensure!(id.len()==24 && id.bytes().all(|c|c.is_ascii_hexdigit()), "invalid Judilibre id");
    ensure!(row.source_url == format!("https://api.piste.gouv.fr/cassation/judilibre/v1.0/decision?id={id}"), "source must be the exact official Judilibre URL, without credentials or tracking parameters");
    chrono::DateTime::parse_from_rfc3339(&row.retrieved_at).context("missing valid retrieval date")?;
    ensure!(row.licence == "Licence Ouverte 2.0", "unsupported snapshot licence");
    let text = row.payload.get("text").and_then(Value::as_str).context("full decision text is required")?;
    ensure!(!text.trim().is_empty() && text.len()<=2*1024*1024, "text must be complete and <=2MiB");
    let date = row.payload.get("decision_date").and_then(Value::as_str).context("decision_date is required")?;
    chrono::NaiveDate::parse_from_str(date,"%Y-%m-%d")?;
    ensure!(matches!(row.payload.get("jurisdiction").and_then(Value::as_str),Some("cc"|"ca"|"tj"|"tcom")),"unsupported jurisdiction");
    Ok(())
}

async fn import(pool: &db::Pool, file: &Path) -> anyhow::Result<()> {
    ensure!(file.metadata()?.len()<=MAX_CORPUS_BYTES,"snapshot batch exceeds 64MiB");
    let raw=std::fs::read_to_string(file)?;
    let rows: Vec<Snapshot> = raw.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<Result<_,_>>()?;
    ensure!(!rows.is_empty() && rows.len()<=1000,"import batch must contain 1..1000 snapshots");
    // Validate every row before touching the database. Keep original input bytes unchanged.
    for row in &rows {validate_snapshot(row)?;}
    let mut counts=[0_u64;3];
    for row in rows {
        let parsed=lj_parse::parse_judilibre(&row.payload,None);
        ensure!(parsed.jurisdiction_type.is_some(),"unrecognized official jurisdiction");
        let bytes=serde_json::to_vec(&row.payload)?;
        let checksum=digest::digest(&digest::SHA256,&bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
        let id_hash=digest::digest(&digest::SHA256,parsed.source_uid.as_bytes());
        let public_id=URL_SAFE_NO_PAD.encode(&id_hash.as_ref()[..9]);
        let mut fields=lj_parse::build_source_fields(&row.payload,&parsed.sections);
        fields.as_object_mut().context("object fields")?.insert("_sol_fable_provenance".into(),json!({"source_url":row.source_url,"retrieved_at":row.retrieved_at,"licence":row.licence,"checksum":checksum}));
        let conn=pool.get().await?;
        conn.batch_execute("BEGIN").await?;
        let result=DecisionRepository::new(&conn).upsert(&parsed,&checksum,&public_id,None,None,&fields,None,"json").await;
        match result {
            Ok(result)=> {conn.batch_execute("COMMIT").await?;let i=match result.status {UpsertStatus::Created=>0,UpsertStatus::Updated=>1,UpsertStatus::Skipped=>2};counts[i]+=1;},
            Err(e)=>{let _=conn.batch_execute("ROLLBACK").await;return Err(e.into());}
        }
    }
    println!("IMPORT_CREATED={}; IMPORT_UPDATED={}; IMPORT_SKIPPED={}; EMBEDDINGS=disabled",counts[0],counts[1],counts[2]);
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut settings=Settings::from_env()?;
    // A real refusing backend, never DummyEmbedder and never an external provider.
    settings.embed_backend="disabled".to_string();
    settings.pool_max=settings.pool_max.min(8);
    settings.pool_min=1;
    let pool=db::build_pool(&settings.db_url,settings.pool_max)?;
    let action=std::env::args().nth(1).unwrap_or_else(||"serve".to_string());
    if action=="migrate" || action=="boot" {
        ensure!(std::env::var("LJ_ALLOW_SCHEMA_MIGRATIONS").as_deref()==Ok("1"),"migrations require LJ_ALLOW_SCHEMA_MIGRATIONS=1 on a dedicated database");
        let conn=pool.get().await?;
        let applied=apply_migrations(&conn,MigrationIntent::Apply).await?;
        println!("MIGRATIONS_APPLIED={}",applied.len());
        if action=="migrate" {return Ok(());}
    }
    {let conn=pool.get().await?;apply_migrations(&conn,MigrationIntent::Verify).await?;}
    if action=="import" {
        let file=std::env::args().nth(2).context("import requires a JSONL snapshot path")?;
        return import(&pool,Path::new(&file)).await;
    }
    if action!="serve" && action!="boot" {bail!("expected serve, boot, migrate or import");}
    if let Ok(file)=std::env::var("LJ_BOOTSTRAP_CORPUS") {import(&pool,Path::new(&file)).await?;}
    let secret=std::env::var("LJ_ACCESS_TOKEN").context("LJ_ACCESS_TOKEN must be supplied from the host secret store")?;
    ensure!(secret.len()>=32 && secret.len()<=256,"access token must contain 32..256 characters");
    let key=hmac::Key::new(hmac::HMAC_SHA256,b"sol-fable-lexical-auth-v1");
    let tag=hmac::sign(&key,secret.as_bytes());
    let rt=Runtime{api:AppState::build(Arc::new(settings.clone()),pool),auth:Arc::new((key,tag))};
    let reads=Router::new().route("/search",get(search)).route("/decision/{id}",get(decision)).route("/corpus",get(corpus)).route_layer(middleware::from_fn_with_state(rt.clone(),authorize));
    let app=reads.route("/health",get(health)).with_state(rt);
    let port=std::env::var("PORT").ok().map(|p|p.parse::<u16>()).transpose()?.unwrap_or(settings.bind_port);
    let listener=tokio::net::TcpListener::bind((settings.bind_host.as_str(),port)).await?;
    println!("SERVICE=librejustice-sol-fable; MODE=lexical_only; MODEL_DOWNLOADS=none; RERANKING=disabled");
    axum::serve(listener,app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query(mode:Option<&str>, ai:Option<bool>)->SearchQuery {SearchQuery{q:"obligation de sécurité".into(),limit:Some(10),offset:Some(0),mode:mode.map(str::to_string),ai_mode:ai}}
    #[test] fn rejects_unavailable_modes_and_ai(){for mode in ["semantic","auto","reference"]{assert!(request(query(Some(mode),None)).is_err());}assert!(request(query(None,Some(true))).is_err());}
    #[test] fn default_is_real_lexical(){let r=request(query(None,None)).unwrap();assert_eq!(r.mode,SearchMode::Lexical);assert!(!r.ai_mode);}
    #[test] fn rejects_oversized_query(){let mut q=query(None,None);q.q="x".repeat(513);assert!(request(q).is_err());}
    #[tokio::test] async fn disabled_embedder_refuses_without_network(){use lj_llm::backend::{AnyEmbedder,Embedder};let e=AnyEmbedder::Disabled;assert!(e.embed_query(&["test".into()]).await.is_err());assert!(e.embed_passages(&["test".into()]).await.is_err());}
}
