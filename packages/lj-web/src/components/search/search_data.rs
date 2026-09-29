//! Couche de fetch de la recherche.
//!
//! On utilise `leptos-fetch` (`QueryClient` fourni au contexte racine) via
//! `QueryClient::resource` : SWR par clé `(query, filtres, page)`, dédup des nav
//! vers la même recherche.
//!
//! `run_search` est `Send` sous SSR (reqwest tokio) et `!Send` sous wasm (le
//! client HTTP est le `fetch` du navigateur, dont la future tient des `JsValue`).
//! On le passe à travers `sendable` (identité sous SSR ; `SendWrapper` sous wasm)
//! pour satisfaire la borne `Send` de `resource`. La recherche SSR est anonyme.

use std::collections::HashSet;

use crate::api::client::SearchFailure;
use leptos::prelude::*;
use leptos_fetch::{QueryClient, QueryScope};
use lj_dtos::{SearchCountResponse, SearchResponse};

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

/// Résultat d'une recherche : la réponse, ou de quoi le dire à l'utilisateur.
pub type SearchResult = Result<SearchResponse, SearchFailure>;

/// Résultat du décompte, qui arrive après la liste.
pub type CountResult = Result<SearchCountResponse, SearchFailure>;

/// Effectue une recherche via le client API (base URL résolue par cible).
async fn run_search(key: SearchKey) -> SearchResult {
    let client = ApiClient::from_context();
    let request = request_from_key(&key);
    client
        .search(&request, lj_dtos::SearchContext::User)
        .await
        .map_err(|e| {
            #[cfg(feature = "hydrate")]
            crate::incident::signaler_recherche_refusee(e.status);
            SearchFailure::from_error(&e, &key.query)
        })
}

/// Décompte et facettes : second appel, lancé en même temps que la recherche.
async fn run_count(key: SearchKey) -> CountResult {
    let client = ApiClient::from_context();
    let request = request_from_key(&key);
    client
        .search_count(&request)
        .await
        .map_err(|e| SearchFailure::from_error(&e, &key.query))
}

/// Scope de cache partagé : `QueryScope::new` dérive sa `cache_key` du `TypeId`
/// du fetcher, donc cette closure définie une seule fois donne une clé stable
/// entre appels. La `Resource` et `subscribe_is_loading` doivent viser la MÊME
/// entrée de cache.
fn search_scope() -> QueryScope<SearchKey, SearchResult> {
    QueryScope::new(|key: SearchKey| sendable(run_search(key)))
}

/// `Resource` de recherche, clé sur l'état URL. Une requête vide n'est pas un
/// état inerte : c'est le mode parcours, servi par le même appel.
/// SSR-streamable : prime la liste côté serveur tout en conservant le cache SWR
/// client.
pub fn search_resource(key: Signal<SearchKey>) -> Resource<SearchResult> {
    let client = expect_context::<QueryClient>();
    // Non-bloquant (streaming, pas `resource_blocking`) : le shell (barre + rail +
    // skeleton) est émis immédiatement et la liste arrive en chunk out-of-order.
    // La page de recherche n'est pas indexée. Les facettes du rail sont
    // alimentées par un `Effect` client.
    client.resource(search_scope(), move || key.get())
}

fn count_scope() -> QueryScope<SearchKey, CountResult> {
    QueryScope::new(|key: SearchKey| sendable(run_count(key)))
}

/// `Resource` du décompte, sur la part de la clé dont il dépend
/// ([`SearchKey::count_key`]) : il part en même temps que la liste, le serveur
/// les rattache au même bundle, et tourner une page ne le rejoue pas.
pub fn count_resource(key: Signal<SearchKey>) -> Resource<CountResult> {
    let client = expect_context::<QueryClient>();
    client.resource(count_scope(), move || key.get().count_key())
}

/// What the « Textes cités » input names, and the search it is counted on. The
/// count key and not the full one: paging does not change what a text is worth.
pub type InstrumentKey = (SearchKey, String);

/// Empty needle, no call: the panel then shows the facet, which the search has
/// already brought back.
async fn run_instruments(key: InstrumentKey) -> lj_dtos::LegalInstrumentSuggestions {
    let (search, needle) = key;
    if needle.trim().chars().count() < 2 {
        return lj_dtos::LegalInstrumentSuggestions::default();
    }
    ApiClient::from_context()
        .search_instruments(&request_from_key(&search), &needle)
        .await
        .unwrap_or_default()
}

fn instruments_scope() -> QueryScope<InstrumentKey, lj_dtos::LegalInstrumentSuggestions> {
    QueryScope::new(|key: InstrumentKey| sendable(run_instruments(key)))
}

pub fn instruments_resource(
    key: Signal<InstrumentKey>,
) -> Resource<lj_dtos::LegalInstrumentSuggestions> {
    expect_context::<QueryClient>().resource(instruments_scope(), move || key.get())
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
    client.subscribe_is_loading(search_scope(), move || key.get())
}
