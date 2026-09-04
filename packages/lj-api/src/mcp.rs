//! Remote MCP server (Streamable HTTP, JSON response) mounted under `/mcp`.
//! The JSON-RPC layer is hand-rolled on top of axum.

use axum::{
    extract::{Request, State},
    http::header::{AUTHORIZATION, WWW_AUTHENTICATE},
    http::{HeaderMap, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use lj_dtos::mcp_tools::{
    enum_values, nullable, read_only_annotations, ACTIVITY_KINDS, SERVER_INSTRUCTIONS,
};
use lj_dtos::Domain;
use opentelemetry::KeyValue;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::Instrument;

use crate::annonces::FONDS;
use crate::bookmarks::fetch_bookmarks;
use crate::decision_views::{fetch_views, record_decision_view};
use crate::error::ApiError;
use crate::mcp_presenters::{
    present_annonce_search, present_bookmarks, present_decision_detail, present_law_article,
    present_law_search, present_law_text, present_reading_history, present_saved_searches,
    present_search_response, McpActivityResponse,
};
use crate::search_history::{fetch_history, record_search};
use crate::state::AppState;

#[derive(Clone, Copy)]
struct Provenance<'a> {
    first_party: bool,
    user_agent: Option<&'a str>,
}

fn mcp_caller(
    user: Option<&str>,
    prov: Provenance<'_>,
    pseudo: Option<&crate::identity::Pseudonymiser>,
) -> crate::usage::Caller {
    // MCP is always authenticated, so `caller` names the caller and `visit` stays
    // empty.
    crate::usage::Caller {
        source: lj_dtos::ActivitySource::Mcp,
        authenticated: user.is_some(),
        context: lj_dtos::SearchContext::User,
        first_party: prov.first_party,
        caller: mcp_pseudonym(user, pseudo),
        visit: None,
        user_agent: prov.user_agent.map(str::to_owned),
    }
}

fn mcp_pseudonym(
    user: Option<&str>,
    pseudo: Option<&crate::identity::Pseudonymiser>,
) -> Option<String> {
    pseudo
        .zip(user)
        .and_then(|(p, sub)| p.caller(&crate::identity::Identity::Account(sub.to_string())))
}

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "librejustice";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Served on both `/mcp/` and `/mcp`: MCP clients POST the URL as typed and do
/// not follow a redirect on POST.
pub fn mcp_router(state: AppState) -> Router {
    Router::new()
        .route("/mcp/", post(handle_rpc))
        .route("/mcp", post(handle_rpc))
        .layer(from_fn_with_state(state.clone(), mcp_auth))
        .with_state(state)
}

// ── MCP auth ─────────────────────────────────────────────────────────────────

/// `None` when MCP auth is off (`mcp_require_auth=false`, tests and local dev).
#[derive(Debug, Clone)]
struct McpUser(Option<String>);

/// The `WWW-Authenticate` challenge (RFC 9728 + MCP 2025-03-26) is what triggers
/// OAuth auto-discovery in clients.
fn mcp_unauthorized(challenge: &str, error: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, challenge.to_string())],
        Json(json!({ "error": error })),
    )
        .into_response()
}

/// Timing breakdown of the MCP path, written into the response extensions by
/// [`mcp_auth`] and read back by the queue log ([`crate::routes`]).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct McpPhases {
    /// Both `pool.get()` of the middleware, summed.
    pub pool: Duration,
    pub lookup: Duration,
    /// `UPDATE users.last_seen_at`.
    pub bump: Duration,
    /// Everything past auth: JSON-RPC layer and tool.
    pub dispatch: Duration,
    /// Canonical tool, aliases folded. `None` on protocol methods, where
    /// `dispatch` only measures the JSON-RPC envelope.
    pub tool: Option<&'static str>,
}

/// Carries the executed tool up to [`mcp_auth`], the only writer of [`McpPhases`].
#[derive(Debug, Clone, Copy)]
struct CalledTool(Option<&'static str>);

/// `None` when the token is absent from the table or expired. The comparison is
/// on the SHA-256 digest: the database no longer holds enough to impersonate a
/// bearer.
async fn lookup_mcp_user(
    state: &AppState,
    token: &str,
    phases: &mut McpPhases,
) -> Result<Option<String>, ApiError> {
    let digest = crate::oauth::sha256(token.as_bytes());
    let digest: &[u8] = &digest;
    let (conn, waited) = lj_store::db::get_timed(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    phases.pool += waited;
    let t = std::time::Instant::now();
    let row = conn
        .query_opt(
            "SELECT user_id FROM mcp_tokens WHERE access_token_sha256 = $1 AND expires_at > now()",
            &[&digest],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("mcp token lookup: {e}")))?;
    phases.lookup = t.elapsed();
    Ok(row.map(|r| r.get::<_, String>(0)))
}

async fn bump_last_seen(state: &AppState, user_id: &str, phases: &mut McpPhases) {
    let Ok((conn, waited)) = lj_store::db::get_timed(&state.pool).await else {
        return;
    };
    phases.pool += waited;
    let t = std::time::Instant::now();
    if let Err(e) = conn
        .execute(
            "UPDATE users SET last_seen_at = now() WHERE sub = $1",
            &[&user_id],
        )
        .await
    {
        tracing::warn!("bump last_seen (mcp) failed: {e}");
    }
    phases.bump = t.elapsed();
}

async fn mcp_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let mut user: Option<String> = None;
    let mut phases = McpPhases::default();
    let reseau = crate::ratelimit::caller_key(None, req.headers());
    if state
        .budgets
        .take(crate::ratelimit::Cost::Porte, reseau.as_ref())
        .await
        == crate::ratelimit::Verdict::Exhausted
    {
        return ApiError::TooManyRequests.into_response();
    }
    if state.settings.mcp_require_auth {
        let base = state.settings.public_base_url.trim_end_matches('/');
        let challenge = format!(
            "Bearer realm=\"mcp\", resource_metadata=\"{base}/.well-known/oauth-protected-resource\""
        );
        let auth = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let Some(token) = auth.strip_prefix("Bearer ") else {
            tracing::debug!("mcp: challenge servi (aucun Bearer)");
            return mcp_unauthorized(&challenge, "auth_required");
        };
        match lookup_mcp_user(&state, token, &mut phases).await {
            Ok(Some(uid)) => {
                bump_last_seen(&state, &uid, &mut phases).await;
                user = Some(uid);
            }
            // The reason is repeated in the message body: a Loki line filter (`|=`)
            // only sees that, never the structured metadata.
            Ok(None) => {
                tracing::warn!(
                    mcp_token_rejete = "invalid_token",
                    "jeton MCP refusé : invalid_token (absent de la table ou expiré)"
                );
                return mcp_unauthorized(&challenge, "invalid_token");
            }
            Err(e) => {
                tracing::warn!(
                    mcp_token_rejete = "lookup_echec",
                    error = %e,
                    "jeton MCP non vérifiable : lookup_echec"
                );
                return e.into_response();
            }
        }
    }
    req.extensions_mut().insert(McpUser(user));
    let t = std::time::Instant::now();
    let mut resp = next.run(req).await;
    phases.dispatch = t.elapsed();
    phases.tool = resp.extensions().get::<CalledTool>().and_then(|o| o.0);
    resp.extensions_mut().insert(phases);
    resp
}

// ── JSON-RPC layer ───────────────────────────────────────────────────────────

fn count_call(outil: &'static str, categorie: &'static str) {
    lj_telemetry::meter()
        .u64_counter("mcp.tool.calls")
        .with_description("Appels d'outils MCP, par outil et par catégorie de sortie.")
        .build()
        .add(
            1,
            &[
                KeyValue::new("tool", outil),
                KeyValue::new("category", categorie),
            ],
        );
}

/// What the tool error contract tells the model to do next.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    /// Invalid arguments: fix the call, an identical retry will fail again.
    Validation,
    /// Absent from the corpus, which never proves the resource does not exist.
    NotFound,
    Auth,
    /// The account's allowance for the minute is spent: wait, then retry.
    RateLimited,
    /// Internal or transient failure: an identical retry may succeed.
    Internal,
}

impl ErrorCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::Auth => "auth",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
    category: ErrorCategory,
    /// `true` when an identical retry may succeed.
    retryable: bool,
    /// Next action suggested to the model.
    hint: Option<String>,
}

impl RpcError {
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            category: ErrorCategory::Validation,
            retryable: false,
            hint: None,
        }
    }
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            category: ErrorCategory::Validation,
            retryable: false,
            hint: None,
        }
    }
    fn tool_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::Internal,
            retryable: true,
            hint: None,
        }
    }
    /// The `hint` must restate that absence from the corpus does not prove
    /// non-existence.
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::NotFound,
            retryable: false,
            hint: None,
        }
    }
    fn auth(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::Auth,
            retryable: false,
            hint: None,
        }
    }
    fn rate_limited() -> Self {
        Self {
            code: -32603,
            message: "rate limited: this account's allowance for the minute is spent".into(),
            category: ErrorCategory::RateLimited,
            retryable: true,
            hint: Some(
                "The allowance refills continuously: wait about a minute, then resume. \
                 Batch what you need from each search rather than fetching every hit."
                    .into(),
            ),
        }
    }
    /// A failure of the API layer, sorted the way the HTTP surface sorts it.
    fn from_api(e: ApiError) -> Self {
        match e {
            ApiError::TooManyRequests => Self::rate_limited(),
            other => Self::tool_error(other.to_string()),
        }
    }
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

async fn handle_rpc(
    State(state): State<AppState>,
    Extension(McpUser(user)): Extension<McpUser>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let params = body.get("params").cloned().unwrap_or(Value::Null);
    let session = mcp_session_hash(&headers);
    let prov = Provenance {
        first_party: crate::usage::is_first_party(&headers, &state.settings),
        user_agent: crate::usage::user_agent(&headers),
    };

    let is_notification = body.get("id").is_none();

    let mut outil = None;
    let result = dispatch(
        &state,
        method,
        params,
        user.as_deref(),
        session.as_deref(),
        prov,
        &mut outil,
    )
    .await;
    let mut resp = if is_notification {
        StatusCode::ACCEPTED.into_response()
    } else {
        match result {
            Ok(result) => {
                Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
            }
            Err(err) => Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": err.code, "message": err.message},
            }))
            .into_response(),
        }
    };
    resp.extensions_mut().insert(CalledTool(outil));
    resp
}

/// Conversation correlator: a hash of the `Mcp-Session-Id` header, never the
/// user's identity.
fn mcp_session_hash(headers: &HeaderMap) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let raw = headers.get("mcp-session-id")?.to_str().ok()?;
    if raw.is_empty() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    raw.hash(&mut hasher);
    Some(format!("{:016x}", hasher.finish()))
}

async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
    prov: Provenance<'_>,
    outil: &mut Option<&'static str>,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize_result()),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(state, params, user, session, prov, outil).await,
        other => Err(RpcError::method_not_found(other)),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "experimental": {},
            "tools": {"listChanged": false},
        },
        "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        "instructions": SERVER_INSTRUCTIONS,
    })
}

// ── Tools: dispatch ──────────────────────────────────────────────────────────

async fn call_tool(
    state: &AppState,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
    prov: Provenance<'_>,
    outil: &mut Option<&'static str>,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Usage pseudonym, never the identity, the `sub` or the address.
    //
    // The span carries the detail, the counter the volumes, the log line of the
    // `internal` case the alert: the counter is born at the first failure of the
    // process, so `increase()` over a short window counted 2.7 out of 6 on
    // 22/08/2026.
    let span = tracing::info_span!(
        "mcp_tool_call",
        librejustice.mcp.tool = name,
        librejustice.mcp.authenticated = user.is_some(),
        librejustice.mcp.origin = crate::usage::origin_value(prov.first_party),
        librejustice.mcp.session = tracing::field::Empty,
        librejustice.mcp.arg = tracing::field::Empty,
        librejustice.mcp.outcome = tracing::field::Empty,
        librejustice.mcp.error_category = tracing::field::Empty,
        librejustice.mcp.error = tracing::field::Empty,
        // Same name as on the `search` span: one query sieves both.
        librejustice.search.caller = mcp_pseudonym(user, state.pseudonymiser.as_ref()).as_deref(),
        user_agent.original = prov.user_agent,
    );
    if let Some(s) = session {
        span.record("librejustice.mcp.session", s);
    }
    let arg: String = arguments.to_string().chars().take(256).collect();
    span.record("librejustice.mcp.arg", arg.as_str());

    async move {
        // MCP semantics: a tool execution failure is not a JSON-RPC error but a
        // result with `isError: true` and the message in `content`. Only protocol
        // errors stay JSON-RPC errors.
        //
        // The counter label is a literal, never `name`: a client calling made-up
        // names would create one series per name.
        let (label, result) = match name {
            "search_decisions" => (
                "search_decisions",
                tool_search_decisions(state, arguments, user, prov).await,
            ),
            "get_decision" => (
                "get_decision",
                tool_get_decision(state, arguments, user).await,
            ),
            "list_my_activity" => (
                "list_my_activity",
                tool_list_my_activity(state, arguments, user).await,
            ),
            "get_legal_text" => (
                "get_legal_text",
                tool_get_legal_text(state, arguments, user).await,
            ),
            "search_legal_texts" => (
                "search_legal_texts",
                tool_search_legal_texts(state, arguments, user, prov).await,
            ),
            "search_annonces" if PUBLISH_SEARCH_ANNONCES => (
                "search_annonces",
                tool_search_annonces(state, arguments, user).await,
            ),
            // An MCP client freezes the tool list when the connector is installed
            // and the model can only emit a name from that list: a rename is
            // recoverable by an alias and nothing else.
            "get_law_article" | "get_legal_article" => (
                "get_legal_text",
                tool_get_legal_text(state, arguments, user).await,
            ),
            "search_law_articles" | "search_legal_articles" => (
                "search_legal_texts",
                tool_search_legal_texts(state, arguments, user, prov).await,
            ),
            other => (
                "unknown",
                Err(RpcError::invalid_params(format!("Unknown tool: {other}"))),
            ),
        };

        *outil = Some(label);

        let span = tracing::Span::current();
        match result {
            Ok((text, structured)) => {
                span.record("librejustice.mcp.outcome", "ok");
                count_call(label, "ok");
                Ok(json!({
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": structured,
                    "isError": false,
                }))
            }
            Err(e) => {
                span.record("librejustice.mcp.outcome", "error");
                span.record("librejustice.mcp.error_category", e.category.as_str());
                span.record("librejustice.mcp.error", e.message.as_str());
                count_call(label, e.category.as_str());
                if e.category == ErrorCategory::Internal {
                    tracing::error!(
                        librejustice.mcp.tool = label,
                        librejustice.mcp.error = e.message.as_str(),
                        "mcp: échec interne d'outil"
                    );
                }
                Ok(tool_error_content(&e))
            }
        }
    }
    .instrument(span)
    .await
}

fn tool_error_content(err: &RpcError) -> Value {
    let mut body = json!({
        "error": err.message,
        "category": err.category.as_str(),
        "retryable": err.retryable,
    });
    if let Some(hint) = &err.hint {
        body["hint"] = json!(hint);
    }
    json!({
        "content": [{"type": "text", "text": body.to_string()}],
        "isError": true,
    })
}

/// One `Read` token on the OAuth account; with no bearer the caller is not limited.
async fn lecture(state: &AppState, user: Option<&str>) -> Result<(), RpcError> {
    let key = crate::ratelimit::caller_key(user, &axum::http::HeaderMap::new());
    match state
        .budgets
        .take(crate::ratelimit::Cost::Read, key.as_ref())
        .await
    {
        crate::ratelimit::Verdict::Allowed => Ok(()),
        crate::ratelimit::Verdict::Exhausted => Err(RpcError::rate_limited()),
    }
}

/// Same JSON in both channels, compact: the text block is what hosts inject
/// into the model.
fn tool_ok<T: serde::Serialize>(value: &T) -> Result<(String, Value), RpcError> {
    let text = serde_json::to_string(value).map_err(|e| RpcError::tool_error(e.to_string()))?;
    let structured =
        serde_json::to_value(value).map_err(|e| RpcError::tool_error(e.to_string()))?;
    Ok((text, structured))
}

async fn tool_search_decisions(
    state: &AppState,
    args: Value,
    user: Option<&str>,
    prov: Provenance<'_>,
) -> Result<(String, Value), RpcError> {
    let mut req = build_search_request(&args)?;
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    // A code outside the referential would silently match zero decisions.
    if let Some(codes) = req.jurisdiction_code.as_deref_mut() {
        relax_jurisdiction_codes(codes, &refs);
    }
    if let Some(bad) = req
        .jurisdiction_code
        .iter()
        .flatten()
        .find(|c| refs.jurisdiction(c).is_none())
    {
        return Err(unknown_jurisdiction_code(bad, &refs));
    }
    if let Some(bad) = req.seat.iter().flatten().find(|p| !refs.has_seat(p)) {
        return Err(
            RpcError::invalid_params(crate::referential::unknown_seat_msg(bad, refs.seat_nodes()))
                .with_hint(
                    "Pass paths from the seat facet of a previous search. A path matches \
                 itself and everything below it, so start short and go deeper."
                        .to_string(),
                ),
        );
    }
    // Same guarantee for the controlled-vocabulary axes.
    if let Some(bad) = req
        .publication
        .as_deref()
        .and_then(|vs| refs.find_unknown_token("publication", vs))
    {
        let tokens = refs.facet_tokens("publication");
        return Err(RpcError::invalid_params(format!(
            "unknown publication '{bad}'; valid values: {}",
            tokens.join(", ")
        ))
        .with_hint(list_hint("publication", &tokens)));
    }
    let filter_rewritten = rewrite_legacy_matiere(&args, &mut req)?;
    // The columns carry catalogue uids, not names.
    resolve_instrument_filters(state, &mut req).await?;
    // Budget per MCP account: the OAuth token is the key, and it is revocable.
    // With no bearer the caller is not identifiable, so not limited.
    let budget_key = crate::ratelimit::caller_key(user, &axum::http::HeaderMap::new());
    // The count is resolved before answering: an agent calls once, and the total
    // is what tells it whether its window covers the matched set.
    let response = crate::search::search_with_count(
        state,
        &req,
        mcp_caller(user, prov, state.pseudonymiser.as_ref()),
        budget_key.as_ref(),
    )
    .await
    .map_err(RpcError::from_api)?;
    if let Some(user_id) = user {
        record_search(
            &state.pool,
            user_id,
            &req.query,
            crate::search_history::filters_from_request(&req),
            lj_dtos::ActivitySource::Mcp,
            lj_dtos::SearchEngine::Decisions,
        )
        .await;
    }
    let mut out = present_search_response(&response, &state.settings.web_base_url);
    out.filter_rewritten = filter_rewritten;
    tool_ok(&out)
}

async fn tool_get_decision(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    lecture(state, user).await?;
    let url = required_str(&args, "url")?;
    // The key is the last segment of the public url `/decision/{key}`.
    let id = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("not a decision url: {url}")))?;
    let detail = crate::decisions::get_decision(state, id)
        .await
        .map_err(|e| match e {
            ApiError::NotFound => RpcError::not_found(format!("no decision at url={url}"))
                .with_hint(
                    "Decision urls cannot be composed or guessed: take the url verbatim \
                     from a search_decisions hit or an inline citation link. If this one \
                     did come from a hit, the decision may have been unpublished since.",
                ),
            other => RpcError::tool_error(other.to_string()),
        })?;
    if let Some(user_id) = user {
        record_decision_view(&state.pool, user_id, id, lj_dtos::ActivitySource::Mcp).await;
    }
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    tool_ok(&present_decision_detail(
        &detail,
        &state.settings.web_base_url,
        &refs,
    ))
}

async fn tool_list_my_activity(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let user_id = user.ok_or_else(|| {
        RpcError::auth(
            "authentication required: connect your LibreJustice account to this MCP \
             connector to access your activity.",
        )
    })?;
    lecture(state, user).await?;
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("all");
    if !ACTIVITY_KINDS.contains(&kind) {
        return Err(RpcError::invalid_params(format!(
            "unknown kind '{kind}'; valid values: {}",
            ACTIVITY_KINDS.join(", ")
        )));
    }
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
    let web = &state.settings.web_base_url;
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;

    let mut out = McpActivityResponse::default();
    if matches!(kind, "searches" | "all") {
        let (entries, _) = fetch_history(&state.pool, user_id, Some(limit), 0)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?;
        out.searches = Some(present_saved_searches(&entries));
    }
    if matches!(kind, "bookmarks" | "all") {
        let (items, _) = fetch_bookmarks(&state.pool, &refs, user_id, Some(limit), 0)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?;
        out.bookmarks = Some(present_bookmarks(&items, web, &refs));
    }
    if matches!(kind, "readingHistory" | "all") {
        let (views, _) = fetch_views(&state.pool, &refs, user_id, Some(limit), 0)
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?;
        out.reading_history = Some(present_reading_history(&views, web, &refs));
    }
    tool_ok(&out)
}

// ── Law-at-date tool ─────────────────────────────────────────────────────────

async fn tool_get_legal_text(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    lecture(state, user).await?;
    let date: Option<String> = deser_field(&args, "date")?;

    let (code, num) = match law_target(&args)? {
        LawTarget::Article { code, num } => (code, num),
        LawTarget::Root { code } => return tool_get_legal_root(state, &code).await,
        LawTarget::Section { code, cid } => {
            return tool_get_legal_section(state, &code, &cid, date.as_deref()).await
        }
        LawTarget::SourceUid(uid) => resolve_source_uid(state, &uid).await?,
    };
    let num = lj_core::article_key::article_key(&num);

    let code = resolve_law_code(state, &code).await?;

    let detail = match date {
        Some(d) => {
            let parsed = parse_mcp_date("date", &d)?;
            crate::legi::article_at_date(state, &code, &num, parsed).await
        }
        None => crate::legi::article_in_force(state, &code, &num).await,
    };
    let detail = match detail {
        Ok(detail) => detail,
        Err(ApiError::NotFound) => return Err(article_not_found(state, &code).await),
        Err(other) => return Err(map_legi_error(other)),
    };
    tool_ok(&present_law_article(&detail, &state.settings.web_base_url))
}

async fn tool_search_legal_texts(
    state: &AppState,
    args: Value,
    user: Option<&str>,
    prov: Provenance<'_>,
) -> Result<(String, Value), RpcError> {
    let query = required_str(&args, "query")?;
    if query.chars().count() > lj_dtos::QUERY_MAX_CHARS {
        return Err(RpcError::invalid_params(query_too_long()));
    }
    let opt = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let limit = clamp_search_limit(&args)?;

    let code = match opt("code") {
        Some(c) => Some(resolve_law_code(state, c).await?),
        None => None,
    };

    let at = match opt("date") {
        Some(d) => Some(parse_mcp_date("date", d)?),
        None => None,
    };

    let response = crate::legi::search_textes(
        state,
        &query,
        code.as_deref(),
        opt("jurisdiction"),
        None,
        None,
        None,
        at,
        limit,
        0,
        mcp_caller(user, prov, state.pseudonymiser.as_ref()),
        crate::ratelimit::caller_key(user, &axum::http::HeaderMap::new()).as_ref(),
    )
    .await
    .map_err(map_legi_error)?;

    if let Some(user_id) = user {
        let mut filters = serde_json::Map::new();
        if let Some(c) = code.as_deref() {
            filters.insert("code".to_string(), Value::String(c.to_string()));
        }
        if let Some(j) = opt("jurisdiction") {
            filters.insert("jurisdiction".to_string(), Value::String(j.to_string()));
        }
        if let Some(d) = opt("date") {
            filters.insert("date".to_string(), Value::String(d.to_string()));
        }
        record_search(
            &state.pool,
            user_id,
            &query,
            Value::Object(filters),
            lj_dtos::ActivitySource::Mcp,
            lj_dtos::SearchEngine::Textes,
        )
        .await;
    }

    tool_ok(&present_law_search(
        &query,
        &response,
        &state.settings.web_base_url,
    ))
}

/// Facets only: this pot has neither BM25 nor embeddings, and `query` is a word
/// filter.
async fn tool_search_annonces(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let strings = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_uppercase)
                    .collect()
            })
            .unwrap_or_default()
    };
    let limit = clamp_search_limit(&args)?;
    lecture(state, user).await?;
    let departement = strings("departement");
    // A code outside the nomenclature filters nothing: a plain error rather than
    // an empty page a model would read as "no tender at all".
    if let Some(bad) = departement
        .iter()
        .find(|c| lj_core::referential_labels::departement_label(c).is_none())
    {
        return Err(RpcError::invalid_params(format!(
            "departement: {bad:?} is not a French département code (\"75\", \"2A\", \"973\")"
        )));
    }
    let famille = strings("nature");
    if let Some(bad) = famille
        .iter()
        .find(|f| !NATURES_MARCHE.contains(&f.as_str()))
    {
        return Err(RpcError::invalid_params(format!(
            "nature: {bad:?} is not one of {NATURES_MARCHE:?}"
        )));
    }
    let fond: Vec<String> = args
        .get("fund")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_lowercase)
                .collect()
        })
        .unwrap_or_else(|| FONDS.iter().map(|f| f.to_string()).collect());
    if let Some(bad) = fond.iter().find(|f| !FONDS.contains(&f.as_str())) {
        return Err(RpcError::invalid_params(format!(
            "fund: {bad:?} is not one of {FONDS:?}"
        )));
    }
    let req = lj_dtos::AnnonceSearchRequest {
        q: args
            .get("query")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        fond,
        departement,
        famille,
        cpv: strings("cpv"),
        echeance_jours: args
            .get("closing_within_days")
            .and_then(Value::as_u64)
            .map(|j| j as u32),
        sort: lj_dtos::AnnonceSort::Echeance,
        limit: limit as u32,
        ..lj_dtos::AnnonceSearchRequest::default()
    };
    let response = crate::annonces::search(state, &req)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;

    if let Some(user_id) = user {
        record_search(
            &state.pool,
            user_id,
            req.q.as_deref().unwrap_or(""),
            serde_json::to_value(&req).unwrap_or_else(|_| json!({})),
            lj_dtos::ActivitySource::Mcp,
            lj_dtos::SearchEngine::Annonces,
        )
        .await;
    }
    tool_ok(&present_annonce_search(
        &response,
        &state.settings.web_base_url,
    ))
}

/// `ta_clermont-ferrand` for `ta_clermont_ferrand`: none of the 376 codes of the
/// referential carries a hyphen, and swapping the hyphen for an underscore puts
/// none of them in collision, so the recovery cannot land on another court.
fn relax_jurisdiction_codes(codes: &mut [String], refs: &crate::referential::Referential) {
    for code in codes.iter_mut().filter(|c| refs.jurisdiction(c).is_none()) {
        let underscored = code.replace('-', "_");
        if refs.jurisdiction(&underscored).is_some() {
            tracing::warn!(
                librejustice.mcp.relaxed_field = "jurisdiction_code",
                librejustice.mcp.relaxed_value = code.as_str(),
                librejustice.mcp.relaxed_slug = underscored.as_str(),
                "mcp: code de juridiction rattrapé sur le souligné"
            );
            *code = underscored;
        }
    }
}

/// The hint carries the call shape on two real codes: the whole list runs to a
/// thousand entries and does not enumerate in a message.
fn unknown_jurisdiction_code(code: &str, refs: &crate::referential::Referential) -> RpcError {
    let examples: Vec<&str> = refs.jurisdictions().map(|(c, _)| c).take(2).collect();
    RpcError::invalid_params(crate::referential::unknown_jurisdiction_code_msg(
        code, refs,
    ))
    .with_hint(list_hint("jurisdiction_code", &examples))
}

/// The 24 values of the retired `matiere` argument, mapped to the subject a
/// caller meant by them. `None`: the value named a court formation, no subject
/// matches it.
const MATIERE_VERS_DOMAINE: [(&str, Option<Domain>); 24] = [
    ("BAUX", Some(Domain::CivilDroitLocatif)),
    ("CIVI", Some(Domain::CivilDroitResponsabilite)),
    ("CIVILE", Some(Domain::Civil)),
    ("COMMERCIALE", Some(Domain::Commercial)),
    ("CONSEIL", None),
    (
        "CONSTRUCTION",
        Some(Domain::CivilDroitImmobilierConstruction),
    ),
    (
        "COPROPRIETE",
        Some(Domain::CivilDroitCoproprieteProprieteImmobiliere),
    ),
    ("CORRECTIONNELLE", Some(Domain::Criminel)),
    ("CRIMINELLE", Some(Domain::Criminel)),
    ("DALO", Some(Domain::PublicDroitAideActionSociale)),
    ("ETRANGERS", Some(Domain::PublicDroitEtrangersNationalite)),
    (
        "EXPROPRIATION",
        Some(Domain::CivilDroitExpropriationPreemption),
    ),
    ("FAMILLE", Some(Domain::CivilDroitPersonnesFamille)),
    ("INSTRUCTION", Some(Domain::Criminel)),
    ("MINEURS", Some(Domain::CivilDroitMineurs)),
    ("NATIONALITE", Some(Domain::PublicDroitEtrangersNationalite)),
    (
        "PROCEDURES_COLLECTIVES",
        Some(Domain::CommercialDroitEntreprisesDifficulte),
    ),
    (
        "PROTECTION_SOCIALE",
        Some(Domain::SocialDroitAideActionSociale),
    ),
    ("PROXIMITE", None),
    ("PRUD_HOMALE", Some(Domain::SocialDroitTravail)),
    ("SOCIALE", Some(Domain::Social)),
    ("SURENDETTEMENT", Some(Domain::CommercialDroitConsommation)),
    ("TERRES", Some(Domain::CivilDroitRural)),
    ("URGENCES", None),
];

/// The `matiere` values named the chamber, not the subject, and `ETRANGERS`
/// missed 82.8 % of the litigation it aimed at (131,575 decisions against
/// 751,570).
fn rewrite_legacy_matiere(
    args: &Value,
    req: &mut lj_dtos::SearchRequest,
) -> Result<Option<String>, RpcError> {
    let legacy: Vec<String> = match deser_list(args, "matiere")? {
        Some(vs) if !vs.is_empty() => vs,
        _ => return Ok(None),
    };
    let mut domains: Vec<Domain> = Vec::new();
    for value in &legacy {
        match MATIERE_VERS_DOMAINE.iter().find(|(m, _)| m == value) {
            Some((_, Some(domain))) if !domains.contains(domain) => domains.push(*domain),
            Some((_, Some(_))) => {}
            Some((_, None)) => {
                return Err(retired_matiere(&format!(
                    "matiere '{value}' is retired and named a court formation, not a subject: \
                     filter by seat for a court's own formation, or by procedure"
                )))
            }
            None => {
                return Err(retired_matiere(&format!(
                    "matiere '{value}' is retired: filter by legal_domain instead"
                )))
            }
        }
    }
    let asked = legacy.join(", ");
    tracing::warn!(
        librejustice.mcp.relaxed_field = "matiere",
        librejustice.mcp.relaxed_value = asked.as_str(),
        "mcp: filtre matiere retiré, réécrit en legal_domain"
    );
    // A `legal_domain` already set says the subject better than the retired axis:
    // widening it would return more than the call asks for.
    if req.legal_domain.iter().flatten().next().is_some() {
        return Ok(Some(format!(
            "The `matiere` filter is retired: `matiere: [{asked}]` was dropped, \
             `legal_domain` was set and carries the subject. Use `legal_domain` alone."
        )));
    }
    let rewritten = domains
        .iter()
        .map(domain_token)
        .collect::<Vec<String>>()
        .join(", ");
    req.legal_domain = Some(domains);
    Ok(Some(format!(
        "The `matiere` filter is retired: `matiere: [{asked}]` was read as \
         `legal_domain: [{rewritten}]`. It named the chamber that heard the case, \
         so it missed every court that names its chambers otherwise, while \
         `legal_domain` names the subject. Filter by `legal_domain` from now on."
    )))
}

fn retired_matiere(message: &str) -> RpcError {
    let tokens: Vec<String> = MATIERE_VERS_DOMAINE
        .iter()
        .filter_map(|(_, d)| d.as_ref().map(domain_token))
        .collect();
    RpcError::invalid_params(message).with_hint(list_hint("legal_domain", &tokens))
}

/// Filter token of a domain (`CIVIL_DROIT_LOCATIF`), its serialised form.
fn domain_token(d: &Domain) -> String {
    match serde_json::to_value(d) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Resolves an instrument value (slug, free name or acronym) to its catalogue
/// `(slug, text_uid)`.
async fn resolve_instrument(
    repo: &lj_store::repository::DecisionRepository<'_>,
    field: &str,
    value: &str,
) -> Result<(String, String), RpcError> {
    use lj_store::repository::InstrumentResolution as R;
    match repo
        .resolve_instrument(value)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?
    {
        R::Direct { slug, text_uid } => Ok((slug, text_uid)),
        R::Rattrape {
            slug,
            text_uid,
            regle: rule,
        } => {
            tracing::warn!(
                librejustice.mcp.relaxed_field = field,
                librejustice.mcp.relaxed_value = value,
                librejustice.mcp.relaxed_slug = slug.as_str(),
                librejustice.mcp.relaxed_rule = ?rule,
                "mcp: nom de texte rattrapé sur son slug de catalogue"
            );
            Ok((slug, text_uid))
        }
        R::Inconnu { candidates } => Err(unknown_instrument(field, value, candidates)),
    }
}

/// Resolves `legal_instrument` / `legal_article` to the alphabet of the columns:
/// catalogue `text_uid`, article number in composite form.
async fn resolve_instrument_filters(
    state: &AppState,
    req: &mut lj_dtos::SearchRequest,
) -> Result<(), RpcError> {
    if req.legal_instrument.is_none() && req.legal_article.is_none() {
        return Ok(());
    }
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| RpcError::tool_error(format!("checkout connexion: {e}")))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    if let Some(values) = req.legal_instrument.as_mut() {
        for v in values.iter_mut() {
            *v = resolve_instrument(&repo, "legal_instrument", v).await?.1;
        }
    }
    if let Some(values) = req.legal_article.as_mut() {
        for v in values.iter_mut() {
            let Some((instrument, num)) = v.split_once('|') else {
                return Err(RpcError::invalid_params(format!(
                    "legal_article '{v}' must be \"<instrument>|<article>\" \
                     (e.g. \"code-civil|1240\", \"code-de-justice-administrative|L761-1\")"
                )));
            };
            let (_, uid) = resolve_instrument(&repo, "legal_article", instrument.trim()).await?;
            *v = format!("{uid}|{}", lj_core::article_key::article_key(num));
        }
    }
    Ok(())
}

fn unknown_instrument_msg(
    field: &str,
    value: &str,
    candidates: &[lj_store::repository::InstrumentCandidate],
) -> String {
    let shown: Vec<String> = candidates
        .iter()
        .take(5)
        .map(|c| format!("\"{}\" ({})", c.slug, c.title))
        .collect();
    let did_you_mean = if shown.is_empty() {
        String::new()
    } else {
        format!("; did you mean: {}", shown.join(", "))
    };
    format!("unknown {field} '{value}'{did_you_mean}")
}

/// Search-filter side: two list fields, hence the call shape on top of the
/// candidates.
fn unknown_instrument(
    field: &str,
    value: &str,
    candidates: Vec<lj_store::repository::InstrumentCandidate>,
) -> RpcError {
    let msg = unknown_instrument_msg(field, value, &candidates);
    let slugs: Vec<String> = candidates.into_iter().take(5).map(|c| c.slug).collect();
    RpcError::invalid_params(msg).with_hint(format!(
        "Pass slugs (from the legal_instrument facet of a previous search) or \
         exact text names. {}",
        list_hint(field, &slugs)
    ))
}

/// Scalar `code` field: no list shape to give, only the alphabet of its values.
async fn resolve_law_code(state: &AppState, raw: &str) -> Result<String, RpcError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| RpcError::tool_error(format!("checkout connexion: {e}")))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    resolve_instrument(&repo, "code", raw)
        .await
        .map(|(slug, _)| slug)
        .map_err(|e| {
            RpcError::invalid_params(e.message).with_hint(
                "Pass a slug (from the legal_instrument facet of a previous search) \
                 or an exact text name.",
            )
        })
}

/// `/texte/{code}` with no article segment: the table of contents of a text with
/// articles, bodies attached below `INLINE_READING_MAX`.
async fn tool_get_legal_root(state: &AppState, code: &str) -> Result<(String, Value), RpcError> {
    let slug = resolve_law_code(state, code).await?;
    let web = &state.settings.web_base_url;
    match crate::legi::law_root(state, &slug)
        .await
        .map_err(map_legi_error)?
    {
        crate::legi::LawRoot::Body(summary) => tool_ok(&present_law_text(&summary, web)),
        crate::legi::LawRoot::Toc(summary, toc) => tool_ok(
            &crate::mcp_presenters::present_law_text_root(&summary, &toc, web),
        ),
    }
}

/// `/texte/{code}/section/{cid}`: the division the table of contents addresses.
/// Without it, reading a chapter costs one call per article.
async fn tool_get_legal_section(
    state: &AppState,
    code: &str,
    cid: &str,
    date: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let slug = resolve_law_code(state, code).await?;
    let at = date.map(|d| parse_mcp_date("date", d)).transpose()?;
    let section = crate::legi::law_section(state, &slug, cid, at)
        .await
        .map_err(map_legi_error)?;
    tool_ok(&crate::mcp_presenters::present_law_section(
        &section,
        &state.settings.web_base_url,
    ))
}

async fn resolve_source_uid(state: &AppState, uid: &str) -> Result<(String, String), RpcError> {
    let mut targets = crate::legi::article_refs_by_source_uid(state, uid)
        .await
        .map_err(map_legi_error)?;
    match targets.len() {
        1 => Ok(targets.remove(0)),
        0 => Err(
            RpcError::not_found(format!("unknown Légifrance id '{uid}'")).with_hint(
                "Ids from another source than our LEGI import are unknown here — \
             search the article with search_legal_texts, its hits carry the url.",
            ),
        ),
        n => Err(RpcError::invalid_params(format!(
            "Légifrance id '{uid}' designates {n} distinct articles"
        ))
        .with_hint("Pass the url of the one you mean (/texte/{code}/{article}).")),
    }
}

/// Three accepted forms: the public url, absolute or relative
/// (`/texte/{code}/{article}`), the legacy `/loi/` prefix, and the bare path
/// (`code-civil/1240`).
///
/// The bare path stays reserved to inputs with no scheme: on an absolute url the
/// prefix is required, otherwise the split would bite on the `//` of `https://`.
///
/// Second member `None`: the code is there, the article is missing.
fn split_article_url(url: &str) -> Option<(&str, Option<&str>)> {
    let path = url
        .split("/texte/")
        .nth(1)
        .or_else(|| url.split("/loi/").nth(1))
        .or_else(|| (!url.contains("://")).then(|| url.trim_start_matches('/')))?
        .trim_end_matches('/');
    let mut segs = path.splitn(2, '/');
    let code = segs.next().filter(|s| !s.is_empty())?;
    Some((code, segs.next().filter(|s| !s.is_empty())))
}

/// Target designated by the `get_legal_text` arguments, whichever door was used.
#[derive(Debug, PartialEq, Eq)]
enum LawTarget {
    Article {
        code: String,
        num: String,
    },
    /// Text named without an article, served whole when it carries none.
    Root {
        code: String,
    },
    Section {
        code: String,
        cid: String,
    },
    /// Légifrance article id, resolved against the database.
    SourceUid(String),
}

/// The schema announces only the first of each line; the others are what callers
/// compose on their own, collected from the MCP errors of 01 to 14/08/2026.
const CODE_KEYS: [&str; 3] = ["code", "code_name", "codeName"];
const ARTICLE_KEYS: [&str; 4] = ["article", "article_number", "articleNumber", "num"];
const ID_KEYS: [&str; 3] = ["textId", "text_id", "legiarti"];

fn arg_str<'a>(args: &'a Value, keys: &[&'static str]) -> Option<(&'static str, &'a str)> {
    keys.iter().find_map(|k| {
        args.get(*k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|v| (*k, v))
    })
}

/// A Légifrance article id is recognised by its shape, not by the key carrying
/// it: callers put it in `textId` as readily as in `url`.
fn is_legiarti(value: &str) -> bool {
    value
        .strip_prefix("LEGIARTI")
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

fn law_target(args: &Value) -> Result<LawTarget, RpcError> {
    let url = arg_str(args, &["url"]);
    let id = arg_str(args, &ID_KEYS);
    if let Some((key, value)) = [url, id]
        .into_iter()
        .flatten()
        .find(|(_, v)| is_legiarti(v))
    {
        relaxed_key(key, "textId");
        return Ok(LawTarget::SourceUid(value.to_string()));
    }
    if let Some((_, url)) = url {
        return match split_article_url(url) {
            Some((code, Some(reste))) => Ok(match reste.strip_prefix("section/") {
                Some(cid) => LawTarget::Section {
                    code: code.to_string(),
                    cid: cid.to_string(),
                },
                None => LawTarget::Article {
                    code: code.to_string(),
                    num: reste.split('/').next().unwrap_or(reste).to_string(),
                },
            }),
            Some((code, None)) => Ok(LawTarget::Root {
                code: code.to_string(),
            }),
            None => Err(RpcError::invalid_params(format!(
                "not an article url (expected …/texte/{{code}}/{{article}}): {url}"
            ))),
        };
    }
    let Some((code_key, code)) = arg_str(args, &CODE_KEYS) else {
        return Err(
            RpcError::invalid_params("missing tool argument: url").with_hint(
                "Address the article by url — /texte/{code-slug}/{article-key}, e.g. \
             {\"url\": \"/texte/code-civil/1240\"} — or by the pair {code, article}. \
             A LEGIARTI id works too; LEGITEXT and CID do not.",
            ),
        );
    };
    relaxed_key(code_key, "code");
    match arg_str(args, &ARTICLE_KEYS) {
        Some((article_key, num)) => {
            relaxed_key(article_key, "article");
            Ok(LawTarget::Article {
                code: code.to_string(),
                num: num.to_string(),
            })
        }
        None => Ok(LawTarget::Root {
            code: code.to_string(),
        }),
    }
}

fn relaxed_key(used: &'static str, canonical: &'static str) {
    if used != canonical {
        tracing::warn!(
            librejustice.mcp.relaxed_field = canonical,
            librejustice.mcp.relaxed_value = used,
            "mcp: argument de get_legal_text rattrapé sur son orthographe de schéma"
        );
    }
}

fn query_too_long() -> String {
    format!(
        "query: String should have at most {} characters",
        lj_dtos::QUERY_MAX_CHARS
    )
}

fn required_str(args: &Value, key: &str) -> Result<String, RpcError> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        Some(_) => Err(RpcError::invalid_params(format!(
            "empty tool argument: {key}"
        ))),
        None => Err(RpcError::invalid_params(format!(
            "missing tool argument: {key}"
        ))),
    }
}

/// A missing article and a text whose body was never loaded answered the same
/// bare `not_found`, which sends the caller correcting a key that is right.
/// 944,382 fiches of the 1,335,851 of the catalogue carry neither body nor
/// article, so the second case is the common one.
async fn article_not_found(state: &AppState, code: &str) -> RpcError {
    match crate::legi::code_summary(state, code).await {
        Ok(s) if s.article_count == 0 && s.body.is_none() => RpcError::not_found(format!(
            "\"{}\" is in the catalogue with no text: we hold its identity, not its articles",
            s.titre
        ))
        .with_hint(
            "Nothing to fix in the call, the article is nowhere to be read here. \
             Its official publisher carries the text.",
        ),
        _ => map_legi_error(ApiError::NotFound),
    }
}

fn map_legi_error(e: ApiError) -> RpcError {
    match e {
        ApiError::TooManyRequests => RpcError::rate_limited(),
        ApiError::NotFound => RpcError::not_found("law article or code not found").with_hint(
            "Find the article with search_legal_texts, or compose the url as \
             /texte/{code-slug}/{article-key} (lowercase key: \"l761-1\" for L. 761-1).",
        ),
        ApiError::NoVersionAtDate(msg) => RpcError::not_found(msg).with_hint(
            "The article exists: this is a date problem, not a spelling one. Ask again \
             within the covered span, or omit date to get the version in force.",
        ),
        other => RpcError::tool_error(other.to_string()),
    }
}

/// Largest window served, the one the three search schemas announce.
const SEARCH_LIMIT_MAX: i64 = 20;

/// A wrong type stays an error, an out-of-range window does not: refused, it was
/// asked again unchanged, 21 searches lost on 27 and 28/08/2026, 15 out of 36 of
/// them on a single account.
fn clamp_search_limit(args: &Value) -> Result<i64, RpcError> {
    let asked: i64 = deser_field(args, "limit")?.unwrap_or(10);
    Ok(asked.clamp(1, SEARCH_LIMIT_MAX))
}

fn build_search_request(args: &Value) -> Result<lj_dtos::SearchRequest, RpcError> {
    // Server-side parity with the `additionalProperties: false` the schema
    // announces: an unknown filter silently ignored would skew the search with no
    // signal.
    const FIELDS: [&str; 18] = [
        "query",
        "jurisdiction_type",
        "solution",
        "procedure",
        "office",
        "legal_domain",
        "jurisdiction_code",
        // Outside the published schema, still accepted: rewritten into
        // `legal_domain` by [`rewrite_legacy_matiere`].
        "matiere",
        "seat",
        "legal_instrument",
        "legal_article",
        "publication",
        "date_from",
        "date_to",
        "mode",
        "sort",
        "limit",
        "ai_rerank",
    ];
    if let Some(obj) = args.as_object() {
        if let Some(unknown) = obj.keys().find(|k| !FIELDS.contains(&k.as_str())) {
            return Err(RpcError::invalid_params(format!(
                "unknown field '{unknown}'; valid fields: {}",
                FIELDS.join(", ")
            )));
        }
    }
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("missing tool argument: query"))?
        .to_string();
    // Parity with the announced schema (`query.maxLength`) and the HTTP path.
    if query.chars().count() > lj_dtos::QUERY_MAX_CHARS {
        return Err(RpcError::invalid_params(query_too_long()));
    }

    // Format and range validated here, otherwise a malformed date reaches the SQL.
    let date_from: Option<String> = deser_field(args, "date_from")?;
    let date_to: Option<String> = deser_field(args, "date_to")?;
    validate_mcp_date("date_from", &date_from)?;
    validate_mcp_date("date_to", &date_to)?;
    let limit = clamp_search_limit(args)?;

    Ok(lj_dtos::SearchRequest {
        query,
        jurisdiction_type: deser_list(args, "jurisdiction_type")?,
        solution: deser_list(args, "solution")?,
        procedure: deser_list(args, "procedure")?,
        office: deser_list(args, "office")?,
        legal_domain: deser_list(args, "legal_domain")?,
        jurisdiction_code: deser_list(args, "jurisdiction_code")?,
        seat: deser_list(args, "seat")?,
        legal_instrument: deser_list(args, "legal_instrument")?,
        legal_article: deser_list(args, "legal_article")?,
        publication: deser_list(args, "publication")?,
        date_from,
        date_to,
        mode: deser_field(args, "mode")?.unwrap_or(lj_dtos::SearchMode::Auto),
        sort: deser_field(args, "sort")?.unwrap_or(lj_dtos::SortOrder::Relevance),
        limit: limit as u32,
        offset: 0,
        ai_mode: deser_field(args, "ai_rerank")?.unwrap_or(true),
    })
}

fn validate_mcp_date(field: &str, raw: &Option<String>) -> Result<(), RpcError> {
    match raw {
        Some(s) => parse_mcp_date(field, s).map(|_| ()),
        None => Ok(()),
    }
}

fn parse_mcp_date(field: &str, s: &str) -> Result<chrono::NaiveDate, RpcError> {
    crate::search::parse_search_date(s).map_err(|e| match e {
        crate::search::DateError::Parse(m) => {
            RpcError::invalid_params(format!("invalid {field}: {m}")).with_hint(format!(
                "{field} is a calendar date in strict YYYY-MM-DD form, \
                 e.g. \"2024-03-15\" — zero-padded, no time part."
            ))
        }
        crate::search::DateError::TooEarly => {
            RpcError::invalid_params(format!("{field} must be >= {}", crate::search::DATE_GE))
        }
        crate::search::DateError::TooLate => {
            RpcError::invalid_params(format!("{field} must be <= {}", crate::search::DATE_LE))
        }
    })
}

/// One hint for every list field, whatever the cause of the rejection. It says
/// two things no value message says: that several values combine at once, where
/// "expected one of" reads as "only one", and the shape of the call. `examples`
/// are real values of the field, that the error site has just computed, so
/// always valid.
fn list_hint<S: AsRef<str>>(key: &str, examples: &[S]) -> String {
    let example = if examples.is_empty() {
        "\"VALUE\", \"OTHER\"".to_string()
    } else {
        examples
            .iter()
            .take(2)
            .map(|v| format!("\"{}\"", v.as_ref()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "{key} takes a list, not a single value — e.g. \
         {{\"{key}\": [{example}]}}. Any number of valid entries combine; \
         one entry per value, never a comma-joined string."
    )
}

/// Valid values as enumerated by a serde message ("unknown variant
/// `judiciaire`, expected one of `TA`, `CAA`, …"), the source of examples of
/// [`list_hint`] for the typed enums of the DTO.
fn serde_variants(err: &str) -> Vec<&str> {
    err.split_once("expected one of ")
        .map(|(_, list)| list.split('`').skip(1).step_by(2).collect())
        .unwrap_or_default()
}

/// A scalar where the schema announces an array is wrapped into a singleton,
/// which separates two cases: good value, the singleton is taken and a WARN is
/// emitted; bad value, the error returned is the one after wrapping, which
/// enumerates the valid variants.
fn deser_list<T: serde::de::DeserializeOwned>(
    args: &Value,
    key: &str,
) -> Result<Option<Vec<T>>, RpcError> {
    let Some(value) = args.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let coerced = (!value.is_array()).then(|| json!([value]));
    match serde_json::from_value(coerced.clone().unwrap_or_else(|| value.clone())) {
        Ok(parsed) => {
            if coerced.is_some() {
                tracing::warn!(
                    librejustice.mcp.coerced_arg = key,
                    librejustice.mcp.coerced_value = %value,
                    "mcp: scalaire accepté comme tableau singleton"
                );
            }
            Ok(Some(parsed))
        }
        Err(e) => {
            let msg = e.to_string();
            Err(
                RpcError::invalid_params(format!("invalid argument {key}: {msg}"))
                    .with_hint(list_hint(key, &serde_variants(&msg))),
            )
        }
    }
}

/// The message names the field: the bare serde message names none.
fn deser_field<T: serde::de::DeserializeOwned>(
    args: &Value,
    key: &str,
) -> Result<Option<T>, RpcError> {
    let Some(value) = args.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|e| RpcError::invalid_params(format!("invalid argument {key}: {e}")))
}

// ── Tools: the annonces fund, not published ──────────────────────────────────

/// The five published tools carry one definition, in `lj_dtos::mcp_tools`.
fn tool_definitions() -> Vec<Value> {
    let mut tools = lj_dtos::mcp_tools::tool_definitions();
    if PUBLISH_SEARCH_ANNONCES {
        tools.push(json!({
            "name": "search_annonces",
            "title": "Search Legal Notices",
            "description": SEARCH_ANNONCES_DESC,
            "inputSchema": search_annonces_schema(),
            "outputSchema": search_annonces_output_schema(),
            "annotations": read_only_annotations(),
        }));
    }
    tools
}

/// `search_annonces` is neither declared nor reachable. The constant holds both
/// doors, `tools/list` and the `tools/call` routing, so that a client guessing
/// the name does not reach it; the handler and the schemas stay in place.
///
/// Republishing also means fixing the annotations: the live BODACC and JOAFE
/// proxies take the tool outside the indexed corpus, so it cannot inherit the
/// `openWorldHint: false` of the five others.
const PUBLISH_SEARCH_ANNONCES: bool = false;

/// Suffixes of the `nature_marche:*` uids.
const NATURES_MARCHE: [&str; 4] = ["TRAVAUX", "SERVICES", "FOURNITURES", "AUTRES"];

fn search_annonces_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [],
        "properties": {
            "query": nullable(
                json!({"type": "string", "maxLength": lj_dtos::QUERY_MAX_CHARS}),
                json!({"description": "Word filter over the tender subject and the buyer name (substring, case-insensitive) — not a semantic search. Omit it and rely on the filters when you want a whole department or a whole CPV division."}),
            ),
            "fund": {
                "type": "array",
                "items": enum_values(&FONDS),
                "description": "Which body of public notices to search. \"marche\": open public tenders, aggregated from the BOAMP and 8 buyer platforms, with a bid deadline. \"bodacc\": BODACC company notices (insolvency proceedings, incorporations, sales, filings) served live from the official API — no deadline. \"joafe\": Journal officiel des associations notices (creations, modifications, dissolutions), live too — no deadline. Omit to search all three, returned in separate groups.",
            },
            "departement": {
                "type": "array",
                "items": {"type": "string"},
                "description": "French département codes: \"75\", \"2A\", \"973\". Several codes are OR'd. Reuse the keys of facets.departement.",
            },
            "nature": {
                "type": "array",
                "items": enum_values(&NATURES_MARCHE),
                "description": "Kind of contract: TRAVAUX (works), SERVICES, FOURNITURES (supplies), AUTRES.",
            },
            "cpv": {
                "type": "array",
                "items": {"type": "string"},
                "description": "CPV divisions, two digits (\"45\" construction works, \"72\" IT services, \"85\" health). Reuse the keys of facets.cpv.",
            },
            "closing_within_days": nullable(
                json!({"type": "integer", "minimum": 1, "maximum": 365}),
                json!({"description": "Keep only tenders whose bid deadline falls within N days. Tenders with no known deadline are excluded by this filter."}),
            ),
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (default 10), ordered by bid deadline, soonest first.",
            },
        },
    })
}

fn search_annonces_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["total", "hits", "facets"],
        "properties": {
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "fund", "actor", "actorRole", "url", "platformUrl", "published", "channels"],
                    "properties": {
                        "title": {"type": "string"},
                        "fund": {"type": "string", "description": "Body this notice comes from: \"marche\" or \"bodacc\"."},
                        "actor": {"type": "string", "description": "Who the notice is about: the buyer of a tender, the company of a BODACC notice."},
                        "actorRole": {"type": "string", "description": "What that body calls this actor (\"Acheteur\", \"Entreprise\")."},
                        "url": {"type": "string", "description": "Page to hand to the user: the LibreJustice tender page (/marche/…), or the official notice page for bodies served live."},
                        "platformUrl": {"type": "string", "description": "Buyer's platform page: where the dossier is downloaded and the bid is filed. Not openable by an MCP tool."},
                        "departement": nullable(json!({"type": "string"}), json!({})),
                        "nature": nullable(json!({"type": "string"}), json!({})),
                        "cpv": nullable(json!({"type": "string"}), json!({})),
                        "deadline": nullable(json!({"type": "string"}), json!({"description": "Bid deadline, RFC 3339. Absent = no source publishes one."})),
                        "published": {"type": "string"},
                        "reference": nullable(json!({"type": "string"}), json!({})),
                        "channels": {"type": "array", "items": {"type": "string"}},
                    },
                },
            },
            "facets": {
                "type": "object",
                "description": "Per axis key, a map of filter value to notice count under the current filters. The axes present are those the searched bodies can execute — `fond` always, the body's own axes once a single `fund` is selected. Reuse keys verbatim as filter values; each axis is capped to its top counts.",
                "additionalProperties": true,
            },
        },
    })
}

const SEARCH_ANNONCES_DESC: &str =
    "Search French legal notices — the events in progress that publicity law \
requires to be published. Three bodies, picked with `fund`, returned in \
separate groups when several are searched: `marche`, public procurement \
notices open for bids (soonest deadline first), covering the BOAMP AND the \
buyer platforms that carry the below-threshold ones (MAPA) the BOAMP never \
publishes — that half exists nowhere else as a searchable list; `bodacc`, \
company notices (insolvency proceedings, incorporations, sales, account \
filings) served live from the official register, newest first; and `joafe`, \
association notices from the Journal officiel (creations, modifications, \
dissolutions), served live too, newest first. `query` is a plain word filter, \
not a semantic search, so prefer the filters. The `facets` block gives the axes \
the searched bodies can actually execute, with their counts: reuse those keys \
verbatim to refine — filtering on one `fund` unlocks that body's own axes. A \
tender hit gives `platformUrl` (where the dossier is downloaded and the bid is \
filed — hand it to the user, no tool opens it) and `deadline`. Closed tenders \
are not returned, and awards are not in the corpus yet: this answers « what is \
open », not « who won ».";

#[cfg(test)]
mod tests {
    use super::*;

    /// `matiere` is out of the schema and still served: translated to a subject,
    /// announced, and refused with a fix when the value names no subject.
    #[test]
    fn le_filtre_matiere_retire_se_traduit_en_domaine() {
        let mut req = build_search_request(&json!({"query": "OQTF"})).unwrap();
        let notice = rewrite_legacy_matiere(&json!({"matiere": ["ETRANGERS"]}), &mut req)
            .unwrap()
            .expect("la réécriture s'annonce");
        assert_eq!(
            req.legal_domain,
            Some(vec![Domain::PublicDroitEtrangersNationalite])
        );
        assert!(notice.contains("matiere"));
        assert!(notice.contains("PUBLIC_DROIT_ETRANGERS_NATIONALITE"));

        let mut req = build_search_request(&json!({"query": "x"})).unwrap();
        rewrite_legacy_matiere(&json!({"matiere": ["ETRANGERS", "NATIONALITE"]}), &mut req)
            .unwrap();
        assert_eq!(
            req.legal_domain,
            Some(vec![Domain::PublicDroitEtrangersNationalite])
        );
    }

    #[test]
    fn un_legal_domain_pose_garde_la_main_sur_la_matiere() {
        let args = json!({"query": "x", "legal_domain": ["CIVIL_DROIT_LOCATIF"], "matiere": ["ETRANGERS"]});
        let mut req = build_search_request(&args).unwrap();
        let notice = rewrite_legacy_matiere(&args, &mut req).unwrap().unwrap();
        assert_eq!(req.legal_domain, Some(vec![Domain::CivilDroitLocatif]));
        assert!(notice.contains("dropped"));
    }

    #[test]
    fn une_matiere_qui_ne_nommait_pas_un_sujet_rend_un_correctif() {
        let mut req = build_search_request(&json!({"query": "x"})).unwrap();
        for value in ["URGENCES", "CONSEIL", "PROXIMITE", "PAS_UNE_VALEUR"] {
            let err = rewrite_legacy_matiere(&json!({"matiere": [value]}), &mut req).unwrap_err();
            assert_eq!(err.code, -32602);
            assert!(err.hint.as_deref().unwrap().contains("legal_domain"));
        }
        assert_eq!(req.legal_domain, None);
    }

    #[test]
    fn la_table_de_traduction_couvre_le_vocabulaire_retire() {
        assert_eq!(MATIERE_VERS_DOMAINE.len(), 24);
        for (_, cible) in MATIERE_VERS_DOMAINE {
            if let Some(d) = cible {
                assert!(Domain::ALL.contains(&d));
            }
        }
    }

    #[test]
    fn tool_list_exposes_all_tools() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "search_decisions",
                "get_decision",
                "list_my_activity",
                "get_legal_text",
                "search_legal_texts",
            ]
        );
    }

    #[test]
    fn build_request_maps_referential_filters_and_ai_rerank() {
        let args = json!({
            "query": "bail commercial",
            "publication": ["PUBLIE_BULLETIN"],
            "jurisdiction_type": ["CC", "CA"],
            "solution": ["REJET", "CASSATION"],
            "legal_domain": ["CIVIL_DROIT_LOCATIF"],
            "jurisdiction_code": ["ca_paris"],
            "ai_rerank": false,
            "limit": 5,
        });
        let req = build_search_request(&args).unwrap();
        assert_eq!(req.query, "bail commercial");
        assert_eq!(req.publication, Some(vec!["PUBLIE_BULLETIN".to_string()]));
        assert_eq!(
            req.solution,
            Some(vec![lj_dtos::Solution::Rejet, lj_dtos::Solution::Cassation])
        );
        assert_eq!(
            req.legal_domain,
            Some(vec![lj_dtos::Domain::CivilDroitLocatif])
        );
        assert_eq!(req.jurisdiction_code, Some(vec!["ca_paris".to_string()]));
        assert_eq!(
            req.jurisdiction_type,
            Some(vec![
                lj_dtos::JurisdictionType::Cc,
                lj_dtos::JurisdictionType::Ca
            ])
        );
        assert!(!req.ai_mode);
        assert_eq!(req.limit, 5);
        assert_eq!(req.mode, lj_dtos::SearchMode::Auto);
        assert_eq!(req.sort, lj_dtos::SortOrder::Relevance);
        assert_eq!(req.offset, 0);
    }

    #[test]
    fn build_request_defaults_ai_rerank_true_and_limit_10() {
        let req = build_search_request(&json!({"query": "x"})).unwrap();
        assert!(req.ai_mode);
        assert_eq!(req.limit, 10);
    }

    #[test]
    fn build_request_coerces_scalar_to_singleton_when_value_is_valid() {
        let req = build_search_request(&json!({
            "query": "bail",
            "legal_domain": "CIVIL_DROIT_LOCATIF",
            "jurisdiction_type": "CA",
        }))
        .unwrap();
        assert_eq!(
            req.legal_domain,
            Some(vec![lj_dtos::Domain::CivilDroitLocatif])
        );
        assert_eq!(
            req.jurisdiction_type,
            Some(vec![lj_dtos::JurisdictionType::Ca])
        );
    }

    #[test]
    fn out_of_enum_value_reports_variants_and_list_shape() {
        for bad in [json!("judiciaire"), json!(["judiciaire"])] {
            let err = build_search_request(&json!({"query": "bail", "jurisdiction_type": bad}))
                .unwrap_err();
            assert!(err.message.contains("jurisdiction_type"));
            assert!(err.message.contains("unknown variant"));
            assert!(err.message.contains("CA"), "{}", err.message);
            let hint = err.hint.expect("hint on list field");
            assert!(hint.contains("takes a list, not a single value"), "{hint}");
            assert!(
                hint.contains("{\"jurisdiction_type\": [\"TA\", \"CAA\"]}"),
                "{hint}"
            );
            assert!(
                hint.contains("Any number of valid entries combine"),
                "{hint}"
            );
        }

        let err = build_search_request(&json!({
            "query": "bail",
            "solution": "ANNULATION,REJET",
        }))
        .unwrap_err();
        assert!(err.message.contains("unknown variant"));
        assert!(err.message.contains("REJET"), "{}", err.message);
        assert!(err.hint.is_some_and(|h| h.contains("comma-joined")));
    }

    #[test]
    fn build_request_rejects_empty_query() {
        assert!(build_search_request(&json!({"query": ""})).is_err());
        assert!(build_search_request(&json!({})).is_err());
    }

    #[test]
    fn out_of_range_limit_is_clamped() {
        for (asked, served) in [(100, 20), (50, 20), (30, 20), (0, 1), (-5, 1)] {
            let limit = clamp_search_limit(&json!({"limit": asked})).unwrap();
            assert_eq!(limit, served, "limit={asked}");
        }
        for asked in [1, 10, 20] {
            assert_eq!(clamp_search_limit(&json!({"limit": asked})).unwrap(), asked);
        }
        assert_eq!(clamp_search_limit(&json!({})).unwrap(), 10);
        assert!(clamp_search_limit(&json!({"limit": "20"})).is_err());
    }

    #[test]
    fn unknown_jurisdiction_code_suggests_by_source_label_and_typo() {
        let refs = crate::referential::Referential::new(
            vec![],
            vec![lj_store::repository::JurisdictionRow {
                code: "tj_paris".into(),
                source_code: "tj75056".into(),
                jurisdiction_type: "TJ".into(),
                city: Some("Paris".into()),
                label: "Tribunal judiciaire de Paris".into(),
            }],
            Vec::new(),
        );
        let err = unknown_jurisdiction_code("tj75056", &refs);
        assert!(err.message.contains("tj_paris"));
        assert!(err.message.contains("Tribunal judiciaire de Paris"));
        let err = unknown_jurisdiction_code("tribunal_paris", &refs);
        assert!(err.message.contains("tj_paris"));
        let err = unknown_jurisdiction_code("tj_parsi", &refs);
        assert!(err.message.contains("tj_paris"));
    }

    #[test]
    fn relax_jurisdiction_codes_ne_reecrit_que_ce_qui_devient_connu() {
        let refs = crate::referential::Referential::new(
            vec![],
            vec![lj_store::repository::JurisdictionRow {
                code: "ca_aix_en_provence".into(),
                source_code: "ca13".into(),
                jurisdiction_type: "CA".into(),
                city: Some("Aix-en-Provence".into()),
                label: "Cour d'appel d'Aix-en-Provence".into(),
            }],
            Vec::new(),
        );
        let mut codes = [
            "ca_aix-en-provence".to_string(),
            "ca_aix_en_provence".to_string(),
            "ca-nulle-part".to_string(),
        ];
        relax_jurisdiction_codes(&mut codes, &refs);
        assert_eq!(codes[0], "ca_aix_en_provence");
        assert_eq!(codes[1], "ca_aix_en_provence");
        assert_eq!(codes[2], "ca-nulle-part");
    }

    /// The four shapes collected from the `missing tool argument: url` errors,
    /// plus the text root and the Légifrance id.
    #[test]
    fn law_target_lit_les_portes_observees() {
        let article = |code: &str, num: &str| LawTarget::Article {
            code: code.into(),
            num: num.into(),
        };
        assert_eq!(
            law_target(&json!({"url": "/texte/code-civil/1240"})).unwrap(),
            article("code-civil", "1240")
        );
        assert_eq!(
            law_target(
                &json!({"code_name": "code-de-procedure-penale", "article_number": "706-88"})
            )
            .unwrap(),
            article("code-de-procedure-penale", "706-88")
        );
        assert_eq!(
            law_target(&json!({"code": "code-civil", "article": "1240"})).unwrap(),
            article("code-civil", "1240")
        );
        assert_eq!(
            law_target(&json!({"textId": "LEGIARTI000048784916"})).unwrap(),
            LawTarget::SourceUid("LEGIARTI000048784916".into())
        );
        assert_eq!(
            law_target(&json!({"url": "LEGIARTI000048784916"})).unwrap(),
            LawTarget::SourceUid("LEGIARTI000048784916".into())
        );
        assert_eq!(
            law_target(&json!({"code": "circulaire-x", "query": "séjour"})).unwrap(),
            LawTarget::Root {
                code: "circulaire-x".into()
            }
        );
        assert_eq!(
            law_target(&json!({"url": "https://librejustice.fr/texte/code-du-travail"})).unwrap(),
            LawTarget::Root {
                code: "code-du-travail".into()
            }
        );
        assert_eq!(
            law_target(&json!({
                "url": "https://librejustice.fr/texte/code-du-travail/section/LEGISCTA000006195791"
            }))
            .unwrap(),
            LawTarget::Section {
                code: "code-du-travail".into(),
                cid: "LEGISCTA000006195791".into()
            }
        );
        assert_eq!(
            law_target(&json!({"url": "/texte/code-civil/1240/2016-09-05"})).unwrap(),
            article("code-civil", "1240")
        );
        let err = law_target(&json!({"date": "2026-08-08"})).unwrap_err();
        assert!(err.message.contains("missing tool argument: url"));
        assert!(err.hint.is_some_and(|h| h.contains("{code, article}")));
        assert!(!is_legiarti("LEGITEXT000006070721"));
        assert!(!is_legiarti("LEGIARTI"));
        assert!(is_legiarti("LEGIARTI000006419282"));
    }

    #[test]
    fn build_request_rejects_unknown_fields() {
        let err = build_search_request(&json!({
            "query": "bail",
            "jurisdiction_name": ["ca_paris"],
        }))
        .unwrap_err();
        assert!(err.message.contains("jurisdiction_name"));
    }

    #[test]
    fn tool_error_content_carries_error_contract() {
        let err = RpcError::not_found("no decision at url=x")
            .with_hint("Take the url from a search_decisions hit.");
        let content = tool_error_content(&err);
        assert_eq!(content["isError"], json!(true));
        let body: Value =
            serde_json::from_str(content["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["error"], json!("no decision at url=x"));
        assert_eq!(body["category"], json!("not_found"));
        assert_eq!(body["retryable"], json!(false));
        assert!(body["hint"].as_str().unwrap().contains("search_decisions"));

        let internal = tool_error_content(&RpcError::tool_error("pool: timeout"));
        let body: Value =
            serde_json::from_str(internal["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["category"], json!("internal"));
        assert_eq!(body["retryable"], json!(true));
        assert!(body.get("hint").is_none());

        let auth = tool_error_content(&RpcError::auth("authentication required"));
        let body: Value =
            serde_json::from_str(auth["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["category"], json!("auth"));
    }

    /// A root combinator in an `inputSchema` makes the client drop the tool, with
    /// no error and no trace: measured on Claude Code 2.1.233, 19/08/2026.
    #[test]
    fn input_schemas_carry_no_root_combinator() {
        for tool in tool_definitions() {
            for combinateur in ["anyOf", "oneOf", "allOf", "not"] {
                assert!(
                    tool["inputSchema"].get(combinateur).is_none(),
                    "{}: {combinateur} à la racine de l'inputSchema",
                    tool["name"]
                );
            }
        }
    }

    /// The offending field is named, and a scalar field does not inherit the
    /// "takes a list" hint.
    #[test]
    fn scalar_field_error_names_the_field_without_the_array_hint() {
        let err = build_search_request(&json!({"query": "x", "mode": "lexicale"})).unwrap_err();
        assert!(err.message.contains("mode"), "{}", err.message);
        assert!(err.hint.is_none(), "{:?}", err.hint);

        assert!(build_search_request(&json!({"query": "x", "mode": "lexical"})).is_ok());
        assert!(build_search_request(&json!({
            "query": "x",
            "solution": ["ANNULATION", "REJET"],
        }))
        .is_ok());
    }

    /// Same for dates: the expected shape when the format failed, nothing more
    /// when the value is out of bounds, the message already carrying the bound.
    #[test]
    fn date_error_gives_the_expected_shape_only_when_the_format_failed() {
        let err = parse_mcp_date("date", "15/03/2024").unwrap_err();
        assert!(err.message.contains("invalid date"), "{}", err.message);
        let hint = err.hint.expect("hint on malformed date");
        assert!(hint.contains("YYYY-MM-DD"), "{hint}");

        assert!(parse_mcp_date("date", "2024-1-1")
            .unwrap_err()
            .hint
            .is_some());

        let too_early = parse_mcp_date("date_from", "1600-01-01").unwrap_err();
        assert!(
            too_early.message.contains("1678-01-01"),
            "{}",
            too_early.message
        );
        assert!(too_early.hint.is_none());

        assert!(parse_mcp_date("date", "2024-03-15").is_ok());
    }

    #[test]
    fn article_url_accepts_absolute_relative_legacy_and_bare_path() {
        use super::split_article_url as split;
        let civ = Some(("code-civil", Some("1240")));
        assert_eq!(split("https://librejustice.fr/texte/code-civil/1240"), civ);
        assert_eq!(split("/texte/code-civil/1240"), civ);
        assert_eq!(split("/loi/code-civil/1240"), civ);
        assert_eq!(split("code-civil/1240"), civ);
        assert_eq!(split("code-civil/1240/"), civ);

        assert_eq!(
            split("/texte/code-du-travail"),
            Some(("code-du-travail", None))
        );
        assert_eq!(split("code-du-travail"), Some(("code-du-travail", None)));

        assert_eq!(split("https://librejustice.fr/decision/abc"), None);
        assert_eq!(split("/texte/"), None);
        assert_eq!(split(""), None);
    }

    #[test]
    fn initialize_carries_protocol_and_instructions() {
        let res = initialize_result();
        assert_eq!(res["protocolVersion"], json!(PROTOCOL_VERSION));
        assert_eq!(res["serverInfo"]["name"], json!(SERVER_NAME));
        assert!(res["instructions"]
            .as_str()
            .unwrap()
            .contains("French and European case law"));
    }
}
