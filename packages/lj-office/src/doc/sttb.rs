//! String tables (STTB) and the font table built on one.

use std::ops::Range;

use super::bytes::{u16_at, utf16};

/// One string of an STTB: where its characters sit in the table, and what
/// they say.
pub struct Entry {
    pub bytes: Range<usize>,
    pub text: String,
    /// Two bytes per character, else one.
    pub wide: bool,
}

/// The strings of an STTB, in order; an empty slot is an empty string.
pub fn parse(data: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let wide = u16_at(data, 0) == Some(0xFFFF);
    let mut at = if wide { 2 } else { 0 };
    let Some(count) = u16_at(data, at) else {
        return out;
    };
    let Some(extra) = u16_at(data, at + 2) else {
        return out;
    };
    at += 4;
    for _ in 0..count {
        let (cch, head) = if wide {
            match u16_at(data, at) {
                Some(c) => (c as usize, 2),
                None => break,
            }
        } else {
            match data.get(at) {
                Some(c) => (*c as usize, 1),
                None => break,
            }
        };
        let start = at + head;
        let len = if wide { 2 * cch } else { cch };
        let Some(raw) = data.get(start..start + len) else {
            break;
        };
        let text = if wide {
            utf16(raw)
        } else {
            raw.iter().map(|b| char::from(*b)).collect()
        };
        out.push(Entry {
            bytes: start..start + len,
            text,
            wide,
        });
        at = start + len + extra as usize;
    }
    out
}

#[derive(Clone, Debug)]
pub struct Font {
    pub name: String,
    pub alt: Option<String>,
    /// Pitch (bits 0-1), TrueType (bit 2), family (bits 4-6).
    pub ffid: u8,
    pub weight: i16,
    pub charset: u8,
}

impl Font {
    /// 0 don't care, 1 roman, 2 swiss, 3 modern, 4 script, 5 decorative.
    pub fn family(&self) -> u8 {
        (self.ffid >> 4) & 7
    }

    pub fn is_symbol(&self) -> bool {
        self.charset == 2
    }
}

pub fn fonts(data: &[u8]) -> Vec<Font> {
    let mut out = Vec::new();
    let Some(count) = u16_at(data, 0) else {
        return out;
    };
    let mut at = 4;
    for _ in 0..count {
        let Some(cb) = data.get(at).map(|c| *c as usize) else {
            break;
        };
        let Some(ffn) = data.get(at + 1..at + 1 + cb) else {
            break;
        };
        at += 1 + cb;
        if ffn.len() < 39 {
            continue;
        }
        let units: Vec<u16> = ffn[39..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let mut parts = units.split(|u| *u == 0);
        let name = String::from_utf16_lossy(parts.next().unwrap_or(&[]));
        let alt_at = ffn[4] as usize;
        let alt = (alt_at > 0)
            .then(|| units.get(alt_at..))
            .flatten()
            .map(|rest| String::from_utf16_lossy(rest.split(|u| *u == 0).next().unwrap_or(&[])))
            .filter(|s| !s.is_empty());
        out.push(Font {
            name,
            alt,
            ffid: ffn[0],
            weight: i16::from_le_bytes([ffn[1], ffn[2]]),
            charset: ffn[3],
        });
    }
    out
}
