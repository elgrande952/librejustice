//! Scraper des caches moka in-process : occupation et taux de succès.
//!
//! Un plafond se règle sur l'occupation observée. Celui du cache parcours a valu
//! 5 000 entrées de 421 Kio, soit 2 Gio, sans que rien ne le montre.
//!
//! `entry_count` et `weighted_size` ne sont exacts qu'après `run_pending_tasks`,
//! d'où l'instantané asynchrone : les callbacks OTel sont synchrones. Et les
//! caches étant par processus, ce scraper tourne dans tous les environnements,
//! à l'inverse de `host_metrics` et `pg_metrics`.

use opentelemetry::{global, KeyValue};
use std::sync::Mutex;
use std::time::Duration;
use tracing::info;

use crate::state::AppState;

/// Période de scrape (1 DPM, free tier).
const SCRAPE_INTERVAL: Duration = Duration::from_secs(60);

/// Occupation d'un cache au dernier tour.
struct Usage {
    cache: &'static str,
    entries: u64,
    weight: u64,
    capacity: Option<u64>,
}

static SNAPSHOT: Mutex<Vec<Usage>> = Mutex::new(Vec::new());

/// Un accès au cache vu de l'appelant : `hit` = il n'a pas payé le calcul. Un
/// suiveur de single-flight compte donc comme un succès.
pub(crate) fn record_access(cache: &'static str, hit: bool) {
    let counter = global::meter("librejustice-cache")
        .u64_counter("librejustice.cache.requests")
        .with_description("Accès au cache, par issue.")
        .build();
    counter.add(
        1,
        &[
            KeyValue::new("cache", cache),
            KeyValue::new("outcome", if hit { "hit" } else { "miss" }),
        ],
    );
}

/// Démarre le scraper. Conçu pour `tokio::spawn`.
pub async fn run(state: AppState) {
    register_gauges();
    info!(
        interval_secs = SCRAPE_INTERVAL.as_secs(),
        "cache_metrics : scraper démarré"
    );

    let mut ticker = tokio::time::interval(SCRAPE_INTERVAL);
    loop {
        ticker.tick().await;
        scrape(&state).await;
    }
}

fn register_gauges() {
    let meter = global::meter("librejustice-cache");

    let _entries = meter
        .u64_observable_gauge("librejustice.cache.entries")
        .with_description("Entrées vivantes dans le cache.")
        .with_unit("{entry}")
        .with_callback(|observer| {
            for u in SNAPSHOT.lock().unwrap().iter() {
                observer.observe(u.entries, &[KeyValue::new("cache", u.cache)]);
            }
        })
        .build();

    let _utilization = meter
        .f64_observable_gauge("librejustice.cache.utilization")
        .with_description(
            "Fraction du plafond occupée, au sens du `weigher` : 1 signifie que \
             le cache évince à chaque insertion.",
        )
        .with_unit("1")
        .with_callback(|observer| {
            for u in SNAPSHOT.lock().unwrap().iter() {
                if let Some(cap) = u.capacity.filter(|c| *c > 0) {
                    observer.observe(
                        u.weight as f64 / cap as f64,
                        &[KeyValue::new("cache", u.cache)],
                    );
                }
            }
        })
        .build();
}

/// Un tour de mesure. Les caches à clé unique (`Cache<(), _>`) sont dehors :
/// leur plafond vaut une entrée, il n'y a rien à régler.
async fn scrape(state: &AppState) {
    macro_rules! usage {
        ($name:literal, $cache:expr) => {{
            let c = &$cache;
            c.run_pending_tasks().await;
            Usage {
                cache: $name,
                entries: c.entry_count(),
                weight: c.weighted_size(),
                capacity: c.policy().max_capacity(),
            }
        }};
    }

    let mut out = vec![
        usage!("recherche", state.search_cache),
        usage!("parcours", state.browse_cache),
        usage!("textes", state.textes_cache),
        usage!("rerank", state.rerank_cache),
        usage!("notes_rerank", state.rerank_scores_cache),
        usage!("annonces", state.annonces_cache),
        usage!("registre", state.registre_cache),
        usage!("texte_identite", state.text_ref_cache),
    ];
    let (entries, weight, capacity) = state.embedding_cache.usage().await;
    out.push(Usage {
        cache: "embedding",
        entries,
        weight,
        capacity,
    });

    *SNAPSHOT.lock().unwrap() = out;
}
