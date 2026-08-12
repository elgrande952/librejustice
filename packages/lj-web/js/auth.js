// Shim d'authentification borne + helpers navigateur (port de
// apps/web/src/lib/supabase.ts et de divers effets DOM des pages auth). Un seul
// module pour toutes les ops auth du front.
//
// Bundle par wasm-bindgen (`#[wasm_bindgen(module = "/js/auth.js")]`) : ce
// fichier devient un "snippet" inline dans le bundle wasm, charge par le
// glue-code JS de cargo-leptos. Il N'a PAS acces a `import.meta.env` (pas de
// passe Vite) — la config Supabase est injectee au SSR dans
// `window.__LJ_SUPABASE__` (cf. app.rs::shell), avec fallback sur des
// placeholders (auth inerte).
//
// Ce shim ne CONSTRUIT plus l'AuthClient : il adopte celui qu'amorce le `<head>`
// (`app.rs::auth_bootstrap_script`), parke en promesse sur
// `window.__LJ_AUTH__`. Le `<head>` s'execute pendant le parsing, donc
// `getSession()` — et son aller-retour de rafraichissement — partent pendant le
// transfert du bundle WASM au lieu de l'attendre. Deux instances sur la meme
// `storageKey` se disputeraient le meme refresh token, d'ou l'adoption plutot
// qu'une seconde construction ; et les options du client ne s'ecrivent qu'a un
// seul endroit, le `<head>`.
//
// Bindings cote Rust : `src/auth/bindings.rs` (substrate auth) et
// `src/pages/browser.rs` (ops auth page-level + helpers DOM).

function config() {
  const c = (typeof window !== "undefined" && window.__LJ_SUPABASE__) || {};
  return { url: c.url || "", anonKey: c.anonKey || "" };
}

// Client amorce par le `<head>`. Attendu, pas construit : au premier appel du
// shim l'import du module vendore est deja parti depuis longtemps, donc l'attente
// est nulle en pratique.
function authClient() {
  return window.__LJ_AUTH__;
}

// `storageKey` de l'AuthClient, calculee par l'amorcage : "sb-<projectref>-auth-token",
// celle de createClient(supabase-js), pour preserver les sessions existantes.
function storageKey() {
  return window.__LJ_AUTH_STORAGE_KEY__;
}

function configured() {
  const { url, anonKey } = config();
  return Boolean(url && anonKey);
}

// --- API auth (bindee dans src/auth/bindings.rs + src/pages/browser.rs) ------

// Plafond d'attente du jeton. `getSession()` declenche un aller-retour reseau
// vers Supabase quand l'access token doit etre rafraichi, et il est attendu AVANT
// la construction de CHAQUE requete /api : s'il traine, ce n'est pas une requete
// qui gele mais toutes les ressources de la page a la fois, sans qu'aucun octet
// n'atteigne le serveur (donc sans la moindre trace cote prod).
const TOKEN_DEADLINE_MS = 5000;

// Jeton d'acces de la session courante, ou null (anonyme / non configure / SSR).
// Au-dela de `TOKEN_DEADLINE_MS` on renvoie null : la requete part en anonyme
// plutot que de bloquer la page, et la session revient d'elle-meme des que le
// rafraichissement aboutit.
export async function getAccessToken() {
  if (typeof window === "undefined" || !configured()) {
    return null;
  }
  let timer;
  const deadline = new Promise((resolve) => {
    timer = setTimeout(() => {
      console.warn(
        `[librejustice] getSession() sans reponse en ${TOKEN_DEADLINE_MS} ms : requete envoyee en anonyme`,
      );
      resolve(null);
    }, TOKEN_DEADLINE_MS);
  });
  try {
    const session = await Promise.race([
      authClient()
        .then((client) => client.getSession())
        .then(({ data }) => data.session),
      deadline,
    ]);
    return session?.access_token ?? null;
  } finally {
    clearTimeout(timer);
  }
}

// `true` si une session vit dans le storage local. Lecture SYNCHRONE, sans
// reseau : contrairement a `getAccessToken()`, elle ne peut pas etre victime du
// `TOKEN_DEADLINE_MS`. Sert a distinguer, quand le jeton manque, le visiteur
// reellement deloggé du connecte degrade (rafraichissement Supabase qui
// n'aboutit pas) — le client HTTP en fait un header, le serveur un attribut de
// span. Sans ce temoin les deux cas sont indiscernables cote prod.
export function sessionInStorage() {
  if (typeof window === "undefined" || !configured()) {
    return false;
  }
  return Boolean(window.localStorage.getItem(storageKey()));
}

// Email de l'utilisateur de la session courante, ou null (anonyme / non
// configure / SSR). Utilise par l'etat d'auth reactif de la top-bar.
export async function getUserEmail() {
  if (typeof window === "undefined" || !configured()) {
    return null;
  }
  const client = await authClient();
  const {
    data: { session },
  } = await client.getSession();
  return session?.user?.email ?? null;
}

// `true` si une session existe (utilise par l'AuthGuard).
export async function hasSession() {
  if (typeof window === "undefined" || !configured()) {
    return false;
  }
  const client = await authClient();
  const {
    data: { session },
  } = await client.getSession();
  return Boolean(session);
}

// Connexion email/mot de passe. Rejette avec l'AuthError supabase brut.
export async function signInPassword(email, password) {
  const client = await authClient();
  const { error } = await client.signInWithPassword({ email, password });
  if (error) {
    throw error;
  }
}

// Connexion OAuth (PKCE). `provider` ex. "google". `redirectTo` URL de retour.
export async function signInOauth(provider, redirectTo) {
  const client = await authClient();
  const { error } = await client.signInWithOAuth({
    provider,
    options: { redirectTo },
  });
  if (error) {
    throw error;
  }
}

// Inscription email/mot de passe. `redirectTo` = lien de confirmation.
export async function signUp(email, password, redirectTo) {
  const client = await authClient();
  const { error } = await client.signUp({
    email,
    password,
    options: { emailRedirectTo: redirectTo },
  });
  if (error) {
    throw error;
  }
}

// Demande de reinitialisation de mot de passe (email de reset).
export async function resetPassword(email, redirectTo) {
  const client = await authClient();
  const { error } = await client.resetPasswordForEmail(email, { redirectTo });
  if (error) {
    throw error;
  }
}

// Echange d'un lien de mail (`token_hash`) contre une session. `type` : signup |
// magiclink | invite | email_change | recovery — "recovery" emet
// PASSWORD_RECOVERY au lieu de SIGNED_IN.
export async function verifyEmailOtp(tokenHash, type) {
  const client = await authClient();
  const { error } = await client.verifyOtp({ token_hash: tokenHash, type });
  if (error) {
    throw error;
  }
}

// Deconnexion de CE navigateur.
//
// `scope: "local"` explicitement : le defaut d'auth-js est "global", qui revoque
// les refresh tokens de TOUTES les sessions de l'utilisateur. Un clic sur
// Deconnexion tuait donc aussi sa session sur son telephone et sur son autre
// poste, ce que personne n'attend d'un bouton de deconnexion. Une action
// « me deconnecter partout », si elle arrive un jour, passera "global" — c'est a
// elle de le demander, pas au cas courant de le subir.
//
// Sans effet sur les autorisations MCP : elles vivent dans nos tables
// `mcp_tokens` / `mcp_refresh_tokens`, verifiees par lookup direct sans passer
// par Supabase. Retirer sa session web ne coupe pas le connecteur Claude.ai, et
// c'est voulu — ce sont deux cycles de vie distincts.
export async function signOut() {
  const client = await authClient();
  await client.signOut({ scope: "local" });
}

// Renvoi de l'email de confirmation (type "signup"). Rejette avec l'AuthError.
export async function resendSignup(email) {
  const client = await authClient();
  const { error } = await client.resend({ type: "signup", email });
  if (error) {
    throw error;
  }
}

// Mise a jour du mot de passe de l'utilisateur courant. Rejette avec l'AuthError.
export async function updatePassword(password) {
  const client = await authClient();
  const { error } = await client.updateUser({ password });
  if (error) {
    throw error;
  }
}

// Abonnement aux changements d'etat d'auth. `cb` est appele a chaque evenement
// (SIGNED_IN / SIGNED_OUT / TOKEN_REFRESHED / PASSWORD_RECOVERY…) avec le nom de
// l'evenement. Renvoie une fn de desabonnement.
//
// Reste SYNCHRONE alors que le client est attendu : la signature est bindee cote
// Rust et le garde de `bindings.rs` appelle la fn rendue a son `Drop`. On
// s'abonne donc a l'arrivee du client, et le desabonnement couvre les deux cas
// — deja abonne, ou pas encore.
export function onAuthStateChange(cb) {
  if (typeof window === "undefined" || !configured()) {
    return () => {};
  }
  let subscription = null;
  let cancelled = false;
  authClient().then((client) => {
    if (cancelled) {
      return;
    }
    subscription = client.onAuthStateChange((event) => cb(event)).data.subscription;
  });
  return () => {
    cancelled = true;
    subscription?.unsubscribe();
  };
}

// --- Helpers DOM (web-sys indisponible sans features) -----------------------

// Origine courante (`window.location.origin`).
export function locationOrigin() {
  return typeof window === "undefined" ? "" : window.location.origin;
}

// Chemin + query courant (`pathname + search`).
export function locationPathSearch() {
  if (typeof window === "undefined") {
    return "/";
  }
  return `${window.location.pathname}${window.location.search}`;
}

// Hash courant (`window.location.hash`).
export function locationHash() {
  return typeof window === "undefined" ? "" : window.location.hash;
}

// Efface le hash en preservant pathname + search (history.replaceState).
export function clearLocationHash() {
  if (typeof window === "undefined") {
    return;
  }
  if (window.location.hash) {
    history.replaceState(null, "", window.location.pathname + window.location.search);
  }
}

// Navigation native dure (window.location.href = url).
export function navigateHard(url) {
  if (typeof window !== "undefined") {
    window.location.href = url;
  }
}

// Confirmation native (window.confirm).
export function confirmNative(message) {
  return typeof window === "undefined" ? false : window.confirm(message);
}

// Construit une URL en posant des parametres de query, renvoie sa string. Port
// de `new URL(base); url.searchParams.set(...)`.
export function buildUrlWithParams(base, pairsJson) {
  const url = new URL(base);
  const pairs = JSON.parse(pairsJson);
  for (const [k, v] of pairs) {
    if (v !== null && v !== undefined) {
      url.searchParams.set(k, v);
    }
  }
  return url.toString();
}

// POST JSON avec bearer (consentement OAuth /oauth/approve). Renvoie le `code`
// renvoye par l'API. Rejette avec le texte d'erreur si `!res.ok` (port fidele de
// authorize-mcp-page.tsx::handleApprove).
export async function oauthApprove(baseUrl, token, bodyJson) {
  const res = await fetch(`${baseUrl}/oauth/approve`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: bodyJson,
  });
  if (!res.ok) {
    throw new Error(await res.text());
  }
  const { code } = await res.json();
  return code;
}
