//! Downloaders : opendata ZIP + sync incrémental Judilibre.
//!
//! Persiste un [`Manifest`] JSON sur disque pour la reprise idempotente. Les
//! deux sources ont des formats de manifeste distincts (opendata : `entries`
//! par `{jur}/{yyyymm}` ; Judilibre : `jurisdictions` + watermark/cursor
//! `/transactionalhistory`). [`Manifest`] porte les deux formes ; `load`
//! détecte le format présent sur disque et `save` réécrit la forme courante.
//!
//! - opendata : I/O HTTP synchrone via `reqwest::blocking`, pas de
//!   parallélisme ;
//! - judilibre : `sync_judilibre` est mono-thread, ordre séquentiel.

mod calendar;
mod compact;
pub(crate) mod http;
mod judilibre;
mod manifest;
mod opendata;
mod sha256;

pub use compact::compact_archives;
pub use judilibre::{parse_decision_line, read_jsonl_gz_lines, sync_judilibre};
pub use manifest::{Entry, JurState, Manifest, MonthState};
pub use opendata::sync_opendata;

pub(crate) use http::{get_to_file_retrying, get_with_body_retrying};
