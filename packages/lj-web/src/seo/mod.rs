//! Module SEO. Les `PageMeta` sont rendus via `leptos_meta` (`<Title>`/`<Meta>`)
//! dans chaque page ; le shell pose les valeurs generiques de `site_default()`.
//!
//! - `generic` : meta site/404, constantes canoniques.
//! - `decision` : titre/description/JSON-LD des pages decision (donnee depuis
//!   les DTO `lj-dtos`).
//! - `search` : meta de la page recherche.

pub mod decision;
pub mod generic;
pub mod law;
pub mod search;

pub use generic::{
    canonical_url, not_found_meta, site_default, PageMeta, CANONICAL_BASE, OG_IMAGE, SITE_JSONLD,
};
