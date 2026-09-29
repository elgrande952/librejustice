//! OpenDocument Text: the package, its parts, the document they describe, and
//! edits that rewrite only the characters they name.

pub mod chart;
pub mod container;
mod drawing;
pub mod geometry;
pub mod lists;
pub mod model;
pub mod package;
pub mod reader;
pub mod styles;
pub mod text;
pub mod units;
pub mod xml;

pub use container::OdtPackage;
pub mod to_model;

#[cfg(test)]
mod tests;

pub use model::Document;
pub use to_model::to_model;

/// Read an `.odt` into its document model.
pub fn read(bytes: &[u8]) -> anyhow::Result<Document> {
    reader::read(&OdtPackage::open(bytes)?)
}

/// The document's text as LibreOffice exports it.
pub fn read_text(bytes: &[u8]) -> anyhow::Result<String> {
    Ok(text::plain_text(&read(bytes)?))
}
