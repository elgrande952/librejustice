//! Signal d'usage de la recherche, commun aux deux moteurs.
//!
//! Une recherche — décisions ou textes, web ou MCP — émet deux signaux portant le
//! même vocabulaire `librejustice.search.*` : le span `search` (attributs posés
//! avant l'exécution, `results_count` après, donc une recherche qui échoue reste
//! attribuable) et un event `INFO` sur la cible [`LOG_TARGET`], dont les champs
//! sont les mêmes en snake_case nu.
//!
//! Aucune identité dans l'un ni l'autre : `identity` dit connecté ou non, jamais
//! qui, et la query part intacte. Le `trace_id` de l'event relie les deux.

use lj_dtos::{ActivitySource, SearchContext, SearchEngine};

/// Cible tracing des events d'usage. Seule cible que le bridge logs de l'API
/// laisse passer sous `WARN` : la changer ici sans la changer dans
/// `telemetry::init_telemetry` coupe le signal.
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
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub source: ActivitySource,
    pub authenticated: bool,
    pub context: SearchContext,
    /// Le front avait une session en storage mais n'a pas obtenu son jeton
    /// ([`lj_dtos::SESSION_DEGRADED_HEADER`]) : l'appel est compté anonyme. Toujours
    /// `false` en MCP.
    pub session_degraded: bool,
}

impl Caller {
    /// Appel web anonyme en contexte utilisateur — le rendu SSR in-process.
    pub fn web_anonymous(context: SearchContext) -> Self {
        Self {
            source: ActivitySource::Web,
            authenticated: false,
            context,
            session_degraded: false,
        }
    }

    /// Provenance d'un appel web, lue aux bords : identité résolue par
    /// `OptionalUser`, dégradation de session par son header.
    pub fn web(
        authenticated: bool,
        context: SearchContext,
        headers: &axum::http::HeaderMap,
    ) -> Self {
        Self {
            source: ActivitySource::Web,
            authenticated,
            context,
            session_degraded: headers.contains_key(lj_dtos::SESSION_DEGRADED_HEADER),
        }
    }

    /// Pose les attributs d'usage sur le span courant, avant l'exécution.
    pub(crate) fn record_span(self, engine: SearchEngine, query: &str, shape: Shape<'_>) {
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
        span.record(
            "librejustice.search.session",
            session_value(self.session_degraded),
        );
        span.record("librejustice.search.context", context_value(self.context));
        span.record("librejustice.search.query", query);
        span.record("librejustice.search.page", shape.page);
        span.record("librejustice.search.filters", shape.filters);
    }

    /// Complète le span (`results_count`) et émet l'event d'usage vers Loki.
    pub(crate) fn record_outcome(
        self,
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
            session = session_value(self.session_degraded),
            context = context_value(self.context),
            query = query,
            page = shape.page,
            filters = shape.filters,
            results_count = results_count,
            "recherche"
        );
    }
}

/// `ok` | `degraded` : `degraded` = le front avait une session en storage mais
/// pas de jeton, l'appel est donc compté anonyme à tort.
pub(crate) fn session_value(degraded: bool) -> &'static str {
    if degraded {
        "degraded"
    } else {
        "ok"
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

/// `user` | `teaser` : un `teaser` est le fetch machine du pont croisé entre
/// moteurs, à exclure de tout comptage d'usage.
pub(crate) fn context_value(context: SearchContext) -> &'static str {
    match context {
        SearchContext::User => "user",
        SearchContext::Teaser => "teaser",
    }
}
