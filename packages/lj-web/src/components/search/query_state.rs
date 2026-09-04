//! Mutation de l'état URL de recherche. Toutes les mutations partent du
//! `ParamsMap` courant (lu
//! via `use_query_map`), appliquent un changement, et renvoient la *query
//! string sans le `?` initial* prête pour `navigate("/decisions?{qs}")`.
//!
//! Invariants :
//! - toute mutation de filtre/tri supprime `page` (retour page 1) ;
//! - `relevance` ⇒ `sort` supprimé ; `aiMode=false` ⇒ `aiMode` supprimé ;
//! - les autres clés sont conservées telles quelles (multi-valeurs incluses).
//!
//! Module privé de la tranche, rattaché via `#[path]` depuis un composant (le
//! `mod.rs` figé de `search/` n'expose que les composants).

use leptos_router::params::ParamsMap;

pub const SEARCH_PATH: &str = "/decisions";
/// Route de la recherche de textes (page distincte : moteurs et filtres
/// propres, aucune recherche transverse).
pub const TEXTES_PATH: &str = "/textes";
/// Route du moteur d'annonces (pot des événements en cours).
pub const ANNONCES_PATH: &str = "/annonces";

/// Le corpus que la route interroge. Trois pages de recherche, trois corpus
/// disjoints : la barre, le mode IA et l'autocomplétion en dépendent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corpus {
    Decisions,
    Textes,
    Annonces,
}

impl Corpus {
    /// Mode d'autocomplétion, vide quand le corpus n'en a pas.
    pub fn suggest_mode(self) -> &'static str {
        match self {
            Corpus::Decisions => "jurisprudence",
            Corpus::Textes => "textes",
            // Le pot n'a pas d'index de suggestions : ses entrées sont des
            // facettes, pas des mots-clés.
            Corpus::Annonces => "",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Corpus::Decisions => "Rechercher dans la jurisprudence…",
            Corpus::Textes => "Rechercher dans les codes et lois…",
            Corpus::Annonces => "Objet ou acteur — « voirie », « redressement »…",
        }
    }
}

pub fn corpus() -> Corpus {
    match current_search_path() {
        TEXTES_PATH => Corpus::Textes,
        ANNONCES_PATH => Corpus::Annonces,
        _ => Corpus::Decisions,
    }
}

/// Chemin de recherche courant : `/textes` si on y est déjà (les mutations et
/// soumissions y restent), sinon `/decisions`. Lu sur `window.location` — les
/// appels ne surviennent que dans des handlers client (clic, submit).
#[cfg(feature = "hydrate")]
fn current_search_path() -> &'static str {
    let path = web_sys::window().and_then(|w| w.location().pathname().ok());
    match path.as_deref() {
        Some(TEXTES_PATH) => TEXTES_PATH,
        Some(ANNONCES_PATH) => ANNONCES_PATH,
        _ => SEARCH_PATH,
    }
}

#[cfg(not(feature = "hydrate"))]
fn current_search_path() -> &'static str {
    SEARCH_PATH
}

/// Href complet de la recherche pour une query string (sans `?` de tête).
pub fn search_href(qs: &str) -> String {
    let path = current_search_path();
    if qs.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{qs}")
    }
}

/// Clone le map, applique `f` (mutation libre), supprime `page`, et renvoie la
/// query string (sans `?`).
fn mutate(map: &ParamsMap, drop_page: bool, f: impl FnOnce(&mut ParamsMap)) -> String {
    let mut next = map.clone();
    f(&mut next);
    if drop_page {
        next.remove("page");
    }
    strip_leading_q(next.to_query_string())
}

/// `to_query_string` renvoie `"?a=b"` ou `""` ; on retire le `?` de tête.
fn strip_leading_q(qs: String) -> String {
    qs.strip_prefix('?').map(str::to_string).unwrap_or(qs)
}

/// Réécrit une clé mono-valeur : `Some` ⇒ remplace, `None` ⇒ supprime. Optionnel
/// drop de `page`.
pub fn with_param(map: &ParamsMap, key: &str, value: Option<&str>, drop_page: bool) -> String {
    mutate(map, drop_page, |next| {
        next.remove(key);
        if let Some(v) = value {
            next.insert(key.to_string(), v.to_string());
        }
    })
}

/// Bascule une valeur dans une clé multi-valeur (présente ⇒ retirée, absente ⇒
/// ajoutée). Drop `page`.
pub fn toggle_multi(map: &ParamsMap, key: &str, value: &str) -> String {
    mutate(map, true, |next| {
        let current = next.remove(key).unwrap_or_default();
        let kept: Vec<String> = if current.iter().any(|v| v == value) {
            current.into_iter().filter(|v| v != value).collect()
        } else {
            let mut v = current;
            v.push(value.to_string());
            v
        };
        for v in kept {
            next.insert(key.to_string(), v);
        }
    })
}

/// Réécrit les bornes de dates (`from`/`to`) en une passe. Drop `page`. Port du
/// `onChange` du DateRangePicker.
pub fn with_dates(map: &ParamsMap, from: Option<&str>, to: Option<&str>) -> String {
    mutate(map, true, |next| {
        next.remove("dateFrom");
        next.remove("dateTo");
        if let Some(f) = from.filter(|s| !s.is_empty()) {
            next.insert("dateFrom".to_string(), f.to_string());
        }
        if let Some(t) = to.filter(|s| !s.is_empty()) {
            next.insert("dateTo".to_string(), t.to_string());
        }
    })
}

/// Supprime un ensemble de clés (conserve le reste). Pas de drop `page`
/// supplémentaire.
pub fn without_keys(map: &ParamsMap, keys: &[&str]) -> String {
    mutate(map, false, |next| {
        for key in keys {
            next.remove(key);
        }
    })
}

/// Construit la query string d'une soumission de requête : `q` (trim), drop
/// `page`, `aiMode` selon le toggle. Conserve filtres + sort existants. Une
/// requête vide **retire** `q` : la barre vidée sur un corpus qui l'accepte
/// revient à l'état sans requête.
///
/// `ai_mode` : `None` pour un corpus sans mode IA (textes) — aucun param posé ;
/// `Some(true)` = défaut, param retiré ; `Some(false)` = OFF explicite, seul cas
/// écrit dans l'URL.
pub fn with_query(map: &ParamsMap, query: &str, ai_mode: Option<bool>) -> String {
    mutate(map, true, |next| {
        next.remove("q");
        if !query.is_empty() {
            next.insert("q".to_string(), query.to_string());
        }
        next.remove("aiMode");
        if ai_mode == Some(false) {
            next.insert("aiMode".to_string(), "false".to_string());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(pairs: &[(&str, &str)]) -> ParamsMap {
        let mut m = ParamsMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), (*v).to_string());
        }
        m
    }

    #[test]
    fn toggle_adds_then_removes() {
        let m = map_of(&[("q", "x")]);
        let qs = toggle_multi(&m, "jurisdictionType", "TA");
        assert!(qs.contains("jurisdictionType=TA"));
        assert!(qs.contains("q=x"));
        // Re-toggle retire la valeur.
        let m2 = map_of(&[("q", "x"), ("jurisdictionType", "TA")]);
        let qs2 = toggle_multi(&m2, "jurisdictionType", "TA");
        assert!(!qs2.contains("jurisdictionType=TA"));
    }

    #[test]
    fn toggle_drops_page() {
        let m = map_of(&[("q", "x"), ("page", "3")]);
        let qs = toggle_multi(&m, "jurisdictionType", "TA");
        assert!(!qs.contains("page="));
    }

    #[test]
    fn une_requete_vide_retire_le_parametre() {
        let m = map_of(&[("q", "voirie"), ("fond", "marche")]);
        let qs = with_query(&m, "", None);
        assert!(!qs.contains("q="));
        assert!(qs.contains("fond=marche"));
    }

    #[test]
    fn relevance_clears_sort() {
        let m = map_of(&[("q", "x"), ("sort", "date_desc")]);
        let qs = with_param(&m, "sort", None, true);
        assert!(!qs.contains("sort="));
    }
}
