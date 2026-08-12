//! Validation JWT Supabase + JIT provisioning du profil user local.
//!
//! À chaque requête authentifiée, le row `users(sub, …)` est upserté (création +
//! bump `last_seen_at`). Le `sub` Supabase sert de PK opaque.
//!
//! Vérification : ES256/RSA via JWKS Supabase
//! (`/auth/v1/.well-known/jwks.json`), audience `authenticated`. Pilotée par
//! l'extracteur [`OptionalUser`].

use crate::error::{ApiError, Result};
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Forme minimale des claims qu'on lit après vérification de signature.
#[derive(Debug, Deserialize)]
struct SupabaseClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    user_metadata: Option<serde_json::Value>,
}

/// Extrait un `display_name` depuis les claims JWT Supabase.
///
/// Préfère `user_metadata.full_name` (rempli par les providers OAuth), fallback
/// sur `user_metadata.name`. Sinon `None` — l'utilisateur le remplira lui-même
/// via `PATCH /me`. Tronqué à 80 caractères.
fn display_name_hint(user_metadata: Option<&serde_json::Value>) -> Option<String> {
    let meta = user_metadata?.as_object()?;
    for key in ["full_name", "name"] {
        if let Some(v) = meta.get(key).and_then(|v| v.as_str()) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.chars().take(80).collect());
            }
        }
    }
    None
}

// ── Cache JWKS par URL Supabase ─────────────────────────────────────────────

struct JwksEntry {
    set: JwkSet,
    /// `None` = jamais récupéré avec succès (entrée créée par un échec).
    fetched_at: Option<Instant>,
    /// Dernière tentative de fetch, succès ou échec. Porte le throttle.
    last_attempt: Option<Instant>,
    /// Portail du fetch : au plus un fetch par projet à la fois, les concurrents
    /// **attendent** son résultat au lieu d'être refusés.
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl JwksEntry {
    /// Jeu de clés utilisable maintenant : récupéré et non périmé.
    fn is_fresh(&self) -> bool {
        self.fetched_at.is_some_and(|t| t.elapsed() < JWKS_LIFESPAN)
    }

    /// Une tentative trop récente interdit d'en relancer une.
    fn is_throttled(&self) -> bool {
        self.last_attempt
            .is_some_and(|t| t.elapsed() < JWKS_MIN_REFETCH)
    }
}

/// Durée de vie du cache JWKS.
const JWKS_LIFESPAN: Duration = Duration::from_secs(3600);

/// Intervalle minimal entre deux fetchs du JWKS d'un même projet. Un `kid`
/// absent du cache déclenche un fetch, ce qui fait voir une rotation de clé avant
/// l'expiration du TTL ; le throttle par projet borne le coût d'une rafale de `kid`
/// aléatoires venue d'un client non authentifié.
const JWKS_MIN_REFETCH: Duration = Duration::from_secs(30);

/// Délai maximal d'un fetch JWKS. `reqwest` sans timeout attend indéfiniment.
const JWKS_TIMEOUT: Duration = Duration::from_secs(5);

fn jwks_cache() -> &'static Mutex<HashMap<String, JwksEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, JwksEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Issue d'une recherche de clé de signature. Les trois échecs sont distincts :
/// c'est le motif logué par [`reject`].
enum KeyLookup {
    Found(DecodingKey, Algorithm),
    /// Le JWKS est frais et ne publie pas ce `kid` : rotation non propagée, ou
    /// jeton d'un autre projet.
    KidInconnu,
    /// Un fetch trop récent a échoué : on ne retape pas Supabase avant
    /// [`JWKS_MIN_REFETCH`].
    Throttle,
    /// Le fetch de ce tour a échoué (réseau, timeout, corps illisible).
    FetchEchec,
}

/// Récupère le JWKS (cache 1 h, refetch throttlé) puis sélectionne la clé par
/// `kid`.
///
/// Les concurrents attendent le fetch en vol, ils ne sont pas refusés : le portail
/// `gate` sérialise l'accès au fetch par projet, et le throttle n'est consulté
/// qu'une fois le portail obtenu.
///
/// Le verrou d'état (`std::sync::Mutex`) n'est jamais tenu à travers un `await` ;
/// seul le portail l'est, ce pour quoi il est un mutex tokio.
async fn signing_key_for(supabase_url: &str, kid: &str) -> KeyLookup {
    // 1. Chemin rapide, sans portail : cache frais et `kid` présent. C'est le cas
    //    nominal, il ne paie aucune synchronisation. L'entrée est créée dès le premier
    //    passage, pour que le throttle ait un état même si le tout premier fetch échoue.
    let gate = {
        let Ok(mut cache) = jwks_cache().lock() else {
            return KeyLookup::FetchEchec;
        };
        let entry = cache
            .entry(supabase_url.to_string())
            .or_insert_with(|| JwksEntry {
                set: JwkSet { keys: Vec::new() },
                fetched_at: None,
                last_attempt: None,
                gate: Arc::new(tokio::sync::Mutex::new(())),
            });
        if entry.is_fresh() {
            if let Some((key, alg)) = key_from_set(&entry.set, kid) {
                return KeyLookup::Found(key, alg);
            }
        }
        Arc::clone(&entry.gate)
    };

    // 2. Portail : au plus un fetch par projet. Les concurrents attendent ici au
    //    lieu de repartir sans identité.
    let _fetching = gate.lock().await;

    // 3. Re-lecture sous le portail : le gagnant a peut-être déjà rempli le cache
    //    pendant qu'on attendait. Le throttle n'est consulté qu'ici, et un `kid` absent
    //    d'un jeu POURTANT frais tombe volontairement dans le fetch.
    {
        let Ok(cache) = jwks_cache().lock() else {
            return KeyLookup::FetchEchec;
        };
        let Some(entry) = cache.get(supabase_url) else {
            return KeyLookup::FetchEchec;
        };
        if entry.is_fresh() {
            if let Some((key, alg)) = key_from_set(&entry.set, kid) {
                return KeyLookup::Found(key, alg);
            }
        }
        if entry.is_throttled() {
            // On ne retape pas Supabase : si le jeu en main est frais, il ne publie
            // simplement pas ce `kid`.
            return if entry.is_fresh() {
                KeyLookup::KidInconnu
            } else {
                KeyLookup::Throttle
            };
        }
    }

    // 4. Fetch + mise en cache. `last_attempt` est posé quoi qu'il arrive : c'est
    //    lui qui borne le coût d'une rafale de `kid` aléatoires.
    let jwks_url = format!(
        "{}/auth/v1/.well-known/jwks.json",
        supabase_url.trim_end_matches('/')
    );
    let fetched = async {
        reqwest::Client::builder()
            .timeout(JWKS_TIMEOUT)
            .build()
            .ok()?
            .get(&jwks_url)
            .send()
            .await
            .ok()?
            .json::<JwkSet>()
            .await
            .ok()
    }
    .await;

    let now = Instant::now();
    let Ok(mut cache) = jwks_cache().lock() else {
        return KeyLookup::FetchEchec;
    };
    let Some(set) = fetched else {
        if let Some(entry) = cache.get_mut(supabase_url) {
            entry.last_attempt = Some(now);
        }
        return KeyLookup::FetchEchec;
    };
    let outcome = match key_from_set(&set, kid) {
        Some((key, alg)) => KeyLookup::Found(key, alg),
        None => KeyLookup::KidInconnu,
    };
    if let Some(entry) = cache.get_mut(supabase_url) {
        entry.set = set;
        entry.fetched_at = Some(now);
        entry.last_attempt = Some(now);
    }
    outcome
}

/// Clé de vérification d'un `kid` du JWKS, et algorithme attendu des jetons
/// qu'elle signe.
///
/// L'algorithme se déduit du **matériau de clé** publié par le projet — courbe EC
/// (RFC 7518 §3.4) ou module RSA —, donc d'une source authentifiée et non d'un champ
/// de l'en-tête du jeton présenté.
fn key_from_set(set: &JwkSet, kid: &str) -> Option<(DecodingKey, Algorithm)> {
    let jwk = set.find(kid)?;
    match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(ec) => {
            let alg = match ec.curve {
                EllipticCurve::P256 => Algorithm::ES256,
                EllipticCurve::P384 => Algorithm::ES384,
                _ => return None,
            };
            Some((DecodingKey::from_ec_components(&ec.x, &ec.y).ok()?, alg))
        }
        AlgorithmParameters::RSA(rsa) => Some((
            DecodingKey::from_rsa_components(&rsa.n, &rsa.e).ok()?,
            Algorithm::RS256,
        )),
        _ => None,
    }
}

/// Politique de vérification d'un jeton Supabase : algorithme, audience,
/// émetteur.
///
/// `alg` est celui de la clé du JWKS ([`key_from_set`]), et la liste d'algorithmes
/// n'en porte que lui : `jsonwebtoken` exige que **chaque** algorithme de
/// `Validation::algorithms` soit de la famille de la clé de vérification. Une liste
/// `[ES256, RS256]` rejette donc tout jeton signé par une clé EC, valide inclus.
///
/// L'`iss` attendu est celui de la discovery OIDC du projet, sans slash final.
fn validation_for(alg: Algorithm, supabase_url: &str) -> Validation {
    let mut validation = Validation::new(alg);
    validation.set_audience(&["authenticated"]);
    validation.set_issuer(&[format!("{}/auth/v1", supabase_url.trim_end_matches('/'))]);
    validation
}

/// JIT upsert du profil user local.
///
/// Idempotent : ne réécrit jamais un `email` / `display_name` déjà renseigné
/// (`COALESCE`), bump `last_seen_at`.
async fn jit_upsert_user(
    state: &AppState,
    sub: &str,
    email: Option<&str>,
    display_name: Option<&str>,
) -> Result<()> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("pool: {e}")))?;
    conn.execute(
        "INSERT INTO users (sub, email, display_name) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (sub) DO UPDATE SET \
           email        = COALESCE(users.email, EXCLUDED.email), \
           display_name = COALESCE(users.display_name, EXCLUDED.display_name), \
           last_seen_at = now()",
        &[&sub, &email, &display_name],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("jit_upsert_user: {e}")))?;
    Ok(())
}

/// Un jeton a été présenté et refusé. Toujours `None` — seule la trace apparaît.
///
/// Ce chemin n'est atteint **que** si un `Authorization: Bearer` est là : un
/// visiteur déconnecté n'envoie pas de jeton, et le MCP a sa propre auth. Une ligne
/// = une session réelle qu'on vient de perdre. Clé d'alerte : `jwt_rejete`.
fn reject(motif: &'static str, detail: &str) -> Option<String> {
    // Le motif est AUSSI dans le corps du message : un filtre de ligne Loki
    // (`|=`, `!=`) ne voit que lui, jamais les métadonnées structurées. C'est ce qui
    // permet à l'alerte d'exclure `ExpiredSignature`, seul refus routinier.
    tracing::warn!(
        jwt_rejete = motif,
        detail,
        "jeton Supabase refusé : {motif}/{detail}"
    );
    None
}

/// Décode + vérifie un token Supabase (ES256/RSA via JWKS, audience
/// `authenticated`) et fait le JIT upsert. Renvoie `Some(sub)` ou `None` (token
/// absent/invalide/JWKS HS), jamais d'erreur.
async fn resolve_supabase_user(state: &AppState, token: &str) -> Option<String> {
    let supabase_url = state.settings.supabase_url.as_deref()?;

    let header = match decode_header(token) {
        Ok(h) => h,
        Err(e) => return reject("header_illisible", &e.to_string()),
    };
    let Some(kid) = header.kid.as_deref() else {
        return reject("kid_absent", "en-tête sans kid");
    };
    // Trois motifs distincts, pour trois pannes distinctes : `kid_inconnu` dit
    // « rotation de clé non publiée », `jwks_throttle` et `jwks_fetch_echec` disent
    // « Supabase injoignable ».
    let (signing_key, alg) = match signing_key_for(supabase_url, kid).await {
        KeyLookup::Found(key, alg) => (key, alg),
        KeyLookup::KidInconnu => return reject("kid_inconnu", kid),
        KeyLookup::Throttle => return reject("jwks_throttle", kid),
        KeyLookup::FetchEchec => return reject("jwks_fetch_echec", kid),
    };

    let validation = validation_for(alg, supabase_url);
    let data = match decode::<SupabaseClaims>(token, &signing_key, &validation) {
        Ok(d) => d,
        // Le motif porte la cause exacte (`InvalidAlgorithm`, `ExpiredSignature`,
        // `InvalidIssuer`…).
        Err(e) => return reject("verification", &format!("{:?}", e.kind())),
    };
    let claims = data.claims;

    let sub = claims.sub.clone();
    let email = claims.email.clone();
    let display = display_name_hint(claims.user_metadata.as_ref());
    // Best-effort : un échec d'upsert ne casse pas la requête lue — on log et on
    // rend quand même le sub.
    if let Err(e) = jit_upsert_user(state, &sub, email.as_deref(), display.as_deref()).await {
        tracing::warn!("jit_upsert_user failed: {e}");
    }
    Some(sub)
}

/// Extrait le bearer token de l'en-tête `Authorization`.
fn bearer_token(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(axum::http::header::AUTHORIZATION)?;
    let text = value.to_str().ok()?;
    text.strip_prefix("Bearer ")
        .or_else(|| text.strip_prefix("bearer "))
        .map(str::to_string)
}

/// Extracteur axum : `user_id` (sub Supabase) ou `None`. Token
/// absent/invalide → `None` (jamais 401).
#[derive(Debug, Clone)]
pub struct OptionalUser(pub Option<String>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Some(token) = bearer_token(parts) else {
            return Ok(OptionalUser(None));
        };
        Ok(OptionalUser(resolve_supabase_user(state, &token).await))
    }
}

/// Extracteur axum : `user_id` requis, sinon 401 `auth_required`.
#[derive(Debug, Clone)]
pub struct RequiredUser(pub String);

impl FromRequestParts<AppState> for RequiredUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Ok(OptionalUser(user)) = OptionalUser::from_request_parts(parts, state).await;
        user.map(RequiredUser).ok_or(ApiError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Paire de clés P-256 de test : la privée signe les jetons, ses coordonnées
    /// publiques peuplent le JWKS de test.
    const TEST_EC_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
        MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQglpNIUCRSyB9cAT0h\n\
        DcxObKwgiVoeS+4AJNeTVkfckEOhRANCAARfXhMNhGon/SvY25TCwjFdzirgdQNl\n\
        PeqZ0myjJxlhFdW73/NJoH+GzxlKDAZT1T00R7tzbVST7UJRsIbTxBYl\n\
        -----END PRIVATE KEY-----";
    const TEST_EC_X: &str = "X14TDYRqJ_0r2NuUwsIxXc4q4HUDZT3qmdJsoycZYRU";
    const TEST_EC_Y: &str = "1bvf80mgf4bPGUoMBlPVPTRHu3NtVJPtQlGwhtPEFiU";
    const TEST_KID: &str = "5732e8d0-23a6-435b-97fa-400674ca7e0a";
    const TEST_SUPABASE_URL: &str = "https://projet.supabase.co";

    /// JWKS au format que Supabase publie sur
    /// `/auth/v1/.well-known/jwks.json` : une clé EC P-256, `alg` ES256.
    fn test_jwks() -> JwkSet {
        serde_json::from_value(json!({"keys": [{
            "alg": "ES256",
            "crv": "P-256",
            "ext": true,
            "key_ops": ["verify"],
            "kid": TEST_KID,
            "kty": "EC",
            "use": "sig",
            "x": TEST_EC_X,
            "y": TEST_EC_Y,
        }]}))
        .unwrap()
    }

    /// Jeton ES256 signé par la clé de test, `kid` renseigné comme le fait GoTrue.
    fn mint(claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(TEST_KID.to_string());
        let key = EncodingKey::from_ec_pem(TEST_EC_PEM.as_bytes()).unwrap();
        encode(&header, &claims, &key).unwrap()
    }

    /// Claims d'un access token GoTrue : `iss` du projet, `aud` `authenticated`.
    fn claims() -> serde_json::Value {
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        json!({
            "sub": "d7f1e0aa-0000-4000-8000-000000000001",
            "aud": "authenticated",
            "iss": format!("{TEST_SUPABASE_URL}/auth/v1"),
            "role": "authenticated",
            "email": "jean@example.test",
            "exp": exp,
        })
    }

    #[test]
    fn jeton_signe_par_la_cle_du_jwks_est_accepte() {
        let (key, alg) = key_from_set(&test_jwks(), TEST_KID).expect("clé ES256 du JWKS");
        assert_eq!(alg, Algorithm::ES256);
        let data = decode::<SupabaseClaims>(
            &mint(claims()),
            &key,
            &validation_for(alg, TEST_SUPABASE_URL),
        )
        .expect("jeton ES256 valide accepté");
        assert_eq!(data.claims.sub, "d7f1e0aa-0000-4000-8000-000000000001");
        assert_eq!(data.claims.email.as_deref(), Some("jean@example.test"));
    }

    #[test]
    fn jeton_hors_audience_ou_hors_emetteur_est_refuse() {
        let (key, alg) = key_from_set(&test_jwks(), TEST_KID).unwrap();
        let validation = validation_for(alg, TEST_SUPABASE_URL);

        let mut autre_aud = claims();
        autre_aud["aud"] = json!("anon");
        assert!(decode::<SupabaseClaims>(&mint(autre_aud), &key, &validation).is_err());

        let mut autre_iss = claims();
        autre_iss["iss"] = json!("https://attaquant.supabase.co/auth/v1");
        assert!(decode::<SupabaseClaims>(&mint(autre_iss), &key, &validation).is_err());
    }

    #[test]
    fn kid_inconnu_ne_rend_pas_de_cle() {
        assert!(key_from_set(&test_jwks(), "kid-inconnu").is_none());
    }

    /// Sert le JWKS de test sur un port éphémère, après un délai qui garantit le
    /// chevauchement des appels concurrents. Rend l'URL du projet et le compteur de
    /// fetchs reçus. Un port par appel ⇒ une URL par appel : le cache JWKS est un
    /// statique de processus, deux tests ne peuvent pas se polluer.
    async fn serveur_jwks() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let fetchs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let compteur = Arc::clone(&fetchs);
        let corps = serde_json::to_string(&test_jwks()).unwrap();
        let app = axum::Router::new().route(
            "/auth/v1/.well-known/jwks.json",
            axum::routing::get(move || {
                let corps = corps.clone();
                let compteur = Arc::clone(&compteur);
                async move {
                    compteur.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        corps,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), fetchs)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrents_sur_cache_froid_obtiennent_tous_la_cle() {
        let (url, fetchs) = serveur_jwks().await;

        let issues = futures::future::join_all((0..8).map(|_| {
            let url = url.clone();
            async move { signing_key_for(&url, TEST_KID).await }
        }))
        .await;

        let trouvees = issues
            .iter()
            .filter(|i| matches!(i, KeyLookup::Found(..)))
            .count();
        assert_eq!(trouvees, 8, "un jeton valide a été refusé pendant le fetch");
        assert_eq!(
            fetchs.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "le simple-vol ne tient plus : un fetch par requête entrante"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rafale_sur_kid_inconnu_ne_declenche_qu_un_fetch() {
        let (url, fetchs) = serveur_jwks().await;

        let issues = futures::future::join_all((0..8).map(|i| {
            let url = url.clone();
            async move { signing_key_for(&url, &format!("kid-aleatoire-{i}")).await }
        }))
        .await;

        assert!(
            issues
                .iter()
                .all(|i| matches!(i, KeyLookup::KidInconnu | KeyLookup::Throttle)),
            "un kid absent du JWKS ne doit pas rendre de clé"
        );
        assert_eq!(
            fetchs.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "la rafale a percé le throttle"
        );
    }

    #[test]
    fn display_name_prefers_full_name() {
        let meta = json!({"full_name": "  Jean Dupont  ", "name": "jdupont"});
        assert_eq!(
            display_name_hint(Some(&meta)).as_deref(),
            Some("Jean Dupont")
        );
    }

    #[test]
    fn display_name_falls_back_to_name() {
        let meta = json!({"name": "jdupont"});
        assert_eq!(display_name_hint(Some(&meta)).as_deref(), Some("jdupont"));
    }

    #[test]
    fn display_name_none_when_blank_or_missing() {
        assert_eq!(display_name_hint(None), None);
        let blank = json!({"full_name": "   ", "name": ""});
        assert_eq!(display_name_hint(Some(&blank)), None);
        let not_obj = json!("nope");
        assert_eq!(display_name_hint(Some(&not_obj)), None);
    }

    #[test]
    fn display_name_truncated_to_80() {
        let long = "a".repeat(200);
        let meta = json!({ "full_name": long });
        assert_eq!(display_name_hint(Some(&meta)).unwrap().chars().count(), 80);
    }
}
