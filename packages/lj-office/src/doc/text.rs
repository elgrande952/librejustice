//! The piece table: which bytes of the WordDocument stream hold each
//! character position (CP), and in which encoding.

use std::ops::Range;

use anyhow::{bail, Result};

use super::bytes::{u16_at, u32_at};
use super::codepage::Codepage;
use super::fib::{lcb, Fib, Generation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub cp: u32,
    pub cp_end: u32,
    /// Byte offset of the piece's first character in the WordDocument stream.
    pub fc: u32,
    /// One byte per character, else UTF-16LE.
    pub compressed: bool,
    /// Property modifier applied to every character of the piece.
    pub prm: u16,
}

impl Piece {
    pub fn width(&self) -> u32 {
        if self.compressed {
            1
        } else {
            2
        }
    }

    /// Where `cp` is stored in the WordDocument stream, if the offset has an address.
    fn byte_at(&self, cp: u32) -> Option<usize> {
        let at = u64::from(self.fc) + u64::from(cp - self.cp) * u64::from(self.width());
        usize::try_from(at).ok()
    }
}

/// The subdocuments a `.doc` concatenates in CP order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Story {
    #[default]
    Main,
    Footnotes,
    Headers,
    Macro,
    Comments,
    Endnotes,
    Textboxes,
    HeaderTextboxes,
}

impl Story {
    pub const ALL: [Story; 8] = [
        Story::Main,
        Story::Footnotes,
        Story::Headers,
        Story::Macro,
        Story::Comments,
        Story::Endnotes,
        Story::Textboxes,
        Story::HeaderTextboxes,
    ];

    /// Position in CP order, which also numbers the story in an anchor.
    pub fn index(self) -> u8 {
        self as u8
    }
}

pub struct Text {
    pub pieces: Vec<Piece>,
    /// One UTF-16 unit per CP, over every story.
    pub chars: Vec<u16>,
    /// The grpprls of the CLX, which a complex PRM indexes.
    pub grpprls: Vec<Vec<u8>>,
    pub stories: Vec<(Story, Range<u32>)>,
}

impl Text {
    pub fn read(fib: &Fib, wd: &[u8], table: &[u8], codepage: Codepage) -> Result<Self> {
        let all = fib.ccp_all();
        let (mut pieces, grpprls) = match (fib.generation, fib.complex) {
            (Generation::Word6, false) => (
                vec![Piece {
                    cp: 0,
                    cp_end: all,
                    fc: fib.fc_min,
                    compressed: !fib.ext_char,
                    prm: 0,
                }],
                Vec::new(),
            ),
            (Generation::Word6, true) => parse_clx(fib.table(wd, lcb::CLX), fib)?,
            (Generation::Word97, _) => parse_clx(fib.table(table, lcb::CLX), fib)?,
        };
        pieces.retain(|p| p.cp < p.cp_end);
        let end = pieces
            .last()
            .map(|p| p.cp_end)
            .unwrap_or(0)
            .min(all.max(fib.ccp[0]))
            .min(wd.len() as u32);
        let mut chars = vec![0u16; end as usize];
        for p in &pieces {
            for cp in p.cp..p.cp_end.min(end) {
                let at = p.byte_at(cp).unwrap_or(usize::MAX);
                chars[cp as usize] = if p.compressed {
                    match wd.get(at) {
                        Some(b) => codepage.decode(*b),
                        None => 0,
                    }
                } else {
                    u16_at(wd, at).unwrap_or(0)
                };
            }
        }
        let mut stories = Vec::new();
        let mut cp = 0u32;
        for (story, len) in Story::ALL.iter().zip(fib.ccp) {
            let stop = cp.saturating_add(len).min(end);
            if len > 0 && cp < stop {
                stories.push((*story, cp..stop));
            }
            cp = cp.saturating_add(len);
        }
        Ok(Self {
            pieces,
            chars,
            grpprls,
            stories,
        })
    }

    pub fn story(&self, story: Story) -> Option<Range<u32>> {
        self.stories
            .iter()
            .find(|(s, _)| *s == story)
            .map(|(_, r)| r.clone())
    }

    /// The piece holding `cp`.
    pub fn piece(&self, cp: u32) -> Option<&Piece> {
        let i = self.pieces.partition_point(|p| p.cp_end <= cp);
        self.pieces.get(i).filter(|p| p.cp <= cp)
    }

    /// Byte range of `cp` in the WordDocument stream.
    pub fn bytes_of(&self, cp: u32) -> Option<Range<usize>> {
        let p = self.piece(cp)?;
        let at = p.byte_at(cp)?;
        Some(at..at.checked_add(p.width() as usize)?)
    }

    /// The CP of the character stored at byte `fc`, for the formatting tables,
    /// which are keyed by stream offset.
    pub fn cp_ranges_of(&self, fc: Range<u32>) -> impl Iterator<Item = Range<u32>> + '_ {
        self.pieces.iter().filter_map(move |p| {
            let start = p.fc;
            let stop = p.fc + (p.cp_end - p.cp) * p.width();
            let lo = fc.start.max(start);
            let hi = fc.end.min(stop);
            (lo < hi).then(|| {
                let w = p.width();
                p.cp + (lo - start) / w..p.cp + (hi - start).div_ceil(w)
            })
        })
    }
}

fn parse_clx(clx: &[u8], fib: &Fib) -> Result<(Vec<Piece>, Vec<Vec<u8>>)> {
    let mut grpprls = Vec::new();
    let mut at = 0usize;
    let plc = loop {
        match clx.get(at) {
            Some(1) => {
                let cb = u16_at(clx, at + 1).unwrap_or(0) as usize;
                let stop = (at + 3 + cb).min(clx.len());
                grpprls.push(clx[at + 3..stop].to_vec());
                at = stop;
            }
            Some(2) => {
                let len = u32_at(clx, at + 1).unwrap_or(0) as usize;
                let stop = (at + 5).saturating_add(len).min(clx.len());
                break &clx[(at + 5).min(stop)..stop];
            }
            Some(other) => bail!("CLX entry of type {other} at {at}"),
            None => bail!("CLX without a piece table"),
        }
    };
    if plc.len() < 16 {
        bail!("piece table of {} bytes", plc.len());
    }
    let n = (plc.len() - 4) / 12;
    let mut pieces = Vec::with_capacity(n);
    for i in 0..n {
        let cp = u32_at(plc, 4 * i).unwrap_or(0);
        let cp_end = u32_at(plc, 4 * (i + 1)).unwrap_or(0);
        let pcd = 4 * (n + 1) + 8 * i;
        let raw = u32_at(plc, pcd + 2).unwrap_or(0);
        let prm = u16_at(plc, pcd + 6).unwrap_or(0);
        let flagged = raw & 0x4000_0000 != 0;
        let (fc, compressed) = match fib.generation {
            Generation::Word97 if flagged => ((raw & 0x3FFF_FFFF) / 2, true),
            Generation::Word97 => (raw, false),
            Generation::Word6 if flagged => ((raw & 0x3FFF_FFFF) / 2, !fib.ext_char),
            Generation::Word6 => (raw, !fib.ext_char),
        };
        if let Some(last) = pieces.last() {
            let last: &Piece = last;
            if cp < last.cp_end {
                bail!("piece table out of order at piece {i}");
            }
        }
        pieces.push(Piece {
            cp,
            cp_end,
            fc,
            compressed,
            prm,
        });
    }
    Ok((pieces, grpprls))
}
