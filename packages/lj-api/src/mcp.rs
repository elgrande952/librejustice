//! Endpoint MCP.
//!
//! Serveur MCP distant (Streamable HTTP, JSON response) montable dans l'app axum
//! sous `/mcp`. Outils exposés : `search_decisions`, `get_decision`,
//! `list_my_activity`.
//!
//! rmcp 0.1.5 n'embarque pas de transport Streamable HTTP côté serveur : la couche
//! JSON-RPC est portée à la main au-dessus d'axum. Les corps et sorties d'outils
//! sont assurés par [`crate::mcp_presenters`], la logique métier par
//! [`crate::search`] / [`crate::decisions`].

use axum::{
    extract::{Request, State},
    http::header::{AUTHORIZATION, WWW_AUTHENTICATE},
    http::{HeaderMap, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use serde_json::{json, Value};
use tracing::Instrument;

use crate::annonces::FONDS;
use crate::bookmarks::fetch_bookmarks;
use crate::decision_views::{fetch_views, record_decision_view};
use crate::error::ApiError;
use crate::mcp_presenters::{
    present_annonce_search, present_bookmarks, present_decision_detail, present_law_article,
    present_law_search, present_reading_history, present_saved_searches, present_search_response,
    McpActivityResponse,
};
use crate::search_history::{fetch_history, record_search};
use crate::state::AppState;

/// Provenance MCP d'une recherche : canal `mcp`, toujours une intention
/// d'utilisateur.
fn mcp_caller(authenticated: bool) -> crate::usage::Caller {
    crate::usage::Caller {
        source: lj_dtos::ActivitySource::Mcp,
        authenticated,
        context: lj_dtos::SearchContext::User,
        session_degraded: false,
    }
}

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "librejustice";
/// `serverInfo.version` : version propre du serveur, alignée sur celle du crate.
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Instructions serveur.
const SERVER_INSTRUCTIONS: &str =
    "Search French and European case law: all French courts plus, among \
others, the CNDA, the Conseil constitutionnel, CNIL sanctions, the \
CEDH and the CJUE. The engine understands meaning, not just keywords: \
phrase the query as a natural question or a list of descriptive terms; \
synonyms and reformulations are handled. Typical workflow: \
search_decisions for a shortlist, then get_decision to read candidates \
in full. Previews only orient: never judge relevance, quote, or state \
what a decision holds from one, as the matched passage may be a \
party's argument, not the court's ruling. Iterate by refining the \
query or filters. Open any `url` with the matching tool: /decision/ \
links with get_decision, /texte/ links with get_legal_text. When \
mentioning a decision, hyperlink its `title` (or a conventional \
citation) to its `url`.";

/// Construit le service MCP adossé à l'état app. Le montage effectif passe par
/// [`mcp_router`] ; cette fonction vérifie que l'état est exploitable.
pub fn build_mcp_service(_state: AppState) -> anyhow::Result<()> {
    Ok(())
}

/// Router axum exposant l'endpoint MCP (JSON-RPC Streamable HTTP), à `.merge()`
/// dans [`crate::routes::create_app`] quand `enable_mcp`.
///
/// Servi sur `/mcp/` et `/mcp` : les clients MCP POSTent l'URL telle que saisie et
/// ne suivent pas de redirection sur POST.
pub fn mcp_router(state: AppState) -> Router {
    Router::new()
        .route("/mcp/", post(handle_rpc))
        .route("/mcp", post(handle_rpc))
        .layer(from_fn_with_state(state.clone(), mcp_auth))
        .with_state(state)
}

// ── Auth MCP ─────────────────────────────────────────────────────────────────

/// Utilisateur MCP résolu, porté dans les extensions de requête. `None` quand
/// l'auth MCP est désactivée (`mcp_require_auth=false`, tests / dev local).
#[derive(Debug, Clone)]
struct McpUser(Option<String>);

/// Réponse 401 avec le challenge `WWW-Authenticate` (RFC 9728 + MCP 2025-03-26)
/// qui déclenche l'auto-discovery OAuth chez les clients.
fn mcp_unauthorized(challenge: &str, error: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, challenge.to_string())],
        Json(json!({ "error": error })),
    )
        .into_response()
}

/// Résout le `user_id` d'un access token MCP via la table `mcp_tokens`. `None` si
/// le token est absent de la table ou expiré.
///
/// La comparaison porte sur l'empreinte SHA-256, clé primaire de la table : le
/// lookup est un accès par index, et la base ne détient plus de quoi se faire
/// passer pour un porteur.
async fn lookup_mcp_user(state: &AppState, token: &str) -> Result<Option<String>, ApiError> {
    let digest = crate::oauth::sha256(token.as_bytes());
    let digest: &[u8] = &digest;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    let row = conn
        .query_opt(
            "SELECT user_id FROM mcp_tokens WHERE access_token_sha256 = $1 AND expires_at > now()",
            &[&digest],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("mcp token lookup: {e}")))?;
    Ok(row.map(|r| r.get::<_, String>(0)))
}

/// Bump `users.last_seen_at` pour l'activité MCP (best-effort). Le user MCP est
/// résolu hors du chemin Supabase, qui bumpe déjà le champ.
async fn bump_last_seen(state: &AppState, user_id: &str) {
    let Ok(conn) = state.pool.get().await else {
        return;
    };
    if let Err(e) = conn
        .execute(
            "UPDATE users SET last_seen_at = now() WHERE sub = $1",
            &[&user_id],
        )
        .await
    {
        tracing::warn!("bump last_seen (mcp) failed: {e}");
    }
}

/// Middleware d'auth MCP : si `mcp_require_auth`, vérifie le Bearer token contre
/// `mcp_tokens` et résout le `user_id` (401 + challenge sinon) ; sinon laisse
/// passer en anonyme. Le `user_id` résolu (ou `None`) est injecté en extension
/// pour les handlers d'outils.
async fn mcp_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let mut user: Option<String> = None;
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
        match lookup_mcp_user(&state, token).await {
            Ok(Some(uid)) => {
                bump_last_seen(&state, &uid).await;
                user = Some(uid);
            }
            // Le motif est aussi dans le corps du message : un filtre de ligne Loki (`|=`)
            // ne voit que lui, jamais les métadonnées structurées.
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
    next.run(req).await
}

// ── Couche JSON-RPC ──────────────────────────────────────────────────────────

/// Catégorie machine-lisible du contrat d'erreur outil : dit au modèle la
/// nature de l'échec, donc la conduite à tenir (corriger l'appel, chercher
/// autrement, se connecter, retenter).
#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    /// Arguments invalides : corriger l'appel, un retry à l'identique échouera.
    Validation,
    /// Absent du corpus — ce qui ne prouve jamais que la ressource n'existe pas.
    NotFound,
    /// Authentification requise.
    Auth,
    /// Panne interne ou transitoire : un retry à l'identique peut réussir.
    Internal,
}

impl ErrorCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::Auth => "auth",
            Self::Internal => "internal",
        }
    }
}

/// Erreur JSON-RPC remontée comme `error` dans l'enveloppe (codes standard MCP)
/// ou, pour un échec d'outil, rendue en contrat d'erreur structuré par
/// [`tool_error_content`].
#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
    category: ErrorCategory,
    /// `true` si un retry à l'identique peut réussir (panne transitoire).
    retryable: bool,
    /// Prochaine action suggérée au modèle (outil à essayer, correction).
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
    /// Erreur applicative outil : code d'erreur interne MCP, retryable (pool,
    /// upstream — transitoire a priori).
    fn tool_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::Internal,
            retryable: true,
            hint: None,
        }
    }
    /// Ressource absente du corpus : non-retryable, et le `hint` doit rappeler que
    /// l'absence du corpus ne prouve pas l'inexistence.
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::NotFound,
            retryable: false,
            hint: None,
        }
    }
    /// Authentification requise (outils personnels).
    fn auth(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            category: ErrorCategory::Auth,
            retryable: false,
            hint: None,
        }
    }
    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Point d'entrée HTTP : décode l'enveloppe JSON-RPC, route vers la méthode.
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

    let is_notification = body.get("id").is_none();

    match dispatch(&state, method, params, user.as_deref(), session.as_deref()).await {
        Ok(result) => {
            if is_notification {
                return StatusCode::ACCEPTED.into_response();
            }
            Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
        }
        Err(err) => {
            if is_notification {
                return StatusCode::ACCEPTED.into_response();
            }
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": err.code, "message": err.message},
            }))
            .into_response()
        }
    }
}

/// Corrélateur de session MCP : hash court, non réversible, du header
/// `Mcp-Session-Id` — jeton par conversation, éphémère, jamais l'identité de
/// l'utilisateur. Absent si le client n'envoie pas de session id.
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

/// Routage des méthodes MCP standard.
async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize_result()),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(state, params, user, session).await,
        // Resources : liste vide assumée. Pour exposer des données au modèle, la voie
        // est un tool.
        "resources/list" => Ok(json!({ "resources": [] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": prompt_definitions() })),
        "prompts/get" => get_prompt(&params),
        other => Err(RpcError::method_not_found(other)),
    }
}

/// Réponse d'`initialize` : capacités serveur + métadonnées d'implémentation.
fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "experimental": {},
            "prompts": {"listChanged": false},
            "resources": {"subscribe": false, "listChanged": false},
            "tools": {"listChanged": false},
        },
        "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        "instructions": SERVER_INSTRUCTIONS,
    })
}

// ── Outils : appel ───────────────────────────────────────────────────────────

/// Dispatch `tools/call` : décode `name` + `arguments`, exécute l'outil, emballe
/// le résultat structuré dans `content` (texte JSON) + `structuredContent`.
async fn call_tool(
    state: &AppState,
    params: Value,
    user: Option<&str>,
    session: Option<&str>,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Span parent d'appel d'outil : attributs domaine, jamais l'identité. Enveloppe
    // l'exécution, donc les spans store fan-out deviennent descendants et partagent le
    // `traceID` ; `mcp.session` regroupe les appels d'une même conversation.
    let span = tracing::info_span!(
        "mcp_tool_call",
        librejustice.mcp.tool = name,
        librejustice.mcp.authenticated = user.is_some(),
        librejustice.mcp.session = tracing::field::Empty,
        librejustice.mcp.arg = tracing::field::Empty,
        librejustice.mcp.outcome = tracing::field::Empty,
        librejustice.mcp.error_category = tracing::field::Empty,
        librejustice.mcp.error = tracing::field::Empty,
    );
    if let Some(s) = session {
        span.record("librejustice.mcp.session", s);
    }
    let arg: String = arguments.to_string().chars().take(256).collect();
    span.record("librejustice.mcp.arg", arg.as_str());

    async move {
        // Sémantique MCP : un échec d'exécution d'outil n'est pas une erreur JSON-RPC
        // mais un résultat avec `isError: true` et le message dans `content`. Seules les
        // erreurs de protocole restent des erreurs JSON-RPC.
        let result = match name {
            "search_decisions" => tool_search_decisions(state, arguments, user).await,
            "get_decision" => tool_get_decision(state, arguments, user).await,
            "list_my_activity" => tool_list_my_activity(state, arguments, user).await,
            "get_legal_text" => tool_get_legal_text(state, arguments).await,
            "search_legal_texts" => tool_search_legal_texts(state, arguments, user).await,
            "search_annonces" if PUBLIER_SEARCH_ANNONCES => {
                tool_search_annonces(state, arguments, user).await
            }
            // Deux générations de noms de la jambe normes. Un client MCP fige la liste
            // d'outils à l'installation du connecteur et le modèle ne peut émettre qu'un nom
            // de sa liste : un renommage n'est rattrapable que par un alias.
            "get_law_article" | "get_legal_article" => tool_get_legal_text(state, arguments).await,
            "search_law_articles" | "search_legal_articles" => {
                tool_search_legal_texts(state, arguments, user).await
            }
            other => Err(RpcError::invalid_params(format!("Unknown tool: {other}"))),
        };

        let span = tracing::Span::current();
        match result {
            Ok((text, structured)) => {
                span.record("librejustice.mcp.outcome", "ok");
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
                Ok(tool_error_content(&e))
            }
        }
    }
    .instrument(span)
    .await
}

/// Résultat `tools/call` en erreur applicative : `isError: true` + contrat
/// d'erreur structuré en `content` texte. `category` dit la nature de l'échec,
/// `retryable` si un retry à l'identique a un sens, `hint` la prochaine action.
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

/// Sérialise une sortie d'outil en `(content texte, structuredContent)` — même
/// JSON dans les deux canaux, en compact : le bloc texte est ce que les hôtes
/// injectent au modèle.
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
) -> Result<(String, Value), RpcError> {
    let mut req = build_search_request(&args)?;
    let refs = crate::referential::referential(state)
        .await
        .map_err(|e| RpcError::tool_error(e.to_string()))?;
    // Un code hors référentiel matcherait silencieusement zéro décision.
    if let Some(bad) = req
        .jurisdiction_code
        .iter()
        .flatten()
        .find(|c| refs.jurisdiction(c).is_none())
    {
        return Err(unknown_jurisdiction_code(bad, &refs));
    }
    // Même garantie pour les axes à catégorie contrôlée.
    for (field, values) in [("chamber", &req.chamber), ("publication", &req.publication)] {
        if let Some(bad) = values
            .as_deref()
            .and_then(|vs| refs.find_unknown_token(field, vs))
        {
            let tokens = refs.facet_tokens(field);
            return Err(RpcError::invalid_params(format!(
                "unknown {field} '{bad}'; valid values: {}",
                tokens.join(", ")
            ))
            .with_hint(list_hint(field, &tokens)));
        }
    }
    // Filtres instrument/article : les colonnes portent des uids de catalogue, pas
    // des noms. Chaque valeur est résolue via le slug ; inconnue → erreur corrective
    // avec les slugs les plus proches.
    resolve_instrument_filters(state, &mut req).await?;
    // Budget par compte MCP : le jeton OAuth est la clé, et il est révocable. Sans
    // porteur, l'appelant n'est pas identifiable donc pas limité.
    let budget_key = crate::ratelimit::caller_key(user, &axum::http::HeaderMap::new());
    let response =
        crate::search::search(state, &req, mcp_caller(user.is_some()), budget_key.as_ref())
            .await
            .map_err(|e| RpcError::tool_error(e.to_string()))?;
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
    tool_ok(&present_search_response(
        &response,
        &state.settings.web_base_url,
    ))
}

async fn tool_get_decision(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let url = required_str(&args, "url")?;
    // La clé est le dernier segment de l'URL publique `/decision/{clé}`.
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

/// `list_my_activity` : agrège les tranches demandées (recherches/signets/
/// lectures). Personnel → exige un utilisateur MCP authentifié (remonté en
/// `isError` par `call_tool`).
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

// ── Outil law-at-date ────────────────────────────────────────────────────────

/// `get_legal_text` : article de référentiel à une date (ou en vigueur si
/// `date` absente), par `url` `/texte/{code}/{article}`. La réponse inclut la
/// timeline des versions (champ `versions`). 404 (code/article inconnu) →
/// `isError`.
async fn tool_get_legal_text(state: &AppState, args: Value) -> Result<(String, Value), RpcError> {
    let url = required_str(&args, "url").map_err(|e| {
        e.with_hint(
            "url is the only way in: /texte/{code-slug}/{article-key}, e.g. \
             {\"url\": \"/texte/code-civil/1240\"}. There is no lookup by Légifrance \
             id (LEGIARTI…, LEGITEXT…, CID) nor by {code_name, article_number} — \
             call search_legal_texts, its hits carry the url to pass here.",
        )
    })?;
    let date: Option<String> = deser_field(&args, "date")?;

    let (code, num) = match split_article_url(&url) {
        Some((code, Some(num))) => (code.to_string(), num.to_string()),
        Some((code, None)) => return Err(code_root_url_error(state, &url, code).await),
        None => {
            return Err(RpcError::invalid_params(format!(
                "not an article url (expected …/texte/{{code}}/{{article}}): {url}"
            )))
        }
    };
    let num = lj_core::article_key::article_key(&num);

    let code = resolve_law_code(state, &code).await?;

    let detail = match date {
        Some(d) => {
            let parsed = parse_mcp_date("date", &d)?;
            crate::legi::article_at_date(state, &code, &num, parsed).await
        }
        None => crate::legi::article_in_force(state, &code, &num).await,
    }
    .map_err(map_legi_error)?;
    tool_ok(&present_law_article(&detail, &state.settings.web_base_url))
}

/// `search_legal_texts` : recherche plein-texte d'articles de référentiel,
/// filtrable par `code`/`jurisdiction`. Renvoie le `total` exact et
/// une page de hits (extrait surligné + `url` + `num`), à chaîner ensuite vers
/// `get_legal_text` pour le texte complet à date.
async fn tool_search_legal_texts(
    state: &AppState,
    args: Value,
    user: Option<&str>,
) -> Result<(String, Value), RpcError> {
    let query = required_str(&args, "query")?;
    if query.chars().count() > 512 {
        return Err(RpcError::invalid_params(
            "query: String should have at most 512 characters",
        ));
    }
    let opt = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(10);
    if !(1..=20).contains(&limit) {
        return Err(RpcError::invalid_params(
            "limit: Input should be between 1 and 20",
        ));
    }

    let code = match opt("code") {
        Some(c) => Some(resolve_law_code(state, c).await?),
        None => None,
    };

    // Recherche à date : versions valides à `date`, même alphabet que
    // `get_legal_text`.
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
        mcp_caller(user.is_some()),
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

/// `search_annonces` : le pot Annonces — fenêtre vivante des marchés publics et
/// proxys live BODACC / JOAFE. Facettes seulement : le pot n'a ni BM25 ni
/// embeddings, et `query` est un filtre par mot.
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
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(10);
    if !(1..=20).contains(&limit) {
        return Err(RpcError::invalid_params(
            "limit: Input should be between 1 and 20",
        ));
    }
    let departement = strings("departement");
    // Un code hors nomenclature ne filtre rien : erreur franche plutôt qu'une page
    // vide qu'un modèle interpréterait comme « aucun marché ».
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
    // Le fond est l'axe d'entrée du pot : omis, l'outil interroge tous les fonds.
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

/// Erreur pour un `jurisdiction_code` hors référentiel, avec suggestions. Le hint
/// ajoute la forme de l'appel sur deux codes réels : la liste entière fait un
/// millier d'entrées, elle ne s'énumère pas dans un message.
fn unknown_jurisdiction_code(code: &str, refs: &crate::referential::Referential) -> RpcError {
    let examples: Vec<&str> = refs.jurisdictions().map(|(c, _)| c).take(2).collect();
    RpcError::invalid_params(crate::referential::unknown_jurisdiction_code_msg(
        code, refs,
    ))
    .with_hint(list_hint("jurisdiction_code", &examples))
}

/// Slug d'un nom de texte légal, dans l'alphabet des `legal_text.slug` :
/// minuscules, accents pliés, toute séquence non alphanumérique réduite à `-`.
fn law_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.to_lowercase().chars() {
        let folded = match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'î' | 'ï' => 'i',
            'ó' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'œ' => {
                out.push('o');
                'e'
            }
            c => c,
        };
        if folded.is_ascii_alphanumeric() {
            out.push(folded);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Résout les filtres `legal_instrument` / `legal_article` (slug ou nom libre,
/// slugifié) vers l'alphabet des colonnes (`text_uid` de catalogue, numéro en
/// forme composite). Valeur inconnue → erreur corrective avec les slugs les
/// plus proches — jamais un filtre qui matche silencieusement zéro.
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
            match repo
                .resolve_instrument_uid(&law_slug(v))
                .await
                .map_err(|e| RpcError::tool_error(e.to_string()))?
            {
                Some(uid) => *v = uid,
                None => return Err(unknown_instrument(&repo, "legal_instrument", v).await),
            }
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
            let instrument = instrument.trim();
            match repo
                .resolve_instrument_uid(&law_slug(instrument))
                .await
                .map_err(|e| RpcError::tool_error(e.to_string()))?
            {
                Some(uid) => *v = format!("{uid}|{}", lj_core::article_key::article_key(num)),
                None => return Err(unknown_instrument(&repo, "legal_article", instrument).await),
            }
        }
    }
    Ok(())
}

/// Message « unknown {field} '{value}' » + slugs candidats par similarité
/// trigramme (titre + slug, tolérante aux fautes). Les candidats ressortent
/// pour servir d'exemples au hint de l'appelant, qui seul sait si son champ est
/// une liste.
async fn unknown_instrument_msg(
    repo: &lj_store::repository::DecisionRepository<'_>,
    field: &str,
    value: &str,
) -> (String, Vec<String>) {
    let suggestions = repo.suggest_instruments(value, 5).await.unwrap_or_default();
    let did_you_mean = if suggestions.is_empty() {
        String::new()
    } else {
        format!(
            "; did you mean: {}",
            suggestions
                .iter()
                .map(|(slug, title)| format!("\"{slug}\" ({title})"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let slugs = suggestions.into_iter().map(|(slug, _)| slug).collect();
    (format!("unknown {field} '{value}'{did_you_mean}"), slugs)
}

/// Erreur corrective pour un instrument hors catalogue, côté **filtres de
/// recherche** — deux champs liste, d'où la forme d'appel en plus des candidats.
async fn unknown_instrument(
    repo: &lj_store::repository::DecisionRepository<'_>,
    field: &str,
    value: &str,
) -> RpcError {
    let (msg, slugs) = unknown_instrument_msg(repo, field, value).await;
    RpcError::invalid_params(msg).with_hint(format!(
        "Pass slugs (from the legal_instrument facet of a previous search) or \
         exact text names. {}",
        list_hint(field, &slugs)
    ))
}

/// Résout un nom de code vers son slug catalogue — l'alphabet unique de `/texte`,
/// partagé par `get_legal_text` et `search_legal_texts`. Trois entrées acceptées,
/// dans l'ordre : le slug lui-même, un nom libre slugifié, puis l'acronyme d'usage
/// du dataset d'alias. Inconnu → erreur corrective avec les slugs les plus proches.
async fn resolve_law_code(state: &AppState, raw: &str) -> Result<String, RpcError> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| RpcError::tool_error(format!("checkout connexion: {e}")))?;
    let repo = lj_store::repository::DecisionRepository::new(&conn);
    let exists = |slug: String| async {
        repo.law_slug_exists(&slug)
            .await
            .map(|ok| ok.then_some(slug))
            .map_err(|e| RpcError::tool_error(e.to_string()))
    };
    if let Some(slug) = exists(law_slug(raw)).await? {
        return Ok(slug);
    }
    if let Some(alias) = lj_core::aliases::canonical_slug(raw) {
        if let Some(slug) = exists(alias.to_string()).await? {
            return Ok(slug);
        }
    }
    // `code` est scalaire : pas de forme de liste à donner, seulement l'alphabet des
    // valeurs acceptées.
    let (msg, _) = unknown_instrument_msg(&repo, "code", raw).await;
    Err(RpcError::invalid_params(msg).with_hint(
        "Pass a slug (from the legal_instrument facet of a previous search) \
         or an exact text name.",
    ))
}

/// URL `/texte/{code}` sans segment d'article. Le nom du code est là, donc
/// l'erreur nomme l'appel à faire au lieu de répéter le gabarit d'URL ; code non
/// résolu → suggestions par trigramme de [`resolve_law_code`].
async fn code_root_url_error(state: &AppState, url: &str, code: &str) -> RpcError {
    match resolve_law_code(state, code).await {
        Ok(slug) => RpcError::invalid_params(format!(
            "not an article url: {url} is the root of code \"{slug}\", not an article"
        ))
        .with_hint(format!(
            "Call search_legal_texts with code=\"{slug}\" to find the article, or \
             append its key to the url (/texte/{slug}/{{article}}, lowercase key: \
             \"l761-1\" for L. 761-1)."
        )),
        Err(unresolved) => unresolved,
    }
}

/// Découpe l'`url` de `get_legal_text` en `(code, article)`. Trois entrées :
/// l'URL publique absolue ou relative (`/texte/{code}/{article}`), l'ancien préfixe
/// `/loi/`, et le chemin nu (`code-civil/1240`).
///
/// Le chemin nu reste réservé aux entrées sans schéma : sur une URL absolue, le
/// préfixe est exigé, sinon le découpage mordrait sur le `//` de `https://`.
///
/// Second membre `None` : le code est là, l'article manque.
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

/// Onglets d'activité de `list_my_activity` — source unique du schéma annoncé
/// et du contrôle d'entrée, qui portaient chacun leur copie de la liste.
const ACTIVITY_KINDS: [&str; 4] = ["searches", "bookmarks", "readingHistory", "all"];

/// Extrait un argument chaîne requis non vide (erreur JSON-RPC sinon). « missing »
/// et « empty » sont deux messages distincts.
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

/// Mappe une erreur des handlers LEGI en `RpcError` (NotFound → message dédié).
fn map_legi_error(e: ApiError) -> RpcError {
    match e {
        ApiError::NotFound => RpcError::not_found("law article or code not found").with_hint(
            "Find the article with search_legal_texts, or compose the url as \
             /texte/{code-slug}/{article-key} (lowercase key: \"l761-1\" for L. 761-1).",
        ),
        other => RpcError::tool_error(other.to_string()),
    }
}

/// Construit un [`SearchRequest`] à partir des arguments MCP (`ai_rerank` →
/// `ai_mode`, dates ISO conservées telles quelles ; filtres référentiels
/// solution/procedure/office/legal_domain/jurisdiction_code/publication).
fn build_search_request(args: &Value) -> Result<lj_dtos::SearchRequest, RpcError> {
    // Parité serveur du `additionalProperties: false` annoncé par le schéma : un
    // filtre inconnu ignoré en silence fausserait la recherche sans signal.
    const FIELDS: [&str; 18] = [
        "query",
        "jurisdiction_type",
        "solution",
        "procedure",
        "office",
        "legal_domain",
        "jurisdiction_code",
        "chamber",
        "legal_instrument",
        "legal_article",
        "significance",
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
    // Parité du schéma annoncé (`query.maxLength = 512`) et du chemin HTTP.
    if query.chars().count() > 512 {
        return Err(RpcError::invalid_params(
            "query: String should have at most 512 characters",
        ));
    }

    // Dates validées ici (format + plage), sinon une date malformée atteindrait le
    // SQL.
    let date_from: Option<String> = deser_field(args, "date_from")?;
    let date_to: Option<String> = deser_field(args, "date_to")?;
    validate_mcp_date("date_from", &date_from)?;
    validate_mcp_date("date_to", &date_to)?;
    let limit: u32 = deser_field(args, "limit")?.unwrap_or(10);
    if !(1..=20).contains(&limit) {
        return Err(RpcError::invalid_params(
            "limit: Input should be between 1 and 20",
        ));
    }

    Ok(lj_dtos::SearchRequest {
        query,
        jurisdiction_type: deser_list(args, "jurisdiction_type")?,
        solution: deser_list(args, "solution")?,
        procedure: deser_list(args, "procedure")?,
        office: deser_list(args, "office")?,
        legal_domain: deser_list(args, "legal_domain")?,
        jurisdiction_code: deser_list(args, "jurisdiction_code")?,
        chamber: deser_list(args, "chamber")?,
        legal_instrument: deser_list(args, "legal_instrument")?,
        legal_article: deser_list(args, "legal_article")?,
        significance: deser_list(args, "significance")?,
        publication: deser_list(args, "publication")?,
        date_from,
        date_to,
        mode: deser_field(args, "mode")?.unwrap_or(lj_dtos::SearchMode::Auto),
        sort: deser_field(args, "sort")?.unwrap_or(lj_dtos::SortOrder::Relevance),
        limit,
        offset: 0,
        ai_mode: deser_field(args, "ai_rerank")?.unwrap_or(true),
    })
}

/// Valide une date d'argument MCP (`date_from`/`date_to`). L'erreur remonte en
/// `invalid_params` (donc `isError` côté client, via [`call_tool`]).
fn validate_mcp_date(field: &str, raw: &Option<String>) -> Result<(), RpcError> {
    match raw {
        Some(s) => parse_mcp_date(field, s).map(|_| ()),
        None => Ok(()),
    }
}

/// Parse une date d'argument MCP, pour `date_from`/`date_to` de
/// `search_decisions` comme pour la `date` de consultation des normes. Les bornes
/// de [`crate::search::parse_search_date`] valent pour les deux.
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

/// Hint unique de tous les champs liste, quelle que soit la cause du rejet
/// (forme, valeur hors énumération, token hors référentiel).
///
/// Il dit deux choses qu'aucun message de valeur ne dit : que plusieurs valeurs
/// sont acceptées à la fois — « expected one of » se lit « une seule » — et la
/// forme de l'appel. `examples` sont des valeurs réelles du champ, que le site
/// d'erreur vient de calculer, donc toujours valides.
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

/// Valeurs valides énumérées par un message serde (« unknown variant
/// `judiciaire`, expected one of `TA`, `CAA`, … ») — la source d'exemples de
/// [`list_hint`] pour les enums typés du DTO.
fn serde_variants(err: &str) -> Vec<&str> {
    err.split_once("expected one of ")
        .map(|(_, list)| list.split('`').skip(1).step_by(2).collect())
        .unwrap_or_default()
}

/// Désérialise l'argument liste `key` de `args` vers `Vec<T>`.
///
/// Un scalaire là où le schéma annonce un tableau est emballé en singleton, ce qui
/// sépare deux cas : valeur bonne → on prend le singleton et on émet un WARN ;
/// valeur mauvaise → l'erreur rendue est celle d'après emballage, qui énumère les
/// variantes valides.
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

/// Désérialise l'argument scalaire `key` de `args` vers `T` (erreur JSON-RPC
/// `invalid_params` en cas de type incompatible). Le message nomme le champ : le
/// message nu de serde n'en désigne aucun.
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

// ── Outils : schémas ─────────────────────────────────────────────────────────

/// Liste des définitions d'outils (`tools/list`). Chaque schéma est inline, sans
/// `title`, avec `additionalProperties: false` au niveau racine.
fn tool_definitions() -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "search_decisions",
            "title": "Search Decisions",
            "description": SEARCH_DECISIONS_DESC,
            "inputSchema": search_decisions_schema(),
            "outputSchema": search_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_decision",
            "title": "Get Decision",
            "description": GET_DECISION_DESC,
            "inputSchema": get_decision_schema(),
            "outputSchema": get_decision_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "list_my_activity",
            "title": "List My Activity",
            "description": LIST_ACTIVITY_DESC,
            "inputSchema": list_activity_schema(),
            "outputSchema": activity_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_legal_text",
            "title": "Get Legal Text",
            "description": GET_LEGAL_TEXT_DESC,
            "inputSchema": get_legal_text_schema(),
            "outputSchema": get_legal_text_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "search_legal_texts",
            "title": "Search Legal Texts",
            "description": SEARCH_LEGAL_TEXTS_DESC,
            "inputSchema": search_legal_texts_schema(),
            "outputSchema": search_legal_texts_output_schema(),
            "annotations": read_only_annotations(),
        }),
    ];
    if PUBLIER_SEARCH_ANNONCES {
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

/// `search_annonces` n'est ni déclaré ni joignable. La constante garde les deux
/// portes — `tools/list` et le routage de `tools/call` — pour qu'un client qui
/// devine le nom ne l'atteigne pas. Le gestionnaire et les schémas restent en
/// place ; republier, c'est repasser cette constante à `true`.
///
/// À rouvrir aussi : les proxys live BODACC / JOAFE font sortir l'outil du corpus
/// indexé, donc il ne peut pas hériter du `openWorldHint: false` des cinq autres.
const PUBLIER_SEARCH_ANNONCES: bool = false;

/// Annotations communes : tous les outils sont en lecture seule. Le log
/// d'activité de `search_decisions` / `get_decision` est une journalisation privée
/// du compte, pas une modification d'environnement au sens MCP.
fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

// ── Prompts MCP ──────────────────────────────────────────────────────────────
//
// Workflows invocables par l'utilisateur depuis le client (menu prompts,
// slash-command), servis à tout client connecté au serveur distant. Titres,
// descriptions et messages en français : c'est du texte face utilisateur, inséré
// dans sa conversation.

/// Liste des prompts (`prompts/list`).
fn prompt_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "recherche_jurisprudence",
            "title": "Recherche de jurisprudence",
            "description": "Recherche des décisions sur une question juridique, les lit en texte intégral et rend une synthèse sourcée.",
            "arguments": [{
                "name": "question",
                "description": "Question ou sujet juridique, juridiction et période comprises si utiles (ex. « décisions du TA de Paris annulant une OQTF pour atteinte à l'intérêt supérieur de l'enfant »)",
                "required": true,
            }],
        }),
        json!({
            "name": "article_a_date",
            "title": "Article en vigueur à une date",
            "description": "Retrouve le texte exact d'un article de loi tel qu'il s'appliquait à une date donnée.",
            "arguments": [
                {
                    "name": "article",
                    "description": "L'article et son texte (ex. « article 1240 du code civil »)",
                    "required": true,
                },
                {
                    "name": "date",
                    "description": "Date de consultation YYYY-MM-DD (ex. la date des faits) ; vide = version en vigueur",
                    "required": false,
                },
            ],
        }),
        json!({
            "name": "textes_applicables",
            "title": "Textes applicables à un sujet",
            "description": "Identifie les articles de loi qui régissent un sujet et les lit dans leur version pertinente.",
            "arguments": [{
                "name": "sujet",
                "description": "Le sujet ou la situation juridique (ex. « frais irrépétibles en procédure civile »)",
                "required": true,
            }],
        }),
    ]
}

/// `prompts/get` : instancie le prompt demandé avec ses arguments. Le message
/// rendu encode la méthode anti-hallucination : lire le texte intégral avant de
/// citer, version à date, avouer l'introuvable.
fn get_prompt(params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing prompt name"))?;
    let arg = |key: &str| -> Result<String, RpcError> {
        params
            .get("arguments")
            .and_then(|a| a.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| RpcError::invalid_params(format!("missing prompt argument: {key}")))
    };
    let (description, text) = match name {
        "recherche_jurisprudence" => {
            let question = arg("question")?;
            (
                "Recherche de jurisprudence avec lecture des décisions en texte intégral.",
                format!(
                    "Recherche la jurisprudence pertinente sur : {question}.\n\n\
                     Méthode : lance search_decisions — juridiction, période et tri \
                     (récence) dans les filtres structurés, la question réservée au \
                     problème de droit ; reformule ou filtre si la première salve déçoit. \
                     Puis ouvre chaque décision candidate avec get_decision \
                     avant de la citer — un extrait ou un résumé ne suffit jamais pour \
                     affirmer ce qu'une décision juge. Vérifie appellateFate avant de citer \
                     une décision comme autorité. Rends une synthèse sourcée : pour chaque \
                     décision retenue, juridiction, date, numéro, apport, et son url en lien. \
                     Si rien de probant ne ressort, dis-le : le corpus n'est pas exhaustif."
                ),
            )
        }
        "article_a_date" => {
            let article = arg("article")?;
            let quand = match params
                .get("arguments")
                .and_then(|a| a.get("date"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                Some(d) => format!("en vigueur au {d} (passe date={d} à get_legal_text)"),
                None => "actuellement en vigueur".to_string(),
            };
            (
                "Texte exact d'un article de loi à une date donnée (law-at-date).",
                format!(
                    "Donne le texte exact de {article}, dans sa version {quand}.\n\n\
                     Méthode : get_legal_text par l'url /texte/{{code}}/{{article}} (passe par \
                     search_legal_texts si le numéro est incertain). Cite le texte intégral \
                     de la version servie, précise ses dates de validité, et signale les \
                     versions voisines si la date demandée tombe près d'un changement."
                ),
            )
        }
        "textes_applicables" => {
            let sujet = arg("sujet")?;
            (
                "Textes et articles applicables à un sujet, lus dans la bonne version.",
                format!(
                    "Identifie les textes qui régissent : {sujet}.\n\n\
                     Méthode : search_legal_texts pour constituer la liste (affine avec la \
                     facette code), puis ouvre chaque article pertinent avec get_legal_text — \
                     avec date si le litige relève d'une version antérieure. Cite chaque \
                     article par numéro et code, avec sa version et son url."
                ),
            )
        }
        other => return Err(RpcError::invalid_params(format!("unknown prompt: {other}"))),
    };
    Ok(json!({
        "description": description,
        "messages": [{"role": "user", "content": {"type": "text", "text": text}}],
    }))
}

/// Enum brut d'un type sérialisable (codes attendus dans le schéma MCP).
fn enum_values(values: &[&str]) -> Value {
    json!(values)
}

/// Codes sérialisés d'un enum de facette DTO — source unique : les variantes
/// `ALL` de `lj_dtos::schema`, pas de liste copiée.
fn facet_enum_codes<T: serde::Serialize>(all: &[T]) -> Value {
    json!(all
        .iter()
        .filter_map(|v| serde_json::to_value(v).ok())
        .collect::<Vec<Value>>())
}

/// Variante pour un vocabulaire porté en `&[&str]` (suffixes d'uid).
fn facet_enum_codes_str(values: &[&str]) -> Value {
    json!(values)
}

/// Champ optionnel sérialisé `{"anyOf": [inner, {"type": "null"}],
/// "default": null, <extra…>}`. `extra` apporte les clés soeurs de `anyOf`
/// (`description` côté inputSchema, `format` côté outputSchema).
fn nullable(inner: Value, extra: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("anyOf".into(), json!([inner, {"type": "null"}]));
    m.insert("default".into(), Value::Null);
    if let Value::Object(e) = extra {
        m.extend(e);
    }
    Value::Object(m)
}

fn search_decisions_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": 512,
                "description": SEARCH_QUERY_DESC,
            },
            "jurisdiction_type": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": enum_values(JURISDICTION_TYPE_CODES)}}),
                json!({"description": "Restrict to one or more court categories: TJ (tribunal judiciaire), CA (cour d'appel), CC (Cour de cassation), TCOM (tribunal des activités économiques), TA (tribunal administratif), CAA (cour administrative d'appel), CE (Conseil d'État), CNDA (asylum), CONSTIT (Conseil constitutionnel), TC (Tribunal des conflits), CNIL (sanctions), CEDH and CJUE (European courts)."}),
            ),
            "solution": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Solution::ALL)}}),
                json!({"description": "Filter by the ruling of the operative part (référentiel solution, ADR 0146). REJET / IRRECEVABILITE / DESISTEMENT / NON_LIEU_A_STATUER are procedural or negative endings; CONFIRMATION / INFIRMATION* / REFORMATION are appeal outcomes; CASSATION* is cassation-specific; ANNULATION covers administrative annulment; SATISFACTION_TOTALE / SATISFACTION_PARTIELLE cover first-instance civil rulings granting the claim."}),
            ),
            "procedure": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Procedure::ALL)}}),
                json!({"description": "Filter by procedural track (référés, QPC, EU referral, révision, tierce opposition…). Absent value = ordinary contentious procedure."}),
            ),
            "office": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Office::ALL)}}),
                json!({"description": "Filter by specialised judge/office (JLD, JAF, JCP, JEX, juge des enfants, premier président, magistrat désigné). Absent value = ordinary bench."}),
            ),
            "legal_domain": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Domain::ALL)}}),
                json!({"description": "Filter by legal domain (curated domain tree, ADR 0146): 9 roots and their leaves (e.g. CIVIL_DROIT_LOCATIF). Selecting a root also matches all its leaves."}),
            ),
            "publication": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes_str(PUBLICATION_CODES)}}),
                json!({"description": "Filter by publication level (any-of, référentiel publication): PUBLIE_BULLETIN / INEDIT_BULLETIN (Cour de cassation), PUBLIE_LEBON / MENTIONNE_LEBON / INEDIT_LEBON (administrative), AUTRE (no publication statement in the source)."}),
            ),
            "significance": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Significance::ALL)}}),
                json!({"description": "Filter by jurisprudential significance (any-of, ADR 0167), derived from publication codes at strongest rank: MAJEURE (rapport annuel / recueil Lebon), IMPORTANTE (bulletin, tables du Lebon, lettres de chambre, communiqués), LIMITEE (unpublished), INDETERMINEE (no publication statement — lower courts, European courts)."}),
            ),
            "jurisdiction_code": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 40}}),
                json!({"description": "Restrict to one or more precise court units by referential code. Code shapes: \"cc\" (Cour de cassation), \"ce\" (Conseil d'État), \"cnda\", \"cedh\", \"cjue\"; \"ca_<city>\", \"caa_<city>\", \"ta_<city>\", \"tj_<city>\", \"tcom_<city>\" (e.g. \"ca_paris\", \"ta_marseille\", \"tj_paris\", \"tcom_lyon\"). When unsure, guess with the city name in the code — the error names the nearest valid ones. Each code is a court; the chamber is a separate axis (see chamber); use jurisdiction_type for the broad category."}),
            ),
            "chamber": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 40}}),
                json!({"description": "Filter by chamber category (any-of, uniform across orders, e.g. \"CIVILE\", \"SOCIALE\", \"COMMERCIALE\", \"PROCEDURES_COLLECTIVES\", \"ETRANGERS\", \"PROTECTION_SOCIALE\", \"INSTRUCTION\"). Codes come from the chamber facet of a previous search."}),
            ),
            "legal_instrument": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 100}}),
                json!({"description": "Restrict to decisions citing one or more given codes or statutes. Accepts a slug from facets.legal_instrument (e.g. \"code-civil\") or an exact text name resolved server-side (e.g. \"Code civil\")."}),
            ),
            "legal_article": nullable(
                json!({"type": "array", "maxItems": 20, "items": {"type": "string", "maxLength": 140}}),
                json!({"description": "Restrict to decisions citing a specific article of a specific code, as a composite key \"<instrument>|<article>\" where <instrument> is a slug or an exact text name, resolved like legal_instrument (e.g. \"code-civil|1240\", \"code-de-justice-administrative|L761-1\"). The instrument prefix is required — the same article number exists in several codes."}),
            ),
            "date_from": nullable(
                json!({"type": "string"}),
                json!({"description": "Earliest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "date_to": nullable(
                json!({"type": "string"}),
                json!({"description": "Latest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "mode": {
                "type": "string",
                "enum": ["auto", "lexical", "semantic"],
                "default": "auto",
                "description": SEARCH_MODE_DESC,
            },
            "sort": {
                "type": "string",
                "enum": ["relevance", "date_desc", "date_asc"],
                "default": "relevance",
                "description": "Result ordering. Use 'relevance' (default) unless the user wants chronological order.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (1–20, default 10).",
            },
            "ai_rerank": {
                "type": "boolean",
                "default": true,
                "description": "When enabled (default), reorders results by actual relevance to the query using an LLM reranker (cf. ADR 0041). Keep on for agentic use — shortlist quality is significantly higher. Cost: a few seconds of extra latency. Disable only for high-rate exploratory searches where latency matters more than ranking quality.",
            },
        },
    })
}

fn get_decision_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": {
                "type": "string",
                "minLength": 1,
                "description": "A librejustice.fr decision URL, from a search_decisions hit or an inline citation link; search_decisions first if you have neither.",
            },
        },
    })
}

fn list_activity_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "kind": {
                "type": "string",
                "enum": enum_values(&ACTIVITY_KINDS),
                "default": "all",
                "description": "Which tab to list: 'searches', 'bookmarks', 'readingHistory', or 'all' (default) for the three at once.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 100,
                "default": 50,
                "description": "Maximum number of entries per tab (1–100, default 50).",
            },
        },
    })
}

/// Schéma d'entrée de `get_legal_text` : `url` requise + `date` optionnelle.
fn get_legal_text_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": {
                "type": "string",
                "description": LAW_URL_DESC,
            },
            "date": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD) — returns the version in force at that date. Omit for the version currently in force."}),
            ),
        },
    })
}

/// Schéma d'entrée de `search_legal_texts` (`query` requis + filtres
/// `code`/`jurisdiction`/`nature`/`source` optionnels + `limit`).
fn search_legal_texts_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": 512,
                "description": "French query over statutory articles. Matches article titles (boosted) and bodies; alias expansion handles acronyms and usual names.",
            },
            "code": nullable(
                json!({"type": "string"}),
                json!({"description": "Restrict to one code/text by its URL slug (\"code-civil\", as in facets.code) or exact name. Omit to search the whole navigable referential."}),
            ),
            "jurisdiction": nullable(
                json!({"type": "string"}),
                json!({"description": "Filter by country/legal order, as an ISO 3166 alpha-2 country code: \"FR\" (France, the bulk of the corpus) or a foreign code (\"SN\", \"DZ\", \"MA\", \"VN\", \"PE\", …); plus \"UE\" for EU law and \"INTL\" for treaties/international law."}),
            ),
            "date": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD) — searches the versions valid at that date (point-in-time, same semantics as get_legal_text). Omit to search the versions currently in force."}),
            ),
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (1–20, default 10).",
            },
        },
    })
}

/// Natures de marché (suffixes d'uid `nature_marche:*`).
const NATURES_MARCHE: [&str; 4] = ["TRAVAUX", "SERVICES", "FOURNITURES", "AUTRES"];

fn search_annonces_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [],
        "properties": {
            "query": nullable(
                json!({"type": "string", "maxLength": 512}),
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
                "description": "Maximum number of results (1–20, default 10). Ordered by bid deadline, soonest first.",
            },
        },
    })
}

// ── Outils : schémas de sortie (`outputSchema`) ───────────────────────────────

/// Schéma de sortie de `search_decisions` (= `McpSearchResponse`).
fn search_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "hits"],
        "properties": {
            "query": {"type": "string"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "description": HIT_METADATA_DESC,
                    "required": ["title", "url", "chars", "jurisdictionType"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "aiSummary": nullable(json!({"type": "string"}), json!({"description": "AI-written summary of what the decision is about (semantic search) — a machine paraphrase, NEVER the court's words: nothing in it is quotable. Exactly one of aiSummary/snippet is served per hit."})),
                        "snippet": nullable(json!({"type": "string"}), json!({"description": "Verbatim passage of the decision text where the query terms matched (keyword search). Exactly one of aiSummary/snippet is served per hit."})),
                        "chars": {"type": "integer"},
                        "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
                        "docketNumbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
                        "jurisdictionType": {"type": "string"},
                        "jurisdictionCode": nullable(json!({"type": "string"}), json!({})),
                        "chamber": nullable(json!({"type": "string"}), json!({})),
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "procedure": nullable(json!({"type": "string"}), json!({})),
                        "office": nullable(json!({"type": "string"}), json!({})),
                        "legalDomain": nullable(json!({"type": "string"}), json!({})),
                        "publication": nullable(json!({"type": "string"}), json!({})),
                    },
                },
            },
            "facets": {
                "type": "object",
                "description": "Per filter name, a map of filter value to decision count under the current query (jurisdiction_type, jurisdiction_code, chamber, office, legal_domain, solution, significance, publication, date_lecture_year, legal_instrument). Reuse keys verbatim as filter values. jurisdiction_code is capped to the top 15 courts (other_courts counts the rest), legal_instrument to the top 10 statutes.",
                "additionalProperties": true,
            },
        },
    })
}

/// Schéma de sortie de `get_decision` (= `McpDecisionDetail`).
fn get_decision_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "description": HIT_METADATA_DESC,
        "required": ["title", "url", "jurisdictionType", "text"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "docketNumbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
            "jurisdictionType": {"type": "string"},
            "jurisdictionCode": nullable(json!({"type": "string"}), json!({})),
            "chamber": nullable(json!({"type": "string"}), json!({})),
            "solution": nullable(json!({"type": "string"}), json!({})),
            "procedure": nullable(json!({"type": "string"}), json!({})),
            "office": nullable(json!({"type": "string"}), json!({})),
            "legalDomain": nullable(json!({"type": "string"}), json!({})),
            "publication": nullable(json!({"type": "string"}), json!({})),
            "appellateFate": nullable(json!({"type": "string"}), json!({"description": "Fate of THIS decision before the court that reviewed it, precomputed from caseChronology: 'INFIRMATION — Cour d'appel de Paris, 1 juillet 2025 (url)'. INFIRMATION = this decision was reversed and no longer stands; CONFIRMATION = upheld. Absent = no recourse known to the corpus, which never proves the decision is final."})),
            "caseChronology": {
                "type": "array",
                "description": "Linked prior and subsequent decisions of the same case (appeal, pourvoi, renvoi), most recent first, current decision included. The fate of a judgment reads in the solution of the decision that reviewed it (INFIRMATION = reversed, CONFIRMATION = upheld). Absent chronology never proves no recourse exists.",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "current"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "current": {"type": "boolean"},
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "linkToNext": nullable(json!({"type": "string"}), json!({"description": "Link nature to the step below (the attacked decision): APPEL_DE | POURVOI_CONTRE | RENVOI_APRES_CASSATION."})),
                    },
                },
            },
            "commentaires": {
                "type": "array",
                "description": COMMENTAIRES_DESC,
                "items": commentaire_schema(),
            },
            "text": {"type": "string"},
        },
    })
}

/// Schéma d'un commentaire institutionnel (= `lj_dtos::Commentaire`), partagé
/// entre `get_decision` et `get_legal_text`.
fn commentaire_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["kind"],
        "properties": {
            "kind": {"type": "string", "description": "analyse = official court-written abstract, full content in `body`. conclusions = the rapporteur public's conclusions, `url` only (copyright-protected, existence + official link). note = linked institutional document (report, avocat général's opinion, press release, doctrine note), `url` + `title` + `publisher`."},
            "author": nullable(json!({"type": "string"}), json!({})),
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "body": nullable(json!({"type": "string"}), json!({})),
            "title": nullable(json!({"type": "string"}), json!({})),
            "publisher": nullable(json!({"type": "string"}), json!({})),
            "access": nullable(json!({"type": "string"}), json!({"description": "libre | abonnes"})),
            "rubriques": {"type": "array", "items": {"type": "string"}, "description": "Official classification headings (analyse)."},
            "renvois": {"type": "array", "items": {"type": "string"}, "description": "Doctrinal cross-references of the analyse (« Cf. CE, … »)."},
            "url": nullable(json!({"type": "string"}), json!({"description": "External link, to hand to the user — not openable by an MCP tool."})),
        },
    })
}

/// Référence décision inline (= `McpDecisionRef`). Apparaît deux fois dans
/// l'`outputSchema` d'activité, après inlining des `$ref`.
fn decision_ref_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "description": DECISION_REF_DESC,
        "required": ["title", "url"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "summary": nullable(json!({"type": "string"}), json!({})),
            "dateLecture": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "solution": nullable(json!({"type": "string"}), json!({})),
            "bookmarkedAt": nullable(json!({"type": "string"}), json!({"format": "date-time"})),
            "viewCount": nullable(json!({"type": "integer"}), json!({})),
            "lastSource": nullable(json!({"type": "string"}), json!({})),
            "lastViewedAt": nullable(json!({"type": "string"}), json!({"format": "date-time"})),
        },
    })
}

/// Schéma de sortie de `list_my_activity` (= `McpActivityResponse`).
fn activity_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "description": ACTIVITY_RESPONSE_DESC,
        "properties": {
            "searches": nullable(
                json!({
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["total", "searches"],
                    "properties": {
                        "total": {"type": "integer"},
                        "searches": {
                            "type": "array",
                            "items": {
                                "additionalProperties": false,
                                "type": "object",
                                "required": ["query", "source", "searchedAt"],
                                "properties": {
                                    "query": {"type": "string"},
                                    "filters": nullable(json!({"additionalProperties": true, "type": "object"}), json!({})),
                                    "source": {"type": "string"},
                                    "searchedAt": {"format": "date-time", "type": "string"},
                                },
                            },
                        },
                    },
                }),
                json!({}),
            ),
            "bookmarks": nullable(
                json!({
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["total", "bookmarks"],
                    "properties": {
                        "total": {"type": "integer"},
                        "bookmarks": {"type": "array", "items": decision_ref_schema()},
                    },
                }),
                json!({}),
            ),
            "readingHistory": nullable(
                json!({
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["total", "decisions"],
                    "properties": {
                        "total": {"type": "integer"},
                        "decisions": {"type": "array", "items": decision_ref_schema()},
                    },
                }),
                json!({}),
            ),
        },
    })
}

/// Schéma de sortie de `get_legal_text` (= `McpLawArticle`).
fn get_legal_text_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["code", "num", "title", "url", "etat", "dateDebut", "versions"],
        "properties": {
            "code": {"type": "string"},
            "num": {"type": "string"},
            "title": {"type": "string"},
            "url": {"type": "string"},
            "etat": {"type": "string"},
            "dateDebut": {"type": "string", "format": "date"},
            "dateFin": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "text": nullable(json!({"type": "string"}), json!({"description": "Full text; cross-references to other articles render as inline markdown links (/texte/ urls, open with get_legal_text)."})),
            "sourceUrl": nullable(json!({"type": "string"}), json!({})),
            "nota": nullable(json!({"type": "string"}), json!({})),
            "commentaires": {
                "type": "array",
                "description": COMMENTAIRES_DESC,
                "items": commentaire_schema(),
            },
            "versions": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["etat", "dateDebut"],
                    "properties": {
                        "etat": {"type": "string"},
                        "dateDebut": {"type": "string", "format": "date"},
                        "dateFin": nullable(json!({"type": "string"}), json!({"format": "date"})),
                    },
                },
            },
        },
    })
}

/// Schéma de sortie de `search_legal_texts` (= `McpLawSearchResponse`).
fn search_legal_texts_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "total", "hits", "facets"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "snippet", "source"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "snippet": {"type": "string"},
                        "titlePath": nullable(json!({"type": "string"}), json!({})),
                        "source": {"type": "string"},
                    },
                },
            },
            "facets": {
                "type": "object",
                "description": "Per filter name, a map of filter value to article count under the current query (code, jurisdiction). Reuse keys verbatim as filter values. Each axis is capped to its top 10.",
                "additionalProperties": true,
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

// ── Codes d'enum (alignés sur les renames serde lj-core) ─────────────────────

const JURISDICTION_TYPE_CODES: &[&str] = &[
    "TA", "CAA", "CE", "CONSTIT", "TC", "CC", "CA", "TJ", "TCOM", "CEDH", "CJUE", "CNDA", "CNIL",
];

/// Suffixes d'uid `publication:*` (facette de place à 6 valeurs, seed 0100).
const PUBLICATION_CODES: &[&str] = &[
    "PUBLIE_BULLETIN",
    "INEDIT_BULLETIN",
    "PUBLIE_LEBON",
    "MENTIONNE_LEBON",
    "INEDIT_LEBON",
    "AUTRE",
];

// ── Descriptions verbatim ─────────────────────────────────────────────────────

const SEARCH_DECISIONS_DESC: &str = "Search decisions by meaning and keywords combined. Returns a \
shortlist: `title`, `url`, an overview (`aiSummary`: AI-written \
summary of what the decision is about, a machine paraphrase never \
quotable as the court's words; or `snippet`: the verbatim passage \
where your keywords matched) and metadata; get_decision reads the \
full text. Put constraints in the structured filters (jurisdiction, \
dates, articles, codes), keep the query for the legal issue. Values \
within one filter are OR'd; different filters are AND'd. The response \
carries a `facets` block: per filter name, a map of filter value to \
decision count under the current query. Reuse those keys verbatim to \
refine (a code from `facets.jurisdiction_code`, a slug from \
`facets.legal_instrument`). Hit metadata fields carry the same tokens \
under the same names: a hit's `jurisdictionCode`, `chamber` or \
`solution` passes back verbatim into the matching filter.";

const GET_DECISION_DESC: &str = "Fetch the full text and metadata of a decision by its `url`. The \
text carries inline markdown links to cited articles (/texte/, open with \
get_legal_text) and cited decisions (/decision/, open with \
get_decision); a citation spanning several articles (« articles 3 à 6 \
», « et suivants ») links its first article and appends the others as \
labelled links right after the span. `appellateFate` states in one line what became of THIS \
decision on review (INFIRMATION = reversed, it no longer stands; \
CONFIRMATION = upheld) — read it before citing the decision as \
authority. `caseChronology` lists the linked prior AND subsequent \
decisions of the same case (appeal, pourvoi, renvoi) known to the \
corpus. An absent fate or chronology never proves no recourse exists — \
only that none is linked in the corpus. `commentaires` carries the \
institutional commentary (official analyses inline, links to the \
rapporteur public's conclusions and related court documents).";

/// Description partagée du bloc `commentaires` dans les deux `outputSchema`.
const COMMENTAIRES_DESC: &str =
    "Institutional commentary anchored on this document: court-written \
analyses served inline (`body`), plus outbound links (`url`) to the \
rapporteur public's conclusions and to related institutional documents \
(reports, opinions, press releases). Context, never the ruling itself \
— quote the decision text, not a commentaire, as the court's words.";

/// Description partagée des objets décision (hit de recherche et détail) :
/// la règle « champ = filtre, valeur = token » y est dite une fois.
const HIT_METADATA_DESC: &str = "Metadata fields carry search_decisions filter tokens: each field \
matches the filter of the same name (jurisdictionType → \
jurisdiction_type, legalDomain → legal_domain…) and its value passes \
back verbatim to filter.";

const GET_LEGAL_TEXT_DESC: &str =
    "Fetch a statutory article as it read on a given date, by its `url` \
(any /texte/ link). Returns the version in force at `date` (omit for \
today): full text, status, validity dates, and the timeline of all \
versions — say which version you quote. The text carries inline \
markdown links to cross-referenced articles (/texte/, open with \
get_legal_text; when served at a date, the links point to the same \
date). `commentaires` carries institutional commentary anchored on the \
article (analyses inline, links otherwise). Covers French codes and \
statutes, plus curated foreign codes and treaties.";

const SEARCH_LEGAL_TEXTS_DESC: &str =
    "Find statutory articles from their subject or wording when the article \
number is unknown; returns a ranked shortlist with highlighted \
snippets and the exact `total`. Query in French, descriptive terms (« \
délai de recours contentieux refus implicite »); put the code in the \
`code` filter (slug or exact name), keep the query for the subject. \
The response carries a `facets` block (`code`, `jurisdiction`): per \
filter name, a map of filter value to article count — reuse those \
keys verbatim to refine. Chain a hit into get_legal_text with its \
`url`, plus `date` when the dispute is governed by an earlier version.";

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

const LAW_URL_DESC: &str = "A librejustice.fr article URL `/texte/{code}/{article}`: \
take it from a search_legal_texts hit or an inline citation link, or compose it — \
{code} is the code slug (\"code-civil\", as in facets.legal_instrument), \
{article} the lowercase article key (\"l761-1\" for L. 761-1, \"1240\" \
for 1240). Never a Légifrance id — LEGIARTI/LEGITEXT/CID are not accepted.";

/// Description inline de `McpDecisionRef` dans l'outputSchema d'activité (retours
/// à la ligne `\n` conservés tels quels dans le schéma émis).
const DECISION_REF_DESC: &str =
    "Référence décision pour signets/consultations : assez pour citer ou
\
chaîner vers ``get_decision`` via ``url``, sans le texte intégral.";

/// Description de `McpActivityResponse` (retours à la ligne inclus).
const ACTIVITY_RESPONSE_DESC: &str =
    "Sortie unique de ``list_my_activity`` : chaque tranche n'est remplie que\n\
si elle a été demandée (``kind`` ciblé ou ``all``). Un seul modèle à plat\n\
(plutôt qu'une union par ``kind``) évite une enveloppe ``result``. Noter\n\
qu'après inlining des ``$ref`` par ``_clean_schema`` (cf. mcp_server),\n\
``McpDecisionRef`` apparaît deux fois dans le schéma émis — une fois sous\n\
``bookmarks``, une fois sous ``readingHistory``.";

const LIST_ACTIVITY_DESC: &str =
    "List the authenticated user's own activity, most recent first. `kind` \
selects which tab: 'searches' (past queries with their structured \
`filters` and `source`), 'bookmarks' or 'readingHistory' (decisions, \
each with `title`, a `summary` and the `url` to chain into \
get_decision), or 'all' to get the three at once. Only the requested \
tab(s) are populated. In readingHistory, `lastSource` == 'web' means \
the user opened the decision manually at least once (a genuine read); \
'mcp' means it was only ever opened through this connector. Requires a \
connected account.";

const SEARCH_QUERY_DESC: &str = "French query, the primary input. Two regimes — pick the right \
tool for the job: \
(a) Natural language or descriptive keywords for legal-issue \
searches; the engine handles synonyms and reformulations. \
Examples: \
« responsabilité hôpital infection nosocomiale » ; \
« étranger malade soins inaccessibles dans son pays d'origine » ; \
« licenciement discrimination syndicale charge de la preuve ». \
(b) Quoted exact phrases and ET / OU / SAUF operators for a \
named entity (company, municipality, person), a precise legal \
formula, or the direction of a holding: semantic matching \
ignores negations (« n'est pas X » ranks like « est X »), and \
only an exact phrase targets which way the court ruled. Examples: \
« \"Société Générale\" » ; \
« \"commune de Saint-Denis\" SAUF Réunion » ; \
« \"force majeure\" ET épidémie ». \
Using operators or quotes switches the engine to keyword-only \
matching for the whole query (no synonyms), so do not mix the \
two regimes.";

const SEARCH_MODE_DESC: &str = "'auto' is the right choice in almost all cases — it picks \
the regime from the query shape. \
auto: combines meaning and keyword matching on plain queries; \
switches to keyword-only when the query contains ET / OU / \
SAUF or quoted phrases. \
lexical: force keyword-only matching. The right choice when \
searching for a named entity (company, municipality, person), \
a precise legal formula, or an exact quote — cuts semantic \
noise. \
semantic: force the meaning+keyword hybrid even on quoted or \
operator queries (where auto would fall back to keyword-only).";

#[cfg(test)]
mod tests {
    use super::*;

    /// Alphabet des `legal_text.slug` : le nom libre d'un texte doit tomber
    /// sur le slug servi par `/texte/{slug}`.
    #[test]
    fn law_slug_matches_catalog_alphabet() {
        assert_eq!(law_slug("Code civil"), "code-civil");
        assert_eq!(
            law_slug("Code de procédure civile"),
            "code-de-procedure-civile"
        );
        assert_eq!(
            law_slug("Code de l'entrée et du séjour des étrangers et du droit d'asile"),
            "code-de-l-entree-et-du-sejour-des-etrangers-et-du-droit-d-asile"
        );
        assert_eq!(law_slug("code-civil"), "code-civil");
    }

    #[test]
    fn search_schema_is_strict_and_inlined() {
        let schema = search_decisions_schema();
        assert_eq!(schema["additionalProperties"], json!(false));
        assert!(schema.get("title").is_none());
        assert!(schema["properties"]["query"].get("title").is_none());
        assert_eq!(schema["required"], json!(["query"]));
        let dumped = serde_json::to_string(&schema).unwrap();
        assert!(!dumped.contains("$ref"));
        assert!(!dumped.contains("$defs"));
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

    /// Scalaire là où le schéma annonce un tableau : accepté en singleton quand la
    /// valeur, elle, est bonne.
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

    /// Valeur hors énumération : l'erreur liste les variantes valides et le hint
    /// donne la forme, sur des valeurs de l'énumération.
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
    fn build_request_rejects_unknown_fields() {
        let err = build_search_request(&json!({
            "query": "bail",
            "jurisdiction_name": ["ca_paris"],
        }))
        .unwrap_err();
        assert!(err.message.contains("jurisdiction_name"));
    }

    /// Contrat d'erreur outil : `category` + `retryable` (+ `hint`) en JSON dans
    /// le bloc texte, pour que le modèle sache corriger / basculer / retenter.
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

    #[test]
    fn prompts_list_and_get_render_messages() {
        let prompts = prompt_definitions();
        let names: Vec<&str> = prompts
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "recherche_jurisprudence",
                "article_a_date",
                "textes_applicables",
            ]
        );
        let got = get_prompt(&json!({
            "name": "article_a_date",
            "arguments": {"article": "article 1240 du code civil", "date": "2018-01-01"},
        }))
        .unwrap();
        assert_eq!(got["messages"][0]["role"], json!("user"));
        let text = got["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("article 1240 du code civil"));
        assert!(text.contains("2018-01-01"));
        let now = get_prompt(&json!({
            "name": "article_a_date",
            "arguments": {"article": "article 700 du CPC"},
        }))
        .unwrap();
        assert!(now["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("actuellement en vigueur"));
        assert!(get_prompt(&json!({"name": "recherche_jurisprudence", "arguments": {}})).is_err());
        assert!(get_prompt(&json!({"name": "nope"})).is_err());
    }

    /// Contrat d'erreur dont dépend la correction par l'agent : le champ fautif est
    /// nommé, et un champ attendu scalaire n'hérite pas du hint « tableau ».
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

    /// Même exigence pour les dates : la forme attendue au cas format, et rien de
    /// plus au cas hors bornes, dont le message porte déjà la borne.
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

    /// Formes d'`url` acceptées par `get_legal_text`.
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
