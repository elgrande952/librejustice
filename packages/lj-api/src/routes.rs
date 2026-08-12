//! Assemblage du routeur axum.
//!
//! Les handlers de données (`search`, `decision/*`, `me/*`…) sont des fonctions
//! pures `(&AppState, …) -> Result<…>` ; ce module les adapte en handlers axum
//! (extraction des query/path params + alias camelCase, application des headers
//! `Cache-Control`).

use crate::auth::{OptionalUser, RequiredUser};
use crate::cache::{CachePolicy, CACHE_DECISION, CACHE_SEARCH};
use crate::error::{validation, ApiError, Result};
use crate::state::AppState;
use crate::{
    annonces, bookmarks, decision_views, decisions, entities, jurisdiction_hubs, legi, mcp, me,
    norm_hubs, oauth, pdf_export, redirect, registre, robots, search, search_history, sitemap,
    stats, usage,
};
use axum::extract::{Path, Query, RawQuery, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use lj_dtos::{
    ActivitySource, ActivityTrackingUpdate, BookmarksResponse, DecisionViewsResponse, Domain,
    HealthResponse, JurisdictionType, Office, Procedure, SearchHistoryResponse, SearchMode,
    SearchRequest, Significance, Solution, SortOrder, UserProfileUpdate,
};
use serde::Deserialize;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

/// Assemble les routes (data API `/api`, OAuth, discovery `.well-known`, MCP
/// optionnel) avec l'état injecté — sans fallback ni middleware.
fn assemble_routes(state: AppState, enable_mcp: bool) -> Router {
    // ── DATA API (préfixe `/api`) ─────────────────────────────────────────────
    let api = Router::new()
        .route("/health", get(health))
        .route("/health/deep", get(health_deep))
        .route("/search", get(search_endpoint))
        .route("/search-textes", get(search_textes_endpoint))
        .route("/suggest", get(suggest_endpoint))
        .route("/codes", get(codes_catalogue))
        .route("/corpus-stats", get(corpus_stats))
        .route("/juridictions", get(jurisdictions_catalogue))
        .route("/juridictions/{code}", get(jurisdiction_hub))
        .route("/juridictions/{code}/{annee}", get(jurisdiction_year))
        .route("/normes", get(normes_catalogue))
        .route("/normes/{fond}", get(norm_hub))
        .route("/normes/{fond}/{annee}", get(norm_year))
        .route("/annonces", get(annonces_endpoint))
        .route("/annonces/catalogue", get(annonces_catalogue))
        .route("/annonces/{uid}", get(annonce_detail))
        .route("/decision/{decision_id}", get(decision))
        .route("/decision/{decision_id}/similar", get(decision_similar))
        .route("/decision/{decision_id}/preview", get(decision_preview))
        .route("/decision/{decision_id}/parties", get(decision_parties))
        .route("/entity/{ns}/{id}", get(entity_page))
        .route("/entity/{ns}/{id}/decisions", get(entity_decisions))
        .route("/entity/{ns}/{id}/registre", get(entity_registre))
        .route("/entities/search", get(entities_search))
        .route("/entities/directory", get(entities_directory))
        .route("/entities/stats", get(entities_stats))
        .route(
            "/decision/{decision_id}/download.pdf",
            get(decision_download_pdf),
        )
        .nest("/texte", legi_router())
        .route("/me", get(get_me).patch(patch_me).delete(delete_me))
        .route("/me/activity-tracking", put(set_activity_tracking))
        .route("/me/bookmarks", get(list_bookmarks))
        .route(
            "/me/bookmarks/{decision_id}",
            put(add_bookmark).delete(remove_bookmark),
        )
        .route(
            "/me/search-history",
            get(list_history).delete(clear_history),
        )
        .route("/me/search-history/{entry_id}", delete(delete_entry))
        .route("/me/decision-views", get(list_views).delete(clear_views))
        .route(
            "/me/decision-views/{decision_id}",
            post(record_view).delete(delete_view),
        )
        .route(
            "/client/hydration-failure",
            post(client_hydration_failure).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-robots-tag"),
            HeaderValue::from_static("noindex"),
        ));

    // ── Racine : OAuth + discovery `.well-known` (consommés hors `/api`) ──────
    let mut app: Router = Router::new()
        .nest("/api", api)
        .merge(oauth::router())
        .merge(oauth::well_known_router())
        .merge(sitemap::router())
        .merge(robots::router())
        .merge(annonces_feeds_router())
        .with_state(state.clone());

    // ── MCP (gated) ───────────────────────────────────────────────────────────
    if enable_mcp {
        app = app.merge(mcp::mcp_router(state.clone()));
    }

    // ── IndexNow ──────────────────────────────────────────────────────────────
    // Le protocole exige la clé servie à `https://<host>/<clé>.txt`.
    if let Some(key) = state.settings.indexnow_key.clone() {
        let path = format!("/{key}.txt");
        app = app.route(&path, get(move || std::future::ready(key.clone())));
    }
    app
}

/// Empile le middleware commun (CORS, compression, trace OTel) sur un routeur
/// assemblé.
///
/// L'ordre `.layer` s'applique de l'extérieur vers l'intérieur : trace OTel la
/// plus externe, puis compression, puis CORS.
fn apply_layers(app: Router, cors: CorsLayer) -> Router {
    // Couche la plus interne : réécrit l'erreur PG marquée par le handler avant que
    // CORS, gzip et OTel ne la voient.
    app.layer(middleware::from_fn(pg_error_boundary))
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(OtelAxumLayer::default())
        .layer(OtelInResponseLayer)
}

/// Frontière HTTP qui reclasse en 422 les erreurs Postgres marquées par
/// [`ApiError::into_response`] (extension [`crate::error::PgErrKind`]) : class 22
/// → 422 `invalid_request_bytes` ; message de syntaxe `paradedb.parse` → 422
/// `invalid_query_syntax`. Le `path`/`q` sont réinjectés ici depuis la requête
/// entrante.
async fn pg_error_boundary(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let raw_query = req.uri().query().unwrap_or("").to_string();
    let resp = next.run(req).await;
    let Some(kind) = resp.extensions().get::<crate::error::PgErrKind>().copied() else {
        return resp;
    };
    let detail = match kind {
        crate::error::PgErrKind::DataException => {
            serde_json::json!({ "error": "invalid_request_bytes", "path": path })
        }
        crate::error::PgErrKind::ParseSyntax => {
            let q = parse_qs(&raw_query)
                .into_iter()
                .find(|(k, _)| k == "q")
                .map(|(_, v)| v)
                .unwrap_or_default();
            serde_json::json!({ "error": "invalid_query_syntax", "query": q })
        }
    };
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({ "detail": detail })),
    )
        .into_response()
}

/// Construit le routeur API (routes + middleware CORS/compression/OTel) sans
/// fallback racine.
pub fn build_router(state: AppState, enable_mcp: bool) -> Router {
    let cors = cors_layer(&state.settings.cors_origins);
    apply_layers(assemble_routes(state, enable_mcp), cors)
}

/// Construit l'app axum complète : routes, fallback de redirection
/// trailing-slash et middleware. `enable_mcp` monte l'endpoint MCP.
pub fn create_app(state: AppState, enable_mcp: bool) -> Router {
    let cors = cors_layer(&state.settings.cors_origins);
    let app = assemble_routes(state, enable_mcp).fallback(redirect::slash_redirect_fallback);
    apply_layers(app, cors)
}

fn cors_layer(origins: &[String]) -> CorsLayer {
    let parsed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| o.parse::<HeaderValue>().ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(parsed))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
        .allow_headers(Any)
}

fn with_cache<T: serde::Serialize>(policy: CachePolicy, body: T) -> Response {
    let mut resp = Json(body).into_response();
    if let Ok(value) = HeaderValue::from_str(&policy.header_value()) {
        resp.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    resp
}

// ── Health ────────────────────────────────────────────────────────────────────

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: state.settings.version.clone(),
    })
}

/// Requête sentinelle de la sonde profonde : large et intemporelle, son `total`
/// ne peut tomber à zéro que si la recherche est cassée.
const DEEP_PROBE_QUERY: &str = "responsabilité de l'État";

/// Égalité de deux secrets sans sortie anticipée. La longueur, comparée d'abord,
/// n'est pas protégée.
fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[tracing::instrument(name = "health_deep", skip_all, fields(
    librejustice.probe.outcome = tracing::field::Empty,
    librejustice.probe.total = tracing::field::Empty,
    librejustice.probe.elapsed_ms = tracing::field::Empty,
))]
async fn health_deep(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(token) = state.settings.deep_probe_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(presented) = presented else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !secret_eq(presented, token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let req = SearchRequest {
        query: DEEP_PROBE_QUERY.to_string(),
        jurisdiction_type: None,
        solution: None,
        procedure: None,
        office: None,
        legal_domain: None,
        jurisdiction_code: None,
        chamber: None,
        legal_instrument: None,
        legal_article: None,
        publication: None,
        significance: None,
        date_from: None,
        date_to: None,
        mode: SearchMode::Auto,
        sort: SortOrder::Relevance,
        limit: 1,
        offset: 0,
        ai_mode: false,
    };
    let started = std::time::Instant::now();
    let outcome = search::search_nocache(&state, &req).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let span = tracing::Span::current();
    span.record("librejustice.probe.elapsed_ms", elapsed_ms);

    let (status, state_label) = match &outcome {
        Ok(resp) => {
            span.record("librejustice.probe.total", resp.total);
            if resp.total > 0 {
                (StatusCode::OK, "ok")
            } else {
                (StatusCode::SERVICE_UNAVAILABLE, "empty")
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "sonde profonde : recherche en échec");
            (StatusCode::SERVICE_UNAVAILABLE, "error")
        }
    };
    span.record("librejustice.probe.outcome", state_label);
    (
        status,
        Json(serde_json::json!({
            "status": state_label,
            "total": outcome.map(|r| r.total).unwrap_or(0),
            "elapsed_ms": elapsed_ms,
            "version": state.settings.version,
        })),
    )
        .into_response()
}

// ── Sonde d'hydratation ─────────────────────────────────────────────────────

/// Corps de la balise de la sonde d'hydratation.
#[derive(Deserialize)]
struct HydrationFailure {
    path: String,
    assets: Vec<String>,
    elapsed_ms: u64,
}

/// Consigne une hydratation absente côté client, signalée par la page elle-même.
async fn client_hydration_failure(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<HydrationFailure>,
) -> StatusCode {
    let budget_key = crate::ratelimit::caller_key(None, &headers);
    if state.hydration_budget.take(budget_key.as_ref()).await
        == crate::ratelimit::Verdict::Exhausted
    {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    tracing::warn!(
        librejustice.hydration.path = %body.path,
        librejustice.hydration.assets = %body.assets.join(","),
        librejustice.hydration.elapsed_ms = body.elapsed_ms,
        "hydratation absente côté client",
    );
    StatusCode::NO_CONTENT
}

// ── Search ──────────────────────────────────────────────────────────────────

/// Paramètres de la query `/api/search`, alias camelCase.
///
/// Construits depuis la query string brute (`serde_urlencoded` ne gère pas les
/// listes répétées `?jurisdictionType=A&jurisdictionType=B`), puis convertis en
/// [`SearchRequest`].
#[derive(Debug)]
struct SearchParams {
    q: Option<String>,
    jurisdiction_type: Vec<JurisdictionType>,
    solution: Vec<Solution>,
    procedure: Vec<Procedure>,
    office: Vec<Office>,
    legal_domain: Vec<Domain>,
    jurisdiction_code: Vec<String>,
    chamber: Vec<String>,
    legal_instrument: Vec<String>,
    legal_article: Vec<String>,
    significance: Vec<Significance>,
    publication: Vec<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    mode: SearchMode,
    sort: SortOrder,
    limit: u32,
    offset: u32,
    ai_mode: bool,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            q: None,
            jurisdiction_type: Vec::new(),
            solution: Vec::new(),
            procedure: Vec::new(),
            office: Vec::new(),
            legal_domain: Vec::new(),
            jurisdiction_code: Vec::new(),
            chamber: Vec::new(),
            legal_instrument: Vec::new(),
            legal_article: Vec::new(),
            significance: Vec::new(),
            publication: Vec::new(),
            date_from: None,
            date_to: None,
            mode: SearchMode::Auto,
            sort: SortOrder::Relevance,
            limit: 20,
            offset: 0,
            ai_mode: false,
        }
    }
}

async fn search_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let raw = raw.as_deref().unwrap_or("");
    let req = parse_search_query(raw)?;
    validate_referential_filters(&state, &req).await?;
    let context = parse_search_context(raw);
    let budget_key = crate::ratelimit::caller_key(user_id.as_deref(), &headers);
    let result = search::search(
        &state,
        &req,
        usage::Caller::web(user_id.is_some(), context, &headers),
        budget_key.as_ref(),
    )
    .await?;
    if context == lj_dtos::SearchContext::User {
        if let Some(user) = user_id.as_deref() {
            search_history::record_search(
                &state.pool,
                user,
                &req.query,
                search_history::filters_from_request(&req),
                ActivitySource::Web,
                lj_dtos::SearchEngine::Decisions,
            )
            .await;
        }
    }
    Ok(with_cache(CACHE_SEARCH, result))
}

/// Contexte d'appel depuis la query string brute : la paire exacte
/// `context=teaser` marque un fetch machine des ponts croisés ; tout le reste
/// (absent, autre valeur) est une recherche utilisateur.
fn parse_search_context(raw: &str) -> lj_dtos::SearchContext {
    if raw.split('&').any(|pair| pair == "context=teaser") {
        lj_dtos::SearchContext::Teaser
    } else {
        lj_dtos::SearchContext::User
    }
}

/// Parse la query string `/api/search` → [`SearchRequest`], avec les bornes
/// (`q` min/max, `limit` 1–50, `offset` 0–99). Erreur 400 sur entrée invalide.
fn parse_search_query(raw: &str) -> Result<SearchRequest> {
    let mut p = SearchParams {
        mode: SearchMode::Auto,
        sort: SortOrder::Relevance,
        limit: 20,
        offset: 0,
        ..SearchParams::default()
    };
    for (key, value) in parse_qs(raw) {
        match key.as_str() {
            "q" => p.q = Some(value),
            "jurisdictionType" => p.jurisdiction_type.push(de_enum_strict(
                "jurisdictionType",
                &value,
                &JurisdictionType::ALL,
            )?),
            "solution" => p
                .solution
                .push(de_enum_strict("solution", &value, &Solution::ALL)?),
            "procedure" => p
                .procedure
                .push(de_enum_strict("procedure", &value, &Procedure::ALL)?),
            "office" => p
                .office
                .push(de_enum_strict("office", &value, &Office::ALL)?),
            "legalDomain" => {
                p.legal_domain
                    .push(de_enum_strict("legalDomain", &value, &Domain::ALL)?)
            }
            "jurisdictionCode" => p.jurisdiction_code.push(value),
            "chamber" => p.chamber.push(value),
            "legalInstrument" => p.legal_instrument.push(value),
            "legalArticle" => p.legal_article.push(value),
            "significance" => {
                p.significance
                    .push(de_enum_strict("significance", &value, &Significance::ALL)?)
            }
            "publication" => p.publication.push(value),
            "dateFrom" => p.date_from = Some(value),
            "dateTo" => p.date_to = Some(value),
            "mode" => {
                p.mode = de_enum::<SearchMode>(&value).ok_or_else(|| {
                    ApiError::Unprocessable(validation::enum_error(
                        &["query", "mode"],
                        &value,
                        &["auto", "lexical", "semantic"],
                    ))
                })?
            }
            "sort" => {
                p.sort = de_enum::<SortOrder>(&value).ok_or_else(|| {
                    ApiError::Unprocessable(validation::enum_error(
                        &["query", "sort"],
                        &value,
                        &["relevance", "date_desc", "date_asc"],
                    ))
                })?
            }
            "limit" => {
                p.limit = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "limit"], &value))
                })?
            }
            "offset" => {
                p.offset = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "offset"], &value))
                })?
            }
            "aiMode" => p.ai_mode = matches!(value.as_str(), "1" | "true" | "yes" | "on"),
            _ => {}
        }
    }

    let Some(query) = p.q else {
        return Err(ApiError::Unprocessable(validation::missing(&[
            "query", "q",
        ])));
    };
    let q_len = query.chars().count() as u64;
    if q_len < 1 {
        return Err(ApiError::Unprocessable(validation::string_too_short(
            &["query", "q"],
            &query,
            1,
        )));
    }
    if q_len > 512 {
        return Err(ApiError::Unprocessable(validation::string_too_long(
            &["query", "q"],
            &query,
            512,
        )));
    }
    if p.limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            1,
        )));
    }
    if p.limit > 50 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            50,
        )));
    }
    if p.offset > 99 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "offset"],
            &p.offset.to_string(),
            99,
        )));
    }
    validate_date_param("dateFrom", &p.date_from)?;
    validate_date_param("dateTo", &p.date_to)?;

    Ok(SearchRequest {
        query,
        jurisdiction_type: opt_vec(p.jurisdiction_type),
        solution: opt_vec(p.solution),
        procedure: opt_vec(p.procedure),
        office: opt_vec(p.office),
        legal_domain: opt_vec(p.legal_domain),
        jurisdiction_code: opt_vec(p.jurisdiction_code),
        chamber: opt_vec(p.chamber),
        legal_instrument: opt_vec(p.legal_instrument),
        legal_article: opt_vec(p.legal_article),
        significance: opt_vec(p.significance),
        publication: opt_vec(p.publication),
        date_from: p.date_from,
        date_to: p.date_to,
        mode: p.mode,
        sort: p.sort,
        limit: p.limit,
        offset: p.offset,
        ai_mode: p.ai_mode,
    })
}

/// Valide les axes à vocabulaire référentiel (`chamber`, `publication`,
/// `jurisdictionCode`) — la frontière pure ([`parse_search_query`]) ne connaît pas
/// le référentiel. Token inconnu → 422.
async fn validate_referential_filters(state: &AppState, req: &SearchRequest) -> Result<()> {
    if req.chamber.is_none() && req.publication.is_none() && req.jurisdiction_code.is_none() {
        return Ok(());
    }
    let refs = crate::referential::referential(state).await?;
    for (field, values) in [("chamber", &req.chamber), ("publication", &req.publication)] {
        if let Some(bad) = values
            .as_deref()
            .and_then(|vs| refs.find_unknown_token(field, vs))
        {
            let tokens = refs.facet_tokens(field);
            return Err(ApiError::Unprocessable(validation::enum_error(
                &["query", field],
                bad,
                &tokens,
            )));
        }
    }
    if let Some(bad) = req
        .jurisdiction_code
        .iter()
        .flatten()
        .find(|c| refs.jurisdiction(c).is_none())
    {
        return Err(ApiError::Unprocessable(validation::value_error(
            &["query", "jurisdictionCode"],
            bad,
            &crate::referential::unknown_jurisdiction_code_msg(bad, &refs),
        )));
    }
    Ok(())
}

/// Valide une date `dateFrom`/`dateTo` à la frontière HTTP : 422 sur
/// format/plage invalide.
fn validate_date_param(field: &str, raw: &Option<String>) -> Result<()> {
    let Some(s) = raw else { return Ok(()) };
    match search::parse_search_date(s) {
        Ok(_) => Ok(()),
        Err(search::DateError::Parse(msg)) => Err(ApiError::Unprocessable(
            validation::date_parsing(&["query", field], s, msg),
        )),
        Err(search::DateError::TooEarly) => Err(ApiError::Unprocessable(
            validation::date_greater_than_equal(&["query", field], s, search::DATE_GE),
        )),
        Err(search::DateError::TooLate) => Err(ApiError::Unprocessable(
            validation::date_less_than_equal(&["query", field], s, search::DATE_LE),
        )),
    }
}

fn opt_vec<T>(v: Vec<T>) -> Option<Vec<T>> {
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Désérialise une valeur de query en enum via serde (les enums DTO ont leur
/// `rename_all` propre — on passe par `serde_json` pour respecter ce mapping).
fn de_enum<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

/// Désérialise un token d'axe enum de `/api/search` ; token inconnu → 422 `enum`
/// listant les variantes valides.
fn de_enum_strict<T: serde::de::DeserializeOwned + serde::Serialize>(
    field: &str,
    value: &str,
    all: &[T],
) -> Result<T> {
    de_enum(value).ok_or_else(|| {
        let variants: Vec<String> = all
            .iter()
            .filter_map(|v| match serde_json::to_value(v) {
                Ok(serde_json::Value::String(s)) => Some(s),
                _ => None,
            })
            .collect();
        let variants: Vec<&str> = variants.iter().map(String::as_str).collect();
        ApiError::Unprocessable(validation::enum_error(&["query", field], value, &variants))
    })
}

/// Parse une query string `a=1&b=2` en paires décodées (`+`→espace, `%XX`).
fn parse_qs(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (pct_decode(k), pct_decode(v)),
            None => (pct_decode(pair), String::new()),
        })
        .collect()
}

/// Décode un composant `application/x-www-form-urlencoded` (`+`→espace, `%XX`).
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── Décision (détail + sous-ressources) ───────────────────────────────────────

async fn decision(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let result = decisions::get_decision(&state, &decision_id).await?;
    Ok(with_cache(CACHE_DECISION, result))
}

async fn decision_preview(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let result = decisions::decision_preview(&state, &decision_id).await?;
    Ok(with_cache(CACHE_DECISION, result))
}

/// Encart « Parties » d'une décision. Décision inconnue → 404.
async fn decision_parties(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let body = entities::decision_parties(&state, &decision_id).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

// ── Fiche entité (`/api/entity/*`) ───────────────────────────────────────────

async fn entity_page(
    State(state): State<AppState>,
    Path((ns, id)): Path<(String, String)>,
) -> Result<Response> {
    let body = entities::entity_page(&state, &ns, &id).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Volet registre d'une entité — APIs publiques à l'affichage, sections absentes
/// si l'amont est indisponible (jamais de 5xx).
async fn entity_registre(
    State(state): State<AppState>,
    Path((ns, id)): Path<(String, String)>,
) -> Result<Response> {
    let body = registre::entity_registre(&state, &ns, &id).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Pagination des décisions d'une entité : `page` 1-basé (défaut 1),
/// `page_size` 1–100 (défaut 20). Hors bornes → 422.
#[derive(Debug, Deserialize)]
struct EntityDecisionsQuery {
    #[serde(default = "default_entity_page")]
    page: i64,
    #[serde(default = "default_entity_page_size")]
    page_size: i64,
}
fn default_entity_page() -> i64 {
    1
}
fn default_entity_page_size() -> i64 {
    20
}
impl EntityDecisionsQuery {
    fn validate(&self) -> Result<()> {
        if self.page < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "page"],
                &self.page.to_string(),
                1,
            )));
        }
        if self.page_size < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "page_size"],
                &self.page_size.to_string(),
                1,
            )));
        }
        if self.page_size > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "page_size"],
                &self.page_size.to_string(),
                100,
            )));
        }
        Ok(())
    }
}

async fn entity_decisions(
    State(state): State<AppState>,
    Path((ns, id)): Path<(String, String)>,
    Query(p): Query<EntityDecisionsQuery>,
) -> Result<Response> {
    p.validate()?;
    let body = entities::entity_decisions(&state, &ns, &id, p.page, p.page_size).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

// ── Annuaire des entités (`/api/entities/*`) ─────────────────────────────────

/// Slugs de catégorie acceptés par `kind`, alignés sur la route web
/// `/annuaire/{kind}`.
const ENTITY_KINDS: &[&str] = &[
    "entreprises",
    "personnes-publiques",
    "associations",
    "avocats",
    "cabinets",
];

/// Résout un `kind` optionnel en catégorie stockée. `None` en entrée → `None`
/// (pas de filtre) ; kind inconnu → 422.
fn resolve_kind(kind: Option<&str>) -> Result<Option<&'static str>> {
    match kind {
        None => Ok(None),
        Some(k) => Ok(Some(entities::kind_to_category(k).ok_or_else(|| {
            ApiError::Unprocessable(validation::enum_error(&["query", "kind"], k, ENTITY_KINDS))
        })?)),
    }
}

/// Query `/api/suggest` : `q` (min 2 codepoints), `mode` (`jurisprudence` |
/// `textes` | `annuaire`, défaut `jurisprudence`).
#[derive(Debug, Deserialize)]
struct SuggestQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

async fn suggest_endpoint(
    State(state): State<AppState>,
    Query(p): Query<SuggestQuery>,
) -> Result<Response> {
    let q = p.q.unwrap_or_default();
    if q.chars().count() < entities::ENTITY_SEARCH_MIN_QUERY {
        return Err(ApiError::Unprocessable(validation::string_too_short(
            &["query", "q"],
            &q,
            entities::ENTITY_SEARCH_MIN_QUERY as u64,
        )));
    }
    let body =
        search::suggest::suggest(&state, &q, p.mode.as_deref().unwrap_or("jurisprudence")).await?;
    Ok(with_cache(CACHE_SEARCH, body))
}

/// Query `/api/entities/search` : `q` (préfixe, min 2 codepoints), `kind`
/// (catégorie optionnelle), `limit` (1–50, défaut 10).
#[derive(Debug, Deserialize)]
struct EntitySearchQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_entity_search_limit")]
    limit: i64,
}
fn default_entity_search_limit() -> i64 {
    10
}

async fn entities_search(
    State(state): State<AppState>,
    Query(p): Query<EntitySearchQuery>,
) -> Result<Response> {
    let q = p.q.unwrap_or_default();
    let q_len = q.chars().count();
    if q_len < entities::ENTITY_SEARCH_MIN_QUERY {
        return Err(ApiError::Unprocessable(validation::string_too_short(
            &["query", "q"],
            &q,
            entities::ENTITY_SEARCH_MIN_QUERY as u64,
        )));
    }
    if p.limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            1,
        )));
    }
    if p.limit > entities::ENTITY_SEARCH_MAX_LIMIT {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &p.limit.to_string(),
            entities::ENTITY_SEARCH_MAX_LIMIT,
        )));
    }
    let category = resolve_kind(p.kind.as_deref())?;
    let body = entities::entity_search(&state, &q, category, p.limit).await?;
    Ok(with_cache(CACHE_SEARCH, body))
}

/// Query `/api/entities/directory` : `kind` (catégorie, requise), `barreau`
/// (filtre optionnel, avocats), `page` (≥ 1, défaut 1), `page_size` (1–100,
/// défaut 20).
#[derive(Debug, Deserialize)]
struct EntityDirectoryQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    barreau: Option<String>,
    #[serde(default = "default_entity_page")]
    page: i64,
    #[serde(default = "default_entity_page_size")]
    page_size: i64,
}

async fn entities_directory(
    State(state): State<AppState>,
    Query(p): Query<EntityDirectoryQuery>,
) -> Result<Response> {
    let kind = p
        .kind
        .as_deref()
        .ok_or_else(|| ApiError::Unprocessable(validation::missing(&["query", "kind"])))?;
    let category = entities::kind_to_category(kind).ok_or_else(|| {
        ApiError::Unprocessable(validation::enum_error(
            &["query", "kind"],
            kind,
            ENTITY_KINDS,
        ))
    })?;
    if p.page < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "page"],
            &p.page.to_string(),
            1,
        )));
    }
    if p.page_size < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "page_size"],
            &p.page_size.to_string(),
            1,
        )));
    }
    if p.page_size > 100 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "page_size"],
            &p.page_size.to_string(),
            100,
        )));
    }
    if p.page.saturating_mul(p.page_size) > entities::ENTITY_DIRECTORY_MAX_DEPTH {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "page"],
            &p.page.to_string(),
            entities::ENTITY_DIRECTORY_MAX_DEPTH / p.page_size,
        )));
    }
    let body =
        entities::entity_directory(&state, category, p.barreau.as_deref(), p.page, p.page_size)
            .await?;
    Ok(with_cache(CACHE_SEARCH, body))
}

async fn entities_stats(State(state): State<AppState>) -> Result<Response> {
    let body = entities::annuaire_stats(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

#[derive(Debug, Deserialize)]
struct SimilarQuery {
    limit: Option<String>,
}

async fn decision_similar(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
    Query(q): Query<SimilarQuery>,
) -> Result<Response> {
    let limit: u32 = match q.limit {
        None => 4,
        Some(v) => v.parse().map_err(|_| {
            ApiError::Unprocessable(validation::int_parsing(&["query", "limit"], &v))
        })?,
    };
    if limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &limit.to_string(),
            1,
        )));
    }
    if limit > 12 {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &limit.to_string(),
            12,
        )));
    }
    let body = decisions::similar_decisions(&state, &decision_id, limit).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

fn decision_filename(detail: &lj_dtos::DecisionDetail) -> String {
    let raw = detail.jurisdiction_name.clone().unwrap_or_else(|| {
        serde_json::to_value(detail.jurisdiction_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    });
    let jur = lj_dtos::abbreviate_jurisdiction_name(&raw);

    const MONTHS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    let date_str = detail
        .date_lecture
        .as_deref()
        .map(|d| {
            let parts: Vec<&str> = d.split('-').collect();
            match (parts.first(), parts.get(1), parts.get(2)) {
                (Some(y), Some(m), Some(day)) => match (day.parse::<usize>(), m.parse::<usize>()) {
                    (Ok(dd), Ok(mm)) if (1..=12).contains(&mm) => {
                        format!("{dd} {} {y}", MONTHS[mm - 1])
                    }
                    _ => d.to_string(),
                },
                _ => d.to_string(),
            }
        })
        .unwrap_or_default();

    let docket = detail
        .docket_numbers
        .as_ref()
        .and_then(|d| d.first())
        .cloned()
        .unwrap_or_default();

    let parts: Vec<String> = [jur, date_str, docket]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        format!("decision-{}", detail.id)
    } else {
        // Les n° RG judiciaires contiennent un `/` (« 25/01119 »), interdit dans un nom
        // de fichier.
        parts.join(" ").replace('/', "_")
    }
}

async fn decision_download_pdf(
    State(state): State<AppState>,
    Path(decision_id): Path<String>,
) -> Result<Response> {
    let result = decisions::get_decision(&state, &decision_id).await?;
    let filename = decision_filename(&result);
    let content = tokio::task::spawn_blocking(move || pdf_export::build_decision_pdf(&result))
        .await
        .map_err(|e| ApiError::Internal(format!("pdf render task: {e}")))?;
    Ok(attachment_response(
        content,
        "application/pdf",
        &format!("attachment; filename=\"{filename}.pdf\""),
    ))
}

fn attachment_response(content: Vec<u8>, content_type: &str, disposition: &str) -> Response {
    let mut resp = (StatusCode::OK, content).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Ok(v) = HeaderValue::from_str(&CACHE_DECISION.header_value()) {
        headers.insert(header::CACHE_CONTROL, v);
    }
    resp
}

// ── Référentiel LEGI (`/api/texte/*`) ───────────────────────────────────────

/// Sous-routeur LEGI monté sous `/api/texte`. Les sous-ressources littérales
/// `citing`/`related` sont déclarées avant la capture `{date}` : matchit privilégie
/// le segment statique sur le paramètre.
fn legi_router() -> Router<AppState> {
    Router::new()
        .route("/{code}", get(legi_code_summary))
        .route("/{code}/sommaire", get(legi_code_toc))
        .route("/{code}/section/{cid}", get(legi_section_reading))
        .route("/{code}/section/{cid}/citing", get(legi_section_citing))
        .route("/{code}/{num}", get(legi_article_in_force))
        .route("/{code}/{num}/citing", get(legi_article_citing))
        .route("/{code}/{num}/related", get(legi_article_related))
        .route("/{code}/{num}/{date}", get(legi_article_at_date))
        // Bornes du comparateur : dates ISO de fenêtre de version, `initiale` = borne
        // ouverte.
        .route("/{code}/{num}/compare/{de}/{a}", get(legi_article_compare))
}

async fn legi_code_summary(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Response> {
    let body = legi::code_summary(&state, &code).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_code_toc(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let date = q.date.as_deref().map(parse_legi_date).transpose()?;
    let body = legi::code_toc(&state, &code, date).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_section_reading(
    State(state): State<AppState>,
    Path((code, cid)): Path<(String, String)>,
    Query(q): Query<LegiDateQuery>,
) -> Result<Response> {
    let date = q.date.as_deref().map(parse_legi_date).transpose()?;
    let body = legi::law_section(&state, &code, &cid, date).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_in_force(
    State(state): State<AppState>,
    Path((code, num)): Path<(String, String)>,
) -> Result<Response> {
    let body = legi::article_in_force(&state, &code, &num).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_at_date(
    State(state): State<AppState>,
    Path((code, num, date)): Path<(String, String, String)>,
) -> Result<Response> {
    let date = parse_legi_date(&date)?;
    let body = legi::article_at_date(&state, &code, &num, date).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_compare(
    State(state): State<AppState>,
    Path((code, num, de, a)): Path<(String, String, String, String)>,
) -> Result<Response> {
    let body = legi::article_compare(&state, &code, &num, &de, &a).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_citing(
    State(state): State<AppState>,
    Path((code, num)): Path<(String, String)>,
    Query(p): Query<LegiCitingQuery>,
) -> Result<Response> {
    p.validate()?;
    let body =
        legi::article_citing(&state, &code, &num, p.date.as_deref(), p.limit, p.offset).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_section_citing(
    State(state): State<AppState>,
    Path((code, cid)): Path<(String, String)>,
    Query(p): Query<LegiCitingQuery>,
) -> Result<Response> {
    p.validate()?;
    let body =
        legi::section_citing(&state, &code, &cid, p.date.as_deref(), p.limit, p.offset).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn legi_article_related(
    State(state): State<AppState>,
    Path((code, num)): Path<(String, String)>,
) -> Result<Response> {
    let body = legi::article_co_cited(&state, &code, &num).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Date de consultation Chronolégi (`?date=YYYY-MM-DD`) des sommaires/sections ;
/// absente = en vigueur aujourd'hui.
#[derive(Debug, Deserialize)]
struct LegiDateQuery {
    #[serde(default)]
    date: Option<String>,
}

/// Pagination des décisions citantes (`limit` 1–100, `offset` ≥ 0, fenêtre
/// `offset + limit ≤ 100`) + `date` de la version servie (ISO `YYYY-MM-DD`,
/// absente = en vigueur) qui borne les citantes. Sur une division, `date` choisit
/// la composition du sous-arbre agrégé.
#[derive(Debug, Deserialize)]
struct LegiCitingQuery {
    #[serde(default = "default_citing_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    date: Option<String>,
}
fn default_citing_limit() -> i64 {
    20
}
impl LegiCitingQuery {
    fn validate(&self) -> Result<()> {
        if self.limit < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                1,
            )));
        }
        if self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                100,
            )));
        }
        if self.offset < 0 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                0,
            )));
        }
        if self.offset + self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                100 - self.limit,
            )));
        }
        Ok(())
    }
}

/// Recherche plein-texte d'articles (`GET /api/search-textes`). `q` vide ⇒ 422
/// `string_too_short`. Mêmes bornes `limit`/`offset` que les décisions citantes ;
/// `code` optionnel borne la recherche à un texte.
async fn search_textes_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    headers: HeaderMap,
    Query(p): Query<ArticleSearchQuery>,
) -> Result<Response> {
    p.validate()?;
    if p.q.trim().is_empty() {
        return Err(ApiError::Unprocessable(validation::string_too_short(
            &["query", "q"],
            &p.q,
            1,
        )));
    }
    let context = if p.context.as_deref() == Some("teaser") {
        lj_dtos::SearchContext::Teaser
    } else {
        lj_dtos::SearchContext::User
    };
    let at = p.date.as_deref().map(parse_legi_date).transpose()?;
    let body = legi::search_textes(
        &state,
        &p.q,
        p.code.as_deref(),
        p.jurisdiction.as_deref(),
        p.nature.as_deref(),
        p.source.as_deref(),
        p.scope.as_deref(),
        at,
        p.limit,
        p.offset,
        usage::Caller::web(user_id.is_some(), context, &headers),
    )
    .await?;
    if let Some(user) = user_id
        .as_deref()
        .filter(|_| p.context.as_deref() != Some("teaser"))
    {
        let mut filters = legi::textes_filters(
            p.code.as_deref(),
            p.jurisdiction.as_deref(),
            p.nature.as_deref(),
            p.source.as_deref(),
            p.scope.as_deref(),
            at,
        );
        if let (Some(obj), Some(page)) = (
            filters.as_object_mut(),
            search_history::page_number(p.offset, p.limit),
        ) {
            obj.insert("page".to_string(), page.into());
        }
        search_history::record_search(
            &state.pool,
            user,
            p.q.trim(),
            filters,
            ActivitySource::Web,
            lj_dtos::SearchEngine::Textes,
        )
        .await;
    }
    Ok(with_cache(CACHE_SEARCH, body))
}

// ── Annonces ────────────────────────────────────────────────────────────────

const ANNONCES_MAX_LIMIT: u32 = 100;
const ANNONCES_MAX_OFFSET: u32 = 5_000;

/// Parse la query string du moteur d'annonces (`/api/annonces` et les flux) —
/// mêmes clés que l'URL de la page, listes en paramètres répétés
/// (`?departement=75&departement=92`). Seuls les jetons de `fond` connus passent.
fn parse_annonces_query(raw: &str) -> Result<lj_dtos::AnnonceSearchRequest> {
    let mut req = lj_dtos::AnnonceSearchRequest::default();
    for (key, value) in parse_qs(raw) {
        match key.as_str() {
            "q" => req.q = Some(value),
            "fond" => req.fond.push(value),
            "departement" => req.departement.push(value.to_uppercase()),
            // L'égalité ODSQL des fonds proxy est sensible à la casse : la valeur passe
            // telle quelle.
            "famille" => req.famille.push(value),
            "cpv" => req.cpv.push(value),
            "echeanceJours" => {
                req.echeance_jours = Some(value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(
                        &["query", "echeanceJours"],
                        &value,
                    ))
                })?)
            }
            "closes" => req.closes = matches!(value.as_str(), "1" | "true" | "yes" | "on"),
            "sort" => {
                req.sort = de_enum::<lj_dtos::AnnonceSort>(&value).ok_or_else(|| {
                    ApiError::Unprocessable(validation::enum_error(
                        &["query", "sort"],
                        &value,
                        &["echeance", "publication", "nouveautes"],
                    ))
                })?
            }
            "limit" => {
                req.limit = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "limit"], &value))
                })?
            }
            "offset" => {
                req.offset = value.parse().map_err(|_| {
                    ApiError::Unprocessable(validation::int_parsing(&["query", "offset"], &value))
                })?
            }
            _ => {}
        }
    }
    if let Some(inconnu) = req
        .fond
        .iter()
        .find(|f| !annonces::FONDS.contains(&f.as_str()))
    {
        return Err(ApiError::Unprocessable(validation::enum_error(
            &["query", "fond"],
            inconnu,
            &annonces::FONDS,
        )));
    }
    if req.limit < 1 {
        return Err(ApiError::Unprocessable(validation::greater_than_equal(
            &["query", "limit"],
            &req.limit.to_string(),
            1,
        )));
    }
    if req.limit > ANNONCES_MAX_LIMIT {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "limit"],
            &req.limit.to_string(),
            i64::from(ANNONCES_MAX_LIMIT),
        )));
    }
    if req.offset > ANNONCES_MAX_OFFSET {
        return Err(ApiError::Unprocessable(validation::less_than_equal(
            &["query", "offset"],
            &req.offset.to_string(),
            i64::from(ANNONCES_MAX_OFFSET),
        )));
    }
    Ok(req)
}

/// Moteur facetté du pot Annonces (`GET /api/annonces`). Historique enregistré
/// pour les connectés, moteur `annonces`.
async fn annonces_endpoint(
    State(state): State<AppState>,
    OptionalUser(user_id): OptionalUser,
    RawQuery(raw): RawQuery,
) -> Result<Response> {
    let req = parse_annonces_query(raw.as_deref().unwrap_or(""))?;
    let body = annonces::search(&state, &req).await?;
    if let Some(user) = user_id.as_deref() {
        search_history::record_search(
            &state.pool,
            user,
            req.q.as_deref().unwrap_or("").trim(),
            annonces_filters(&req),
            ActivitySource::Web,
            lj_dtos::SearchEngine::Annonces,
        )
        .await;
    }
    Ok(with_cache(CACHE_SEARCH, body))
}

/// Filtres d'historique : le filtre entier moins la fenêtre, plus le numéro de
/// page — mêmes clés que l'URL de `/annonces`.
fn annonces_filters(req: &lj_dtos::AnnonceSearchRequest) -> serde_json::Value {
    let mut value = serde_json::to_value(req).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.remove("q");
        obj.remove("limit");
        obj.remove("offset");
        if let Some(page) = search_history::page_number(i64::from(req.offset), i64::from(req.limit))
        {
            obj.insert("page".to_string(), page.into());
        }
    }
    value
}

async fn annonces_catalogue(State(state): State<AppState>) -> Result<Response> {
    let body = annonces::catalogue(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn annonce_detail(
    State(state): State<AppState>,
    Path(uid): Path<String>,
) -> Result<Response> {
    let body = annonces::detail(&state, &uid).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Flux d'alerte, montés à la racine : `/annonces.rss` (nouveautés) et
/// `/annonces.ics` (échéances). Une alerte est un état de filtres — la même query
/// string que la page, aucun compte.
fn annonces_feeds_router() -> Router<AppState> {
    Router::new()
        .route("/annonces.rss", get(annonces_rss))
        .route("/annonces.ics", get(annonces_ics))
}

/// Requête d'un flux : le filtre de la page, la fenêtre forcée au plafond du flux
/// et l'ordre imposé par le média (nouveautés pour un lecteur, échéance pour un
/// agenda).
fn feed_request(raw: &str, sort: lj_dtos::AnnonceSort) -> Result<lj_dtos::AnnonceSearchRequest> {
    let mut req = parse_annonces_query(raw)?;
    req.sort = sort;
    req.limit = annonces::FEED_MAX_ITEMS;
    req.offset = 0;
    req.closes = false;
    Ok(req)
}

async fn annonces_rss(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Result<Response> {
    let raw = raw.as_deref().unwrap_or("");
    let req = feed_request(raw, lj_dtos::AnnonceSort::Nouveautes)?;
    let hits = annonces::hits_flux(&state, &req).await?;
    let base = state.settings.web_base_url.trim_end_matches('/');
    let self_url = if raw.is_empty() {
        format!("{base}/annonces.rss")
    } else {
        format!("{base}/annonces.rss?{raw}")
    };
    Ok(feed_response(
        annonces::rss(&req, &hits, &self_url, base),
        "application/rss+xml; charset=utf-8",
    ))
}

async fn annonces_ics(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Result<Response> {
    let mut req = feed_request(raw.as_deref().unwrap_or(""), lj_dtos::AnnonceSort::Echeance)?;
    // Un agenda ne parle que des fonds qui ouvrent un délai.
    if req.fond.is_empty() {
        req.fond = annonces::FONDS
            .iter()
            .filter(|f| lj_dtos::annonce_fond_a_echeance(f))
            .map(|f| (*f).to_string())
            .collect();
    } else {
        req.fond.retain(|f| lj_dtos::annonce_fond_a_echeance(f));
    }
    let hits = annonces::hits_flux(&state, &req).await?;
    let base = state.settings.web_base_url.trim_end_matches('/');
    Ok(feed_response(
        annonces::ics(&req, &hits, base),
        "text/calendar; charset=utf-8",
    ))
}

/// Réponse d'un flux : corps typé, cache CDN court et `noindex`.
fn feed_response(body: String, content_type: &'static str) -> Response {
    let mut resp = (StatusCode::OK, body).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(v) = HeaderValue::from_str(&CACHE_SEARCH.header_value()) {
        headers.insert(header::CACHE_CONTROL, v);
    }
    headers.insert(
        header::HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex"),
    );
    resp
}

/// Query `/api/codes` : `scope=head` borne aux familles de tête (codes,
/// constitutions).
#[derive(Deserialize)]
struct CodesCatalogueQuery {
    #[serde(default)]
    scope: Option<String>,
}

async fn codes_catalogue(
    State(state): State<AppState>,
    Query(p): Query<CodesCatalogueQuery>,
) -> Result<Response> {
    let body = legi::code_catalogue(&state, p.scope.as_deref() == Some("head")).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn corpus_stats(State(state): State<AppState>) -> Result<Response> {
    let body = stats::corpus_stats(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn jurisdictions_catalogue(State(state): State<AppState>) -> Result<Response> {
    let body = jurisdiction_hubs::catalogue(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn jurisdiction_hub(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Response> {
    let body = jurisdiction_hubs::hub(&state, &code).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Query `/api/juridictions/{code}/{annee}` : `page` ≥ 1 (défaut 1).
#[derive(Deserialize)]
struct JurisdictionYearQuery {
    #[serde(default)]
    page: Option<u32>,
}

async fn jurisdiction_year(
    State(state): State<AppState>,
    Path((code, annee)): Path<(String, i32)>,
    Query(p): Query<JurisdictionYearQuery>,
) -> Result<Response> {
    let body = jurisdiction_hubs::year_page(&state, &code, annee, p.page.unwrap_or(1)).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn normes_catalogue(State(state): State<AppState>) -> Result<Response> {
    let body = norm_hubs::catalogue(&state).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn norm_hub(State(state): State<AppState>, Path(fond): Path<String>) -> Result<Response> {
    let body = norm_hubs::hub(&state, &fond).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

async fn norm_year(
    State(state): State<AppState>,
    Path((fond, annee)): Path<(String, String)>,
    Query(p): Query<JurisdictionYearQuery>,
) -> Result<Response> {
    let year = match annee.as_str() {
        norm_hubs::UNDATED_TOKEN => None,
        y => Some(y.parse::<i32>().map_err(|_| ApiError::NotFound)?),
    };
    let body = norm_hubs::year_page(&state, &fond, year, p.page.unwrap_or(1)).await?;
    Ok(with_cache(CACHE_DECISION, body))
}

/// Query `/api/search-textes` : `q` (requête), `code` (slug optionnel), facettes
/// `jurisdiction`/`nature`/`source`, `date` de consultation (ISO `YYYY-MM-DD` —
/// versions valides à cette date, absente = en vigueur aujourd'hui), `limit`
/// 1–100, `offset` ≥ 0.
#[derive(Debug, Deserialize)]
struct ArticleSearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    jurisdiction: Option<String>,
    #[serde(default)]
    nature: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    date: Option<String>,
    /// `teaser` = fetch machine des ponts croisés : pas d'historique. Absent ou autre
    /// valeur = recherche utilisateur.
    #[serde(default)]
    context: Option<String>,
    #[serde(default = "default_citing_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
impl ArticleSearchQuery {
    fn validate(&self) -> Result<()> {
        if self.limit < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                1,
            )));
        }
        if self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                100,
            )));
        }
        if self.offset < 0 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                0,
            )));
        }
        Ok(())
    }
}

/// Parse le segment `{date}` d'URL (ISO `YYYY-MM-DD`). Format invalide → 422 à la
/// frontière HTTP.
fn parse_legi_date(raw: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(validation::date_parsing(
            &["path", "date"],
            raw,
            "invalid date",
        ))
    })
}

// ── Compte utilisateur (`/api/me/*`, RequiredUser) ────────────────────────────

async fn get_me(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<Response> {
    Ok(Json(me::get_me(&state, &sub).await?).into_response())
}

async fn patch_me(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Json(body): Json<UserProfileUpdate>,
) -> Result<Response> {
    if let Some(name) = body.display_name.as_deref() {
        let n = name.chars().count() as u64;
        if n < 1 {
            return Err(ApiError::Unprocessable(validation::string_too_short(
                &["body", "displayName"],
                name,
                1,
            )));
        }
        if n > 80 {
            return Err(ApiError::Unprocessable(validation::string_too_long(
                &["body", "displayName"],
                name,
                80,
            )));
        }
    }
    let profile = me::patch_me(&state, &sub, body.display_name.as_deref()).await?;
    Ok(Json(profile).into_response())
}

async fn delete_me(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<StatusCode> {
    me::delete_me(&state, &sub).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_activity_tracking(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Json(body): Json<ActivityTrackingUpdate>,
) -> Result<Response> {
    let profile = me::set_activity_tracking(&state, &sub, body.enabled).await?;
    Ok(Json(profile).into_response())
}

// ── Signets ─────────────────────────────────────────────────────────────────

async fn list_bookmarks(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<Response> {
    let (items, total) = bookmarks::list_bookmarks(&state, &sub).await?;
    Ok(Json(BookmarksResponse { items, total }).into_response())
}

async fn add_bookmark(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    bookmarks::add_bookmark(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_bookmark(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    bookmarks::remove_bookmark(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Historique de recherche ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PageQuery {
    #[serde(default = "default_page_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_page_limit() -> i64 {
    50
}
impl PageQuery {
    /// Bornes de `list_history`/`list_views` : `limit` 1–100, `offset` ≥ 0. Hors
    /// bornes → 422.
    fn validate(&self) -> Result<()> {
        if self.limit < 1 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                1,
            )));
        }
        if self.limit > 100 {
            return Err(ApiError::Unprocessable(validation::less_than_equal(
                &["query", "limit"],
                &self.limit.to_string(),
                100,
            )));
        }
        if self.offset < 0 {
            return Err(ApiError::Unprocessable(validation::greater_than_equal(
                &["query", "offset"],
                &self.offset.to_string(),
                0,
            )));
        }
        Ok(())
    }
}

async fn list_history(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Query(p): Query<PageQuery>,
) -> Result<Response> {
    p.validate()?;
    let (items, total) = search_history::list_history(&state, &sub, p.limit, p.offset).await?;
    Ok(Json(SearchHistoryResponse { items, total }).into_response())
}

async fn delete_entry(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(entry_id): Path<String>,
) -> Result<StatusCode> {
    let entry_id: i64 = entry_id.parse().map_err(|_| {
        ApiError::Unprocessable(validation::int_parsing(&["path", "entry_id"], &entry_id))
    })?;
    search_history::delete_entry(&state, &sub, entry_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_history(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<StatusCode> {
    search_history::clear_history(&state, &sub).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Décisions consultées ──────────────────────────────────────────────────────

async fn list_views(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Query(p): Query<PageQuery>,
) -> Result<Response> {
    p.validate()?;
    let (items, total) = decision_views::list_views(&state, &sub, p.limit, p.offset).await?;
    Ok(Json(DecisionViewsResponse { items, total }).into_response())
}

async fn record_view(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    decision_views::record_view(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_view(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
    Path(decision_id): Path<String>,
) -> Result<StatusCode> {
    decision_views::delete_view(&state, &sub, &decision_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_views(
    State(state): State<AppState>,
    RequiredUser(sub): RequiredUser,
) -> Result<StatusCode> {
    decision_views::clear_views(&state, &sub).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_eq_compare_le_contenu_pas_le_prefixe() {
        assert!(secret_eq("s3cret", "s3cret"));
        assert!(!secret_eq("s3cret", "s3crft"));
        assert!(!secret_eq("s3cret", "s3cretX"));
        assert!(!secret_eq("s3cretX", "s3cret"));
        assert!(!secret_eq("", "s3cret"));
        assert!(secret_eq("", ""));
    }

    #[test]
    fn search_query_rejects_malformed_date() {
        let err = parse_search_query("q=x&dateFrom=not-a-date").unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable, eu {err:?}");
        };
        assert_eq!(
            obj,
            json!({
                "type": "date_from_datetime_parsing",
                "loc": ["query", "dateFrom"],
                "msg": "Input should be a valid date or datetime, invalid character in year",
                "input": "not-a-date",
                "ctx": { "error": "invalid character in year" }
            })
        );
    }

    #[test]
    fn search_query_rejects_out_of_range_date() {
        let err = parse_search_query("q=x&dateTo=2300-01-01").unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable");
        };
        assert_eq!(obj["type"], json!("less_than_equal"));
        assert_eq!(obj["loc"], json!(["query", "dateTo"]));
    }

    #[test]
    fn search_query_accepts_valid_date() {
        let req = parse_search_query("q=x&dateFrom=2024-01-01&dateTo=2024-12-31").unwrap();
        assert_eq!(req.date_from.as_deref(), Some("2024-01-01"));
        assert_eq!(req.date_to.as_deref(), Some("2024-12-31"));
    }

    #[test]
    fn search_query_rejects_unknown_enum_token() {
        let err = parse_search_query("q=x&procedure=JAF").unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable, eu {err:?}");
        };
        assert_eq!(obj["type"], json!("enum"));
        assert_eq!(obj["loc"], json!(["query", "procedure"]));
        assert!(obj["msg"].as_str().unwrap().contains("'REFERE_SUSPENSION'"));

        let err = parse_search_query("q=x&jurisdictionType=TRIBUNAL").unwrap_err();
        let ApiError::Unprocessable(obj) = err else {
            panic!("attendu Unprocessable");
        };
        assert_eq!(obj["loc"], json!(["query", "jurisdictionType"]));
        assert!(obj["msg"].as_str().unwrap().contains("'TA'"));
    }

    #[test]
    fn search_query_accepts_valid_enum_tokens() {
        let req =
            parse_search_query("q=x&procedure=REFERE_SUSPENSION&jurisdictionType=TA&office=JAF")
                .unwrap();
        assert_eq!(req.procedure.as_deref().map(<[_]>::len), Some(1));
        assert_eq!(req.office.as_deref().map(<[_]>::len), Some(1));
    }
}
