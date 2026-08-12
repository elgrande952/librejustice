//! Budget de coût par appelant : seau à jetons borné, clé identité ou réseau.
//!
//! Ce qu'on compte n'est pas la requête HTTP mais la dépense : un appel
//! d'embedding, un rerank Mistral. Tout ce que les caches moka servent est gratuit
//! et ne prélève rien. Corollaire à tenir quand un cache s'ajoute : le prélèvement
//! descend avec lui.
//!
//! Les plafonds vivent dans l'environnement
//! (`LIBREJUSTICE_API_SEARCH_BUDGET_PER_MIN`, `…_RERANK_BUDGET_PER_MIN`,
//! `…_SEARCH_BUDGET_ENFORCE`) ; ceux de `config.rs` sont des défauts génériques.
//!
//! Fail-open : un appelant qu'on ne sait pas nommer n'est pas limité
//! ([`caller_key`] rend `None`). Et le refus n'appartient pas à ce module — il rend
//! un verdict, l'appelant décide quoi en faire.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use moka::future::Cache;
use tokio::sync::Mutex;

/// Cible tracing des dépassements, ouverte à `INFO` dans le bridge logs de l'API
/// (`crate::telemetry`). La changer ici sans la changer là coupe le signal.
pub const LOG_TARGET: &str = "lj_api::ratelimit";

/// Nombre de seaux gardés en mémoire. Au-delà, moka évince les moins récemment
/// utilisés : le `time_to_idle` valant le temps de recharge complète, un seau
/// évincé serait de toute façon revenu plein.
const MAX_KEYS: u64 = 100_000;

/// Sondes d'hydratation par appelant et par minute. Pas de réglage
/// d'environnement.
pub const HYDRATION_PROBES_PER_MIN: u32 = 5;

/// Rafale tolérée, en multiple du débit par minute. Seul le régime soutenu
/// rencontre la recharge.
const BURST_FACTOR: f64 = 2.0;

/// Verdict d'un prélèvement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Jeton prélevé : la dépense peut avoir lieu.
    Allowed,
    /// Seau vide : l'appelant a dépassé son débit soutenu.
    Exhausted,
}

impl Verdict {
    /// Valeur d'attribut pour la télémétrie.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allowed => "allowed",
            Verdict::Exhausted => "exhausted",
        }
    }
}

/// Clé de comptage d'un appelant. Construite par [`caller_key`] uniquement : le
/// type est opaque pour qu'aucun appelant ne bricole une clé à la main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerKey(String);

impl CallerKey {
    /// Nature de l'appelant pour la télémétrie — jamais la clé elle-même, qui porte
    /// un `sub` ou une adresse.
    fn kind(&self) -> &'static str {
        if self.0.starts_with("u:") {
            "account"
        } else {
            "network"
        }
    }
}

/// Clé de comptage d'un appel : le compte s'il est authentifié, sinon le réseau.
/// L'identité ne part jamais en télémétrie : elle vit dans ce module et dans les
/// seaux.
///
/// Anonyme : l'IP lue dans `CF-Connecting-IP`. Cet en-tête n'est digne de foi que
/// parce que l'origine n'accepte `:443` que des plages Cloudflare.
///
/// `None` = appelant non identifiable, donc non limité. Le seul cas est l'appel
/// in-process du rendu SSR (`lj_web::api::client`), qui n'a pas de socket client.
pub fn caller_key(user: Option<&str>, headers: &HeaderMap) -> Option<CallerKey> {
    if let Some(sub) = user {
        return Some(CallerKey(format!("u:{sub}")));
    }
    client_network(headers).map(CallerKey)
}

/// Réseau de l'appelant. L'IPv6 est ramenée à son `/64` : un hôte v6 s'en voit
/// attribuer un entier et change d'adresse dedans sans rien payer.
fn client_network(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("cf-connecting-ip")?.to_str().ok()?;
    match raw.parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => Some(format!("n:{v4}")),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            Some(format!(
                "n:{:x}:{:x}:{:x}:{:x}::/64",
                s[0], s[1], s[2], s[3]
            ))
        }
    }
}

/// Seau à jetons d'un appelant.
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Budget partagé d'une surface (recherche, rerank…). Un seau par appelant,
/// recharge continue.
#[derive(Clone)]
pub struct Budget {
    surface: &'static str,
    buckets: Cache<String, Arc<Mutex<Bucket>>>,
    capacity: f64,
    refill_per_sec: f64,
}

impl Budget {
    /// Construit un budget à `per_min` dépenses par minute en régime soutenu, avec
    /// [`BURST_FACTOR`] minutes de rafale accumulable. `surface` nomme le poste de
    /// dépense dans la télémétrie (`search`, `rerank`…).
    pub fn new(surface: &'static str, per_min: u32) -> Self {
        let refill_per_sec = f64::from(per_min) / 60.0;
        let capacity = f64::from(per_min) * BURST_FACTOR;
        Self {
            surface,
            buckets: Cache::builder()
                .max_capacity(MAX_KEYS)
                .time_to_idle(Duration::from_secs_f64(capacity / refill_per_sec))
                .build(),
            capacity,
            refill_per_sec,
        }
    }

    /// Prélève un jeton pour `key`. `None` (appelant non identifiable) passe
    /// toujours.
    pub async fn take(&self, key: Option<&CallerKey>) -> Verdict {
        self.take_at(key, Instant::now()).await
    }

    /// [`Budget::take`] à instant imposé — les tests pilotent le temps.
    async fn take_at(&self, key: Option<&CallerKey>, now: Instant) -> Verdict {
        let Some(key) = key else {
            return Verdict::Allowed;
        };
        let capacity = self.capacity;
        let bucket = self
            .buckets
            .get_with(key.0.clone(), async move {
                Arc::new(Mutex::new(Bucket {
                    tokens: capacity,
                    last: now,
                }))
            })
            .await;
        let mut bucket = bucket.lock().await;
        // `saturating_duration_since` : deux tâches concurrentes peuvent avoir lu
        // `Instant::now()` dans l'ordre inverse de leur prise du mutex.
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.last = now;
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Verdict::Allowed;
        }
        tracing::info!(
            target: LOG_TARGET,
            surface = self.surface,
            caller = key.kind(),
            verdict = Verdict::Exhausted.as_str(),
            "budget épuisé"
        );
        Verdict::Exhausted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> CallerKey {
        CallerKey(s.to_string())
    }

    fn headers(name: &'static str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, value.parse().unwrap());
        h
    }

    #[tokio::test]
    async fn burst_then_refill() {
        let budget = Budget::new("test", 60);
        let k = key("u:alice");
        let t0 = Instant::now();
        for _ in 0..120 {
            assert_eq!(budget.take_at(Some(&k), t0).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&k), t0).await, Verdict::Exhausted);
        let t1 = t0 + Duration::from_secs(10);
        for _ in 0..10 {
            assert_eq!(budget.take_at(Some(&k), t1).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&k), t1).await, Verdict::Exhausted);
    }

    #[tokio::test]
    async fn refill_is_capped_at_capacity() {
        let budget = Budget::new("test", 60);
        let k = key("u:alice");
        let t0 = Instant::now();
        assert_eq!(budget.take_at(Some(&k), t0).await, Verdict::Allowed);
        let t1 = t0 + Duration::from_secs(3600);
        for _ in 0..120 {
            assert_eq!(budget.take_at(Some(&k), t1).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&k), t1).await, Verdict::Exhausted);
    }

    #[tokio::test]
    async fn buckets_are_independent() {
        let budget = Budget::new("test", 1);
        let (a, b) = (key("u:alice"), key("n:203.0.113.7"));
        let t0 = Instant::now();
        for _ in 0..2 {
            assert_eq!(budget.take_at(Some(&a), t0).await, Verdict::Allowed);
        }
        assert_eq!(budget.take_at(Some(&a), t0).await, Verdict::Exhausted);
        assert_eq!(budget.take_at(Some(&b), t0).await, Verdict::Allowed);
    }

    #[tokio::test]
    async fn unidentified_caller_is_never_limited() {
        let budget = Budget::new("test", 1);
        let t0 = Instant::now();
        for _ in 0..1000 {
            assert_eq!(budget.take_at(None, t0).await, Verdict::Allowed);
        }
    }

    #[test]
    fn identity_wins_over_network() {
        let h = headers("cf-connecting-ip", "203.0.113.7");
        assert_eq!(caller_key(Some("sub-42"), &h), Some(key("u:sub-42")));
    }

    #[test]
    fn anonymous_falls_back_to_network() {
        let h = headers("cf-connecting-ip", "203.0.113.7");
        assert_eq!(caller_key(None, &h), Some(key("n:203.0.113.7")));
    }

    #[test]
    fn ipv6_is_grouped_by_64() {
        let a = caller_key(None, &headers("cf-connecting-ip", "2001:db8:1:2:3:4:5:6"));
        let b = caller_key(None, &headers("cf-connecting-ip", "2001:db8:1:2:ffff::1"));
        assert_eq!(a, b);
        assert_eq!(a, Some(key("n:2001:db8:1:2::/64")));
        let c = caller_key(None, &headers("cf-connecting-ip", "2001:db8:1:3::1"));
        assert_ne!(a, c);
    }

    #[test]
    fn no_client_ip_means_unidentified() {
        assert_eq!(caller_key(None, &HeaderMap::new()), None);
        assert_eq!(
            caller_key(None, &headers("cf-connecting-ip", "pas-une-ip")),
            None
        );
    }
}
