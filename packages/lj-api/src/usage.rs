//! Signal d'usage de la recherche, commun aux deux moteurs.
//!
//! Une recherche — décisions ou textes, web ou MCP — émet deux signaux portant le
//! même vocabulaire `librejustice.search.*` : le span `search` (attributs posés
//! avant l'exécution, `results_count` après, donc une recherche qui échoue reste
//! attribuable) et un event `INFO` sur la cible [`LOG_TARGET`], dont les champs
//! sont les mêmes en snake_case nu.
//!
//! Aucune identité en clair dans l'un ni l'autre : `identity` dit connecté ou
//! non, et le `sub` n'en sort que pseudonymisé par [`Caller::pseudonymise`],
//! sous MAC à clé. La query part intacte, et le `trace_id` relie les deux.

use lj_dtos::{ActivitySource, SearchContext, SearchEngine};

/// Cible tracing des events d'usage.
pub const LOG_TARGET: &str = "lj_api::usage";

/// Forme de la requête posée, commune aux deux moteurs : profondeur demandée et
/// sélection de facettes. Construite par le point d'entrée, qui seul connaît la
/// fenêtre et les filtres.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape<'a> {
    /// Page 1-based, toujours posée, `1` sur la première page.
    pub page: i64,
    /// Filtres de facettes sérialisés, sans la query ni la fenêtre
    /// ([`crate::search_history::facet_filters`] côté décisions).
    pub filters: &'a str,
}

/// Provenance d'une recherche : quel canal, connecté ou non, intention
/// d'utilisateur ou fetch machine du pont croisé. Le moteur n'y est pas.
#[derive(Debug, Clone)]
pub struct Caller {
    pub source: ActivitySource,
    pub authenticated: bool,
    pub context: SearchContext,
    /// Set by [`is_first_party`]; usage counts subtract it.
    pub first_party: bool,
    /// Pseudonyme stable du compte, posé par [`Caller::pseudonymise`]. Jamais le
    /// `sub` : un MAC à clé tronqué, que seule notre base résout.
    pub caller: Option<String>,
    /// Pseudonyme de visite d'un appelant anonyme, salé par le jour. Exclusif de
    /// [`Caller::caller`] : un appel est identifié ou il ne l'est pas.
    pub visit: Option<String>,
    /// User-Agent de l'appelant, tel que lu au bord — le client qui a parlé,
    /// assistant IA compris. Absent sur l'appel in-process du rendu SSR, qui n'a
    /// pas de socket client.
    pub user_agent: Option<String>,
}

/// Named after the traffic, not after the `/internal` mount, which it does not
/// guard: that surface is open to any browser.
pub fn is_first_party(headers: &axum::http::HeaderMap, settings: &crate::config::Settings) -> bool {
    settings.traffic_is_first_party()
        || internal_origin(headers, settings.internal_origin_token.as_deref())
}

/// User-Agent de l'appelant, lu au bord. Point d'entrée unique : le span de
/// recherche le porte, et le pseudonyme de visite le hache.
pub fn user_agent(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
}

/// L'appelant présente-t-il le secret d'origine interne ? `expected` à `None`
/// (réglage absent) ⇒ jamais : le défaut est `external`, sans configuration.
pub fn internal_origin(headers: &axum::http::HeaderMap, expected: Option<&str>) -> bool {
    let Some(expected) = expected.filter(|e| !e.is_empty()) else {
        return false;
    };
    let entete = headers
        .get(lj_dtos::INTERNAL_ORIGIN_HEADER)
        .and_then(|v| v.to_str().ok());
    if entete.is_some_and(|v| v == expected) {
        return true;
    }
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| crate::session::read_cookie(v, lj_dtos::INTERNAL_ORIGIN_COOKIE))
        .is_some_and(|v| v == expected)
}

impl Caller {
    /// Appel web anonyme en contexte utilisateur — le rendu SSR in-process.
    pub fn web_anonymous(context: SearchContext) -> Self {
        Self {
            source: ActivitySource::Web,
            authenticated: false,
            context,
            first_party: false,
            caller: None,
            visit: None,
            user_agent: None,
        }
    }

    /// Every non-MCP door, identity resolved through `OptionalUser`.
    pub fn http(
        source: ActivitySource,
        authenticated: bool,
        context: SearchContext,
        headers: &axum::http::HeaderMap,
        settings: &crate::config::Settings,
    ) -> Self {
        Self {
            source,
            authenticated,
            context,
            first_party: is_first_party(headers, settings),
            caller: None,
            visit: None,
            user_agent: user_agent(headers).map(str::to_owned),
        }
    }

    /// Pose `caller` et `visit` depuis l'identité déjà résolue. Sans
    /// [`Pseudonymiser`] — réglage absent — rien n'est posé et rien ne nomme
    /// personne.
    pub fn pseudonymise(
        mut self,
        id: &crate::identity::Identity,
        pseudo: Option<&crate::identity::Pseudonymiser>,
        headers: &axum::http::HeaderMap,
    ) -> Self {
        let Some(pseudo) = pseudo else {
            return self;
        };
        self.caller = pseudo.caller(id);
        self.visit = pseudo.visit(
            id,
            user_agent(headers),
            crate::identity::salt_period(unix_now()),
        );
        self
    }

    /// Pose les attributs d'usage sur le span courant, avant l'exécution.
    pub(crate) fn record_span(&self, engine: SearchEngine, query: &str, shape: Shape<'_>) {
        let span = tracing::Span::current();
        span.record(
            "librejustice.search.engine",
            crate::search_history::engine_value(engine),
        );
        span.record(
            "librejustice.search.source",
            crate::search_history::source_value(self.source),
        );
        span.record(
            "librejustice.search.identity",
            identity_value(self.authenticated),
        );
        span.record("librejustice.search.context", context_value(self.context));
        span.record("librejustice.search.origin", origin_value(self.first_party));
        span.record("librejustice.search.caller", self.caller.as_deref());
        span.record("librejustice.search.visit", self.visit.as_deref());
        span.record("user_agent.original", self.user_agent.as_deref());
        span.record("librejustice.search.query", query);
        span.record("librejustice.search.page", shape.page);
        span.record("librejustice.search.filters", shape.filters);
    }

    /// Complète le span (`results_count`) et émet l'event d'usage vers Loki.
    pub(crate) fn record_outcome(
        &self,
        engine: SearchEngine,
        query: &str,
        shape: Shape<'_>,
        results_count: i64,
    ) {
        tracing::Span::current().record("librejustice.search.results_count", results_count);
        tracing::info!(
            target: LOG_TARGET,
            engine = crate::search_history::engine_value(engine),
            source = crate::search_history::source_value(self.source),
            identity = identity_value(self.authenticated),
            context = context_value(self.context),
            origin = origin_value(self.first_party),
            caller = self.caller.as_deref(),
            visit = self.visit.as_deref(),
            query = query,
            page = shape.page,
            filters = shape.filters,
            results_count = results_count,
            "recherche"
        );
    }
}

/// Horloge murale en secondes, seule source de temps du signal d'usage. Une
/// horloge reculée avant l'époque rendrait un négatif, que `salt_period` traite
/// en division euclidienne.
fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// `authenticated` | `anonymous` : connecté ou non, jamais qui.
pub(crate) fn identity_value(authenticated: bool) -> &'static str {
    if authenticated {
        "authenticated"
    } else {
        "anonymous"
    }
}

/// `internal` | `external` : `internal` = un émetteur qui nous appartient s'est
/// déclaré, à exclure de tout comptage d'usage.
pub(crate) fn origin_value(internal: bool) -> &'static str {
    if internal {
        "internal"
    } else {
        "external"
    }
}

/// `user` | `teaser` : un `teaser` est le fetch machine du pont croisé entre
/// moteurs, à exclure de tout comptage d'usage.
pub(crate) fn context_value(context: SearchContext) -> &'static str {
    match context {
        SearchContext::User => "user",
        SearchContext::Teaser => "teaser",
    }
}

#[cfg(test)]
mod tests {
    use super::internal_origin;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn entetes(nom: &str, valeur: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(
            HeaderName::from_bytes(nom.as_bytes()).unwrap(),
            HeaderValue::from_str(valeur).unwrap(),
        );
        map
    }

    #[test]
    fn les_deux_voies_declarent() {
        let entete = entetes(lj_dtos::INTERNAL_ORIGIN_HEADER, "s3cret");
        assert!(internal_origin(&entete, Some("s3cret")));

        let cookie = entetes(
            "cookie",
            &format!("a=1; {}=s3cret; b=2", lj_dtos::INTERNAL_ORIGIN_COOKIE),
        );
        assert!(internal_origin(&cookie, Some("s3cret")));
    }

    #[test]
    fn une_mauvaise_valeur_ne_declare_pas() {
        let entete = entetes(lj_dtos::INTERNAL_ORIGIN_HEADER, "devine");
        assert!(!internal_origin(&entete, Some("s3cret")));

        let cookie = entetes(
            "cookie",
            &format!("{}=devine", lj_dtos::INTERNAL_ORIGIN_COOKIE),
        );
        assert!(!internal_origin(&cookie, Some("s3cret")));
    }

    /// Réglage absent côté serveur : personne ne peut se retirer des comptages,
    /// pas même en présentant un marqueur vide.
    #[test]
    fn sans_secret_attendu_rien_ne_declare() {
        let cookie = entetes("cookie", &format!("{}=", lj_dtos::INTERNAL_ORIGIN_COOKIE));
        assert!(!internal_origin(&cookie, None));
        assert!(!internal_origin(&cookie, Some("")));
    }
}
