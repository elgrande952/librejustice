//! Barre de décision sticky : contexte partagé + passage d'état
//! recherche→décision.
//!
//! `leptos_router` n'a pas d'équivalent riche à `location.state`. On le
//! remplace par deux contextes réactifs fournis au shell :
//! - `DecisionBarSignal` : ce que la `TopBar` affiche (titre, nav, retour).
//! - `ResultNavSignal` : « graine » posée au clic par une carte résultat / une
//!   voisine / le widget prev-next, lue UNE fois par `DecisionHeader` au montage
//!   pour construire la barre. One-shot (consommée au montage) : un lien décision
//!   sans graine (activité, lien direct) ne réutilise pas une nav obsolète.

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
/// compte et précédent/suivant à l'`id` courant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultNav {
    pub hit_ids: Vec<String>,
}

/// État affiché par la barre sticky.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionBarState {
    pub title: String,
    pub id: String,
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

/// Fournit les signaux au contexte (appelé par `AppShell`, ancêtre de la
/// `TopBar` et des pages routées — donc persistant à travers les navigations).
pub fn provide_decision_bar_contexts() {
    provide_context::<DecisionBarSignal>(RwSignal::new(None));
    provide_context::<ResultNavSignal>(RwSignal::new(None));
    provide_context::<RestoreScrollSignal>(RwSignal::new(None));
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
