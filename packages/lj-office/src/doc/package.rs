//! Edits of a `.doc`: a run replaced by a text, empty or not, goes through
//! the piece table. The container is then rebuilt from its streams, and no
//! free sector keeps the old text.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use anyhow::{bail, Context, Result};

use super::bytes::{u16_at, Plc};
use super::cfb::{Cfb, Kind};
use super::codepage::Codepage;
use super::edit;
use super::fib::{lcb, Generation};
use super::sttb;
use super::summary;
use super::text::Story;
use super::values::{self, Value};
use super::word6;
use super::{Doc, DocChar};
use crate::{field, views_showing, Anchor, Replacement, Views, EVERY_VIEW};

/// Every character of every story, control characters as line breaks so no
/// match spans a paragraph, a cell or a field boundary. Field instructions,
/// hidden and deleted text are included: they are in the file.
pub fn characters(doc: &Doc) -> Vec<(char, DocChar)> {
    let mut out = Vec::new();
    for (story, range) in &doc.text.stories {
        let mut cp = range.start;
        while cp < range.end {
            let unit = doc.text.chars[cp as usize];
            let at = DocChar { story: *story, cp };
            match unit {
                0x09 | 0xA0 => out.push((' ', at)),
                0x00..=0x1F => out.push(('\n', at)),
                0xD800..=0xDBFF => {
                    let low = doc.text.chars.get(cp as usize + 1).copied().unwrap_or(0);
                    if let Some(Ok(c)) = char::decode_utf16([unit, low]).next() {
                        out.push((c, at));
                        cp += 1;
                    }
                }
                u => {
                    if let Some(c) = char::from_u32(u as u32) {
                        out.push((c, at));
                    }
                }
            }
            cp += 1;
        }
        out.push((
            '\n',
            DocChar {
                story: *story,
                cp: range.end,
            },
        ));
    }
    out
}

/// For each character of [`feed`], the views that show it, from the tracked
/// change marks and the hidden text of its character properties. A paragraph
/// mark's line break shows where the mark does.
pub fn views(doc: &Doc) -> Vec<Views> {
    let section_ends: Vec<u32> = doc.sections.iter().map(|(r, _)| r.end).collect();
    let mut shown: HashMap<u8, (u32, Vec<Views>)> = HashMap::new();
    for (story, range) in &doc.text.stories {
        let mut views = Vec::with_capacity(range.len());
        let mut start = range.start;
        for cp in range.clone() {
            let ends = match doc.text.chars[cp as usize] {
                0x0D | 0x07 => true,
                0x0C => *story == Story::Main && section_ends.contains(&(cp + 1)),
                _ => false,
            } || cp + 1 == range.end;
            if !ends {
                continue;
            }
            let (_, style, _) = doc.paragraph_props(cp);
            let mut key = None;
            let mut run = EVERY_VIEW;
            for at in start..=cp {
                let k = doc.char_key(at);
                if key != Some(k) {
                    key = Some(k);
                    let chp = doc.char_props(at, &style);
                    run = views_showing(chp.deleted, chp.inserted, chp.vanish || chp.field_vanish);
                }
                views.push(run);
            }
            start = cp + 1;
        }
        shown.insert(story.index(), (range.start, views));
    }
    feed(doc)
        .iter()
        .map(|(_, anchor)| match anchor {
            Anchor::DocChar { story, cp } => shown
                .get(story)
                .and_then(|(start, views)| views.get(cp.checked_sub(*start)? as usize))
                .copied()
                .unwrap_or(EVERY_VIEW),
            _ => EVERY_VIEW,
        })
        .collect()
}

/// The keyword and switches of each field instruction in [`characters`], from
/// the field's 0x13 to its 0x14, or to its 0x15 when it shows no result.
pub fn markup(doc: &Doc) -> HashSet<Anchor> {
    let mut out = HashSet::new();
    for (story, range) in &doc.text.stories {
        let mut add = |instruction: &[(char, u32)]| {
            let chars: Vec<char> = instruction.iter().map(|(c, _)| *c).collect();
            out.extend(field::markup(&chars).into_iter().map(|at| Anchor::DocChar {
                story: story.index(),
                cp: instruction[at].1,
            }));
        };
        let mut fields: Vec<(bool, Vec<(char, u32)>)> = Vec::new();
        for cp in range.clone() {
            match doc.text.chars[cp as usize] {
                0x13 => fields.push((false, Vec::new())),
                0x14 => {
                    if let Some((separated, instruction)) = fields.last_mut().filter(|f| !f.0) {
                        *separated = true;
                        add(instruction);
                    }
                }
                0x15 => {
                    if let Some((_, instruction)) = fields.pop().filter(|f| !f.0) {
                        add(&instruction);
                    }
                }
                unit => {
                    if let Some((_, instruction)) = fields.last_mut().filter(|f| !f.0) {
                        instruction.extend(char::from_u32(u32::from(unit)).map(|c| (c, cp)));
                    }
                }
            }
        }
        for (_, instruction) in fields.iter().filter(|f| !f.0) {
            add(instruction);
        }
    }
    for (i, value) in values::values(doc).iter().enumerate() {
        if value.markup {
            out.extend(
                (0..value.text.chars().count()).map(|offset| Anchor::DocValue {
                    value: i as u32,
                    offset: offset as u32,
                }),
            );
        }
    }
    out
}

/// The code pages 8-bit text in this file may be in: Word 97 compresses in
/// cp1252 alone, Word 6 in the language's and in each font's.
fn codepages(doc: &Doc) -> Vec<Codepage> {
    let mut numbers = vec![1252];
    if doc.fib.generation == Generation::Word6 {
        numbers.push(Codepage::number_for_lid(doc.fib.lid));
        numbers.extend(
            doc.fonts
                .iter()
                .filter_map(|f| word6::charset_codepage(f.charset)),
        );
    }
    numbers.sort_unstable();
    numbers.dedup();
    numbers.into_iter().map(Codepage::windows).collect()
}

/// The characters as read, then, for Word 6, as each other code page of
/// the file would read its 8-bit text: a font the reader attributes wrongly
/// must not hide a character from a search.
fn feeds(doc: &Doc) -> Vec<Vec<(char, DocChar)>> {
    let first = characters(doc);
    let mut out = Vec::new();
    if doc.fib.generation == Generation::Word6 && !doc.fib.ext_char {
        let wd = doc.word_document();
        for cp in codepages(doc) {
            let other: Vec<(char, DocChar)> = first
                .iter()
                .map(|(c, at)| {
                    let byte = doc
                        .text
                        .bytes_of(at.cp)
                        .filter(|r| r.len() == 1)
                        .and_then(|r| wd.get(r.start).copied());
                    match byte {
                        Some(b) if b >= 0x80 => {
                            (char::from_u32(cp.decode(b) as u32).unwrap_or(*c), *at)
                        }
                        _ => (*c, *at),
                    }
                })
                .collect();
            if other.iter().zip(&first).any(|(a, b)| a.0 != b.0) {
                out.push(other);
            }
        }
    }
    out.insert(0, first);
    out
}

/// [`characters`] as the edits anchor them, then the strings kept outside
/// the stories, one per line.
pub fn feed(doc: &Doc) -> Vec<(char, Anchor)> {
    let mut out = anchored(&characters(doc));
    append_values(&values::values(doc), &mut out);
    out
}

fn append_values(values: &[Value], out: &mut Vec<(char, Anchor)>) {
    for (i, value) in values.iter().enumerate() {
        let at = |offset: usize| Anchor::DocValue {
            value: i as u32,
            offset: offset as u32,
        };
        let mut length = 0;
        for (offset, c) in value.text.chars().enumerate() {
            out.push((if c.is_control() { '\n' } else { c }, at(offset)));
            length = offset + 1;
        }
        out.push(('\n', at(length)));
    }
}

fn anchored(feed: &[(char, DocChar)]) -> Vec<(char, Anchor)> {
    feed.iter()
        .map(|(c, a)| {
            (
                *c,
                Anchor::DocChar {
                    story: a.story.index(),
                    cp: a.cp,
                },
            )
        })
        .collect()
}

/// A `.doc` held in memory: the document as read, and the container the
/// edits rewrite.
pub struct DocPackage {
    doc: Doc,
    cfb: Cfb,
}

impl DocPackage {
    pub fn open(bytes: &[u8]) -> Result<Self> {
        let doc = Doc::parse(bytes)?;
        let cfb = doc.cfb.clone();
        Ok(Self { doc, cfb })
    }

    /// Every character of every story, as the edits left them.
    pub fn characters(&self) -> Vec<(char, Anchor)> {
        feed(&self.doc)
    }

    /// For each character of [`Self::characters`], the views that show it.
    pub fn views(&self) -> Vec<Views> {
        views(&self.doc)
    }

    /// [`Self::characters`], then, for Word 6, the stories as each other code
    /// page of the file reads the 8-bit text as it was opened.
    pub fn readings(&self) -> Vec<Vec<(char, Anchor)>> {
        let mut out: Vec<Vec<(char, Anchor)>> =
            feeds(&self.doc).iter().map(|f| anchored(f)).collect();
        append_values(&values::values(&self.doc), &mut out[0]);
        out
    }

    /// The code pages 8-bit text in this file may be in.
    pub fn codepages(&self) -> Vec<Codepage> {
        codepages(&self.doc)
    }

    /// Apply `replacements` and return how many characters went. Each takes
    /// the characters from its first to its last out of the story, its text
    /// in their place, and every table counting in characters follows.
    /// Replacements that overlap merge into one run, their texts joined in
    /// the order they start, as a ZIP-XML package lays them. A run keeps the
    /// control characters it spans, which the tables anchor to: its text
    /// lands before the first of them, and what follows each is removed.
    pub fn replace(&mut self, replacements: &[Replacement]) -> Result<usize> {
        let (strings, text): (Vec<&Replacement>, Vec<&Replacement>) = replacements
            .iter()
            .partition(|r| matches!(r.anchors.first(), Some(Anchor::DocValue { .. })));
        self.doc.cfb = self.cfb.clone();
        let mut removed = 0;
        if !strings.is_empty() {
            removed += self.replace_values(&strings)?;
        }
        let mut runs: Vec<(Range<u32>, String)> = Vec::new();
        for r in text {
            let cps = r.anchors.iter().map(doc_cp).collect::<Result<Vec<u32>>>()?;
            let (Some(&first), Some(&last)) = (cps.iter().min(), cps.iter().max()) else {
                continue;
            };
            let high = self.doc.text.chars.get(last as usize).copied().unwrap_or(0);
            let end = last + 1 + u32::from((0xD800..0xDC00).contains(&high));
            runs.push((first..end, r.text.clone()));
        }
        if runs.is_empty() {
            return Ok(removed);
        }
        runs.sort_by_key(|(cps, _)| cps.start);
        let mut merged: Vec<(Range<u32>, String)> = Vec::new();
        for (cps, text) in runs {
            match merged.last_mut() {
                Some((last, joined)) if cps.start < last.end => {
                    last.end = last.end.max(cps.end);
                    joined.push_str(&text);
                }
                _ => merged.push((cps, text)),
            }
        }
        let mut edits: Vec<edit::Replacement> = Vec::new();
        for (cps, text) in merged {
            for (k, cps) in self.editable(cps).into_iter().enumerate() {
                let text = if k == 0 { text.clone() } else { String::new() };
                edits.push(edit::Replacement { cps, text });
            }
        }
        let chars = &self.doc.text.chars;
        removed += edits
            .iter()
            .flat_map(|e| e.cps.clone())
            .filter(|cp| !(0xDC00..0xE000).contains(&chars[*cp as usize]))
            .count();
        let cfb = edit::replace(&self.doc, &edits)?;
        *self = Self::open(&cfb.to_bytes())?;
        Ok(removed)
    }

    /// The replacements that fall in the strings kept outside the stories,
    /// each string rewritten whole; how many characters went.
    fn replace_values(&mut self, replacements: &[&Replacement]) -> Result<usize> {
        let mut runs: BTreeMap<u32, Vec<(Range<usize>, &str)>> = BTreeMap::new();
        for r in replacements {
            let mut value = None;
            let mut offsets = Vec::with_capacity(r.anchors.len());
            for anchor in &r.anchors {
                let Anchor::DocValue { value: v, offset } = *anchor else {
                    bail!("a replacement spanning a story and a string outside it");
                };
                if value.replace(v).is_some_and(|w| w != v) {
                    bail!("a replacement spanning two strings");
                }
                offsets.push(offset as usize);
            }
            let (Some(value), Some(&first), Some(&last)) =
                (value, offsets.iter().min(), offsets.iter().max())
            else {
                continue;
            };
            runs.entry(value)
                .or_default()
                .push((first..last + 1, r.text.as_str()));
        }
        let values = values::values(&self.doc);
        let mut texts = BTreeMap::new();
        let mut removed = 0;
        for (value, mut spans) in runs {
            let chars: Vec<char> = values
                .get(value as usize)
                .context("a string past the document's")?
                .text
                .chars()
                .collect();
            spans.sort_by_key(|(range, _)| range.start);
            let mut out = String::new();
            let mut at = 0;
            for (range, text) in spans {
                if range.start >= at {
                    out.extend(&chars[at..range.start]);
                }
                out.push_str(text);
                removed += range.end.saturating_sub(range.start.max(at));
                at = at.max(range.end);
            }
            out.extend(&chars[at..]);
            texts.insert(value as usize, out);
        }
        let mut cfb = self.cfb.clone();
        values::rewrite(&self.doc, &mut cfb, &texts)?;
        *self = Self::open(&cfb.to_bytes())?;
        Ok(removed)
    }

    /// `cps` cut around the characters a table anchors to and at the start
    /// of each story, which a replacement may not span.
    fn editable(&self, cps: Range<u32>) -> Vec<Range<u32>> {
        let text = &self.doc.text;
        let mut out = Vec::new();
        let mut open: Option<u32> = None;
        for cp in cps.clone() {
            let unit = text.chars.get(cp as usize).copied().unwrap_or(0);
            let anchored = unit < 0x20 && !matches!(unit, 0x09 | 0x0B | 0x1E | 0x1F);
            let story = text.stories.iter().any(|(_, r)| r.start == cp);
            if let Some(start) = open.filter(|_| anchored || story) {
                out.push(start..cp);
                open = None;
            }
            if !anchored && open.is_none() {
                open = Some(cp);
            }
        }
        out.extend(open.map(|start| start..cps.end));
        out
    }

    /// Overwrite the tables naming who handled the document, and empty its
    /// summary properties.
    pub fn clear_authors(&mut self) {
        scrub_authors(&self.doc, &mut self.cfb);
        scrub_properties(&mut self.cfb);
    }

    /// Rewrite every stream of the container in place.
    pub fn rewrite_streams(&mut self, mut rewrite: impl FnMut(&[Range<usize>], &mut Vec<u8>)) {
        let (main, stories) = (self.main_stream(), self.story_bytes());
        for (at, node) in self.cfb.nodes.iter_mut().enumerate() {
            if node.kind == Kind::Stream {
                let text = if Some(at) == main { &stories[..] } else { &[] };
                rewrite(text, &mut node.data);
            }
        }
    }

    /// The bytes of every node of the container, storages and streams alike,
    /// each with the ranges the stories' characters take in it, which a
    /// search reads as text rather than as bytes.
    pub fn nodes(&self) -> impl Iterator<Item = (Vec<Range<usize>>, &[u8])> {
        let (main, stories) = (self.main_stream(), self.story_bytes());
        self.cfb.nodes.iter().enumerate().map(move |(at, n)| {
            let text = if Some(at) == main {
                stories.clone()
            } else {
                Vec::new()
            };
            (text, n.data.as_slice())
        })
    }

    /// The node of the document's own `WordDocument` stream.
    fn main_stream(&self) -> Option<usize> {
        self.cfb.nodes[0]
            .children
            .iter()
            .copied()
            .find(|&at| self.cfb.nodes[at].name == "WordDocument")
    }

    /// The bytes of the `WordDocument` stream the pieces of the stories take.
    fn story_bytes(&self) -> Vec<Range<usize>> {
        let len = self.doc.word_document().len();
        self.doc
            .text
            .pieces
            .iter()
            .map(|p| {
                let start = (p.fc as usize).min(len);
                let size = p.cp_end.saturating_sub(p.cp) as usize * p.width() as usize;
                start..start.saturating_add(size).min(len)
            })
            .collect()
    }

    /// The container rebuilt from its streams.
    pub fn save(&self) -> Vec<u8> {
        self.cfb.to_bytes()
    }
}

fn doc_cp(anchor: &Anchor) -> Result<u32> {
    match *anchor {
        Anchor::DocChar { cp, .. } => Ok(cp),
        Anchor::XmlText { .. } => bail!("a ZIP-XML character anchored in a .doc"),
        Anchor::DocValue { .. } => bail!("a string outside the stories among their characters"),
    }
}

/// Overwrite every string of the tables that name the people who handled the
/// document: revision authors, comment authors and their initials, the
/// last savers and their paths, and the associated strings (template path,
/// title, author), on which the text of the document does not depend. Each
/// string of a table keeps a mark of its own, since LibreOffice drops the
/// insertion an author deleted as their own: two reviewers made one would
/// lose text.
fn scrub_authors(doc: &Doc, cfb: &mut Cfb) {
    let fib = &doc.fib;
    let name = match fib.generation {
        Generation::Word6 => "WordDocument",
        Generation::Word97 if fib.table1 => "1Table",
        Generation::Word97 => "0Table",
    };
    let Some(table) = cfb.stream_mut(name) else {
        return;
    };
    let mut spans: Vec<(usize, usize, bool, usize)> = Vec::new();
    if fib.generation == Generation::Word6 {
        let (fc, len) = fib.pair(lcb::STTBFASSOC);
        let data = table
            .get(fc as usize..(fc as usize).saturating_add(len as usize).min(table.len()))
            .unwrap_or(&[]);
        spans.extend(
            word6::sttb(data)
                .into_iter()
                .enumerate()
                .map(|(i, r)| (fc as usize + r.start, fc as usize + r.end, false, i)),
        );
    }
    for index in [
        lcb::STTBFRMARK,
        lcb::STTBSAVEDBY,
        lcb::STTBFASSOC,
        lcb::STTBFNM,
    ] {
        if fib.generation == Generation::Word6 {
            break;
        }
        let (fc, len) = fib.pair(index);
        let (fc, len) = (fc as usize, len as usize);
        if len == 0 || fc >= table.len() {
            continue;
        }
        let data = &table[fc..fc.saturating_add(len).min(table.len())];
        for (i, e) in sttb::parse(data).into_iter().enumerate() {
            spans.push((fc + e.bytes.start, fc + e.bytes.end, e.wide, i));
        }
    }
    let (fc, len) = fib.pair(lcb::GRPXSTATNOWNERS);
    let (fc, len) = (fc as usize, len as usize);
    let end = fc.saturating_add(len).min(table.len());
    let mut at = fc;
    let mut owner = 0;
    while at + 2 <= end {
        let cch = u16_at(table, at).unwrap_or(0) as usize;
        spans.push((at + 2, (at + 2 + 2 * cch).min(end), true, owner));
        at += 2 + 2 * cch;
        owner += 1;
    }
    let (fc, len) = fib.pair(lcb::PLCFANDREF);
    let refs = Plc::new(fib.table(table, lcb::PLCFANDREF), 30);
    let data_at = (fc as usize).saturating_add(4 * (refs.n + 1));
    let whole = len as usize == 4 * (refs.n + 1) + 30 * refs.n;
    for i in (0..refs.n).filter(|_| fib.generation == Generation::Word97 && whole) {
        let at = data_at + 30 * i;
        let cch = (u16_at(table, at).unwrap_or(0) as usize).min(9);
        spans.push((at + 2, at + 2 + 2 * cch, true, 0));
    }
    for (start, end, wide, nth) in spans {
        let end = end.min(table.len());
        if start >= end {
            continue;
        }
        let width = if wide { 2 } else { 1 };
        for (k, unit) in table[start..end].chunks_exact_mut(width).enumerate() {
            unit[0] = mark(nth, k);
            if wide {
                unit[1] = 0;
            }
        }
    }
}

/// Character `k` of the placeholder for the `nth` string of a table: `nth`
/// in base 26 over `A`..`Z`, then `X`s.
fn mark(nth: usize, k: usize) -> u8 {
    let digits = std::iter::successors(Some(nth), |n| (*n >= 26).then(|| n / 26 - 1)).count();
    if k >= digits {
        return b'X';
    }
    let mut n = nth;
    for _ in 0..digits - 1 - k {
        n = n / 26 - 1;
    }
    b'A' + (n % 26) as u8
}

fn scrub_properties(cfb: &mut Cfb) {
    for name in ["\u{5}SummaryInformation", "\u{5}DocumentSummaryInformation"] {
        let Some(at) = cfb.find(name) else { continue };
        match summary::scrub(&cfb.nodes[at].data) {
            Some(clean) => cfb.nodes[at].data = clean,
            None => cfb.nodes[0].children.retain(|c| *c != at),
        }
    }
}
