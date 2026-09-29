//! The strings a Word 97 `.doc` keeps outside its stories: the names and
//! values of its document variables, and the text properties of its drawings,
//! floating in the table stream or inline in the Data stream, a hyperlink's
//! strings included.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use anyhow::{bail, Context, Result};

use super::bytes::{u16_at, u32_at, utf16, Plc};
use super::cfb::Cfb;
use super::fib::{lcb, Generation};
use super::fkp::PAGE;
use super::pictures::record;
use super::sprm::{code, sprms};
use super::Doc;

/// One string, its characters as a reader shows them, up to a NUL.
pub struct Value {
    pub text: String,
    /// A name rather than prose: a variable's or a shape's.
    pub markup: bool,
    place: Place,
}

enum Place {
    Variable {
        index: usize,
        name: bool,
    },
    /// A property of a drawing in the table stream: its property table, as an
    /// offset in the drawing blob, and its rank in it.
    Drawing {
        fopt: usize,
        slot: usize,
        part: Option<Part>,
    },
    /// A property of an inline picture: the offset of its PICF in the Data
    /// stream, and its property table and rank there.
    Inline {
        location: usize,
        fopt: usize,
        slot: usize,
        part: Option<Part>,
    },
}

/// One string of a hyperlink property, which holds several.
#[derive(Clone, Debug, PartialEq)]
struct Part {
    /// Its UTF-16 units in the property's data, a terminating NUL left out.
    text: Range<usize>,
    /// The 32-bit counts, in the property's data, that include it.
    counts: Vec<(usize, Count)>,
    /// A frame or a bookmark name rather than prose.
    markup: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Count {
    Units,
    Bytes,
}

const NAME: u16 = 0x0380;
const DESCRIPTION: u16 = 0x0381;
const HYPERLINK: u16 = 0x0382;
const TOOLTIP: u16 = 0x038D;
const WORDART: u16 = 0x00C0;

/// Array properties, whose length some writers give without the six bytes of
/// the array header.
const ARRAYS: [u16; 10] = [
    0x0145, 0x0146, 0x0151, 0x0152, 0x0155, 0x0156, 0x0157, 0x0197, 0x01CE, 0x0383,
];

const MAX_DEPTH: usize = 64;

pub fn values(doc: &Doc) -> Vec<Value> {
    let mut out = Vec::new();
    if doc.fib.generation != Generation::Word97 {
        return out;
    }
    if let Some(variables) = Variables::parse(doc.fib.table(doc.table_stream(), lcb::STWUSER)) {
        for (index, (name, value)) in variables.names.iter().zip(&variables.values).enumerate() {
            for (text, name) in [(&name.0, true), (value, false)] {
                if text.is_empty() {
                    continue;
                }
                out.push(Value {
                    text: text.clone(),
                    markup: name,
                    place: Place::Variable { index, name },
                });
            }
        }
    }
    let drawings = doc.fib.table(doc.table_stream(), lcb::DGGINFO);
    for fopt in drawing_fopts(drawings) {
        for (slot, part, text, markup) in texts(drawings, fopt) {
            out.push(Value {
                text,
                markup,
                place: Place::Drawing { fopt, slot, part },
            });
        }
    }
    let data = doc.data_stream();
    for (location, shape) in inline_shapes(doc) {
        let mut fopts = Vec::new();
        collect_fopts(data, shape, 0, &mut fopts);
        for fopt in fopts {
            for (slot, part, text, markup) in texts(data, fopt) {
                out.push(Value {
                    text,
                    markup,
                    place: Place::Inline {
                        location,
                        fopt,
                        slot,
                        part,
                    },
                });
            }
        }
    }
    out
}

/// Write `texts`, new texts by index in [`values`], into `cfb`. A variable or
/// a floating drawing takes its new length, its table moved to the end of
/// the table stream when it grows. An inline picture whose property changes
/// moves to the end of the Data stream, its old bytes zeroed, and the
/// character properties that locate it follow.
pub fn rewrite(doc: &Doc, cfb: &mut Cfb, texts: &BTreeMap<usize, String>) -> Result<()> {
    let values = values(doc);
    let table = doc.table_stream();
    let blob = doc.fib.table(table, lcb::DGGINFO);
    let mut variables: BTreeMap<(usize, bool), &str> = BTreeMap::new();
    let mut drawings: BTreeMap<usize, BTreeMap<usize, Vec<u8>>> = BTreeMap::new();
    let mut links: BTreeMap<(usize, usize), LinkEdits> = BTreeMap::new();
    let mut inline: BTreeMap<usize, Edits> = BTreeMap::new();
    let mut inline_links: BTreeMap<(usize, usize, usize), LinkEdits> = BTreeMap::new();
    for (&i, text) in texts {
        let value = values.get(i).context("a value past the document's")?;
        match &value.place {
            Place::Variable { index, name } => {
                variables.insert((*index, *name), text);
            }
            Place::Drawing {
                fopt,
                slot,
                part: None,
            } => {
                drawings
                    .entry(*fopt)
                    .or_default()
                    .insert(*slot, terminated(text));
            }
            Place::Drawing {
                fopt,
                slot,
                part: Some(part),
            } => links.entry((*fopt, *slot)).or_default().push((part, text)),
            Place::Inline {
                location,
                fopt,
                slot,
                part: None,
            } => {
                inline
                    .entry(*location)
                    .or_default()
                    .entry(*fopt)
                    .or_default()
                    .insert(*slot, terminated(text));
            }
            Place::Inline {
                location,
                fopt,
                slot,
                part: Some(part),
            } => inline_links
                .entry((*location, *fopt, *slot))
                .or_default()
                .push((part, text)),
        }
    }
    let data = doc.data_stream();
    for ((location, fopt, slot), edits) in inline_links {
        let bytes = link_data(data, fopt, slot)?;
        inline
            .entry(location)
            .or_default()
            .entry(fopt)
            .or_default()
            .insert(slot, edit_link(bytes, edits));
    }
    for ((fopt, slot), edits) in links {
        let bytes = link_data(blob, fopt, slot)?;
        drawings
            .entry(fopt)
            .or_default()
            .insert(slot, edit_link(bytes, edits));
    }
    let mut moved: Vec<(usize, Vec<u8>)> = Vec::new();
    if !variables.is_empty() {
        let mut parsed = Variables::parse(doc.fib.table(table, lcb::STWUSER))
            .context("document variables that no longer parse")?;
        for ((index, name), text) in variables {
            match name {
                true => parsed.names[index].0 = text.to_owned(),
                false => parsed.values[index] = text.to_owned(),
            }
        }
        moved.push((lcb::STWUSER, parsed.bytes()));
    }
    if !drawings.is_empty() {
        moved.push((lcb::DGGINFO, splice_drawings(blob, &drawings)?));
    }
    let table_name = doc.table_name;
    let mut wd = doc.word_document().to_vec();
    let mut table = table.to_vec();
    for (index, bytes) in moved {
        let (fc, len) = doc.fib.pair(index);
        let old = fc as usize..(fc as usize).saturating_add(len as usize);
        let old = old.start.min(table.len())..old.end.min(table.len());
        let at = if bytes.len() <= old.len() {
            table[old.clone()].fill(0);
            old.start
        } else {
            table[old].fill(0);
            table.len()
        };
        let end = at + bytes.len();
        if end > table.len() {
            table.resize(end, 0);
        }
        table[at..end].copy_from_slice(&bytes);
        let Some(pair) = doc.fib.pair_offset(&wd, index) else {
            bail!("a FIB without the pair to move");
        };
        wd[pair..pair + 4].copy_from_slice(&(at as u32).to_le_bytes());
        wd[pair + 4..pair + 8].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    }
    if !inline.is_empty() {
        let shapes: BTreeMap<usize, Range<usize>> = inline_shapes(doc).into_iter().collect();
        let mut new_data = data.to_vec();
        let mut relocated = BTreeMap::new();
        for (location, edits) in &inline {
            let shape = shapes.get(location).context("an inline picture gone")?;
            let end = (location + u32_at(data, *location).context("a PICF cut short")? as usize)
                .min(data.len());
            let mut block = data[*location..shape.start].to_vec();
            splice(data, shape.clone(), edits, &mut block)?;
            block.extend_from_slice(&data[shape.end..end]);
            let len = block.len() as u32;
            block[..4].copy_from_slice(&len.to_le_bytes());
            let at = u32::try_from(new_data.len()).context("a Data stream past 4 GB")?;
            relocated.insert(*location as u32, at);
            new_data[*location..end].fill(0);
            new_data.extend_from_slice(&block);
        }
        relocate_pictures(doc, &mut wd, &mut table, &relocated);
        *cfb.stream_mut("Data").context("no Data stream")? = new_data;
    }
    *cfb.stream_mut("WordDocument")
        .context("no WordDocument stream")? = wd;
    *cfb.stream_mut(table_name).context("no table stream")? = table;
    Ok(())
}

/// New complex properties, by property table then by rank.
type Edits = BTreeMap<usize, BTreeMap<usize, Vec<u8>>>;

type LinkEdits<'a> = Vec<(&'a Part, &'a str)>;

fn link_data(data: &[u8], fopt: usize, slot: usize) -> Result<&[u8]> {
    let props = properties(data, fopt).context("a property table that no longer parses")?;
    let range = props
        .get(slot)
        .and_then(|p| p.data.clone())
        .context("a hyperlink without its data")?;
    Ok(&data[range])
}

/// Point every sprmCPicLocation of the character pages and of the piece
/// table's grpprls that names a key of `relocated` at its value.
fn relocate_pictures(doc: &Doc, wd: &mut [u8], table: &mut [u8], relocated: &BTreeMap<u32, u32>) {
    let patch = |grpprl: &mut [u8]| {
        let operands: Vec<usize> = sprms(grpprl)
            .filter(|(sprm, _)| *sprm == code::C_PIC_LOCATION)
            .map(|(_, op)| op.as_ptr() as usize - grpprl.as_ptr() as usize)
            .collect();
        for at in operands {
            let old = u32_at(grpprl, at).unwrap_or(u32::MAX);
            if let Some(new) = relocated.get(&old) {
                grpprl[at..at + 4].copy_from_slice(&new.to_le_bytes());
            }
        }
    };
    let bte = Plc::new(doc.fib.table(doc.table_stream(), lcb::PLCFBTECHPX), 4);
    for i in 0..bte.n {
        let Some(pn) = u32_at(bte.data(i), 0) else {
            continue;
        };
        let start = (pn & 0x3F_FFFF) as usize * PAGE;
        let Some(page) = wd.get_mut(start..start + PAGE) else {
            continue;
        };
        let crun = page[PAGE - 1] as usize;
        if 5 * crun + 4 > PAGE - 1 {
            continue;
        }
        for k in 0..crun {
            let offset = page[4 * (crun + 1) + k] as usize * 2;
            if offset == 0 {
                continue;
            }
            let cb = page[offset] as usize;
            if let Some(grpprl) = page.get_mut(offset + 1..offset + 1 + cb) {
                patch(grpprl);
            }
        }
    }
    let (fc, len) = doc.fib.pair(lcb::CLX);
    let Some(clx) = table.get_mut(fc as usize..fc as usize + len as usize) else {
        return;
    };
    let mut at = 0;
    while clx.get(at) == Some(&1) {
        let cb = u16_at(clx, at + 1).unwrap_or(0) as usize;
        let stop = (at + 3 + cb).min(clx.len());
        patch(&mut clx[at + 3..stop]);
        at = stop;
    }
}

/// The data of a hyperlink property with the strings of `edits` replaced, and
/// every count that includes one following it.
fn edit_link(data: &[u8], mut edits: LinkEdits) -> Vec<u8> {
    let mut out = data.to_vec();
    edits.sort_by_key(|(part, _)| std::cmp::Reverse(part.text.start));
    for (part, text) in edits {
        let new: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let delta = new.len() as i64 - part.text.len() as i64;
        out.splice(part.text.clone(), new);
        for &(at, count) in &part.counts {
            let old = i64::from(u32_at(&out, at).unwrap_or(0));
            let step = match count {
                Count::Units => delta / 2,
                Count::Bytes => delta,
            };
            out[at..at + 4].copy_from_slice(&((old + step).max(0) as u32).to_le_bytes());
        }
    }
    out
}

fn terminated(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The document variables: an STTB of names, each with its extra bytes, then
/// as many values, each a length and its UTF-16 units.
struct Variables {
    names: Vec<(String, Vec<u8>)>,
    values: Vec<String>,
    tail: Vec<u8>,
}

impl Variables {
    fn parse(data: &[u8]) -> Option<Self> {
        if u16_at(data, 0)? != 0xFFFF {
            return None;
        }
        let count = u16_at(data, 2)? as usize;
        let extra = u16_at(data, 4)? as usize;
        let mut at = 6;
        let xst = |at: &mut usize| -> Option<String> {
            let cch = u16_at(data, *at)? as usize;
            let text = utf16(data.get(*at + 2..*at + 2 + 2 * cch)?);
            *at += 2 + 2 * cch;
            Some(text)
        };
        let mut names = Vec::with_capacity(count);
        for _ in 0..count {
            let name = xst(&mut at)?;
            names.push((name, data.get(at..at + extra)?.to_vec()));
            at += extra;
        }
        let values = (0..count).map(|_| xst(&mut at)).collect::<Option<_>>()?;
        Some(Self {
            names,
            values,
            tail: data[at..].to_vec(),
        })
    }

    fn bytes(&self) -> Vec<u8> {
        let extra = self.names.first().map_or(0, |(_, e)| e.len());
        let mut out = Vec::new();
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        out.extend_from_slice(&(self.names.len() as u16).to_le_bytes());
        out.extend_from_slice(&(extra as u16).to_le_bytes());
        let xst = |out: &mut Vec<u8>, text: &str| {
            let units: Vec<u16> = text.encode_utf16().collect();
            out.extend_from_slice(&(units.len() as u16).to_le_bytes());
            out.extend(units.into_iter().flat_map(u16::to_le_bytes));
        };
        for (name, extra) in &self.names {
            xst(&mut out, name);
            out.extend_from_slice(extra);
        }
        for value in &self.values {
            xst(&mut out, value);
        }
        out.extend_from_slice(&self.tail);
        out
    }
}

/// One entry of a property table, and where its complex data lies.
pub(super) struct Property {
    pub opid: u16,
    op: u32,
    pub data: Option<Range<usize>>,
}

/// The entries of the property table whose record header is at `at`, or
/// none when its complex data does not add up to the record.
pub(super) fn properties(data: &[u8], at: usize) -> Option<Vec<Property>> {
    let r = record(data, at)?;
    let count = usize::from(r.instance);
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let entry = r.body.start + 6 * k;
        out.push(Property {
            opid: u16_at(data, entry)?,
            op: u32_at(data, entry + 2)?,
            data: None,
        });
    }
    let mut cursor = r.body.start + 6 * count;
    for p in out.iter_mut().filter(|p| p.opid & 0x8000 != 0) {
        let mut len = p.op as usize;
        if ARRAYS.contains(&(p.opid & 0x3FFF)) && len > 0 {
            let n = u16_at(data, cursor)? as usize;
            let size = match u16_at(data, cursor + 4)? {
                0xFFF0 => 4,
                s => s as usize,
            };
            if n * size == len {
                len += 6;
            }
        }
        let end = cursor.checked_add(len).filter(|end| *end <= r.body.end)?;
        p.data = Some(cursor..end);
        cursor = end;
    }
    (cursor == r.body.end).then_some(out)
}

/// The text properties of the property table at `at`: rank, string of a
/// hyperlink, text, and whether it is a name rather than prose.
fn texts(data: &[u8], at: usize) -> Vec<(usize, Option<Part>, String, bool)> {
    let Some(props) = properties(data, at) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (slot, p) in props.iter().enumerate() {
        let Some(range) = p.data.clone() else {
            continue;
        };
        let bytes = &data[range];
        match p.opid & 0x3FFF {
            pid @ (NAME | DESCRIPTION | TOOLTIP | WORDART) => {
                let text = utf16_until_nul(bytes);
                if !text.is_empty() {
                    out.push((slot, None, text, pid == NAME));
                }
            }
            HYPERLINK => {
                for part in link_parts(bytes) {
                    let text = utf16(&bytes[part.text.clone()]);
                    if !text.is_empty() {
                        let markup = part.markup;
                        out.push((slot, Some(part), text, markup));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn utf16_until_nul(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

const STD_HLINK: [u8; 16] = [
    0xD0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9, 0x0B,
];
const URL_MONIKER: [u8; 16] = [
    0xE0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9, 0x0B,
];
const FILE_MONIKER: [u8; 16] = [0x03, 0x03, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

/// The strings of a hyperlink object: its display name, target frame, URL or
/// file path, and location, as far as they parse. A file path written in
/// 8 bits only is left to the byte search.
fn link_parts(d: &[u8]) -> Vec<Part> {
    let mut out = Vec::new();
    let string = |at: &mut usize, markup: bool, out: &mut Vec<Part>| -> Option<()> {
        let len = u32_at(d, *at)? as usize;
        let start = *at + 4;
        let end = start.checked_add(len.checked_mul(2)?)?;
        d.get(start..end)?;
        let nul = len > 0 && u16_at(d, end - 2)? == 0;
        out.push(Part {
            text: start..end - 2 * usize::from(nul),
            counts: vec![(*at, Count::Units)],
            markup,
        });
        *at = end;
        Some(())
    };
    let parse = |out: &mut Vec<Part>| -> Option<()> {
        if d.get(..16)? != STD_HLINK {
            return None;
        }
        let flags = u32_at(d, 20)?;
        let mut at = 24;
        if flags & 0x10 != 0 {
            string(&mut at, false, out)?;
        }
        if flags & 0x80 != 0 {
            string(&mut at, true, out)?;
        }
        if flags & 0x01 != 0 && flags & 0x100 != 0 {
            string(&mut at, false, out)?;
        } else if flags & 0x01 != 0 {
            let clsid = d.get(at..at + 16)?;
            at += 16;
            if clsid == URL_MONIKER {
                let len = u32_at(d, at)? as usize;
                let start = at + 4;
                let end = start.checked_add(len)?;
                d.get(start..end)?;
                let units = (start..end - 1)
                    .step_by(2)
                    .take_while(|&i| u16_at(d, i).is_some_and(|u| u != 0))
                    .count();
                out.push(Part {
                    text: start..start + 2 * units,
                    counts: vec![(at, Count::Bytes)],
                    markup: false,
                });
                at = end;
            } else if clsid == FILE_MONIKER {
                let ansi = u32_at(d, at + 2)? as usize;
                let size_at = (at + 6).checked_add(ansi)? + 24;
                let size = u32_at(d, size_at)? as usize;
                if size > 0 {
                    let bytes = u32_at(d, size_at + 4)? as usize;
                    let start = size_at + 10;
                    d.get(start..start.checked_add(bytes)?)?;
                    out.push(Part {
                        text: start..start + bytes,
                        counts: vec![(size_at, Count::Bytes), (size_at + 4, Count::Bytes)],
                        markup: false,
                    });
                }
                at = size_at + 4 + size;
            } else {
                return None;
            }
        }
        if flags & 0x08 != 0 {
            string(&mut at, true, out)?;
        }
        Some(())
    };
    parse(&mut out);
    out
}

fn is_fopt(kind: u16) -> bool {
    matches!(kind, 0xF00B | 0xF121 | 0xF122)
}

/// The property tables among the records of `range`, by the offset of their
/// header, containers searched.
fn collect_fopts(data: &[u8], range: Range<usize>, depth: usize, out: &mut Vec<usize>) {
    let mut at = range.start;
    while at + 8 <= range.end {
        let Some(r) = record(data, at) else { break };
        let end = (at + 8).saturating_add(u32_at(data, at + 4).unwrap_or(0) as usize);
        if end > range.end {
            break;
        }
        let container = u16_at(data, at).is_some_and(|v| v & 0xF == 0xF);
        if container && depth < MAX_DEPTH {
            collect_fopts(data, r.body, depth + 1, out);
        } else if is_fopt(r.kind) {
            out.push(at);
        }
        at = end;
    }
}

/// The records of the drawing blob: the drawing group, then each drawing
/// after the byte that says whether it is the main document's or the
/// headers'.
fn drawing_ranges(data: &[u8]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let Some(dgg) = record(data, 0).filter(|r| r.kind == 0xF000) else {
        return out;
    };
    out.push(0..dgg.body.end);
    let mut at = dgg.body.end;
    while let Some(dg) = record(data, at + 1).filter(|r| r.kind == 0xF002) {
        out.push(at + 1..dg.body.end);
        at = dg.body.end;
    }
    out
}

fn drawing_fopts(data: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    for range in drawing_ranges(data) {
        collect_fopts(data, range, 0, &mut out);
    }
    out
}

/// The drawing blob with the complex properties of `edits`, by table then by
/// rank, replaced, and every enclosing record's length following.
fn splice_drawings(data: &[u8], edits: &Edits) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    for range in drawing_ranges(data) {
        out.extend_from_slice(&data[at..range.start]);
        splice(data, range.clone(), edits, &mut out)?;
        at = range.end;
    }
    out.extend_from_slice(&data[at..]);
    Ok(out)
}

fn splice(data: &[u8], range: Range<usize>, edits: &Edits, out: &mut Vec<u8>) -> Result<()> {
    let mut at = range.start;
    while at + 8 <= range.end {
        let r = record(data, at).context("a drawing record cut short")?;
        let end = (at + 8).saturating_add(u32_at(data, at + 4).unwrap_or(0) as usize);
        if end > range.end {
            break;
        }
        if edits.range(at..end).next().is_none() {
            out.extend_from_slice(&data[at..end]);
            at = end;
            continue;
        }
        let header = out.len();
        out.extend_from_slice(&data[at..at + 8]);
        if let Some(slots) = edits.get(&at) {
            let props = properties(data, at).context("a property table that no longer parses")?;
            for (slot, p) in props.iter().enumerate() {
                let op = match slots.get(&slot) {
                    Some(new) => new.len() as u32,
                    None => p.op,
                };
                out.extend_from_slice(&p.opid.to_le_bytes());
                out.extend_from_slice(&op.to_le_bytes());
            }
            for (slot, p) in props.iter().enumerate() {
                match (slots.get(&slot), &p.data) {
                    (Some(new), _) => out.extend_from_slice(new),
                    (None, Some(bytes)) => out.extend_from_slice(&data[bytes.clone()]),
                    (None, None) => {}
                }
            }
        } else {
            splice(data, r.body, edits, out)?;
        }
        let len = (out.len() - header - 8) as u32;
        out[header + 4..header + 8].copy_from_slice(&len.to_le_bytes());
        at = end;
    }
    out.extend_from_slice(&data[at..range.end]);
    Ok(())
}

/// Each inline picture the character properties point at in the Data
/// stream: the offset of its PICF and the range of its shape container.
fn inline_shapes(doc: &Doc) -> Vec<(usize, Range<usize>)> {
    let data = doc.data_stream();
    let mut locations = BTreeSet::new();
    let grpprls = doc
        .chpx
        .iter()
        .map(|r| r.grpprl.as_slice())
        .chain(doc.text.grpprls.iter().map(Vec::as_slice));
    for grpprl in grpprls {
        for (sprm, op) in sprms(grpprl) {
            if sprm == code::C_PIC_LOCATION {
                locations.extend(u32_at(op, 0));
            }
        }
    }
    locations
        .into_iter()
        .filter_map(|location| {
            let at = location as usize;
            let lcb = u32_at(data, at)? as usize;
            if u16_at(data, at + 4)? != 0x44 {
                return None;
            }
            let end = at.checked_add(lcb)?.min(data.len());
            let mut p = at + 0x44;
            if u16_at(data, at + 6)? == 0x66 {
                p += 1 + *data.get(p)? as usize;
            }
            let shape = record(data, p).filter(|r| r.kind == 0xF004)?;
            (u16_at(data, p)? & 0xF == 0xF && shape.body.end <= end)
                .then_some((at, p..shape.body.end))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counted(text: &str) -> Vec<u8> {
        let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut out = (units.len() as u32).to_le_bytes().to_vec();
        out.extend(units.into_iter().flat_map(u16::to_le_bytes));
        out
    }

    /// A hyperlink object shown as `display`, to `url`, at `location`.
    fn link(display: &str, url: &str, location: &str) -> Vec<u8> {
        let mut out = STD_HLINK.to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend((0x10u32 | 0x01 | 0x08).to_le_bytes());
        out.extend(counted(display));
        out.extend(URL_MONIKER);
        let url: Vec<u8> = url
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        out.extend((url.len() as u32).to_le_bytes());
        out.extend(url);
        out.extend(counted(location));
        out
    }

    fn strings(data: &[u8]) -> Vec<(String, bool)> {
        link_parts(data)
            .into_iter()
            .map(|p| (utf16(&data[p.text]), p.markup))
            .collect()
    }

    #[test]
    fn a_hyperlink_yields_its_display_name_its_url_and_its_location() {
        let data = link("Site de Jean Dupont", "http://dupont.fr/", "haut");
        assert_eq!(
            strings(&data),
            [
                ("Site de Jean Dupont".to_string(), false),
                ("http://dupont.fr/".to_string(), false),
                ("haut".to_string(), true),
            ]
        );
    }

    #[test]
    fn a_hyperlink_string_takes_its_new_length_and_so_do_its_counts() {
        let data = link("Site de Jean Dupont", "http://dupont.fr/", "haut");
        let parts = link_parts(&data);
        let edited = edit_link(
            &data,
            vec![
                (&parts[0], "Site de [A] [B]"),
                (&parts[1], "http://[B].fr/"),
            ],
        );
        assert_eq!(edited, link("Site de [A] [B]", "http://[B].fr/", "haut"));
    }
}
