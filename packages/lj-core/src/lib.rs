//! `lj-core` — noyau pur partagé serveur + ingest.
//!
//! Aucun I/O au runtime : primitives texte (tokenisation, recasse, folding), modèle
//! `Decision`, taxonomie, résumé, publication. Les données statiques sont
//! embarquées via `include_str!`. L'I/O vit dans `lj-store` / `lj-sources` /
//! `lj-llm` / `lj-api` / `lj-ingest`, la pile d'extraction lourde dans
//! `lj-extract`.

pub mod aliases;
pub mod annonces;
pub mod article_key;
pub mod article_order;
pub mod body_tok;
pub mod case_names;
pub mod chamber;
pub mod collocations;
pub mod compare;
pub mod compteurs;
pub mod crawlers;
pub mod decision;
pub mod docket_key;
pub mod error;
pub mod forme_juridique;
pub mod formex;
pub mod instrument_match;
pub mod jurisdictions;
pub mod normalizer;
pub mod official_url;
pub mod outremer;
pub mod parsing;
pub mod publication;
pub mod referential_labels;
pub mod schema;
pub mod source_authority;
pub mod suggest;
pub mod summary;
pub mod text;
pub mod titles;
pub mod tokens;
pub mod truecase;
pub mod usage;

/// Version du pipeline d'extraction : `extract_version` stampé sur `decisions`,
/// comparé pour le gating de ré-extraction. Produite par `lj-extract`
/// (re-exportée en `lj_extract::extract::EXTRACT_VERSION`) et consommée par
/// `lj-store` pour le gating SQL — d'où sa résidence dans le noyau partagé, leur
/// ancêtre commun. À bumper quand la logique d'extraction change.
pub const EXTRACT_VERSION: i16 = 83;

/// Version de l'extraction du quantum du préjudice corporel : axe distinct
/// d'`EXTRACT_VERSION` le temps que leurs cadences convergent, le quantum
/// itérant à chaque règle de validation ajoutée. À bumper quand ce qui est
/// extrait ou validé change.
pub const PC_EXTRACT_VERSION: i16 = 1;

/// Révision manuelle du quantum, plus haute que toute version automatique :
/// `lj-store` refuse de l'écraser, et s'y comparer se réduit à filtrer dessus.
pub const PC_GOLD_VERSION: i16 = 1000;

/// Version du certifieur de capture : axe orthogonal à `EXTRACT_VERSION`. Une
/// décision est certifiée quand l'oracle haut-recall ne trouve rien de plus que le
/// recognizer et que ses citations résolvent. Le re-extract par défaut saute les
/// décisions certifiées à la version courante, même sur un bump
/// d'`EXTRACT_VERSION` ; le re-extract ciblé l'ignore. À bumper quand l'oracle
/// progresse.
pub const CERTIFIER_VERSION: i32 = 1;
