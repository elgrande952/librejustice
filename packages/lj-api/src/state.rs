//! État partagé axum : pool DB, settings, caches in-process, client Mistral.
//! Les ressources coûteuses sont construites une fois au démarrage ; `AppState`
//! est `Clone`, tous ses champs étant des handles partagés.

use crate::config::Settings;
use crate::embedder::build_query_embedder;
use crate::referential::Referential;
use crate::search::RankedResults;
use deadpool_postgres::Pool;
use lj_dtos::CorpusStatsResponse;
use lj_llm::backend::AnyEmbedder;
use lj_llm::cache::EmbeddingCache;
use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;

/// Plafond mémoire du cache résultats, en octets.
const SEARCH_CACHE_MAX_BYTES: u64 = 150 * 1024 * 1024;
/// Overhead moka par entrée, ajouté au poids pesé.
const ENTRY_OVERHEAD: usize = 128;
/// Plafond du cache rerank, en nombre d'entrées.
const RERANK_CACHE_MAX_ENTRIES: u64 = 50_000;
/// Plafond du cache de notes de rerank : une entrée par (query, document).
const RERANK_SCORES_CACHE_MAX_ENTRIES: u64 = 100_000;

/// Plafond du cache des pages d'annonces, en nombre d'entrées.
const ANNONCES_CACHE_MAX_ENTRIES: u64 = 2_000;

/// Plafond du cache registre, en nombre d'entrées.
const REGISTRE_CACHE_MAX_ENTRIES: u64 = 20_000;

/// Plafond du cache d'identité de texte, en nombre d'entrées.
const TEXT_REF_CACHE_MAX_ENTRIES: u64 = 50_000;

/// Pic de connexions DB qu'une recherche tient simultanément (les trois jambes
/// de `retrieve_hybrid`). [`AppState::search_permits`] en dérive.
const PEAK_CONNS_PER_SEARCH: usize = 3;

/// État applicatif injecté dans les handlers (`State<AppState>`).
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub pool: Pool,
    pub embedder: Arc<AnyEmbedder>,
    pub embedding_cache: EmbeddingCache,
    pub search_cache: Cache<String, Arc<RankedResults>>,
    pub rerank_cache: Cache<String, Arc<Vec<i64>>>,
    /// Notes de rerank par `(query, decision_id)`. Elles survivent au changement de
    /// shortlist, là où l'ordre du `rerank_cache` ne survit pas.
    pub rerank_scores_cache: Cache<(Arc<str>, i64), crate::rerank::DocScores>,
    /// Budget de recherche par appelant, décrémenté sur MISS du `search_cache`.
    /// En mode compteur tant que `settings.search_budget_enforce` est faux.
    pub search_budget: crate::ratelimit::Budget,
    /// Budget de rerank par appelant, décrémenté sur MISS du `rerank_cache`. Le
    /// dépassement rend l'ordre RRF, jamais une erreur.
    pub rerank_budget: crate::ratelimit::Budget,
    /// Budget des sondes d'hydratation (`/api/client/hydration-failure`), non
    /// authentifiées.
    pub hydration_budget: crate::ratelimit::Budget,
    /// Cache mono-entrée du référentiel de facettes. Accès via
    /// `referential::referential`.
    pub referential_cache: Cache<(), Arc<Referential>>,
    /// Cache mono-entrée des compteurs corpus de la page d'accueil. Accès via
    /// `stats::corpus_stats`.
    pub corpus_stats_cache: Cache<(), Arc<CorpusStatsResponse>>,
    /// Cache mono-entrée du catalogue des juridictions. Accès via
    /// `jurisdiction_hubs::catalogue`.
    pub jurisdiction_catalogue_cache: Cache<(), Arc<lj_dtos::JurisdictionCatalogueResponse>>,
    /// Cache mono-entrée du catalogue des normes. Accès via `norm_hubs::catalogue`.
    pub norm_catalogue_cache: Cache<(), Arc<lj_dtos::NormCatalogueResponse>>,
    /// Cache des pages de `/annonces`, clé = filtre sérialisé. Accès via
    /// `annonces::search`.
    pub annonces_cache: Cache<String, Arc<lj_dtos::AnnonceSearchResponse>>,
    /// Cache mono-entrée du catalogue des annonces.
    pub annonces_catalogue_cache: Cache<(), Arc<lj_dtos::AnnonceCatalogueResponse>>,
    /// Borne les recherches qui tapent la DB en même temps à
    /// `pool_max / PEAK_CONNS_PER_SEARCH`, pour que toute recherche admise puisse
    /// acquérir toutes ses connexions. Au-delà, les excédentaires attendent un permit.
    pub search_permits: Arc<tokio::sync::Semaphore>,
    /// Cache du volet registre des fiches entité, une entrée par uid. Accès via
    /// `registre::entity_registre`.
    pub registre_cache: Cache<String, Arc<lj_dtos::EntityRegistreResponse>>,
    /// Cache mono-entrée du FST d'autocomplétion. Accès via `routes::suggest_index`.
    pub suggest_cache: Cache<(), Arc<crate::search::suggest::SuggestIndex>>,
    /// Cache de la résolution `slug` → identité de texte. L'entrée `None` est cachée
    /// aussi. Accès via `legi::resolve_text`.
    pub text_ref_cache: Cache<String, Option<Arc<crate::legi::TextRef>>>,
    /// Cache de la fraîcheur d'ingest par source. Accès via `legi::ingest_freshness`.
    pub ingest_freshness_cache: Cache<String, Option<chrono::NaiveDate>>,
    /// Client HTTP partagé des appels registre.
    pub registre_http: reqwest::Client,
    /// Client Mistral du rerank listwise. `None` si le pool de clés est vide.
    pub mistral: Option<Arc<lj_llm::mistral::MistralClient>>,
}

impl AppState {
    /// Construit l'état applicatif au démarrage.
    pub fn build(settings: Arc<Settings>, pool: Pool) -> Self {
        let embedder = Arc::new(build_query_embedder(&settings));
        let settings_search_budget = settings.search_budget_per_min;
        let settings_rerank_budget = settings.rerank_budget_per_min;
        let search_cache = Cache::builder()
            .weigher(|k: &String, v: &Arc<RankedResults>| {
                (k.len() + v.approx_weight() as usize + ENTRY_OVERHEAD).min(u32::MAX as usize)
                    as u32
            })
            .max_capacity(SEARCH_CACHE_MAX_BYTES)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let rerank_cache = Cache::builder()
            .max_capacity(RERANK_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let rerank_scores_cache = Cache::builder()
            .max_capacity(RERANK_SCORES_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(24 * 3600))
            .build();
        let referential_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let corpus_stats_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        let jurisdiction_catalogue_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        let norm_catalogue_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(12 * 3600))
            .build();
        let annonces_cache = Cache::builder()
            .max_capacity(ANNONCES_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(15 * 60))
            .build();
        let annonces_catalogue_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let search_permits = Arc::new(tokio::sync::Semaphore::new(
            (settings.pool_max / PEAK_CONNS_PER_SEARCH).max(1),
        ));
        // Volet registre : payloads de quelques Ko, plafond par nombre
        // d'entrées. TTL 24 h — chronologies et dirigeants bougent au rythme
        // des parutions, pas des requêtes.
        let registre_cache = Cache::builder()
            .max_capacity(REGISTRE_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(24 * 3600))
            .build();
        // FST d'autocomplétion : une seule entrée (clé `()`), ~dizaines de Mo,
        // rechargé de la DB à l'expiration (rebuild offline au plus quotidien).
        let suggest_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(24 * 3600))
            .build();
        // Identité de texte par slug : plafond par nombre d'entrées (payload
        // minuscule), TTL 1 h comme le référentiel de facettes — un texte
        // nouvellement ingéré devient adressable dans l'heure.
        let text_ref_cache = Cache::builder()
            .max_capacity(TEXT_REF_CACHE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(3600))
            .build();
        // Fraîcheur d'ingest : une entrée par source (poignée de valeurs),
        // rafraîchie dans l'heure suivant le cron quotidien.
        let ingest_freshness_cache = Cache::builder()
            .max_capacity(64)
            .time_to_live(Duration::from_secs(3600))
            .build();
        let registre_http = reqwest::Client::builder()
            .timeout(crate::registre::UPSTREAM_TIMEOUT)
            .user_agent("librejustice.fr")
            .build()
            .expect("client HTTP registre");
        // Client Mistral construit sur le pool ENTIER : les clés désactivées en
        // base lui sont signalées à chaque rerank (`set_dead_fingerprints`), ce
        // qui laisse une clé renouvelée revenir sans redémarrer le serveur.
        let mistral = (!settings.mistral_api_keys.is_empty())
            .then(|| {
                lj_llm::mistral::MistralClient::new(
                    settings.mistral_api_keys.clone(),
                    settings.mistral_model.clone(),
                )
                .map(Arc::new)
                .inspect_err(
                    |err| tracing::warn!(error = %err, "client Mistral — rerank désactivé"),
                )
                .ok()
            })
            .flatten();
        Self {
            settings,
            pool,
            embedder,
            embedding_cache: EmbeddingCache::default(),
            search_cache,
            rerank_cache,
            rerank_scores_cache,
            search_budget: crate::ratelimit::Budget::new("search", settings_search_budget),
            rerank_budget: crate::ratelimit::Budget::new("rerank", settings_rerank_budget),
            hydration_budget: crate::ratelimit::Budget::new(
                "hydration",
                crate::ratelimit::HYDRATION_PROBES_PER_MIN,
            ),
            referential_cache,
            corpus_stats_cache,
            jurisdiction_catalogue_cache,
            norm_catalogue_cache,
            annonces_cache,
            annonces_catalogue_cache,
            search_permits,
            registre_cache,
            registre_http,
            suggest_cache,
            text_ref_cache,
            ingest_freshness_cache,
            mistral,
        }
    }
}
