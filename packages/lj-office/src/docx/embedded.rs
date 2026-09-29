//! The font files a package embeds, obfuscated as Word writes them.

use std::sync::Arc;

use crate::ooxml::OoxmlPackage;
use anyhow::Result;

use super::xml::{parse, Ns};
use super::Rels;
use crate::fonts;

/// Every face file the font table part `table` embeds that reads as a font.
pub(super) fn read(package: &OoxmlPackage, table: &str) -> Result<Vec<Arc<[u8]>>> {
    let Some(part) = package.entry(table) else {
        return Ok(Vec::new());
    };
    let root = parse(part)?;
    let rels = Rels::read(package, table)?;
    let mut files = Vec::new();
    for font in root.elements().filter(|e| e.is(Ns::W, "font")) {
        for embed in font.elements() {
            if embed.ns != Ns::W || !embed.name.starts_with("embed") {
                continue;
            }
            let Some(data) = embed
                .attr(Ns::R, "id")
                .and_then(|id| rels.by_id(id))
                .and_then(|target| package.entry(target))
            else {
                continue;
            };
            let file = match embed.w("fontKey") {
                Some(key) => deobfuscate(data, key),
                None => Some(data.to_vec()),
            };
            if let Some(file) = file.filter(|f| fonts::readable(f)) {
                files.push(Arc::from(file));
            }
        }
    }
    Ok(files)
}

/// Word XORs a font's first 32 bytes with its key, the GUID's bytes reversed.
fn deobfuscate(data: &[u8], key: &str) -> Option<Vec<u8>> {
    let hex: Vec<u8> = key
        .chars()
        .filter_map(|c| c.to_digit(16).map(|d| d as u8))
        .collect();
    if hex.len() != 32 || data.len() < 32 {
        return None;
    }
    let key: Vec<u8> = hex.chunks(2).rev().map(|p| p[0] << 4 | p[1]).collect();
    let mut file = data.to_vec();
    for (i, byte) in file[..32].iter_mut().enumerate() {
        *byte ^= key[i % 16];
    }
    Some(file)
}
