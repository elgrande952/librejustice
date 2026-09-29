//! Sticky decision bar: shared context and search-to-decision state handoff.
//!
//! `leptos_router` has no rich equivalent of `location.state`, so two reactive
//! contexts provided to the shell stand in for it:
//! - `DecisionBarSignal`: what the `TopBar` shows (title, nav, back).
//! - `ResultNavSignal`: a seed set on click by a result card, a neighbour or the
//!   prev-next widget, read ONCE by `DecisionHeader` on mount to build the bar.
//!   One-shot: a decision link without a seed (bookmarks, direct link) never
//!   reuses a stale nav.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Origine « recherche » d'une consultation : permet au bouton retour de revenir
/// à la liste exacte (query string) et de restaurer le scroll.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FromSearch {
    /// Query string complète (`?q=…&page=…`), telle quelle.
    pub search: String,
    /// Position de scroll à restaurer au retour (px).
    pub scroll_y: f64,
}

/// Navigation inter-résultats : la liste servie, dont se dérivent le rang, le
/// compte et précédent/suivant au chemin courant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultNav {
    pub hit_paths: Vec<String>,
}

/// État affiché par la barre sticky.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionBarState {
    pub title: String,
    pub path: String,
    pub nav: Option<ResultNav>,
    pub from_search: Option<FromSearch>,
}

/// Graine posée au clic, consommée par `DecisionHeader`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultNavSeed {
    pub nav: Option<ResultNav>,
    pub from_search: Option<FromSearch>,
}

pub type DecisionBarSignal = RwSignal<Option<DecisionBarState>>;
pub type ResultNavSignal = RwSignal<Option<ResultNavSeed>>;
/// Graines consommées, par id de décision. Filet du back/forward navigateur :
/// décision → article de loi → retour arrière remonte la page SANS graine, le
/// one-shot ayant été consommé à la première visite. Clé = id : une décision
/// atteinte sans graine ne récupère que SON dernier contexte, jamais celui d'une
/// autre.
pub type SeedMemorySignal = StoredValue<std::collections::HashMap<String, ResultNavSeed>>;
/// Position de scroll à restaurer sur la page résultats au retour depuis une
/// décision. Posée par le bouton retour de la barre décision, consommée UNE fois
/// par `ResultsBody` à son montage, qui défère l'application au `requestAnimationFrame`
/// (après le layout de la liste, avant le premier paint) → la page peint
/// directement à la bonne position, sans saut.
pub type RestoreScrollSignal = RwSignal<Option<f64>>;

/// The anonymisation workbench on screen, whose controls the `TopBar` carries
/// while a document is open.
pub type WorkbenchSignal = RwSignal<Option<crate::pages::redact::Session>>;

/// Fournit les signaux au contexte (appelé par `AppShell`, ancêtre de la
/// `TopBar` et des pages routées — donc persistant à travers les navigations).
pub fn provide_decision_bar_contexts() {
    provide_context::<DecisionBarSignal>(RwSignal::new(None));
    provide_context::<ResultNavSignal>(RwSignal::new(None));
    provide_context::<RestoreScrollSignal>(RwSignal::new(None));
    provide_context::<WorkbenchSignal>(RwSignal::new(None));
    provide_context::<SeedMemorySignal>(StoredValue::new(std::collections::HashMap::new()));
}

/// Signal de la barre décision (ce que la `TopBar` lit).
pub fn use_decision_bar() -> DecisionBarSignal {
    expect_context::<DecisionBarSignal>()
}

/// Signal de la graine de navigation (posé au clic, consommé au montage).
pub fn use_result_nav() -> ResultNavSignal {
    expect_context::<ResultNavSignal>()
}

/// Signal du scroll à restaurer (posé par le bouton retour, consommé au montage
/// de la liste de résultats).
pub fn use_restore_scroll() -> RestoreScrollSignal {
    expect_context::<RestoreScrollSignal>()
}

/// Mémoire des graines consommées (par id de décision).
pub fn use_seed_memory() -> SeedMemorySignal {
    expect_context::<SeedMemorySignal>()
}

/// Persiste la graine de `id` dans l'onglet (`sessionStorage`).
///
/// La mémoire ci-dessus vit en RAM : elle rattrape le back/forward, pas le
/// rechargement de la page.
#[cfg(feature = "hydrate")]
pub fn persist_seed(id: &str, seed: &ResultNavSeed) {
    if let Ok(json) = serde_json::to_string(seed) {
        crate::dom::nav_seed_set(id, &json);
    }
}

/// Relit la graine persistée de `id` — `None` si cet onglet n'en a pas.
#[cfg(feature = "hydrate")]
pub fn restore_seed(id: &str) -> Option<ResultNavSeed> {
    serde_json::from_str(&crate::dom::nav_seed_get(id)?).ok()
}
