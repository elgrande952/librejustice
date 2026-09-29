//! OLE property sets (`\u{5}SummaryInformation`,
//! `\u{5}DocumentSummaryInformation`): rewritten with their fixed-size
//! values only.
//!
//! Every string, vector and blob goes: title, subject, author, last author,
//! keywords, comments, template, company, manager, category, the heading
//! pairs and part titles that list the document's own headings, and the
//! thumbnail. So does the user-defined section, whose names and values are
//! free text. Counts, dates and the code page stay.

use super::bytes::{u16_at, u32_at};

const VT_I2: u16 = 2;
const VT_I4: u16 = 3;
const VT_BOOL: u16 = 0x0B;
const VT_UI4: u16 = 0x13;
const VT_I8: u16 = 0x14;
const VT_UI8: u16 = 0x15;
const VT_FILETIME: u16 = 0x40;

/// The stream rebuilt without its text, `None` when it does not parse as a
/// property set stream (the caller then drops it).
pub fn scrub(stream: &[u8]) -> Option<Vec<u8>> {
    if u16_at(stream, 0)? != 0xFFFE || u32_at(stream, 24)? == 0 {
        return None;
    }
    let fmtid = stream.get(28..44)?;
    let set = u32_at(stream, 44)? as usize;
    let count = u32_at(stream, set + 4)? as usize;
    let mut kept: Vec<(u32, Vec<u8>)> = Vec::new();
    for i in 0..count.min(4096) {
        let pid = u32_at(stream, set + 8 + 8 * i)?;
        let at = set.checked_add(u32_at(stream, set + 12 + 8 * i)? as usize)?;
        let kind = u16_at(stream, at)?;
        let size = match kind {
            VT_I2 | VT_I4 | VT_BOOL | VT_UI4 => 8,
            VT_I8 | VT_UI8 | VT_FILETIME => 12,
            _ => continue,
        };
        if pid == 0 {
            continue;
        }
        kept.push((pid, stream.get(at..at + size)?.to_vec()));
    }
    let mut body = Vec::new();
    let header = 8 + 8 * kept.len();
    let mut offsets = Vec::new();
    for (_, value) in &kept {
        offsets.push(header + body.len());
        body.extend_from_slice(value);
    }
    let mut out = Vec::new();
    out.extend_from_slice(stream.get(0..24)?);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(fmtid);
    out.extend_from_slice(&48u32.to_le_bytes());
    out.extend_from_slice(&((header + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(kept.len() as u32).to_le_bytes());
    for ((pid, _), offset) in kept.iter().zip(offsets) {
        out.extend_from_slice(&pid.to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
    }
    out.extend_from_slice(&body);
    Some(out)
}
