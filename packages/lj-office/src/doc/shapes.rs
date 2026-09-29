//! Floating shapes: their FSPA anchors and the OfficeArt drawing behind them.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use super::bytes::{i32_at, u16_at, u32_at, Plc};
use super::fib::{lcb, Generation};
use super::model::ImageFormat;
use super::pictures::{blip, record};
use super::Doc;

/// A shape's frame in twips, left, top, right, bottom, from `bx` and `by`.
pub struct Fspa {
    pub spid: u32,
    pub rect: [i32; 4],
    /// Margin, page or column.
    pub bx: u8,
    /// Margin, page or paragraph.
    pub by: u8,
    /// Square, top and bottom, none, tight or through.
    pub wrap: u8,
    /// Both sides, left, right or the largest.
    pub side: u8,
    pub below: bool,
}

/// One OfficeArt shape, a group holding its children.
pub struct Sp {
    pub spid: u32,
    /// The rank in its drawing, the later drawn over the earlier.
    pub z: usize,
    /// The preset type, `msospt`.
    pub kind: u16,
    pub flags: u32,
    props: HashMap<u16, u32>,
    /// The string properties, as the WordArt text and its font.
    texts: HashMap<u16, String>,
    /// The frame within the parent group's space.
    pub child: Option<[i32; 4]>,
    /// A group's own coordinate space.
    pub space: Option<[i32; 4]>,
    pub children: Vec<Sp>,
}

impl Sp {
    pub fn prop(&self, id: u16) -> Option<u32> {
        self.props.get(&id).copied()
    }

    pub fn text(&self, id: u16) -> Option<&str> {
        self.texts.get(&id).map(String::as_str)
    }

    /// Bit `bit` of a boolean property set, when its use bit says it is set.
    pub fn flag(&self, id: u16, bit: u32) -> Option<bool> {
        let v = self.prop(id)?;
        (v >> (bit + 16) & 1 == 1).then_some(v >> bit & 1 == 1)
    }
}

pub const FLIP_H: u32 = 0x40;
pub const FLIP_V: u32 = 0x80;

#[derive(Default)]
pub struct Shapes {
    /// Anchors by the story-relative CP of their character.
    pub main: HashMap<u32, Fspa>,
    pub header: HashMap<u32, Fspa>,
    /// Top-level shapes by id.
    pub sps: HashMap<u32, Sp>,
    /// Pictures by their 1-based BLIP store index less one.
    pub blips: Vec<Option<(ImageFormat, Arc<[u8]>)>>,
    /// The text box story each shape id shows, main then header.
    pub text: HashMap<u32, usize>,
    pub header_text: HashMap<u32, usize>,
}

pub fn read(doc: &Doc) -> Shapes {
    let mut shapes = Shapes::default();
    if doc.fib.generation != Generation::Word97 {
        return shapes;
    }
    let table = doc.table_stream();
    let wd = doc.cfb.stream("WordDocument").unwrap_or_default();
    shapes.main = anchors(doc.fib.table(table, lcb::PLCSPAMOM));
    shapes.header = anchors(doc.fib.table(table, lcb::PLCSPAHDR));
    shapes.text = text_ids(doc.fib.table(table, lcb::PLCFTXBXTXT));
    shapes.header_text = text_ids(doc.fib.table(table, lcb::PLCFHDRTXBXTXT));
    let content = doc.fib.table(table, lcb::DGGINFO);
    let Some(dgg) = record(content, 0).filter(|r| r.kind == 0xF000) else {
        return shapes;
    };
    for r in records(content, dgg.body.clone()) {
        if r.kind == 0xF001 {
            shapes.blips = records(content, r.body.clone())
                .map(|bse| store_entry(content, bse.kind, bse.body, wd))
                .collect();
        }
    }
    let mut at = dgg.body.end;
    while at < content.len() {
        let Some(dg) = record(content, at + 1).filter(|r| r.kind == 0xF002) else {
            break;
        };
        for r in records(content, dg.body.clone()) {
            if r.kind == 0xF003 {
                for child in records(content, r.body.clone()).skip(1) {
                    if let Some(mut sp) = shape(content, child.kind, child.body) {
                        sp.z = shapes.sps.len();
                        shapes.sps.insert(sp.spid, sp);
                    }
                }
            }
        }
        at = dg.body.end;
    }
    shapes
}

fn anchors(plc: &[u8]) -> HashMap<u32, Fspa> {
    let plc = Plc::new(plc, 26);
    (0..plc.n)
        .filter_map(|i| {
            let d = plc.data(i);
            let flags = u16_at(d, 20)?;
            let fspa = Fspa {
                spid: u32_at(d, 0)?,
                rect: [i32_at(d, 4)?, i32_at(d, 8)?, i32_at(d, 12)?, i32_at(d, 16)?],
                bx: (flags >> 1 & 3) as u8,
                by: (flags >> 3 & 3) as u8,
                wrap: (flags >> 5 & 15) as u8,
                side: (flags >> 9 & 15) as u8,
                below: flags >> 14 & 1 == 1,
            };
            Some((plc.pos(i), fspa))
        })
        .collect()
}

/// The text box stories by the id of the shape that shows each.
fn text_ids(plc: &[u8]) -> HashMap<u32, usize> {
    let plc = Plc::new(plc, 22);
    (0..plc.n)
        .filter_map(|i| Some((u32_at(plc.data(i), 14)?, i)))
        .collect()
}

struct Rec {
    kind: u16,
    instance: u16,
    body: Range<usize>,
}

/// The records one after the other in `range`.
fn records(data: &[u8], range: Range<usize>) -> impl Iterator<Item = Rec> + '_ {
    let mut at = range.start;
    std::iter::from_fn(move || {
        if at + 8 > range.end {
            return None;
        }
        let r = record(data, at)?;
        at = r.body.end.max(at + 8);
        Some(Rec {
            kind: r.kind,
            instance: r.instance,
            body: r.body.start..r.body.end.min(range.end),
        })
    })
}

/// A BLIP store entry's picture, after its FBSE or at `foDelay` in `wd`.
fn store_entry(
    content: &[u8],
    kind: u16,
    body: Range<usize>,
    wd: &[u8],
) -> Option<(ImageFormat, Arc<[u8]>)> {
    if kind != 0xF007 {
        return None;
    }
    let fbse = content.get(body.clone())?;
    let size = u32_at(fbse, 20)? as usize;
    let delay = u32_at(fbse, 28)? as usize;
    let inner = 36 + *fbse.get(33)? as usize;
    let (data, at) = if inner + 8 <= fbse.len() {
        (fbse, inner)
    } else if size > 0 && delay < wd.len() {
        (wd, delay)
    } else {
        return None;
    };
    let r = record(data, at)?;
    let (format, bytes) = blip(data.get(r.body)?, r.kind, r.instance)?;
    Some((format?, Arc::from(bytes)))
}

/// Two sets of booleans, each bit of `new` whose use bit is set replacing
/// that of `old`.
fn merge(old: u32, new: u32) -> u32 {
    let used = new >> 16;
    let mask = used | used << 16;
    old & !mask | new & mask
}

/// A shape container, or a group container whose first shape is the group.
fn shape(data: &[u8], kind: u16, body: Range<usize>) -> Option<Sp> {
    match kind {
        0xF004 => {
            let mut sp = Sp {
                spid: 0,
                z: 0,
                kind: 0,
                flags: 0,
                props: HashMap::new(),
                texts: HashMap::new(),
                child: None,
                space: None,
                children: Vec::new(),
            };
            for r in records(data, body) {
                let b = data.get(r.body.clone())?;
                let rect = || Some([i32_at(b, 0)?, i32_at(b, 4)?, i32_at(b, 8)?, i32_at(b, 12)?]);
                match r.kind {
                    0xF00A => {
                        sp.kind = r.instance;
                        sp.spid = u32_at(b, 0)?;
                        sp.flags = u32_at(b, 4)?;
                    }
                    0xF009 => sp.space = rect(),
                    0xF00F => sp.child = rect(),
                    0xF00B | 0xF121 | 0xF122 => {
                        let complex = super::values::properties(data, r.body.start - 8);
                        for p in complex.iter().flatten() {
                            let (id, Some(range)) = (p.opid & 0x3FFF, p.data.clone()) else {
                                continue;
                            };
                            if matches!(id, 0x00C0 | 0x00C5) {
                                let units: Vec<u16> = data[range]
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .map(|b| u16::from_le_bytes(*b))
                                    .take_while(|u| *u != 0)
                                    .collect();
                                sp.texts.insert(id, String::from_utf16_lossy(&units));
                            }
                        }
                        for k in 0..usize::from(r.instance) {
                            let (Some(id), Some(value)) = (u16_at(b, 6 * k), u32_at(b, 6 * k + 2))
                            else {
                                break;
                            };
                            if id & 0x8000 == 0 {
                                let id = id & 0x3FFF;
                                let value = match sp.props.get(&id) {
                                    Some(&old) if id & 0x3F == 0x3F => merge(old, value),
                                    _ => value,
                                };
                                sp.props.insert(id, value);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(sp)
        }
        0xF003 => {
            let mut members = records(data, body);
            let first = members.next()?;
            let mut group = shape(data, first.kind, first.body)?;
            group.children = members
                .filter_map(|r| shape(data, r.kind, r.body))
                .collect();
            Some(group)
        }
        _ => None,
    }
}
