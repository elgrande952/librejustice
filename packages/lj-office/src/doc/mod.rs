//! Word 97-2003 (`.doc`) and Word 6/95 reader, and its in-place edits.
//!
//! Parts of the FIB, piece-table and SPRM handling follow the reading of
//! office_oxide 0.1.12 (MIT OR Apache-2.0, Copyright (c) Yury Fedoseev);
//! the behaviour where the specification is silent follows LibreOffice's
//! `sw/source/filter/ww8`.

pub mod bytes;
pub mod cfb;
pub mod codepage;
pub mod edit;
pub mod fib;
pub mod fkp;
pub mod lists;
pub mod model;
pub mod office;
pub mod package;
pub mod pictures;
pub mod props;
pub mod reader;
pub mod shapes;
pub mod sprm;
pub mod sttb;
pub mod styles;
pub mod summary;
pub mod text;
pub mod values;
pub mod word6;

use std::borrow::Cow;
use std::ops::Range;

use anyhow::{bail, Result};

use bytes::{u16_at, u32_at, Plc};
use cfb::Cfb;
use codepage::Codepage;
use fib::{lcb, Fib, Generation};
use fkp::FcRun;
use lists::Lists;
use props::{Chp, Pap, Sep, Tap};
use sprm::{group, prm0_sprm, Group};
use sttb::Font;
use styles::Styles;
use text::Text;

pub use text::Story;

/// Where one character of a `.doc` lives: its story and its CP. The CP is
/// global (stories follow each other in CP order), so it alone locates the
/// bytes; the story is what a caller shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocChar {
    pub story: Story,
    pub cp: u32,
}

/// A parsed document: the container, and the tables the reader made of it.
pub struct Doc {
    pub cfb: Cfb,
    pub fib: Fib,
    pub text: Text,
    pub codepage: Codepage,
    pub chpx: Vec<FcRun>,
    pub papx: Vec<FcRun>,
    pub styles: Styles,
    pub fonts: Vec<Font>,
    pub lists: Lists,
    /// Main-story CP range and properties of each section.
    pub sections: Vec<(Range<u32>, Sep)>,
    table_name: &'static str,
}

impl Doc {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let cfb = Cfb::parse(bytes)?;
        let Some(wd) = cfb.stream("WordDocument") else {
            bail!("no WordDocument stream");
        };
        let fib = Fib::parse(wd)?;
        let table_name = match fib.generation {
            Generation::Word6 => "WordDocument",
            Generation::Word97 if fib.table1 => "1Table",
            Generation::Word97 => "0Table",
        };
        let Some(table) = cfb.stream(table_name) else {
            bail!("no {table_name} stream");
        };
        let codepage = match fib.generation {
            Generation::Word97 => Codepage::windows(1252),
            Generation::Word6 => Codepage::for_lid(fib.lid),
        };
        let mut text = Text::read(&fib, wd, table, codepage)?;
        let data = cfb.stream("Data").unwrap_or(&[]);
        let (chpx, papx, styles, fonts, lists) = match fib.generation {
            Generation::Word97 => (
                fkp::chpx(wd, fib.table(table, lcb::PLCFBTECHPX)),
                fkp::papx(wd, data, fib.table(table, lcb::PLCFBTEPAPX)),
                Styles::parse(fib.table(table, lcb::STSHF)),
                sttb::fonts(fib.table(table, lcb::STTBFFFN)),
                Lists::parse(table, fib.pair(lcb::PLFLST), fib.pair(lcb::PLFLFO)),
            ),
            Generation::Word6 => (
                word6::chpx(wd, fib.table(table, lcb::PLCFBTECHPX)),
                word6::papx(wd, fib.table(table, lcb::PLCFBTEPAPX)),
                word6::styles(fib.table(table, lcb::STSHF)),
                word6::fonts(fib.table(table, lcb::STTBFFFN)),
                Lists::default(),
            ),
        };
        if fib.generation == Generation::Word6 {
            text.grpprls = text.grpprls.iter().map(|g| word6::translate(g)).collect();
            if !fib.ext_char {
                word6::recode(
                    &mut text,
                    wd,
                    &chpx,
                    &papx,
                    &styles,
                    &fonts,
                    Codepage::number_for_lid(fib.lid),
                );
            }
        }
        let mut sections = Vec::new();
        let plc = Plc::new(fib.table(table, lcb::PLCFSED), 12);
        for i in 0..plc.n {
            let range = plc.pos(i)..plc.pos(i + 1);
            let mut sep = Sep::default();
            let fc = u32_at(plc.data(i), 2).unwrap_or(u32::MAX);
            if fc != u32::MAX {
                let cb = u16_at(wd, fc as usize).unwrap_or(0) as usize;
                let at = (fc as usize).saturating_add(2);
                if let Some(grpprl) = wd.get(at..at.saturating_add(cb)) {
                    match fib.generation {
                        Generation::Word97 => sep.apply(grpprl),
                        Generation::Word6 => sep.apply(&word6::translate(grpprl)),
                    }
                }
            }
            sections.push((range, sep));
        }
        if sections.is_empty() {
            sections.push((0..fib.ccp[0], Sep::default()));
        }
        Ok(Self {
            cfb,
            fib,
            text,
            codepage,
            chpx,
            papx,
            styles,
            fonts,
            lists,
            sections,
            table_name,
        })
    }

    pub fn word_document(&self) -> &[u8] {
        self.cfb.stream("WordDocument").unwrap_or(&[])
    }

    pub fn table_stream(&self) -> &[u8] {
        self.cfb.stream(self.table_name).unwrap_or(&[])
    }

    pub fn data_stream(&self) -> &[u8] {
        self.cfb.stream("Data").unwrap_or(&[])
    }

    /// The sprms a piece's PRM contributes, as a grpprl.
    fn prm(&self, prm: u16) -> Cow<'_, [u8]> {
        if prm & 1 == 1 {
            return Cow::Borrowed(
                self.text
                    .grpprls
                    .get((prm >> 1) as usize)
                    .map(|g| g.as_slice())
                    .unwrap_or(&[]),
            );
        }
        let isprm = ((prm >> 1) & 0x7F) as u8;
        let val = (prm >> 8) as u8;
        if self.fib.generation == Generation::Word6 {
            return Cow::Owned(word6::translate(&[isprm, val]));
        }
        match prm0_sprm(isprm) {
            Some(sprm) if !(isprm == 0 && val == 0) => {
                let [lo, hi] = sprm.to_le_bytes();
                Cow::Owned(vec![lo, hi, val])
            }
            _ => Cow::Borrowed(&[]),
        }
    }

    fn prm_of(&self, cp: u32, want: Group) -> Vec<u8> {
        let Some(piece) = self.text.piece(cp) else {
            return Vec::new();
        };
        let grpprl = self.prm(piece.prm);
        let mut out = Vec::new();
        let mut at = 0;
        for (sprm, op) in sprm::sprms(&grpprl) {
            let len = 2 + op.len();
            if group(sprm) == want || (want == Group::Paragraph && group(sprm) == Group::Table) {
                out.extend_from_slice(&grpprl[at..at + len]);
            }
            at += len;
        }
        out
    }

    /// Properties of the paragraph whose end mark is at `end`: the paragraph,
    /// the character properties its style gives, and the row's table
    /// properties when the mark ends a row.
    pub fn paragraph_props(&self, end: u32) -> (Pap, Chp, Option<Tap>) {
        let run = self
            .text
            .bytes_of(end)
            .and_then(|b| fkp::find(&self.papx, b.start as u32))
            .map(|i| &self.papx[i]);
        let istd = run.map(|r| r.istd).unwrap_or(0);
        let prm = self.prm_of(end, Group::Paragraph);
        let read = |level: Option<&[u8]>| {
            let (mut pap, chp) = self.styles.paragraph(istd);
            pap.istd = istd;
            if let Some(papx) = level {
                pap.apply(papx);
            }
            if let Some(run) = run {
                pap.apply(&run.grpprl);
            }
            pap.apply(&prm);
            (pap, chp)
        };
        let (mut pap, mut chp) = read(None);
        // A list level's paragraph properties come between the style's and the paragraph's own.
        if let Some((_, level)) = self.lists.level(pap.ilfo, pap.ilvl) {
            (pap, chp) = read(Some(&level.papx));
        }
        let tap = (pap.row_end || pap.inner_row_end).then(|| {
            let mut tap = Tap::default();
            if let Some(run) = run {
                tap.apply(&run.grpprl);
            }
            tap.apply(&prm);
            tap
        });
        (pap, chp, tap)
    }

    /// The CHPX run and piece a CP falls in, which together decide its
    /// character properties.
    pub fn char_key(&self, cp: u32) -> (Option<usize>, usize) {
        let piece = self.text.pieces.partition_point(|p| p.cp_end <= cp);
        let run = self
            .text
            .bytes_of(cp)
            .and_then(|b| fkp::find(&self.chpx, b.start as u32));
        (run, piece)
    }

    pub fn char_props(&self, cp: u32, style: &Chp) -> Chp {
        self.char_props_on(cp, style.clone(), style)
    }

    /// The character properties at `cp`, its direct formatting applied over
    /// `under` rather than over the paragraph style's `style`.
    pub fn char_props_on(&self, cp: u32, under: Chp, style: &Chp) -> Chp {
        let mut chp = under;
        let (run, _) = self.char_key(cp);
        let char_style = |istd: u16, under: &Chp| self.styles.character(istd, under);
        if let Some(run) = run {
            chp.apply(&self.chpx[run].grpprl, style, &char_style);
        }
        chp.apply(&self.prm_of(cp, Group::Character), style, &char_style);
        chp
    }
}
