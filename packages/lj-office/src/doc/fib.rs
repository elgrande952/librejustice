//! File Information Block: where every table of the document sits.

use anyhow::{bail, Result};

use super::bytes::{u16_at, u32_at};

/// Indices into FibRgFcLcb97, the (offset, length) pairs into the table stream.
pub mod lcb {
    pub const STSHF: usize = 1;
    pub const PLCFFNDREF: usize = 2;
    pub const PLCFFNDTXT: usize = 3;
    pub const PLCFANDREF: usize = 4;
    pub const PLCFANDTXT: usize = 5;
    pub const PLCFSED: usize = 6;
    pub const PLCFHDD: usize = 11;
    pub const PLCFBTECHPX: usize = 12;
    pub const PLCFBTEPAPX: usize = 13;
    pub const STTBFFFN: usize = 15;
    pub const PLCFFLDMOM: usize = 16;
    pub const STTBFBKMK: usize = 21;
    pub const DOP: usize = 31;
    pub const STTBFASSOC: usize = 32;
    pub const CLX: usize = 33;
    pub const GRPXSTATNOWNERS: usize = 36;
    pub const STTBFATNBKMK: usize = 37;
    pub const PLCSPAMOM: usize = 40;
    pub const PLCSPAHDR: usize = 41;
    pub const PLCFENDREF: usize = 46;
    pub const PLCFENDTXT: usize = 47;
    pub const DGGINFO: usize = 50;
    pub const STTBFRMARK: usize = 51;
    pub const PLCFTXBXTXT: usize = 56;
    pub const PLCFHDRTXBXTXT: usize = 58;
    pub const STWUSER: usize = 60;
    pub const STTBSAVEDBY: usize = 71;
    pub const STTBFNM: usize = 72;
    pub const PLFLST: usize = 73;
    pub const PLFLFO: usize = 74;
    pub const STTBLISTNAMES: usize = 92;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generation {
    /// Word 97 and later, `wIdent` 0xA5EC.
    Word97,
    /// Word 6 and Word 95, Windows or Macintosh: tables in the main stream,
    /// 8-bit text in the document's code page.
    Word6,
}

#[derive(Clone, Debug)]
pub struct Fib {
    pub generation: Generation,
    pub nfib: u16,
    pub lid: u16,
    pub complex: bool,
    pub table1: bool,
    /// Word 6 only: text stored as 16-bit units (Far East builds).
    pub ext_char: bool,
    pub fc_min: u32,
    pub fc_mac: u32,
    /// ccpText, ccpFtn, ccpHdd, ccpMcr, ccpAtn, ccpEdn, ccpTxbx, ccpHdrTxbx.
    pub ccp: [u32; 8],
    fclcb: Vec<(u32, u32)>,
}

impl Fib {
    pub fn parse(wd: &[u8]) -> Result<Self> {
        let Some(wident) = u16_at(wd, 0) else {
            bail!("WordDocument stream is empty");
        };
        let nfib = u16_at(wd, 2).unwrap_or(0);
        let flags = u16_at(wd, 0x0A).unwrap_or(0);
        if flags & 0x0100 != 0 {
            bail!("encrypted document");
        }
        let generation = match wident {
            0xA5EC if nfib >= 0x00C1 => Generation::Word97,
            0xA5EC | 0xA5DC | 0xA697..=0xA699 => Generation::Word6,
            other => bail!("not a Word 6 to 2003 document (wIdent {other:#06x})"),
        };
        let mut ccp = [0u32; 8];
        let fclcb = match generation {
            Generation::Word97 => {
                for (i, c) in ccp.iter_mut().enumerate() {
                    *c = u32_at(wd, 0x4C + 4 * i).unwrap_or(0);
                }
                // ccpMcr is reserved in Word 97 and later.
                ccp[3] = 0;
                let csw = u16_at(wd, 0x20).unwrap_or(14) as usize;
                let at = 0x22 + 2 * csw;
                let cslw = u16_at(wd, at).unwrap_or(22) as usize;
                let at = at + 2 + 4 * cslw;
                let cb = u16_at(wd, at).unwrap_or(0) as usize;
                (0..cb)
                    .map(|i| {
                        let o = at + 2 + 8 * i;
                        (u32_at(wd, o).unwrap_or(0), u32_at(wd, o + 4).unwrap_or(0))
                    })
                    .collect()
            }
            Generation::Word6 => {
                if wd.len() < 0x168 {
                    bail!("Word 6 FIB truncated");
                }
                for (i, c) in ccp.iter_mut().enumerate() {
                    *c = u32_at(wd, 0x34 + 4 * i).unwrap_or(0);
                }
                // Word 6 keeps the same pairs from StshfOrig onwards at 0x58,
                // except the CLX, which sits at 0x160.
                let mut v: Vec<(u32, u32)> = (0..34)
                    .map(|i| {
                        let o = 0x58 + 8 * i;
                        (u32_at(wd, o).unwrap_or(0), u32_at(wd, o + 4).unwrap_or(0))
                    })
                    .collect();
                v[lcb::CLX] = (
                    u32_at(wd, 0x160).unwrap_or(0),
                    u32_at(wd, 0x164).unwrap_or(0),
                );
                v
            }
        };
        Ok(Self {
            generation,
            nfib,
            lid: u16_at(wd, 6).unwrap_or(0x0409),
            complex: flags & 0x0004 != 0,
            table1: flags & 0x0200 != 0,
            ext_char: generation == Generation::Word6 && flags & 0x1000 != 0,
            fc_min: u32_at(wd, 0x18).unwrap_or(0),
            fc_mac: u32_at(wd, 0x1C).unwrap_or(0),
            ccp,
            fclcb,
        })
    }

    /// The `(fc, lcb)` pair at `index`, `(0, 0)` when this FIB is too short to
    /// carry it.
    pub fn pair(&self, index: usize) -> (u32, u32) {
        self.fclcb.get(index).copied().unwrap_or((0, 0))
    }

    /// Where pair `index` sits in the WordDocument stream, for a writer that
    /// patches the FIB.
    pub fn pair_offset(&self, wd: &[u8], index: usize) -> Option<usize> {
        if self.generation != Generation::Word97 || index >= self.fclcb.len() {
            return None;
        }
        let csw = u16_at(wd, 0x20)? as usize;
        let at = 0x22 + 2 * csw;
        let cslw = u16_at(wd, at)? as usize;
        Some(at + 2 + 4 * cslw + 2 + 8 * index)
    }

    /// Slice `data` (the table stream, or the main stream for Word 6) at pair
    /// `index`; empty when absent or out of range.
    pub fn table<'a>(&self, data: &'a [u8], index: usize) -> &'a [u8] {
        let (fc, lcb) = self.pair(index);
        let (fc, lcb) = (fc as usize, lcb as usize);
        if lcb == 0 || fc >= data.len() {
            return &[];
        }
        &data[fc..fc.saturating_add(lcb).min(data.len())]
    }

    /// Total characters the piece table covers: every story, and the one
    /// closing paragraph mark Word appends when any story besides the main one
    /// is present.
    pub fn ccp_all(&self) -> u32 {
        let sub: u32 = self.ccp[1..].iter().fold(0u32, |a, c| a.saturating_add(*c));
        self.ccp[0]
            .saturating_add(sub)
            .saturating_add(u32::from(sub > 0))
    }
}
