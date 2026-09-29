//! Inline pictures: the PICF that sprmCPicLocation points at in the Data
//! stream, and the OfficeArt BLIP that follows it.

use super::bytes::{i16_at, i32_at, u16_at, u32_at};
use super::cfb::Cfb;
use super::model::{ImageFormat, Picture};

pub fn read(data: &[u8], location: u32) -> Option<Picture> {
    let at = location as usize;
    let lcb = i32_at(data, at)?.max(0) as usize;
    let cb_header = u16_at(data, at + 4)? as usize;
    if cb_header < 0x44 || lcb < cb_header {
        return None;
    }
    let end = (at + lcb).min(data.len());
    let mm = i16_at(data, at + 6)?;
    let goal_w = i16_at(data, at + 28)?.max(0) as u32;
    let goal_h = i16_at(data, at + 30)?.max(0) as u32;
    let mx = u16_at(data, at + 32)? as u32;
    let my = u16_at(data, at + 34)? as u32;
    let crop = [
        i16_at(data, at + 36)? as i32,
        i16_at(data, at + 38)? as i32,
        i16_at(data, at + 40)? as i32,
        i16_at(data, at + 42)? as i32,
    ];
    let scale = |goal: u32, m: u32, c: i32| -> u32 {
        let m = if m == 0 { 1000 } else { m };
        ((goal as i64 - c as i64).max(0) * m as i64 / 1000) as u32
    };
    let width = scale(goal_w, mx, crop[0] + crop[2]);
    let height = scale(goal_h, my, crop[1] + crop[3]);
    let mut p = at + cb_header;
    if mm == 0x66 {
        p += 1 + *data.get(p)? as usize;
    }
    let body = data.get(p..end)?;
    let (format, bytes) = find_blip(body)
        .or_else(|| (mm < 0x64).then(|| (Some(ImageFormat::Wmf), body.to_vec())))
        .unwrap_or((None, Vec::new()));
    Some(Picture {
        location,
        width,
        height,
        crop,
        format,
        bytes,
        props: properties(body),
    })
}

/// The picture an embedded object shows, from its storage under ObjectPool.
pub fn object(cfb: &Cfb, id: u32) -> Option<Picture> {
    let storage = format!("ObjectPool/_{id}");
    if let Some(picture) = cfb
        .stream(&format!("{storage}/\u{3}PIC"))
        .and_then(|s| read(s, 0))
    {
        return Some(picture);
    }
    let meta = cfb.stream(&format!("{storage}/\u{3}META"))?;
    let himetric = |v: i16| (i64::from(v).unsigned_abs() * 1440 / 2540) as u32;
    Some(Picture {
        location: 0,
        width: himetric(i16_at(meta, 2)?),
        height: himetric(i16_at(meta, 4)?),
        crop: [0; 4],
        format: Some(ImageFormat::Wmf),
        bytes: meta.get(8..)?.to_vec(),
        props: Vec::new(),
    })
}

pub(super) struct Record {
    pub instance: u16,
    pub kind: u16,
    pub body: std::ops::Range<usize>,
}

pub(super) fn record(data: &[u8], at: usize) -> Option<Record> {
    let ver_inst = u16_at(data, at)?;
    let kind = u16_at(data, at + 2)?;
    let len = u32_at(data, at + 4)? as usize;
    let start = at + 8;
    Some(Record {
        instance: ver_inst >> 4,
        kind,
        body: start..(start.saturating_add(len)).min(data.len()),
    })
}

/// The simple properties of the first property table among `data`'s records.
fn properties(data: &[u8]) -> Vec<(u16, u32)> {
    let mut at = 0;
    while at + 8 <= data.len() {
        let Some(r) = record(data, at) else {
            break;
        };
        let container = u16_at(data, at).is_some_and(|v| v & 0xF == 0xF);
        if container {
            let found = properties(&data[r.body.clone()]);
            if !found.is_empty() {
                return found;
            }
        } else if r.kind == 0xF00B {
            let b = &data[r.body.clone()];
            return (0..usize::from(r.instance))
                .map_while(|k| Some((u16_at(b, 6 * k)?, u32_at(b, 6 * k + 2)?)))
                .filter(|(id, _)| id & 0x8000 == 0)
                .map(|(id, v)| (id & 0x3FFF, v))
                .collect();
        }
        at = r.body.end.max(at + 8);
    }
    Vec::new()
}

/// The first BLIP in an OfficeArt record sequence, searching containers and
/// the FBSE that wraps an embedded BLIP.
fn find_blip(data: &[u8]) -> Option<(Option<ImageFormat>, Vec<u8>)> {
    let mut at = 0;
    while at + 8 <= data.len() {
        let r = record(data, at)?;
        let container = u16_at(data, at)? & 0xF == 0xF;
        match r.kind {
            _ if container => {
                if let Some(found) = find_blip(&data[r.body.clone()]) {
                    return Some(found);
                }
            }
            0xF007 => {
                let cb_name = *data.get(r.body.start + 33)? as usize;
                let inner = r.body.start + 36 + cb_name;
                if inner < r.body.end {
                    if let Some(found) = find_blip(&data[inner..r.body.end]) {
                        return Some(found);
                    }
                }
            }
            0xF018..=0xF117 => return blip(&data[r.body.clone()], r.kind, r.instance),
            _ => {}
        }
        at = r.body.end.max(at + 8);
    }
    None
}

pub(super) fn blip(
    body: &[u8],
    kind: u16,
    instance: u16,
) -> Option<(Option<ImageFormat>, Vec<u8>)> {
    let two_uids = |base: u16| instance == base + 1;
    let (format, metafile, uids) = match kind {
        0xF01A => (ImageFormat::Emf, true, two_uids(0x3D4)),
        0xF01B => (ImageFormat::Wmf, true, two_uids(0x216)),
        0xF01C => (ImageFormat::Pict, true, two_uids(0x542)),
        0xF01D | 0xF02A => (
            ImageFormat::Jpeg,
            false,
            instance == 0x46B || instance == 0x6E3,
        ),
        0xF01E => (ImageFormat::Png, false, two_uids(0x6E0)),
        0xF01F => (ImageFormat::Dib, false, two_uids(0x7A8)),
        0xF029 => (ImageFormat::Tiff, false, two_uids(0x6E4)),
        _ => return None,
    };
    let mut at = if uids { 32 } else { 16 };
    if metafile {
        let cb_save = u32_at(body, at + 28)? as usize;
        let compression = *body.get(at + 32)?;
        at += 34;
        let raw = body.get(at..(at + cb_save).min(body.len()))?;
        let bytes = if compression == 0 {
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(raw), &mut out).ok()?;
            out
        } else {
            raw.to_vec()
        };
        return Some((Some(format), bytes));
    }
    at += 1;
    let raw = body.get(at..)?;
    let bytes = if format == ImageFormat::Dib {
        dib_to_bmp(raw)
    } else {
        raw.to_vec()
    };
    Some((Some(format), bytes))
}

/// A packed DIB with the file header that makes it a `.bmp`.
fn dib_to_bmp(dib: &[u8]) -> Vec<u8> {
    let header = u32_at(dib, 0).unwrap_or(40);
    let bits = u16_at(dib, 14).unwrap_or(24) as u32;
    let used = u32_at(dib, 32).unwrap_or(0);
    let palette = if bits <= 8 {
        if used > 0 {
            used
        } else {
            1 << bits
        }
    } else {
        0
    };
    let offset = 14 + header + 4 * palette;
    let mut out = Vec::with_capacity(dib.len() + 14);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(dib);
    out
}
