//! Couche de fetch de la recherche.
//!
//! On utilise `leptos-fetch` (`QueryClient` fourni au contexte racine) via
//! `QueryClient::resource` : SWR par clé `(query, filtres, page)`, dédup des nav
//! vers la même recherche.
//!
//! `run_search` est `Send` sous SSR (reqwest tokio, auth no-op) et `!Send` sous
//! wasm (le client HTTP fait `get_access_token().await`, un `JsFuture`). On le
//! passe à travers `sendable` (identité sous SSR ; `SendWrapper` sous wasm) pour
//! satisfaire la borne `Send` de `resource`. La recherche SSR est anonyme.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_fetch::{QueryClient, QueryScope};
use lj_dtos::SearchResponse;

use crate::api::ApiClient;
use crate::pages::decision_page::data::sendable;

use super::search_state::{request_from_key, SearchKey};

/// Clés de recherche déjà rendues cette session : l'animation « rise » des
/// cartes ne joue qu'à la PREMIÈRE apparition d'une clé. Portées par un
/// `StoredValue` fourni au contexte racine par [`provide_seen_search_keys`],
/// donc elles SURVIVENT au remontage de `SearchPage`.
#[derive(Clone, Copy)]
pub struct SeenSearchKeys(pub StoredValue<HashSet<SearchKey>>);

/// À appeler une fois dans `App` (racine persistante à travers les navigations).
pub fn provide_seen_search_keys() {
    provide_context(SeenSearchKeys(StoredValue::new(HashSet::new())));
}

/// Résultat d'une recherche : la réponse, ou un message d'erreur.
pub type SearchResult = Result<SearchResponse, String>;

/// Effectue une recherche via le client API (base URL résolue par cible).
async fn run_search(key: SearchKey) -> SearchResult {
    let client = ApiClient::from_context();
    let request = request_from_key(&key);
    client
        .search(&request, lj_dtos::SearchContext::User)
        .await
        .map_err(|e| e.message)
}

/// Scope de cache partagé : `QueryScope::new` dérive sa `cache_key` du `TypeId`
/// du fetcher, donc cette closure définie une seule fois donne une clé stable
/// entre appels. La `Resource` et `subscribe_is_loading` doivent viser la MÊME
/// entrée de cache.
fn search_scope() -> QueryScope<SearchKey, SearchResult> {
    QueryScope::new(|key: SearchKey| sendable(run_search(key)))
}

/// Clé « enabled » : `None` quand la requête est vide → la recherche n'est pas
/// lancée.
fn enabled_key(key: Signal<SearchKey>) -> Option<SearchKey> {
    let k = key.get();
    if k.query.is_empty() {
        None
    } else {
        Some(k)
    }
}

/// `Resource` de recherche, clé sur l'état URL. La valeur est
/// `Option<SearchResult>` : `None` quand la requête est vide, sinon le résultat.
/// SSR-streamable : prime la liste côté serveur tout en conservant le cache SWR
/// client.
pub fn search_resource(key: Signal<SearchKey>) -> Resource<Option<SearchResult>> {
    let client = expect_context::<QueryClient>();
    // Non-bloquant (streaming, pas `resource_blocking`) : le shell (barre + rail +
    // skeleton) est émis immédiatement et la liste arrive en chunk out-of-order.
    // La page de recherche n'est pas indexée. Les facettes du rail sont
    // alimentées par un `Effect` client.
    client.resource(search_scope(), move || enabled_key(key))
}

/// Lecture SYNCHRONE du cache pour une clé donnée. `Some` au retour depuis une
/// décision (recherche déjà chargée cette session) → la liste est rendue dans la
/// frame de montage. `None` au premier chargement (cache froid) → le `<Suspense>`
/// prend le relais (streaming SSR).
pub fn cached_search(key: &SearchKey) -> Option<SearchResult> {
    let client = expect_context::<QueryClient>();
    client.get_cached_query(search_scope(), key)
}

/// Signal `true` tant que la recherche pour la clé courante charge POUR LA
/// PREMIÈRE FOIS (aucune donnée en cache). Reste `false` sur une revisite servie
/// du cache et sur un refetch SWR où des données précédentes existent. Pilote le
/// skeleton.
pub fn search_is_loading(key: Signal<SearchKey>) -> Signal<bool> {
    let client = expect_context::<QueryClient>();
    client.subscribe_is_loading(search_scope(), move || enabled_key(key))
}
