//! A `.doc`: the needles give way to their labels through the piece table,
//! and leave the bytes of every stream, a package embedded as an object
//! redacted as a package.

use anyhow::Result;
use lj_office::doc::codepage::Codepage;
use lj_office::doc::package::DocPackage;

use super::{anchored, replacements, survivors};
use crate::labels::Labels;
use crate::Outcome;

/// Replace every occurrence of `needles` by its label, overwrite the tables
/// naming who handled the document, then verify on the bytes produced that
/// none survives, in the text or in any stream. The stories are searched as
/// text, a needle as whole words; every other byte as bytes, a needle inside
/// a longer word included.
pub(super) fn redact(bytes: &[u8], needles: &[String], labels: &Labels) -> Result<Outcome> {
    let mut package = DocPackage::open(bytes)?;
    let source_chars = package.characters().len();
    let (mut found, mut removed) = (0, 0);
    // An empty label can expose another occurrence the space-stripping pass
    // had skipped because an earlier candidate overlapped it: rescan until none.
    for _ in 0..4 {
        let feeds: Vec<_> = package.readings().into_iter().map(anchored).collect();
        let views = lj_office::view_positions(&package.views());
        let (replaced, edits) = replacements(&feeds, &views, needles, labels)?;
        if edits.is_empty() {
            break;
        }
        found += replaced;
        removed += package.replace(&edits)?;
    }
    package.clear_authors();
    let codepages = package.codepages();
    let mut failed = None;
    package.rewrite_streams(|text, data| {
        if !embedded(data) {
            let kept: Vec<Vec<u8>> = text.iter().map(|r| data[r.clone()].to_vec()).collect();
            for needle in needles {
                erase_everywhere(data, needle, &codepages);
            }
            for (range, bytes) in text.iter().zip(kept) {
                data[range.clone()].copy_from_slice(&bytes);
            }
            return;
        }
        match super::redact_with(data, needles, labels) {
            Ok(outcome) => {
                found += outcome.found;
                removed += outcome.removed;
                *data = outcome.bytes;
            }
            Err(e) => {
                failed.get_or_insert(e);
            }
        }
    });
    if let Some(e) = failed {
        return Err(e.context("redacting a package embedded as an object"));
    }
    let produced = package.save();
    let leaks = leaks(&produced, needles)?;
    Ok(Outcome {
        bytes: produced,
        found,
        removed,
        leaks,
        source_chars,
    })
}

/// Needles a reader still finds in the text, in a package embedded as an
/// object, or in the bytes of any other stream.
pub(super) fn leaks(bytes: &[u8], needles: &[String]) -> Result<Vec<String>> {
    let package = DocPackage::open(bytes)?;
    let codepages = package.codepages();
    let mut leaks: Vec<String> = Vec::new();
    let mut add = |found: Vec<String>| {
        for leak in found {
            if !leaks.contains(&leak) {
                leaks.push(leak);
            }
        }
    };
    let views = lj_office::view_positions(&package.views());
    for feed in package.readings() {
        add(survivors(&anchored(feed), &views, needles));
    }
    for (text, data) in package.nodes() {
        if embedded(data) {
            add(super::leaks(data, needles)?);
            continue;
        }
        let mut data = data.to_vec();
        for range in text {
            data[range].fill(0);
        }
        add(needles
            .iter()
            .filter(|needle| contains(&data, needle, &codepages))
            .cloned()
            .collect());
    }
    Ok(leaks)
}

/// A stream holding a package `lj-office` reads, a `.docx` or an `.xlsx`
/// embedded as an object; any other is searched as bytes.
fn embedded(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04") && lj_office::characters(data).is_ok()
}

/// Where `needle` occurs in `data`, case folded, as UTF-16LE at either byte
/// parity and as 8-bit text in each code page; each hit as `(start, width)`.
fn hits(data: &[u8], needle: &str, codepages: &[Codepage]) -> Vec<(usize, usize)> {
    let folded: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let n = folded.len();
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let units: Vec<Vec<u16>> = folded
        .iter()
        .map(|c| c.encode_utf16(&mut [0; 2]).to_vec())
        .collect();
    if units.iter().all(|u| u.len() == 1) {
        let first: Vec<u16> = std::iter::once(folded[0])
            .chain(folded[0].to_uppercase())
            .filter_map(|c| u16::try_from(c as u32).ok())
            .collect();
        let mut lows: Vec<u8> = first.iter().map(|u| u.to_le_bytes()[0]).collect();
        lows.dedup();
        let mut at = 0;
        while let Some(found) = next_start(data, at, &lows) {
            if found + 2 * n > data.len() {
                break;
            }
            let head = u16::from_le_bytes([data[found], data[found + 1]]);
            let ok = first.contains(&head)
                && (1..n).all(|k| {
                    let u = u16::from_le_bytes([data[found + 2 * k], data[found + 2 * k + 1]]);
                    char::from_u32(u as u32).map(lower) == Some(folded[k])
                });
            if ok {
                out.push((found, 2));
                at = found + 2 * n;
            } else {
                at = found + 1;
            }
        }
    }
    for cp in codepages {
        let table: [char; 256] = std::array::from_fn(|b| {
            char::from_u32(cp.decode(b as u8) as u32)
                .map(lower)
                .unwrap_or('\0')
        });
        let firsts: Vec<u8> = (0..=255u8)
            .filter(|b| table[*b as usize] == folded[0])
            .collect();
        let mut at = 0;
        while let Some(found) = next_start(data, at, &firsts) {
            if found + n > data.len() {
                break;
            }
            let ok = (1..n).all(|k| table[data[found + k] as usize] == folded[k]);
            if ok {
                out.push((found, 1));
                at = found + n;
            } else {
                at = found + 1;
            }
        }
    }
    out
}

/// The first position from `at` on holding one of `bytes`: a stream can run to
/// tens of megabytes, searched once per needle and code page.
fn next_start(data: &[u8], at: usize, bytes: &[u8]) -> Option<usize> {
    let rest = data.get(at..)?;
    let found = match *bytes {
        [] => None,
        [a] => memchr::memchr(a, rest),
        [a, b] => memchr::memchr2(a, b, rest),
        [a, b, c] => memchr::memchr3(a, b, c, rest),
        _ => rest.iter().position(|x| bytes.contains(x)),
    };
    found.map(|i| at + i)
}

fn contains(data: &[u8], needle: &str, codepages: &[Codepage]) -> bool {
    !hits(data, needle, codepages).is_empty()
}

fn erase_everywhere(data: &mut [u8], needle: &str, codepages: &[Codepage]) {
    let folded: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    for (at, width) in hits(data, needle, codepages) {
        for (k, c) in folded.iter().enumerate() {
            if c.is_whitespace() {
                continue;
            }
            let i = at + width * k;
            data[i] = b'X';
            if width == 2 {
                data[i + 1] = 0;
            }
        }
    }
}
