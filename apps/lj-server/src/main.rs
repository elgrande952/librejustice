//! Binaire de déploiement unique : API REST + MCP/OAuth + SSR Leptos + assets
//! statiques, dans un seul process axum partageant un `AppState` construit **une
//! fois**. Écoute en HTTP clair : le TLS d'origine est terminé par le front `caddy`.
//!
//! Le rendu SSR appelle la couche service `lj-api` **in-process** (l'`AppState` est
//! fourni au contexte Leptos par `leptos_routes_with_context`, lu par
//! `lj_web::api::ApiClient`) ; le navigateur tape `/api` en HTTP same-origin.

use anyhow::Result;
use axum::body::{Body, Bytes, HttpBody};
use axum::Router;
use axum_tracing_opentelemetry::middleware::OtelAxumLayer;
use leptos::prelude::*;
use leptos_axum::{generate_route_list, LeptosRoutes};
use lj_api::{config::Settings, pg_metrics, state::AppState, telemetry};
use lj_store::db;
use lj_web::app::{shell, App};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeader;
use tracing::Instrument as _;

mod tracing_executor;

// Runtime **mono-thread** : le SSR Leptos crée des valeurs `_local` (!Send)
// stockées dans l'Owner réactif ; sur un runtime multi-thread leur drop peut
// tomber sur un autre worker (`SendWrapper::invalid_drop`) et tuer le stream
// Suspense. Création et drop ont donc lieu ici, sur le même thread.
//
// Le CPU lourd (highlight snippet, rendu PDF/DOCX) part sur le pool blocking,
// borné aux cœurs réels — `#[tokio::main]` ne règle pas `max_blocking_threads`,
// d'où le builder manuel.
fn main() -> Result<()> {
    let blocking_cap = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(4);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(blocking_cap)
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    let settings = Settings::from_env()?;
    // Guard à garder vivant : son `Drop` flush les batch processors OTLP.
    let _telemetry_guard = telemetry::init_telemetry(&settings)?;
    // À poser avant la première requête : sinon `leptos_axum` installe l'exécuteur
    // tokio nu, qui perd le span courant à chaque resource.
    tracing_executor::install()?;

    let pool = db::build_pool(&settings.db_url, settings.pool_max)?;
    let state = AppState::build(Arc::new(settings.clone()), pool);

    if settings.grafana_otlp_endpoint.is_some()
        && settings.grafana_otlp_user.is_some()
        && settings.grafana_cloud_api_key.is_some()
    {
        tokio::spawn(pg_metrics::run(state.pool.clone()));
    }

    {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = lj_api::jurisdiction_hubs::catalogue(&state).await {
                tracing::warn!("warm-up catalogue juridictions: {e}");
            }
            if let Err(e) = lj_api::norm_hubs::catalogue(&state).await {
                tracing::warn!("warm-up catalogue normes: {e}");
            }
        });
    }

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let leptos_addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    // Bundle `/pkg` (wasm/js/css) : sert les variantes **précompressées** `.br`/`.gz`
    // générées au build quand le client les accepte.
    let pkg_dir = std::path::Path::new(&leptos_options.site_root.to_string())
        .join(leptos_options.site_pkg_dir.to_string());
    // Les noms de `/pkg` sont content-hashés : un fichier donné ne change jamais de
    // contenu, donc cachable un an `immutable`, et le HTML SSR (via `hash.txt` lu à
    // côté du binaire) référence toujours le bundle exact de son build.
    // `Vary: accept-encoding` : la même URL rend un corps brotli, gzip ou clair selon
    // la requête, et `ServeDir` ne le déclare pas.
    // Bundles des déploiements précédents : `<site_root>-keep/<pkg>`, monté depuis le
    // `state_dir`. Un document HTML antérieur au déploiement courant y trouve le
    // bundle exact de son build et s'hydrate ; les noms étant content-hashés, les deux
    // répertoires fusionnent sans collision possible.
    let keep_dir = std::path::Path::new(&format!("{}-keep", leptos_options.site_root))
        .join(leptos_options.site_pkg_dir.to_string());
    let pkg_service = SetResponseHeader::overriding(
        SetResponseHeader::overriding(
            ServeDir::new(pkg_dir)
                .precompressed_br()
                .precompressed_gzip()
                .fallback(
                    ServeDir::new(keep_dir)
                        .precompressed_br()
                        .precompressed_gzip(),
                ),
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
        ),
        axum::http::header::VARY,
        axum::http::HeaderValue::from_static("accept-encoding"),
    );
    // Y compris `/pkg/snippets/**` : leur chemin sur disque n'est pas content-hashé
    // (c'est la clé d'import que le wasm déclare), mais le glue les charge sous
    // `…/auth.js?v=<empreinte>`, posée au build par `build-app`.
    //
    // Fonts immuables : un `.woff2` n'est jamais réécrit en place (changer de police =
    // nouveau fichier), donc cachable un an `immutable` sans hash.
    let fonts_dir = std::path::Path::new(&leptos_options.site_root.to_string()).join("fonts");
    let fonts_service = SetResponseHeader::overriding(
        ServeDir::new(fonts_dir),
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );

    // Libs vendorées (`/vendor`) : chaque bundle porte sa version épinglée dans son
    // nom, donc l'URL est aussi immuable que celles de `/pkg`.
    let vendor_dir = std::path::Path::new(&leptos_options.site_root.to_string()).join("vendor");
    let vendor_service = SetResponseHeader::overriding(
        ServeDir::new(vendor_dir),
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );

    // Routeur SSR Leptos : l'`AppState` est fourni au contexte de chaque requête (le
    // client SSR `lj-web` l'y lit pour appeler la couche service in-process). Le
    // fallback reçoit le même contexte : le Router Leptos matche les chemins à slash
    // final (`/codes/`) sur leurs routes (`/codes`) alors qu'axum ne les route pas,
    // donc son rendu traverse de vraies pages, qui lisent l'`AppState`. On redirige
    // d'abord ces chemins vers la forme canonique sans slash.
    let leptos_fallback = leptos_axum::file_and_error_handler_with_context(
        {
            let state = state.clone();
            move || provide_context(state.clone())
        },
        shell,
    );
    let leptos_router = Router::new()
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            {
                let state = state.clone();
                move || provide_context(state.clone())
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(
            move |uri: axum::http::Uri,
                  st: axum::extract::State<LeptosOptions>,
                  req: axum::extract::Request| {
                let leptos_fallback = leptos_fallback.clone();
                async move {
                    let path = uri.path();
                    if path.len() > 1 && path.ends_with('/') {
                        let canonical = path.trim_end_matches('/');
                        let target = match uri.query() {
                            Some(q) => format!("{canonical}?{q}"),
                            None => canonical.to_string(),
                        };
                        return axum::response::IntoResponse::into_response(
                            axum::response::Redirect::permanent(&target),
                        );
                    }
                    leptos_fallback(uri, st, req).await
                }
            },
        )
        .with_state(leptos_options)
        // Racine de trace des pages SSR ; dans l'ordre `.layer`, le dernier est le plus
        // externe. `OtelAxumLayer` ouvre le span serveur semconv (`{method} {route}`,
        // `http.response.status_code`), `body_in_request_span` l'étend au rendu du corps.
        .layer(axum::middleware::from_fn(body_in_request_span))
        .layer(OtelAxumLayer::default());

    // Fusion : routes données/MCP/OAuth (`lj-api`, sans fallback — c'est le
    // file-handler Leptos qui catch-all) + redirection `/mcp` → `/mcp/`.
    // `CompressionLayer` (br + gzip, négociés via `Accept-Encoding`) pour les réponses
    // dynamiques (HTML SSR, JSON API) ; elle saute les réponses déjà encodées.
    let app = leptos_router
        .merge(lj_api::routes::build_router(state.clone(), true))
        .route(
            "/mcp",
            axum::routing::get(|| async { axum::response::Redirect::temporary("/mcp/") }),
        )
        // Anciennes routes → pages actuelles, query préservée, en 308 : un alias est une
        // redirection HTTP, pas une page rendue.
        .route("/recherche", redirect_preserving_query("/decisions"))
        .route(
            "/recherche-decisions",
            redirect_preserving_query("/decisions"),
        )
        .route(
            "/recherche-textes",
            axum::routing::get(|raw: axum::extract::RawQuery| async move {
                let parts: Vec<String> = raw
                    .0
                    .as_deref()
                    .unwrap_or("")
                    .split('&')
                    .filter(|p| !p.is_empty())
                    .map(|p| match p.strip_prefix("source=") {
                        Some(v) => format!("origine={v}"),
                        None => p.to_string(),
                    })
                    .collect();
                let target = if parts.is_empty() {
                    "/textes".to_string()
                } else {
                    format!("/textes?{}", parts.join("&"))
                };
                axum::response::Redirect::permanent(&target)
            }),
        )
        // Activité : une route explicite par onglet sous `/activite/` ; `/activite` nu
        // redirige vers l'onglet recherches.
        .route(
            "/activite",
            redirect_preserving_query("/activite/recherches"),
        )
        .route(
            "/recherches",
            redirect_preserving_query("/activite/recherches"),
        )
        .route("/signets", redirect_preserving_query("/activite/signets"))
        .route("/lectures", redirect_preserving_query("/activite/lectures"))
        .route(
            "/codes",
            axum::routing::get(|| async { axum::response::Redirect::permanent("/normes/codes") }),
        )
        // Wildcard `/loi/*` → `/texte/*` : `uri.path()` reste percent-encodé tel quel,
        // aucune réécriture des segments.
        .route(
            "/loi/{*rest}",
            axum::routing::get(|uri: axum::http::Uri| async move {
                let path = uri.path().strip_prefix("/loi").unwrap_or(uri.path());
                let dest = match uri.query() {
                    Some(q) => format!("/texte{path}?{q}"),
                    None => format!("/texte{path}"),
                };
                axum::response::Redirect::permanent(&dest)
            }),
        )
        .nest_service("/pkg", pkg_service)
        .nest_service("/fonts", fonts_service)
        .nest_service("/vendor", vendor_service)
        .layer(CompressionLayer::new())
        .layer(axum::middleware::from_fn(html_cache_policy))
        .layer(axum::middleware::from_fn(security_headers))
        .layer(axum::middleware::from_fn(pageview_span))
        .layer(axum::middleware::from_fn(crawler_trace_policy));

    // Un seul chemin d'écoute : HTTP en clair sur l'adresse cargo-leptos, le TLS étant
    // terminé par le front `caddy`. En prod le compose pose
    // `LEPTOS_SITE_ADDR=0.0.0.0:8300` ; en dev c'est l'adresse qu'attend `cargo leptos
    // watch`.
    //
    // `axum_server` est là pour son `Handle`, qui borne le drainage : une requête en
    // vol ne peut pas retenir le process au-delà du `StopTimeout` de podman, qui
    // l'achèverait au SIGKILL — donc sans flush des spans en file.
    let handle = axum_server::Handle::new();
    let arret = handle.clone();
    tokio::spawn(async move {
        signal_arret().await;
        arret.graceful_shutdown(Some(DRAINAGE));
    });
    tracing::info!("lj-server écoute sur {leptos_addr}");
    axum_server::bind(leptos_addr)
        .handle(handle)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

/// Fenêtre laissée aux requêtes en vol après le signal d'arrêt. Tenue **sous** le
/// `StopTimeout` de podman (10 s) : le process doit sortir de lui-même pour que le
/// `Drop` de `TelemetryGuard` flushe les spans encore en file.
const DRAINAGE: std::time::Duration = std::time::Duration::from_secs(5);

/// Attend SIGTERM (`podman stop`) ou Ctrl-C (dev).
///
/// Le binaire est **PID 1** du conteneur : le noyau n'applique pas l'action par
/// défaut d'un signal à PID 1 tant qu'aucun handler n'est installé, SIGTERM serait
/// donc ignoré.
async fn signal_arret() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("handler SIGTERM");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    tracing::info!("signal d'arrêt reçu — drainage des connexions en vol");
}

/// Span `pageview` d'attribution d'acquisition : émis pour les requêtes de
/// **document** (GET, `Accept` contenant `text/html`) portant un `Referer`
/// **externe**. Les navigations internes et les crawlers (pas de Referer) sont
/// exclus ; aucune identité, seulement chemin d'atterrissage et origine.
///
/// **Une arrivée = un span** : posé après la réponse et jamais sur une
/// redirection — le navigateur conserve le `Referer` en suivant un 301, donc la
/// cible émet le sien.
///
/// **Le referer est déclaratif** : un client qui écrit `https://www.google.com/`
/// dans l'en-tête se range dans l'organique sans avoir vu Google. D'où `client` et
/// `country`, qui qualifient l'arrivée sans dire qui elle est.
async fn pageview_span(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_document = req.method() == axum::http::Method::GET
        && req
            .headers()
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"));
    let external_referer = req
        .headers()
        .get(axum::http::header::REFERER)
        .and_then(|v| v.to_str().ok())
        .filter(|r| !r.contains("librejustice.fr"))
        .map(str::to_owned);
    let Some(referer) = external_referer.filter(|_| is_document) else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_owned();
    let user_agent = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let client = client_kind(&user_agent, req.headers());
    let browser = browser_family(&user_agent);
    // Posé par Cloudflare, digne de foi pour la même raison que `CF-Connecting-IP` :
    // l'origine n'accepte que les plages Cloudflare en :443. `XX` est sa valeur pour
    // une IP non localisée.
    let country = req
        .headers()
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("XX")
        .to_owned();
    let response = next.run(req).await;
    if !response.status().is_redirection() {
        let _pageview = tracing::info_span!(
            "pageview",
            librejustice.pageview.path = %path,
            librejustice.pageview.referer = %referer,
            librejustice.pageview.client = client,
            librejustice.pageview.browser = browser,
            librejustice.pageview.country = %country,
        )
        .entered();
    }
    response
}

/// Soustrait à l'export de traces le crawl que personne n'attend.
///
/// Couche la plus externe : elle voit chaque requête, y compris celles que
/// [`pageview_span`] laisse passer sans rien poser. La ligne est celle de
/// [`lj_core::crawlers::Category::is_unattended`].
///
/// La requête part sous un span racine marqué que le sampler abandonne
/// ([`telemetry::unattended_request_span`]) ; toute la descendance suit par
/// `ParentBased`, y compris les spans nés après le retour d'ici. La décision se
/// prend donc une fois, à la création du span racine.
async fn crawler_trace_policy(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let unattended = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .and_then(lj_core::crawlers::category_of)
        .is_some_and(lj_core::crawlers::Category::is_unattended);
    if unattended {
        next.run(req)
            .instrument(telemetry::unattended_request_span())
            .await
    } else {
        next.run(req).await
    }
}

/// Étend le span de la requête au **rendu du corps** de la réponse.
///
/// `leptos_axum` répond par un `Body::from_stream(to_html_stream_out_of_order())` :
/// le future du handler se termine dès que l'en-tête est prêt, et tout le rendu —
/// donc les appels service et leurs spans `db.*` — s'exécute quand hyper poll le
/// corps.
///
/// On capture donc le span courant — celui qu'ouvre [`OtelAxumLayer`], monté juste
/// au-dessus — et on le rend courant à chaque `poll_frame`. Le corps en garde un
/// clone : le span se ferme à la fin du flux, ou plus tôt si le client raccroche et
/// que hyper droppe le corps.
async fn body_in_request_span(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let span = tracing::Span::current();
    let (parts, body) = next.run(req).await.into_parts();
    axum::response::Response::from_parts(parts, Body::new(SpannedBody { inner: body, span }))
}

/// Corps de réponse dont chaque `poll_frame` s'exécute dans `span`.
///
/// Passe-plat strict par ailleurs : la frame remontée est celle du corps enveloppé,
/// données comme trailers. `size_hint` et `is_end_stream` sont délégués — c'est ce
/// dont hyper se sert pour poser un `Content-Length` plutôt que du chunked.
struct SpannedBody {
    inner: Body,
    span: tracing::Span,
}

impl HttpBody for SpannedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        // `SpannedBody` est `Unpin` (ses deux champs le sont) : pas de projection à faire,
        // et le garde de span ne traverse aucun `await`.
        let corps = self.get_mut();
        let _entree = corps.span.enter();
        Pin::new(&mut corps.inner).poll_frame(cx)
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
}

/// Classe l'appelant d'une arrivée : `browser` | `bot` | `unknown`.
///
/// Deux signaux, du plus falsifiable au moins :
///
/// 1. le UA **quand il se déclare**. Ne remontent ici que les déplieurs de lien
///    (Slack, Facebook, Discord), qui posent un `Referer` ; la table complète de la
///    politique d'accès est [`lj_core::crawlers::CRAWLERS`].
/// 2. la cohérence de `Sec-Fetch-*`, qu'un moteur de rendu seul produit : une
///    navigation vers un document pose `Dest: document`, un client HTTP n'en pose
///    aucun, et un `fetch()` d'une autre origine en pose un autre.
///
/// `unknown` est la réponse honnête pour un client qui se présente en navigateur
/// sans le prouver — un scraper à UA plausible y tombe, mais Safari < 16.4 aussi.
fn client_kind(user_agent: &str, headers: &axum::http::HeaderMap) -> &'static str {
    if ["bot", "crawler", "spider", "facebookexternalhit"]
        .iter()
        .any(|marker| user_agent.contains(marker))
    {
        return "bot";
    }
    match headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) {
        Some("document") => "browser",
        Some(_) => "bot",
        None => "unknown",
    }
}

/// Famille du navigateur — `edge` | `chrome` | `firefox` | `safari` | `other`.
/// L'ordre des tests est imposé par l'imbrication des UA : Edge se déclare aussi
/// `chrome` et `safari`, Chrome se déclare aussi `safari`.
fn browser_family(user_agent: &str) -> &'static str {
    if user_agent.contains("edg/") {
        "edge"
    } else if user_agent.contains("chrome/") || user_agent.contains("chromium") {
        "chrome"
    } else if user_agent.contains("firefox/") {
        "firefox"
    } else if user_agent.contains("safari/") {
        "safari"
    } else {
        "other"
    }
}

/// En-têtes de sécurité posés sur **toute** réponse (documents HTML, JSON de
/// l'API, exports binaires, assets statiques).
///
/// La CSP complète du document est posée par le shell SSR, qui seul connaît le
/// `nonce` de la requête (`lj_web::app::content_security_policy`) ; on ne pose ici
/// qu'un socle pour les réponses qui n'en ont pas — API, `/pkg`, `/fonts`,
/// exports —, d'où le test `contains_key`.
///
/// `Strict-Transport-Security` posé sans condition : un UA **doit** ignorer
/// l'en-tête reçu hors transport sécurisé (RFC 6797 §8.1), donc le dev en clair et
/// le hop `caddy → app` en clair ne peuvent pas l'armer par accident. Sans
/// `includeSubDomains` ni `preload`, qui s'étendent à des sous-domaines futurs pour
/// un an sans possibilité de rétractation.
async fn security_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::header;
    use axum::http::HeaderValue;

    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    for (name, value) in [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000"),
    );
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'",
            ),
        );
    }
    resp
}

/// Politique de cache **par défaut** des documents HTML : posée uniquement sur les
/// pages qui n'ont pas déjà la leur (décision / texte / code / entité fixent 7 j de
/// CDN via `ResponseOptions`).
///
/// Une URL de page est son identité et ne peut pas être content-hashée. Deux
/// leviers, que ce header pose tous les deux : `max-age=0, must-revalidate` fait
/// revalider le navigateur à chaque navigation, et `s-maxage` laisse le CDN
/// absorber le trafic, purgé à chaque déploiement.
///
/// Sûr pour un cache partagé : le rendu SSR ne dépend d'aucune identité (la session
/// vit en `localStorage`, lue après hydratation), donc deux visiteurs reçoivent le
/// même document pour la même URL.
async fn html_cache_policy(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut resp = next.run(req).await;
    let is_html = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if is_html
        && !resp
            .headers()
            .contains_key(axum::http::header::CACHE_CONTROL)
    {
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static(
                "public, max-age=0, must-revalidate, s-maxage=300",
            ),
        );
    }
    resp
}

/// Route GET répondant un 308 vers `target`, query string préservée telle quelle.
fn redirect_preserving_query(target: &'static str) -> axum::routing::MethodRouter {
    axum::routing::get(move |raw: axum::extract::RawQuery| async move {
        let dest = match raw.0.filter(|q| !q.is_empty()) {
            Some(q) => format!("{target}?{q}"),
            None => target.to_string(),
        };
        axum::response::Redirect::permanent(&dest)
    })
}

#[cfg(test)]
mod tests {
    use super::{body_in_request_span, browser_family, client_kind, SpannedBody};
    use axum::body::{Body, Bytes, HttpBody};
    use axum::http::HeaderMap;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use tracing::Instrument;

    const SCRAPER_MOBILE: &str = "mozilla/5.0 (linux; android 10; k) applewebkit/537.36 \
        (khtml, like gecko) chrome/132.0.0.0 mobile safari/537.36";
    const APPLEBOT: &str = "mozilla/5.0 (macintosh; intel mac os x 10_15_7) \
        applewebkit/605.1.15 (khtml, like gecko) version/17.4 safari/605.1.15 (applebot/0.1; \
        +http://www.apple.com/go/applebot)";
    const YANDEX: &str = "mozilla/5.0 (compatible; yandexbot/3.0; +http://yandex.com/bots)";

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn ua_declare_prime_sur_sec_fetch() {
        let h = headers(&[("sec-fetch-dest", "document")]);
        assert_eq!(client_kind(APPLEBOT, &h), "bot");
        assert_eq!(client_kind(YANDEX, &h), "bot");
    }

    #[test]
    fn navigation_vers_un_document_est_un_navigateur() {
        let h = headers(&[
            ("sec-fetch-dest", "document"),
            ("sec-fetch-mode", "navigate"),
        ]);
        assert_eq!(client_kind(SCRAPER_MOBILE, &h), "browser");
    }

    #[test]
    fn sec_fetch_incoherent_est_un_bot() {
        let h = headers(&[("sec-fetch-dest", "empty")]);
        assert_eq!(client_kind(SCRAPER_MOBILE, &h), "bot");
    }

    #[test]
    fn sans_sec_fetch_on_ne_sait_pas() {
        assert_eq!(client_kind(SCRAPER_MOBILE, &HeaderMap::new()), "unknown");
        assert_eq!(client_kind("", &HeaderMap::new()), "unknown");
    }

    #[test]
    fn familles_dans_l_ordre_d_imbrication() {
        assert_eq!(
            browser_family(
                "mozilla/5.0 (windows nt 10.0; win64; x64) applewebkit/537.36 (khtml, like \
                 gecko) chrome/133.0.0.0 safari/537.36 edg/133.0.0.0"
            ),
            "edge"
        );
        assert_eq!(browser_family(SCRAPER_MOBILE), "chrome");
        assert_eq!(
            browser_family(
                "mozilla/5.0 (macintosh; intel mac os x 10_15_7) applewebkit/605.1.15 (khtml, \
                 like gecko) version/17.4 safari/605.1.15"
            ),
            "safari"
        );
        assert_eq!(
            browser_family(
                "mozilla/5.0 (x11; linux x86_64; rv:127.0) gecko/20100101 firefox/127.0"
            ),
            "firefox"
        );
        assert_eq!(browser_family("guzzlehttp/7"), "other");
    }

    /// Couche `tracing` d'observation : enregistre les fermetures de spans, dans
    /// l'ordre.
    struct Fermetures(Arc<Mutex<Vec<tracing::Id>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Fermetures {
        fn on_close(&self, id: tracing::Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            self.0.lock().unwrap().push(id);
        }
    }

    /// Ce que chaque `poll_frame` du corps relève : le span courant, et le
    /// nombre de spans déjà refermés à cet instant.
    type Observations = Arc<Mutex<Vec<(Option<tracing::Id>, usize)>>>;

    /// Spec du rendu SSR : le corps streamé est poll DANS le span de sa requête, et ce
    /// span reste ouvert jusqu'à la fin du flux.
    #[tokio::test]
    async fn le_corps_streame_est_poll_dans_le_span_de_requete() {
        use axum::extract::Request;
        use axum::Router;
        use tower::ServiceExt;
        use tracing_subscriber::layer::SubscriberExt;

        let fermetures: Arc<Mutex<Vec<tracing::Id>>> = Arc::default();
        let _abonne = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(Fermetures(fermetures.clone())),
        );

        let observe: Observations = Arc::default();
        let span_requete: Arc<Mutex<Option<tracing::Id>>> = Arc::default();

        let corps = {
            let observe = observe.clone();
            let fermetures = fermetures.clone();
            move || {
                let (observe, fermetures) = (observe.clone(), fermetures.clone());
                let mut restant = 3;
                async move {
                    Body::from_stream(futures::stream::poll_fn(move |_cx| {
                        observe.lock().unwrap().push((
                            tracing::Span::current().id(),
                            fermetures.lock().unwrap().len(),
                        ));
                        restant -= 1;
                        if restant > 0 {
                            Poll::Ready(Some(Ok::<_, std::io::Error>("<p>fragment</p>")))
                        } else {
                            Poll::Ready(None)
                        }
                    }))
                }
            }
        };

        // Tient la place d'`OtelAxumLayer`, qui ouvre le span serveur puis l'abandonne dès
        // la réponse rendue.
        let racine = {
            let span_requete = span_requete.clone();
            axum::middleware::from_fn(move |req: Request, next: axum::middleware::Next| {
                let span_requete = span_requete.clone();
                async move {
                    let span = tracing::info_span!("requete");
                    *span_requete.lock().unwrap() = span.id();
                    next.run(req).instrument(span).await
                }
            })
        };

        let app = Router::new()
            .route("/", axum::routing::get(corps))
            .layer(axum::middleware::from_fn(body_in_request_span))
            .layer(racine);

        let reponse = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let octets = axum::body::to_bytes(reponse.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(&octets[..], b"<p>fragment</p><p>fragment</p>");

        let attendu = span_requete.lock().unwrap().clone();
        assert!(attendu.is_some(), "le span de requête doit avoir un id");
        let observe = observe.lock().unwrap().clone();
        assert_eq!(observe.len(), 3, "trois polls : deux fragments puis la fin");
        for (courant, deja_fermes) in observe {
            assert_eq!(courant, attendu, "poll du corps hors du span de requête");
            assert_eq!(deja_fermes, 0, "span refermé avant la fin du flux");
        }
        assert_eq!(*fermetures.lock().unwrap(), vec![attendu.unwrap()]);
    }

    /// Corps de test à deux frames : une donnée, puis des trailers.
    struct CorpsAvecTrailers(u8);

    impl HttpBody for CorpsAvecTrailers {
        type Data = Bytes;
        type Error = axum::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            self.0 += 1;
            match self.0 {
                1 => Poll::Ready(Some(Ok(http_body::Frame::data(Bytes::from_static(b"abc"))))),
                2 => {
                    let mut trailers = HeaderMap::new();
                    trailers.insert("x-somme", "42".parse().unwrap());
                    Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers))))
                }
                _ => Poll::Ready(None),
            }
        }
    }

    /// Frame suivante d'un corps, poll à la main.
    async fn frame(
        corps: &mut SpannedBody,
    ) -> Option<Result<http_body::Frame<Bytes>, axum::Error>> {
        std::future::poll_fn(|cx| Pin::new(&mut *corps).poll_frame(cx)).await
    }

    /// Spec du passe-plat : envelopper le corps ne coûte ni une frame ni un en-tête de
    /// fin.
    #[tokio::test]
    async fn le_corps_enveloppe_reste_un_passe_plat() {
        let mut corps = SpannedBody {
            inner: Body::new(CorpsAvecTrailers(0)),
            span: tracing::Span::none(),
        };
        let donnee = frame(&mut corps).await.unwrap().unwrap();
        assert_eq!(donnee.into_data().unwrap(), Bytes::from_static(b"abc"));
        let fin = frame(&mut corps).await.unwrap().unwrap();
        assert_eq!(fin.into_trailers().unwrap()["x-somme"], "42");
        assert!(frame(&mut corps).await.is_none(), "fin de flux");

        let fixe = SpannedBody {
            inner: Body::from("abc"),
            span: tracing::Span::none(),
        };
        assert_eq!(fixe.size_hint().exact(), Some(3));
        assert!(!fixe.is_end_stream());
    }
}
