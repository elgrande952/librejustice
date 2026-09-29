//! Cache de requetes (leptos-fetch).
//!
//! - `stale_time` 1 h (aligne sur le TTL du cache recherche API) : tant que la
//!   donnee est fresh, un remontage ou une nav SPA de retour vers une recherche
//!   deja en cache ne refetch pas.
//! - `gc_time` 2 h (> `stale_time`) : une entree inactive survit tout le temps ou
//!   elle est fresh (1 h) PUIS une fenetre stale-while-revalidate (1 h de plus) ou
//!   un retour de nav peint la version stale instantanement puis revalide en
//!   arriere-plan.
//!
//! Un chargement de `/decision-search?q=…` n'execute aucune recherche cote serveur : le
//! SSR peint le squelette (`search_is_loading` vrai, la ressource n'est jamais
//! lue). Tout span `search` `context=user` vient donc du fetch CLIENT.
//!
//! leptos-fetch gere la dualite SSR/navigateur en interne (instance fournie au
//! contexte ; dehydrate/rehydrate des ressources via le SharedContext leptos). On
//! fournit donc une seule fois au contexte racine.

use std::time::Duration;

use leptos_fetch::{QueryClient, QueryOptions};

/// staleTime aligne sur le TTL du cache recherche API (1 h).
const STALE_TIME: Duration = Duration::from_secs(60 * 60);
/// gcTime 2 h : > staleTime, fenetre stale-while-revalidate (voir doc module).
const GC_TIME: Duration = Duration::from_secs(2 * 60 * 60);

/// Construit le `QueryClient` (SWR) et le fournit au contexte Leptos. Appele une
/// fois dans `App`. Renvoie le client pour un usage immediat eventuel.
pub fn provide_query_client() -> QueryClient {
    QueryClient::new()
        .with_options(
            QueryOptions::new()
                .with_stale_time(STALE_TIME)
                .with_gc_time(GC_TIME),
        )
        .provide()
}
