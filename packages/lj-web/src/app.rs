//! Shell HTML + composant racine `App` + table de routage.

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, Title};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::{ParamSegment, SsrMode, StaticSegment};

use crate::components::AppShell;
use crate::pages::{
    ActivityPage, AnnoncesCataloguePage, AnnoncesDepartementPage, AnnoncesPage,
    AnnuaireDirectoryPage, AnnuairePage, AuthorizeMcpPage, CodeCataloguePage, Confidentialite,
    ConfirmPage, DecisionPage, EntityPage, JuridictionHubPage, JuridictionYearPage,
    JuridictionsPage, Landing, LawArticlePage, LawCodePage, LawComparePage, LawSectionPage,
    LoginPage, MarchePage, McpGuide, MentionsLegales, NormFondPage, NormYearPage, NormesPage,
    NotFound, ProfilePage, ResetPasswordPage, SearchPage, SourcesPage, TextesPage,
};
use crate::seo::site_default;

/// Script anti-FOUC. Applique `.dark` avant
/// le premier paint : anonyme (pas de `lj-auth=1`) => toujours light ; connecte
/// => `lj-theme` sinon `prefers-color-scheme`. Pose AVANT
/// HydrationScripts/MetaTags pour s'executer avant l'hydratation.
#[cfg(feature = "ssr")]
const ANTI_FOUC: &str = r#"(() => {
  try {
    if (localStorage.getItem("lj-auth") !== "1") return;
    const v = localStorage.getItem("lj-theme");
    const dark =
      v === "dark" ||
      (v !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
    if (dark) document.documentElement.classList.add("dark");
  } catch {}
})();"#;

/// Sonde d'hydratation. Tout le contenu interactif du site dépend du bundle
/// wasm : quand il ne charge pas, le bootstrap Leptos — un `import().then()`
/// sans `catch` — meurt sans un mot et l'utilisateur reste sur un rendu SSR
/// inerte, sans que rien ne remonte côté serveur. Ce script pose le seul témoin
/// possible : si `__LJ_HYDRATED` (posé par `hydrate()`) n'apparaît pas, il
/// affiche un recours et signale la panne.
///
/// Déclenchement sur la première des deux conditions : un asset `/pkg` répondu
/// en erreur, ou 30 s écoulées sans hydratation. Aucune identité transmise :
/// chemin, assets en échec, temps écoulé.
#[cfg(feature = "ssr")]
const HYDRATION_WATCHDOG: &str = r#"(() => {
  const failedAssets = () =>
    performance
      .getEntriesByType("resource")
      .filter((r) => r.name.includes("/pkg/") && r.responseStatus >= 400)
      .map((r) => r.name.split("/").pop() + ":" + r.responseStatus);

  const report = (assets) => {
    try {
      const body = JSON.stringify({
        path: location.pathname,
        assets,
        elapsed_ms: Math.round(performance.now()),
      });
      navigator.sendBeacon(
        "/api/client/hydration-failure",
        new Blob([body], { type: "application/json" }),
      );
    } catch {}
  };

  const offerReload = () => {
    const bar = document.createElement("div");
    bar.setAttribute("role", "alert");
    bar.style.cssText =
      "position:fixed;left:0;right:0;bottom:0;z-index:9999;display:flex;flex-wrap:wrap;" +
      "gap:1rem;align-items:center;justify-content:center;padding:.75rem 1rem;" +
      "background:#1B1612;color:#FBF9F4;font:14px/1.4 system-ui,sans-serif";
    const text = document.createElement("span");
    text.textContent = "Cette page n'a pas pu charger entièrement.";
    const button = document.createElement("button");
    button.textContent = "Recharger";
    button.style.cssText =
      "padding:.35rem .9rem;border:1px solid currentColor;border-radius:4px;" +
      "background:transparent;color:inherit;cursor:pointer;font:inherit";
    button.onclick = () => location.reload();
    bar.append(text, button);
    document.body.appendChild(bar);
  };

  const check = (elapsed) => {
    if (window.__LJ_HYDRATED) return;
    const assets = failedAssets();
    if (assets.length === 0 && elapsed < 30000) {
      setTimeout(() => check(elapsed + 5000), 5000);
      return;
    }
    report(assets);
    offerReload();
  };

  setTimeout(() => check(5000), 5000);
})();"#;

/// Module d'auth Supabase vendore, importe par [`auth_bootstrap_script`]. Seul
/// endroit du code qui nomme ce fichier ; `/vendor` etant servi `immutable`, la
/// version vit dans le nom, et monter de version = deposer le bundle sous son
/// nouveau nom et corriger cette constante.
#[cfg(feature = "ssr")]
const VENDOR_AUTH_MODULE: &str = "/vendor/supabase-auth-js-2.107.0.js";

/// Amorcage de la session Supabase, des le `<head>`.
///
/// Construit l'`AuthClient` et lance `getSession()` sans attendre l'hydratation.
/// Script CLASSIQUE, donc execute pendant le parsing, la ou un `<script
/// type="module">` serait differe jusqu'a la fin du document : c'est toute la
/// raison de sa forme. `api/client.rs` attend le jeton avant de construire
/// CHAQUE requete `/api`, donc ce rafraichissement est sur le chemin critique de
/// toutes les donnees de la page.
///
/// Le client est parke sur `window.__LJ_AUTH__` (promesse) et le shim l'adopte au
/// lieu d'en construire un second : deux instances sur la meme `storageKey` se
/// disputeraient le meme refresh token. Les options du client ne sont donc
/// ecrites qu'ICI, et nulle part dans `js/auth.js`.
///
/// Les placeholders reproduisent le comportement du shim quand la config manque
/// (auth inerte) ; `getSession()` ne part alors pas, faute de destination reelle.
#[cfg(feature = "ssr")]
fn auth_bootstrap_script() -> String {
    format!(
        r#"(() => {{
  const c = window.__LJ_SUPABASE__ || {{}};
  const url = c.url || "http://placeholder.local";
  const anonKey = c.anonKey || "placeholder";
  const ref = (url.match(/^https?:\/\/([^.]+)/) || [])[1] || "default";
  window.__LJ_AUTH_STORAGE_KEY__ = `sb-${{ref}}-auth-token`;
  window.__LJ_AUTH__ = import("{VENDOR_AUTH_MODULE}").then(({{ AuthClient }}) => {{
    const client = new AuthClient({{
      url: `${{url}}/auth/v1`,
      headers: {{ Authorization: `Bearer ${{anonKey}}`, apikey: anonKey }},
      storageKey: window.__LJ_AUTH_STORAGE_KEY__,
      autoRefreshToken: true,
      persistSession: true,
      detectSessionInUrl: true,
      flowType: "pkce",
    }});
    if (c.url && c.anonKey) client.getSession();
    return client;
  }});
}})();"#
    )
}

/// Config Supabase injectee dans la page pour le shim `js/auth.js` (qui ne peut
/// pas lire `import.meta.env`). Lue via `Settings` ; vide si non configuree =>
/// auth inerte cote shim. `serde_json` echappe les valeurs.
#[cfg(feature = "ssr")]
fn supabase_config_script() -> String {
    let settings = crate::config::Settings::from_env();
    let payload = serde_json::json!({
        "url": settings.supabase_url,
        "anonKey": settings.supabase_anon_key,
    });
    format!("window.__LJ_SUPABASE__={payload};")
}

/// Content-Security-Policy du document HTML.
///
/// Forme « strict CSP » : les scripts inline ne s'exécutent que sous le `nonce`
/// de la requête, celui que `leptos_axum` provisionne et que `HydrationScripts`
/// pose déjà sur le bootstrap d'hydratation. Les trois scripts inline du `<head>`
/// portent le même. `frame-ancestors 'none'` interdit d'encadrer
/// `/authorize-mcp`, dont un clic de consentement piégé accorderait l'accès MCP
/// au compte de la victime.
///
/// Pas de `default-src` : seules sont posées les directives dont l'inventaire
/// des ressources est vérifié (tout est same-origin — fonts, `/vendor`, `/pkg`).
///
/// - `'wasm-unsafe-eval'` : sans lui le navigateur refuse de compiler le bundle
///   `hydrate`.
/// - `script-src 'self'` couvre `/cdn-cgi/challenge-platform/` (JS detections
///   Cloudflare) ; leur script inline reçoit le nonce, que Cloudflare recopie en
///   lisant cet en-tête.
/// - `style-src 'unsafe-inline'` : le rendu émet des attributs `style=`.
///
/// Sur une page cachée, l'en-tête voyage avec le corps dans le cache CDN : le
/// document reste cohérent, mais son `nonce` est public. Ce qui protège alors,
/// et qu'aucun cache n'affaiblit : `script-src 'self'`, `object-src`,
/// `base-uri`, `form-action` et `frame-ancestors`.
#[cfg(feature = "ssr")]
fn content_security_policy(nonce: Option<&str>) -> String {
    let settings = crate::config::Settings::from_env();
    let supabase = settings.supabase_url;
    // Origine seule (`https://hôte`) : `connect-src` matche par origine.
    let supabase_origin = supabase
        .split_once("://")
        .and_then(|(scheme, rest)| {
            rest.split('/')
                .next()
                .map(|host| format!("{scheme}://{host}"))
        })
        .filter(|o| !o.ends_with("://"))
        .unwrap_or_default();

    let script_nonce = match nonce {
        Some(n) => format!(" 'nonce-{n}'"),
        None => String::new(),
    };
    let mut connect_extra = String::new();
    if !supabase_origin.is_empty() {
        connect_extra.push(' ');
        connect_extra.push_str(&supabase_origin);
    }
    if settings.watch {
        connect_extra.push_str(" ws://localhost:* ws://127.0.0.1:*");
    }

    [
        format!("script-src 'self' 'wasm-unsafe-eval'{script_nonce}"),
        "style-src 'self' 'unsafe-inline'".to_string(),
        "img-src 'self' data:".to_string(),
        "font-src 'self'".to_string(),
        "media-src 'self'".to_string(),
        format!("connect-src 'self'{connect_extra}"),
        "object-src 'none'".to_string(),
        "base-uri 'self'".to_string(),
        "form-action 'self'".to_string(),
        "frame-ancestors 'none'".to_string(),
    ]
    .join("; ")
}

/// Document HTML emis par le serveur. cargo-leptos l'utilise pour le SSR et le
/// fallback statique. Aucun `Cache-Control` pose ici (seul le Content-Type
/// text/html, gere par leptos_axum).
#[cfg(feature = "ssr")]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    use leptos_meta::{HashedStylesheet, MetaTags};

    // Nonce de la requête (provisionné par `leptos_axum`) : l'en-tête et les
    // scripts inline ci-dessous doivent venir du même tirage, d'où la lecture
    // unique ici.
    use axum::http::{header::CONTENT_SECURITY_POLICY, HeaderValue};
    let nonce = leptos::nonce::use_nonce().map(|n| n.to_string());
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        if let Ok(hv) = HeaderValue::from_str(&content_security_policy(nonce.as_deref())) {
            resp.insert_header(CONTENT_SECURITY_POLICY, hv);
        }
    }

    view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1.0" />
                <meta name="color-scheme" content="light dark" />
                <meta
                    name="theme-color"
                    content="#FBF9F4"
                    media="(prefers-color-scheme: light)"
                />
                <meta
                    name="theme-color"
                    content="#1B1612"
                    media="(prefers-color-scheme: dark)"
                />
                // Set complet : Google sonde /favicon.ico à la racine et veut un
                // raster ≥ 48 px ; l'ICO 16/32/48 + le PNG 192 servent de repli
                // quand le SVG n'est pas repris. apple-touch-icon pour iOS.
                <link rel="icon" href="/favicon.ico" sizes="48x48" />
                <link rel="icon" href="/icon-192.png" type="image/png" sizes="192x192" />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml" sizes="any" />
                <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
                // Précharge la police du LCP (Geist latin, h1 du hero) en
                // parallèle du CSS plutôt qu'après son parsing. `crossorigin`
                // obligatoire : les fonts sont fetchées en mode CORS, sinon le
                // preload ne matche pas la requête réelle.
                <link
                    rel="preload"
                    href="/fonts/geist-latin-wght-normal.woff2"
                    r#as="font"
                    type="font/woff2"
                    crossorigin="anonymous"
                />
                <script nonce=nonce.clone() inner_html=supabase_config_script()></script>
                <script nonce=nonce.clone() inner_html=auth_bootstrap_script()></script>
                <script nonce=nonce.clone() inner_html=ANTI_FOUC></script>
                <script nonce=nonce.clone() inner_html=HYDRATION_WATCHDOG></script>
                <AutoReload options=options.clone() />
                <HydrationScripts options=options.clone() />
                <HashedStylesheet options id="leptos" />
                <MetaTags />
            </head>
            <body class="antialiased">
                <App />
            </body>
        </html>
    }
}

/// Composant racine : contexte meta, cache de requetes, meta generiques, Router
/// + AppShell.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    crate::query::provide_query_client();
    crate::components::search::compact_search::search_data::provide_seen_search_keys();
    crate::components::client_only::provide_hydrated();
    crate::auth::provide_auth_state();
    provide_auth_runtime();
    let default = site_default();

    view! {
        <Title text=default.title />
        // `<Meta name="description">` generique : pose par `AppShell`, dans le
        // Router, pour que `/decision/*` puisse emettre la sienne a la place (la
        // route remplace la meta racine, jamais deux balises description).
        <Router>
            <AppShell>
                <Routes fallback=NotFound>
                    // ── Pages publiques (SSR streaming par defaut, OutOfOrder) ──
                    <Route path=StaticSegment("") view=Landing />
                    // ── Recherche : `/decisions` (hybride BM25+ANN) et `/textes` (BM25
                    // seul), moteurs et filtres propres, aucune recherche transverse. ──
                    <Route path=StaticSegment("decisions") view=SearchPage />
                    <Route path=StaticSegment("textes") view=TextesPage />
                    <Route path=StaticSegment("sources") view=SourcesPage />
                    // ── Annonces : la publicité légale en cours, plusieurs fonds derrière
                    // un même DTO (marchés publics en base, BODACC en proxy) ; les pages
                    // `departement` restent du SSR bloquant, pour le crawl. ──
                    <Route path=StaticSegment("annonces") view=AnnoncesPage />
                    <Route
                        path=(StaticSegment("annonces"), StaticSegment("departement"))
                        view=AnnoncesCataloguePage
                    />
                    <Route
                        path=(
                            StaticSegment("annonces"),
                            StaticSegment("departement"),
                            ParamSegment("code"),
                        )
                        view=AnnoncesDepartementPage
                    />
                    <Route path=(StaticSegment("marche"), ParamSegment("uid")) view=MarchePage />
                    // ── Decision : similaires en streaming (PartiallyBlocked :
                    // le bloc bloquant est rendu cote serveur, sans JS). ──
                    <Route
                        path=(StaticSegment("decision"), ParamSegment("id"))
                        view=DecisionPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Fiche entité : en-tête et agrégats bloquants SSR, liste de
                    // décisions citantes streamée et paginée par `?page=`. Rendu adapté au
                    // namespace (`siren`/`rna`/`cnb`/`oacc`). ──
                    <Route
                        path=(StaticSegment("entite"), ParamSegment("ns"), ParamSegment("id"))
                        view=EntityPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Annuaire des entités : accueil et listing paginé par catégorie,
                    // bloquants SSR ; compteurs des cartes streamés. ──
                    <Route
                        path=StaticSegment("annuaire")
                        view=AnnuairePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    <Route
                        path=(StaticSegment("annuaire"), ParamSegment("kind"))
                        view=AnnuaireDirectoryPage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Hubs juridiction : catalogue par famille, hub d'une juridiction
                    // (années), liste paginée d'une année. SSR bloquant — le contenu est
                    // fait de liens crawlables. ──
                    <Route path=StaticSegment("juridictions") view=JuridictionsPage />
                    <Route
                        path=(StaticSegment("juridiction"), ParamSegment("code"))
                        view=JuridictionHubPage
                    />
                    <Route
                        path=(
                            StaticSegment("juridiction"),
                            ParamSegment("code"),
                            ParamSegment("annee"),
                        )
                        view=JuridictionYearPage
                    />
                    // ── Hubs normes : même arborescence vers les textes, l'année pouvant
                    // être `sans-date`. ──
                    <Route path=StaticSegment("normes") view=NormesPage />
                    // Le fond `codes` a son propre catalogue (sommaire par
                    // code, pas un hub année) — segment statique prioritaire
                    // sur la capture `{fond}`.
                    <Route
                        path=(StaticSegment("normes"), StaticSegment("codes"))
                        view=CodeCataloguePage
                    />
                    <Route
                        path=(StaticSegment("normes"), ParamSegment("fond"))
                        view=NormFondPage
                    />
                    <Route
                        path=(
                            StaticSegment("normes"),
                            ParamSegment("fond"),
                            ParamSegment("annee"),
                        )
                        view=NormYearPage
                    />
                    // ── Référentiel LEGI : sommaire de code, article en vigueur, article
                    // à une date (route séparée, pas un segment optionnel). L'article est
                    // bloquant SSR, les décisions citantes sont streamées. ──
                    <Route path=(StaticSegment("texte"), ParamSegment("code")) view=LawCodePage />
                    // Vue-lecture d'une section : segment littéral `section`, déclaré
                    // avant la capture `{num}`.
                    <Route
                        path=(
                            StaticSegment("texte"),
                            ParamSegment("code"),
                            StaticSegment("section"),
                            ParamSegment("cid"),
                        )
                        view=LawSectionPage
                    />
                    <Route
                        path=(StaticSegment("texte"), ParamSegment("code"), ParamSegment("num"))
                        view=LawArticlePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    <Route
                        path=(
                            StaticSegment("texte"),
                            ParamSegment("code"),
                            ParamSegment("num"),
                            ParamSegment("date"),
                        )
                        view=LawArticlePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // Comparateur de versions : diff serveur entre deux rédactions,
                    // bornes = dates de fenêtre de version.
                    <Route
                        path=(
                            StaticSegment("texte"),
                            ParamSegment("code"),
                            ParamSegment("num"),
                            StaticSegment("comparer"),
                            ParamSegment("de"),
                            ParamSegment("a"),
                        )
                        view=LawComparePage
                        ssr=SsrMode::PartiallyBlocked
                    />
                    // ── Pages auth client-side : rendu SSR du shell, decision
                    // de session/contenu cote client (AuthGuard / formulaires). ──
                    <Route path=StaticSegment("connexion") view=LoginPage />
                    <Route
                        path=StaticSegment("reinitialiser-mot-de-passe")
                        view=ResetPasswordPage
                    />
                    <Route path=StaticSegment("confirmer") view=ConfirmPage />
                    <Route path=StaticSegment("authorize-mcp") view=AuthorizeMcpPage />
                    <Route path=StaticSegment("profil") view=ProfilePage />
                    // ── Activite : meme composant, une route explicite par onglet sous
                    // `/activite/`, defaut compris. `/activite` nu = 308 vers l'onglet
                    // recherches. ──
                    <Route
                        path=(StaticSegment("activite"), StaticSegment("recherches"))
                        view=ActivityPage
                    />
                    <Route
                        path=(StaticSegment("activite"), StaticSegment("signets"))
                        view=ActivityPage
                    />
                    <Route
                        path=(StaticSegment("activite"), StaticSegment("lectures"))
                        view=ActivityPage
                    />
                    // ── Pages statiques publiques ──
                    <Route path=StaticSegment("mcp-guide") view=McpGuide />
                    <Route path=StaticSegment("mentions-legales") view=MentionsLegales />
                    <Route path=StaticSegment("confidentialite") view=Confidentialite />
                    // Pas de route catch-all : un `WildcardSegment` masquerait les assets
                    // statiques (`/pkg/*`, `/favicon.svg`) qui doivent atteindre le
                    // `file_and_error_handler`. Le no-match est couvert par
                    // `fallback=NotFound`, qui pose lui-même le 404.
                </Routes>
            </AppShell>
        </Router>
    }
}

/// Pilote unique de l'etat d'auth cote client. Une seule lecture de session +
/// un seul abonnement `onAuthStateChange` alimentent les trois consommateurs :
///   1. le signal `email` de l'`AuthState` (avatar + menu compte de la top-bar) ;
///   2. l'invalidation du cache de requetes sur changement REEL d'identite
///      (connexion, deconnexion, bascule de compte) ;
///   3. le flag `lj-auth` + le theme (cf. ANTI_FOUC : sombre reserve aux
///      connectes, anonyme => toujours clair).
///
/// La sync n'a lieu qu'une fois la session connue (jamais sur un etat « pas
/// encore charge »), donc aucun flash clair→sombre au reload d'un connecte.
#[cfg(feature = "hydrate")]
fn provide_auth_runtime() {
    use crate::components::profile::sync_auth_theme;
    use leptos_fetch::QueryClient;

    let email = crate::auth::use_auth().email;
    let client = expect_context::<QueryClient>();
    // Derniere identite RESOLUE : `None` = jamais lue encore, `Some(None)` =
    // anonyme, `Some(Some(mail))` = connecte. Porte la decision de vider le cache,
    // qui se deduit de la donnee et non du site d'appel : supabase-auth-js emet
    // `INITIAL_SESSION` a l'abonnement (session ou pas) puis `TOKEN_REFRESHED`
    // toutes les ~50 min, evenements qui ne changent aucune identite.
    let known: StoredValue<Option<Option<String>>> = StoredValue::new(None);

    // Applique l'etat de session courant, et ne vide le cache que sur un vrai
    // changement d'identite (connexion, deconnexion, bascule de compte). Le contenu
    // cache (recherche) ne depend pas de l'identite ; ce qui en depend est
    // l'enregistrement serveur dans `user_search_history`, d'ou l'eviction pour que
    // la recherche suivante reparte authentifiee.
    let apply = move || {
        leptos::task::spawn_local(async move {
            let mail = crate::auth::current_email().await;
            let authed = mail.is_some();
            let changed = matches!(known.get_value(), Some(previous) if previous != mail);
            known.set_value(Some(mail.clone()));
            email.set(mail);
            if changed {
                client.clear();
            }
            sync_auth_theme(authed);
        });
    };

    apply();
    // Abonnement permanent : le garde n'est ni `Send` ni `Sync` (il tient un
    // `Closure` JS), inconfiable a un cleanup d'Owner (`Send + Sync`) ; sur wasm
    // mono-thread, `forget` traduit « abonnement vivant le temps de la page ».
    let subscription = crate::auth::on_auth_state_change(move |_event| apply());
    std::mem::forget(subscription);
}

/// No-op SSR : pas de session locale ni d'evenement d'auth cote serveur.
#[cfg(feature = "ssr")]
fn provide_auth_runtime() {}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::{auth_bootstrap_script, VENDOR_AUTH_MODULE};

    #[test]
    fn vendor_auth_module_exists_in_public() {
        let path = format!("{}/public{VENDOR_AUTH_MODULE}", env!("CARGO_MANIFEST_DIR"));
        assert!(
            std::path::Path::new(&path).is_file(),
            "{VENDOR_AUTH_MODULE} ne designe aucun fichier ({path}) — \
             montee de version sans depot du bundle"
        );
    }

    #[test]
    fn bootstrap_imports_the_vendor_module() {
        assert!(
            auth_bootstrap_script().contains(&format!("import(\"{VENDOR_AUTH_MODULE}\")")),
            "l'amorcage du <head> n'importe pas {VENDOR_AUTH_MODULE}"
        );
    }
}
