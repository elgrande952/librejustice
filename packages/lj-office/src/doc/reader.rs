//! From the tables of a [`Doc`] to its [`Document`]: paragraphs with their
//! resolved properties, runs of like-formatted text, tables, notes, headers.

use std::ops::Range;

use super::bytes::{i16_at, u16_at, u32_at, utf16, Plc};
use super::fib::lcb;
use super::lists::{self, Numbering};
use super::model::{
    Block, Content, Document, Label, Note, Paragraph, Picture, Row, Run, Section, Table,
};
use super::pictures;
use super::props::{Chp, Tap};
use super::text::Story;
use super::Doc;

/// Tables one may nest in another; a deeper paragraph joins the deepest.
const MAX_NESTING: i32 = 64;

struct Para {
    paragraph: Paragraph,
    depth: i32,
    cell_end: bool,
    row_end: bool,
    tap: Option<Tap>,
}

struct Builder<'a> {
    doc: &'a Doc,
    pictures: Vec<Picture>,
    /// Open fields: whether the separator has been seen, and the instruction
    /// with the CP of each of its characters.
    fields: Vec<(bool, Vec<(char, u32)>)>,
    /// Where in the Data stream the open field's form data starts.
    form_data: Option<u32>,
    /// The DOP's fRMView is off: a deletion is not shown.
    hide_deletions: bool,
    notes: u32,
    /// CP just past the last text run, which the next character extends.
    text_end: u32,
}

impl Document {
    pub fn read(doc: &Doc) -> Document {
        let mut b = Builder {
            doc,
            pictures: Vec::new(),
            fields: Vec::new(),
            form_data: None,
            hide_deletions: doc
                .fib
                .table(doc.table_stream(), lcb::DOP)
                .get(7)
                .is_some_and(|b| b & 0x08 == 0),
            notes: 0,
            text_end: u32::MAX,
        };
        let range = |story: Story| doc.text.story(story).unwrap_or(0..0);
        let main = b.blocks(Story::Main, range(Story::Main));
        let table = doc.table_stream();
        let fib = &doc.fib;

        // A story cut by a PLC of story-relative CPs carrying `cb` bytes each.
        let split = |story: Story, index: usize, cb: usize| -> Vec<Range<u32>> {
            let base = range(story);
            if base.is_empty() {
                return Vec::new();
            }
            let plc = Plc::new(fib.table(table, index), cb);
            (0..plc.n)
                .map(|i| {
                    let s = base.start.saturating_add(plc.pos(i)).min(base.end);
                    let e = base
                        .start
                        .saturating_add(plc.pos(i + 1))
                        .min(base.end)
                        .max(s);
                    s..e
                })
                .collect()
        };
        let headers = split(Story::Headers, lcb::PLCFHDD, 0)
            .into_iter()
            .map(|r| {
                let mut blocks = b.blocks(Story::Headers, r);
                drop_guard(&mut blocks);
                blocks
            })
            .collect();
        let notes =
            |b: &mut Builder, story: Story, refs: usize, txt: usize, cb: usize| -> Vec<Note> {
                let refs = Plc::new(fib.table(table, refs), cb);
                let ranges = split(story, txt, 0);
                (0..refs.n.min(ranges.len()))
                    .map(|i| {
                        b.notes = i as u32 + 1;
                        Note {
                            reference: refs.pos(i),
                            author: None,
                            blocks: b.blocks(story, ranges[i].clone()),
                        }
                    })
                    .collect()
            };
        let footnotes = notes(
            &mut b,
            Story::Footnotes,
            lcb::PLCFFNDREF,
            lcb::PLCFFNDTXT,
            2,
        );
        let endnotes = notes(&mut b, Story::Endnotes, lcb::PLCFENDREF, lcb::PLCFENDTXT, 2);
        let mut comments = notes(
            &mut b,
            Story::Comments,
            lcb::PLCFANDREF,
            lcb::PLCFANDTXT,
            30,
        );
        let owners = xst_list(fib.table(table, lcb::GRPXSTATNOWNERS));
        let refs = Plc::new(fib.table(table, lcb::PLCFANDREF), 30);
        for (i, c) in comments.iter_mut().enumerate() {
            if i < refs.n {
                let ibst = u16_at(refs.data(i), 20).unwrap_or(0) as usize;
                c.author = owners.get(ibst).cloned();
            }
        }
        let boxes = |b: &mut Builder, story: Story, index: usize| -> Vec<Vec<Block>> {
            split(story, index, 22)
                .into_iter()
                .map(|r| b.blocks(story, r))
                .collect()
        };
        let textboxes = boxes(&mut b, Story::Textboxes, lcb::PLCFTXBXTXT);
        let header_textboxes = boxes(&mut b, Story::HeaderTextboxes, lcb::PLCFHDRTXBXTXT);
        Document {
            main,
            sections: doc
                .sections
                .iter()
                .map(|(cp, sep)| Section {
                    cp: cp.clone(),
                    sep: sep.clone(),
                })
                .collect(),
            headers,
            footnotes,
            endnotes,
            comments,
            textboxes,
            header_textboxes,
            fonts: doc.fonts.clone(),
            pictures: b.pictures,
            warnings: Vec::new(),
        }
    }

    /// The main story as plain text: one line per paragraph, list labels
    /// included, field instructions and hidden text left out.
    pub fn main_text(&self) -> String {
        let mut out = String::new();
        let mut paras = Vec::new();
        for b in &self.main {
            b.paragraphs(&mut paras);
        }
        let mut depth = 0usize;
        for p in paras {
            if let Some(label) = &p.label {
                out.push_str(&label.text);
            }
            for run in &p.runs {
                match &run.content {
                    Content::FieldStart(_) => depth += 1,
                    Content::FieldEnd => depth = depth.saturating_sub(1),
                    Content::Text(t) if !run.chp.vanish => out.push_str(t),
                    Content::Tab => out.push('\t'),
                    Content::LineBreak | Content::PageBreak | Content::ColumnBreak => {
                        out.push('\n')
                    }
                    Content::NoteReference(mark) => out.push_str(mark),
                    _ => {}
                }
            }
            out.push('\n');
        }
        out
    }
}

/// The entry a drop-down's FFData selects, else its first: the structure
/// follows a header whose size is at `at + 4`.
fn drop_down_entry(data: &[u8], at: usize) -> Option<String> {
    let data = data.get(at + usize::from(u16_at(data, at + 4)?)..)?;
    if u32_at(data, 0)? != 0xFFFF_FFFF {
        return None;
    }
    let bits = *data.get(4)?;
    if bits & 3 != 2 {
        return None;
    }
    let selected = usize::from((bits >> 2) & 0x1F);
    // Past the maximum length and the size, the name, the default and the
    // five strings a form field carries, each a count, UTF-16 and a nul.
    let mut at = 10;
    at += 2 + 2 * usize::from(u16_at(data, at)?) + 2;
    at += 2;
    for _ in 0..5 {
        at += 2 + 2 * usize::from(u16_at(data, at)?) + 2;
    }
    if u16_at(data, at)? != 0xFFFF {
        return None;
    }
    let count = usize::from(u16_at(data, at + 2)?);
    at += 6;
    let mut entries = Vec::new();
    for _ in 0..count {
        let cch = usize::from(u16_at(data, at)?);
        entries.push(utf16(data.get(at + 2..at + 2 + 2 * cch)?));
        at += 2 + 2 * cch;
    }
    entries.get(selected).or(entries.first()).cloned()
}

/// A run of Xst strings (cch, then UTF-16), as the comment owner table
/// stores them.
fn xst_list(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(cch) = u16_at(data, at) {
        let end = at + 2 + 2 * cch as usize;
        let Some(raw) = data.get(at + 2..end) else {
            break;
        };
        out.push(utf16(raw));
        at = end;
    }
    out
}

impl Builder<'_> {
    fn blocks(&mut self, story: Story, range: Range<u32>) -> Vec<Block> {
        let mut paras = self.paragraphs(story, range);
        let mut i = 0;
        build(&mut paras, &mut i, 0)
    }

    fn paragraphs(&mut self, story: Story, range: Range<u32>) -> Vec<Para> {
        let doc = self.doc;
        let chars = &doc.text.chars;
        let section_ends: Vec<u32> = doc.sections.iter().map(|(r, _)| r.end).collect();
        let mut numbering = Numbering::default();
        let mut out = Vec::new();
        let mut start = range.start;
        let mut cp = range.start;
        while cp < range.end {
            let unit = chars[cp as usize];
            let ends = match unit {
                0x0D | 0x07 => true,
                0x0C => story == Story::Main && section_ends.contains(&(cp + 1)),
                _ => false,
            } || cp + 1 == range.end;
            if ends {
                out.push(self.paragraph(story, start..cp + 1, start > range.start, &mut numbering));
                start = cp + 1;
            }
            cp += 1;
        }
        out
    }

    fn paragraph(
        &mut self,
        story: Story,
        cp: Range<u32>,
        follows: bool,
        numbering: &mut Numbering,
    ) -> Para {
        let doc = self.doc;
        let end = cp.end - 1;
        let (pap, style_chp, tap) = doc.paragraph_props(end);
        // LibreOffice lets the direct formatting of the character before an
        // empty paragraph spill onto its mark, under the mark's own.
        let mark = if follows && cp.start == end {
            doc.char_props_on(end, doc.char_props(end - 1, &style_chp), &style_chp)
        } else {
            doc.char_props(end, &style_chp)
        };
        let mut runs: Vec<Run> = Vec::new();
        let mut key = None;
        let mut chp = style_chp.clone();
        let chars = &doc.text.chars;
        let mut at = cp.start;
        while at < end {
            let k = doc.char_key(at);
            if key != Some(k) {
                key = Some(k);
                chp = doc.char_props(at, &style_chp);
            }
            let unit = chars[at as usize];
            let width = self.push(
                &mut runs,
                at,
                unit,
                chars.get(at as usize + 1).copied(),
                &chp,
            );
            at += width;
        }
        let depth = if pap.in_table {
            pap.depth.clamp(1, MAX_NESTING)
        } else {
            0
        };
        let last = chars[end as usize];
        let (cell_end, row_end) = if depth <= 1 {
            (last == 0x07 && !pap.row_end, last == 0x07 && pap.row_end)
        } else {
            (pap.inner_cell && !pap.inner_row_end, pap.inner_row_end)
        };
        let label = (pap.ilfo > 0 && !row_end && !(mark.deleted && self.hide_deletions))
            .then(|| numbering.next(&doc.lists, pap.ilfo, pap.ilvl))
            .flatten()
            .map(|text| {
                let mut chp = mark.clone();
                let mut bullet = false;
                if let Some((_, level)) = doc.lists.level(pap.ilfo, pap.ilvl) {
                    chp.apply(&level.chpx, &mark, &|istd, under| {
                        doc.styles.character(istd, under)
                    });
                    bullet = level.nfc == 23;
                }
                Label { text, chp, bullet }
            });
        Para {
            paragraph: Paragraph {
                story,
                cp,
                pap,
                mark,
                label,
                runs,
            },
            depth,
            cell_end,
            row_end,
            tap,
        }
    }

    /// Append the content of the CP `at`; answer how many CPs it covered.
    fn push(
        &mut self,
        runs: &mut Vec<Run>,
        at: u32,
        unit: u16,
        next: Option<u16>,
        chp: &Chp,
    ) -> u32 {
        let in_instruction = self.fields.iter().any(|(sep, _)| !sep);
        match unit {
            0x13 => {
                self.fields.push((false, Vec::new()));
                self.form_data = None;
                return 1;
            }
            0x14 => {
                if let Some(top) = self.fields.last_mut() {
                    if !top.0 {
                        top.0 = true;
                        let instr: String = top.1.iter().map(|(c, _)| *c).collect();
                        let instr = instr.trim().to_string();
                        if !self.fields.iter().rev().skip(1).any(|(sep, _)| !sep) {
                            runs.push(Run {
                                cp: at,
                                chp: chp.clone(),
                                content: Content::FieldStart(instr),
                            });
                        }
                    }
                }
                return 1;
            }
            0x15 => {
                if let Some((sep, instr)) = self.fields.pop() {
                    let outer = !self.fields.iter().any(|(s, _)| !s);
                    if sep && outer {
                        runs.push(Run {
                            cp: at,
                            chp: chp.clone(),
                            content: Content::FieldEnd,
                        });
                    } else if !sep && outer {
                        self.macro_button(runs, &instr, chp);
                        self.drop_down(runs, &instr, chp);
                    }
                }
                return 1;
            }
            _ => {}
        }
        if in_instruction {
            if unit == 0x01 && chp.special {
                self.form_data = chp.pic_location.or(self.form_data);
            }
            if let Some(top) = self.fields.iter_mut().rev().find(|(sep, _)| !sep) {
                if let Some(c) = char::from_u32(unit as u32) {
                    top.1.push((c, at));
                }
            }
            return 1;
        }
        let content = match unit {
            0x09 => Content::Tab,
            0x0B => Content::LineBreak,
            0x0C => Content::PageBreak,
            0x0E => Content::ColumnBreak,
            0x01 if chp.special && chp.ole2 => Content::Object(chp.pic_location.unwrap_or(0)),
            0x01 if chp.special => match chp
                .pic_location
                .and_then(|loc| pictures::read(self.doc.data_stream(), loc))
            {
                Some(p) => {
                    self.pictures.push(p);
                    Content::Picture(self.pictures.len() - 1)
                }
                None => return 1,
            },
            0x08 if chp.special => Content::Shape,
            0x02 if chp.special => Content::NoteReference(self.notes_ref(at)),
            0x05 if chp.special => Content::CommentReference,
            0x28 if chp.symbol.is_some() => {
                let (_, ch) = chp.symbol.unwrap_or_default();
                let ch = if ch < 0x100 { ch | 0xF000 } else { ch };
                self.text(
                    runs,
                    at,
                    &char::from_u32(ch as u32).unwrap_or('\u{FFFD}').to_string(),
                    chp,
                );
                return 1;
            }
            0x1E => {
                self.text(runs, at, "\u{2011}", chp);
                return 1;
            }
            0x1F => {
                self.text(runs, at, "\u{AD}", chp);
                return 1;
            }
            0x00..=0x1F => return 1,
            0xD800..=0xDBFF => {
                if let Some(low @ 0xDC00..=0xDFFF) = next {
                    let c = char::decode_utf16([unit, low])
                        .next()
                        .and_then(|r| r.ok())
                        .unwrap_or('\u{FFFD}');
                    self.text(runs, at, &c.to_string(), chp);
                    return 2;
                }
                return 1;
            }
            0xDC00..=0xDFFF => return 1,
            u => {
                let c = char::from_u32(u as u32).unwrap_or('\u{FFFD}');
                self.text(runs, at, c.encode_utf8(&mut [0; 4]), chp);
                return 1;
            }
        };
        runs.push(Run {
            cp: at,
            chp: chp.clone(),
            content,
        });
        1
    }

    fn text(&mut self, runs: &mut Vec<Run>, at: u32, s: &str, chp: &Chp) {
        let width = if s.chars().any(|c| c.len_utf16() == 2) {
            2
        } else {
            1
        };
        if let Some(Run {
            chp: last,
            content: Content::Text(t),
            ..
        }) = runs.last_mut()
        {
            if self.text_end == at && last == chp {
                t.push_str(s);
                self.text_end = at + width;
                return;
            }
        }
        runs.push(Run {
            cp: at,
            chp: chp.clone(),
            content: Content::Text(s.to_string()),
        });
        self.text_end = at + width;
    }

    /// A field with no result that Word still displays: MACROBUTTON shows
    /// the text after the macro name.
    fn macro_button(&mut self, runs: &mut Vec<Run>, instr: &[(char, u32)], chp: &Chp) {
        let words: String = instr.iter().map(|(c, _)| *c).collect();
        let mut parts = words.split_whitespace();
        if !parts
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("MACROBUTTON"))
            || parts.next().is_none()
        {
            return;
        }
        let mut seen = 0;
        let mut in_word = false;
        let mut start = instr.len();
        for (i, (c, _)) in instr.iter().enumerate() {
            let space = c.is_whitespace();
            if !space && !in_word {
                seen += 1;
                if seen == 3 {
                    start = i;
                    break;
                }
            }
            in_word = !space;
        }
        let Some((_, first)) = instr.get(start) else {
            return;
        };
        runs.push(Run {
            cp: *first,
            chp: chp.clone(),
            content: Content::FieldStart(words.trim().to_string()),
        });
        // A format switch and its argument are not part of what is shown.
        let shown = &instr[start..];
        let mut hide = vec![false; shown.len()];
        let mut i = 0;
        while i < shown.len() {
            if shown[i].0 == '\\' && shown.get(i + 1).is_some_and(|(c, _)| *c == '*') {
                let mut j = i + 2;
                while j < shown.len() && shown[j].0.is_whitespace() {
                    j += 1;
                }
                while j < shown.len() && !shown[j].0.is_whitespace() {
                    j += 1;
                }
                hide[i..j].iter_mut().for_each(|h| *h = true);
                i = j;
            } else {
                i += 1;
            }
        }
        for ((c, at), hidden) in shown.iter().zip(hide) {
            if !hidden {
                self.text(runs, *at, c.encode_utf8(&mut [0; 4]), chp);
            }
        }
        let end = instr.last().map(|(_, at)| at + 1).unwrap_or(*first);
        runs.push(Run {
            cp: end,
            chp: chp.clone(),
            content: Content::FieldEnd,
        });
    }

    /// A FORMDROPDOWN field keeps no result: Word shows the entry its form
    /// data selects.
    fn drop_down(&mut self, runs: &mut Vec<Run>, instr: &[(char, u32)], chp: &Chp) {
        let name: String = instr.iter().map(|(c, _)| *c).collect();
        if !name
            .split_whitespace()
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("FORMDROPDOWN"))
        {
            return;
        }
        let Some(entry) = self
            .form_data
            .take()
            .and_then(|at| drop_down_entry(self.doc.data_stream(), at as usize))
        else {
            return;
        };
        runs.push(Run {
            cp: instr.last().map_or(0, |(_, at)| *at),
            chp: chp.clone(),
            content: Content::Choice(entry),
        });
    }

    fn notes_ref(&mut self, at: u32) -> String {
        let table = self.doc.table_stream();
        let dop = self.doc.fib.table(table, lcb::DOP);
        // The document's formats: two 16-bit fields near the end of a Word 97
        // DOP, 4-bit fields beside the endnote position in older ones.
        let (ftn, edn) = match (u16_at(dop, 492), u16_at(dop, 494)) {
            (Some(f), Some(e)) => (f as u8, e as u8),
            _ => {
                let w = u16_at(dop, 0x36).unwrap_or(0);
                (((w >> 2) & 0xF) as u8, ((w >> 6) & 0xF) as u8)
            }
        };
        let sep = self
            .doc
            .sections
            .iter()
            .find(|(r, _)| r.contains(&at))
            .map(|(_, s)| s);
        let formats = [
            (
                lcb::PLCFFNDREF,
                sep.and_then(|s| s.footnote_format).unwrap_or(ftn),
            ),
            (
                lcb::PLCFENDREF,
                sep.and_then(|s| s.endnote_format).unwrap_or(edn),
            ),
        ];
        for (index, nfc) in formats {
            let plc = Plc::new(self.doc.fib.table(table, index), 2);
            if let Some(i) = (0..plc.n).find(|i| plc.pos(*i) == at) {
                // A note with a mark of its own takes no number.
                let auto = (0..=i)
                    .filter(|j| i16_at(plc.data(*j), 0).is_some_and(|n| n > 0))
                    .count();
                return lists::format_number(auto.max(1) as i32, nfc);
            }
        }
        self.notes.to_string()
    }
}

/// Paragraphs at `depth` and deeper tables, from `paras[*i]` until a
/// shallower paragraph or, inside a table, the end of the current cell.
fn build(paras: &mut [Para], i: &mut usize, depth: i32) -> Vec<Block> {
    let mut out = Vec::new();
    while *i < paras.len() {
        let p = &paras[*i];
        if p.depth > depth {
            out.push(Block::Table(table(paras, i, depth + 1)));
            continue;
        }
        if p.depth < depth || (depth > 0 && p.row_end) {
            break;
        }
        let cell_end = p.cell_end;
        out.push(Block::Paragraph(Box::new(paras[*i].paragraph.take())));
        *i += 1;
        if depth > 0 && cell_end {
            break;
        }
    }
    out
}

fn table(paras: &mut [Para], i: &mut usize, depth: i32) -> Table {
    let mut rows = Vec::new();
    while *i < paras.len() && paras[*i].depth >= depth {
        let mut cells = Vec::new();
        let mut tap = Tap::default();
        while let Some(p) = paras.get(*i) {
            if p.depth < depth {
                break;
            }
            if p.depth == depth && p.row_end {
                tap = p.tap.clone().unwrap_or_default();
                *i += 1;
                break;
            }
            let before = *i;
            cells.push(build(paras, i, depth));
            if *i == before {
                *i += 1;
            }
        }
        rows.push(Row { tap, cells });
    }
    Table { depth, rows }
}

/// A header or footer story ends on a paragraph mark of its own, which Word
/// does not draw: the empty paragraph it closes goes.
fn drop_guard(blocks: &mut Vec<Block>) {
    if blocks.len() > 1
        && matches!(blocks.last(), Some(Block::Paragraph(p)) if p.runs.is_empty() && p.label.is_none())
    {
        blocks.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::drop_down_entry;

    #[test]
    fn a_drop_down_shows_the_entry_its_result_selects_else_its_first() {
        let form = |bits: u8| {
            let mut data = vec![0u8; 4];
            data.extend_from_slice(&8u16.to_le_bytes());
            data.extend_from_slice(&[0, 0]);
            data.extend_from_slice(&[0xFF; 4]);
            data.extend_from_slice(&[bits, 0x80, 0, 0, 0, 0]);
            data.extend_from_slice(&[0; 4]);
            data.extend_from_slice(&[0; 2]);
            data.extend_from_slice(&[0; 20]);
            data.extend_from_slice(&[0xFF, 0xFF, 2, 0, 0, 0]);
            for entry in ["1000", "2000"] {
                data.extend_from_slice(&(entry.len() as u16).to_le_bytes());
                data.extend(entry.encode_utf16().flat_map(u16::to_le_bytes));
            }
            data
        };
        assert_eq!(drop_down_entry(&form(0x06), 0).as_deref(), Some("2000"));
        assert_eq!(drop_down_entry(&form(0x02), 0).as_deref(), Some("1000"));
        assert_eq!(drop_down_entry(&form(0x16), 0).as_deref(), Some("1000"));
        assert_eq!(drop_down_entry(&form(0x05), 0), None);
    }
}
