//! Helpers partagés des extracteurs.
//!
//! PUR : aucun I/O au runtime. Le chemin `crate::extract::common::X` reste
//! stable : `mod.rs` réexporte chaque helper depuis son sous-module thématique.
//!
//! ## Note sur les regex
//!
//! `regex` (crate) ne supporte ni lookahead `(?=…)` ni lookbehind `(?<=…)`.
//! Substitutions :
//! - lookbehind/lookahead sur des classes simples (`(?<=\d)…(?=\d)`) → groupes
//!   capturants + replacement (`$1-$2`) : strictement équivalent ;
//! - lookahead de borne en fin de motif (`…(?=\s*[,;.])`) → capture gloutonne
//!   contrôlée + coupe programmatique (`normalize_instrument`) ;
//! - négations de lookahead sur l'identifiant d'article → parcours caractère
//!   par caractère reproduisant la sémantique. Voir `unresolved` du rapport.

mod articles;
mod counsel;
mod dates;
pub(crate) use dates::parse_french_date;
mod instruments;
/// Signaux structurés d'une clé de citation — fonction pure de la chaîne,
/// substrat des règles du résolveur (DatedAct/EuNum/ForeignCode…).
pub mod key_signals;
// Primitives texte promues dans `lj-core` (partagées avec `aliases` côté
// serve) ; re-exportées ici pour que `super::text::X` des sous-modules résolve.
pub(crate) use lj_core::text;

pub use articles::normalize_article;
pub(crate) use articles::ART_NUM_SUFFIX;
pub use instruments::{is_unresolvable_instrument, normalize_instrument};

pub(crate) use instruments::FOREIGN_NATIONALITY_STEMS;
pub(crate) use instruments::{names_territory, territory_qualifier_len};

pub(crate) use counsel::{
    better_cased, dedupe_prefix_variants, is_anonymized_firm, is_anonymized_person, unique_nonempty,
};
pub(crate) use dates::{clean_date_iso, clean_docket_numbers, extract_textual_audience_date};
pub(crate) use lj_fold::fold;
pub(crate) use text::normalize_spaces;
