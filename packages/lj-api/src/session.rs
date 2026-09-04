//! Sessions web : cookie opaque, empreinte en base.
//!
//! Le cookie porte 32 octets de hasard ; la table n'en garde que le SHA-256, donc
//! une copie de la base ne donne aucune session utilisable. Un seul jeton, pas de
//! paire access + refresh : chaque requête fait le lookup de toute façon.

use axum::http::request::Parts;
use axum::http::HeaderMap;

use crate::error::{ApiError, Result};
use crate::oauth::{sha256, token_urlsafe};
use crate::state::AppState;

/// Le préfixe `__Host-` est une contrainte que le navigateur applique lui-même :
/// il refuse le cookie s'il n'est pas `Secure`, s'il n'a pas `Path=/`, ou s'il
/// porte un `Domain`. Aucun sous-domaine ne peut donc en poser un de ce nom pour
/// l'hôte — la zone porte des CNAME vers des tiers, et l'un d'eux repris
/// poserait sinon `Domain=.librejustice.fr` pour masquer la session.
pub const COOKIE: &str = "__Host-lj_session";
/// Nom d'avant le préfixe. Lu le temps que les sessions en circulation migrent :
/// elles sont réémises sous le nouveau nom dès la première requête.
const COOKIE_HERITE: &str = "lj_session";

/// Échéance glissante : ce que la fenêtre d'inactivité tolère.
const TTL_JOURS: i64 = 30;
/// Plafond absolu, quelle que soit l'activité. Il ne remplace aucune révocation
/// — la session est relue en base à chaque requête, `sessions_valid_from` la tue
/// à la demande — donc il ne borne qu'une session oubliée, que les 30 jours
/// glissants ferment déjà. Sous le plafond de 400 jours des navigateurs.
const TTL_ABSOLU_JOURS: i64 = 365;
/// Sous ce délai depuis le dernier usage, on ne réécrit pas la ligne : la session
/// est lue à chaque requête authentifiée, l'UPDATE ne suit pas.
const RAFRAICHIR_APRES_MINUTES: i64 = 60;

/// Ouvre une session et rend la valeur à poser dans le cookie.
pub async fn open(state: &AppState, user_sub: &str) -> Result<String> {
    let token = token_urlsafe(32);
    let conn = state.pool.get().await?;
    conn.execute(
        "INSERT INTO web_sessions \
           (session_sha256, user_sub, expires_at, absolute_expires_at) \
         VALUES ($1, $2, now() + make_interval(days => $3::int), \
                 now() + make_interval(days => $4::int))",
        &[
            &sha256(token.as_bytes()).to_vec(),
            &user_sub,
            &(TTL_JOURS as i32),
            &(TTL_ABSOLU_JOURS as i32),
        ],
    )
    .await?;
    Ok(token)
}

/// Ce que vaut un cookie présenté. Un refus se compte, une base muette ne se
/// compte pas : les confondre rendrait une panne Postgres indiscernable d'une
/// perte de sessions généralisée, qui est ce qu'on surveille.
pub enum Verdict {
    Porteur {
        sub: String,
        /// L'échéance vient de glisser en base. Le cookie du navigateur porte un
        /// `Max-Age` figé à la connexion, qu'il faut donc réémettre : sans ça la
        /// session meurt au 30ᵉ jour même utilisée tous les jours.
        refresh: bool,
    },
    /// Le cookie n'a pas la forme d'un jeton que nous émettons : scanner, valeur
    /// tronquée, ou notre propre suite de contrats. Bruit de fond, pas un signal.
    FormeInvalide,
    /// Bien formé, mais inconnu, expiré, révoqué, ou antérieur à une
    /// déconnexion globale. C'est le refus qui vaut d'être compté.
    Inconnue,
    /// La base n'a pas répondu — l'état de la session est indéterminé.
    Indisponible,
}

/// Résout le porteur du cookie. Fait glisser l'échéance, au plus une fois par
/// heure.
pub async fn resolve(state: &AppState, token: &str) -> Verdict {
    if !bien_forme(token) {
        return Verdict::FormeInvalide;
    }
    let digest = sha256(token.as_bytes()).to_vec();
    let Ok(conn) = state.pool.get().await else {
        return Verdict::Indisponible;
    };
    let ligne = match conn
        .query_opt(
            "SELECT s.user_sub, \
                    s.last_used_at < now() - make_interval(mins => $2::int) AS a_glisser \
               FROM web_sessions s JOIN users u ON u.sub = s.user_sub \
              WHERE s.session_sha256 = $1 \
                AND s.expires_at > now() AND s.absolute_expires_at > now() \
                AND (u.sessions_valid_from IS NULL OR s.created_at >= u.sessions_valid_from)",
            &[&digest, &(RAFRAICHIR_APRES_MINUTES as i32)],
        )
        .await
    {
        Ok(Some(ligne)) => ligne,
        Ok(None) => return Verdict::Inconnue,
        Err(_) => return Verdict::Indisponible,
    };

    // L'échéance ne glisse qu'une fois par heure : la session est lue à chaque
    // requête authentifiée, un UPDATE par lecture ne suit pas.
    let sub = ligne.get::<_, String>("user_sub");
    let refresh = ligne.get::<_, bool>("a_glisser");
    if refresh {
        let _ = conn
            .execute(
                "UPDATE web_sessions SET last_used_at = now(), \
                   expires_at = least(now() + make_interval(days => $2::int), absolute_expires_at) \
                 WHERE session_sha256 = $1",
                &[&digest, &(TTL_JOURS as i32)],
            )
            .await;
        // Le rapport d'usage lit `users.last_seen_at` : sans ce second UPDATE, un
        // compte actif en session cookie apparaîtrait dormant.
        let _ = conn
            .execute(
                "UPDATE users SET last_seen_at = now() WHERE sub = $1",
                &[&sub],
            )
            .await;
    }
    Verdict::Porteur { sub, refresh }
}

/// Forme d'un jeton émis par [`open`] : `token_urlsafe(32)`, donc 43 caractères
/// base64url sans remplissage.
fn bien_forme(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Ferme la session de ce navigateur. Les autres survivent.
pub async fn close(state: &AppState, token: &str) -> Result<()> {
    let conn = state.pool.get().await?;
    conn.execute(
        "DELETE FROM web_sessions WHERE session_sha256 = $1",
        &[&sha256(token.as_bytes()).to_vec()],
    )
    .await?;
    Ok(())
}

/// Ferme toutes les sessions du compte, sauf celle passée en `garder`.
pub async fn close_others(state: &AppState, user_sub: &str, garder: &str) -> Result<()> {
    let conn = state.pool.get().await?;
    conn.execute(
        "DELETE FROM web_sessions WHERE user_sub = $1 AND session_sha256 <> $2",
        &[&user_sub, &sha256(garder.as_bytes()).to_vec()],
    )
    .await?;
    Ok(())
}

/// Cookie de session porté par ces en-têtes, et s'il l'était sous le nom hérité
/// — auquel cas il est à réémettre sous le nom préfixé.
pub fn from_headers(entetes: &HeaderMap) -> Option<(String, bool)> {
    let entete = entetes.get(axum::http::header::COOKIE)?.to_str().ok()?;
    read_cookie(entete, COOKIE)
        .map(|token| (token, false))
        .or_else(|| read_cookie(entete, COOKIE_HERITE).map(|token| (token, true)))
}

/// Idem depuis les `Parts` d'un extracteur.
pub fn from_parts(parts: &Parts) -> Option<(String, bool)> {
    from_headers(&parts.headers)
}

/// Créneau de réémission du cookie, posé dans les extensions de la requête par
/// `renew_session_cookie` et rempli par l'extracteur d'identité : l'extracteur
/// sait que la session a glissé mais ne voit pas la réponse, le middleware voit
/// la réponse mais ne résout aucune session.
#[derive(Clone, Default)]
pub struct CookieRefresh(std::sync::Arc<std::sync::Mutex<Option<String>>>);

impl CookieRefresh {
    pub fn set(&self, valeur: String) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(valeur);
        }
    }

    pub fn take(&self) -> Option<String> {
        self.0.lock().ok()?.take()
    }
}

/// Lit un cookie dans un en-tête `Cookie` (`a=1; b=2`).
pub fn read_cookie(entete: &str, nom: &str) -> Option<String> {
    entete.split(';').find_map(|paire| {
        let (cle, valeur) = paire.split_once('=')?;
        (cle.trim() == nom).then(|| valeur.trim().to_string())
    })
}

/// `Set-Cookie` de pose. `SameSite=Lax` : les POST cross-site n'emportent pas le
/// cookie, ce qui couvre le CSRF sans jeton dédié, et le retour d'un tour OAuth
/// reste une navigation de premier niveau, donc il passe.
pub fn set_cookie(token: &str) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}",
        TTL_JOURS * 86_400
    )
}

/// `Set-Cookie` d'effacement, posé sur tout refus : un cookie refusé ne doit pas
/// être represente à la requête suivante. Un par nom, sinon le cookie hérité
/// survit à la déconnexion et se fait refuser à chaque requête.
pub fn clear_cookies() -> [String; 2] {
    [COOKIE, COOKIE_HERITE]
        .map(|nom| format!("{nom}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0"))
}

/// Cookie court d'aller-retour OAuth (état + nonce du tour Google).
pub fn set_transient(nom: &str, valeur: &str, secondes: i64) -> String {
    format!("{nom}={valeur}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={secondes}")
}

/// En-tête `Set-Cookie` typé, pour les réponses des handlers.
pub fn header(valeur: String) -> Result<(axum::http::HeaderName, axum::http::HeaderValue)> {
    let v = axum::http::HeaderValue::from_str(&valeur)
        .map_err(|_| ApiError::Internal("Set-Cookie invalide".into()))?;
    Ok((axum::http::header::SET_COOKIE, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entetes(cookie: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::COOKIE, cookie.parse().unwrap());
        h
    }

    #[test]
    fn cookie_lu_parmi_dautres() {
        let entete = "theme=dark; __Host-lj_session=abc.def; autre=1";
        assert_eq!(read_cookie(entete, COOKIE).as_deref(), Some("abc.def"));
        assert_eq!(read_cookie(entete, "absent"), None);
    }

    #[test]
    fn cookie_ne_matche_pas_un_suffixe() {
        assert_eq!(read_cookie("x__Host-lj_session=nope", COOKIE), None);
    }

    /// Le nom hérité reste lisible, et se signale comme tel pour être réémis.
    #[test]
    fn nom_herite_lu_et_signale() {
        assert_eq!(
            from_headers(&entetes("lj_session=abc")),
            Some(("abc".into(), true))
        );
        assert_eq!(
            from_headers(&entetes("__Host-lj_session=abc")),
            Some(("abc".into(), false))
        );
    }

    /// Un sous-domaine qui pose `lj_session` ne doit pas masquer la session
    /// préfixée, que `__Host-` lui interdit précisément de contrefaire.
    #[test]
    fn le_nom_prefixe_prime_sur_l_herite() {
        assert_eq!(
            from_headers(&entetes("lj_session=intrus; __Host-lj_session=vrai")),
            Some(("vrai".into(), false))
        );
    }

    /// La forme sépare le bruit de fond (scanner, valeur tronquée) d'une session
    /// vraiment perdue : c'est elle qui rend le refus comptable.
    #[test]
    fn forme_d_un_jeton_emis() {
        assert!(bien_forme(&token_urlsafe(32)));
        assert!(!bien_forme("cookie-qui-n-existe-pas"));
        assert!(!bien_forme(""));
        assert!(!bien_forme(&"a".repeat(43).replace('a', "=")));
    }
}
