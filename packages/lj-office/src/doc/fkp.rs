//! Formatted disk pages: the character and paragraph property runs, keyed by
//! byte offset in the WordDocument stream.

use super::bytes::{slice, u16_at, u32_at, Plc};
use super::sprm::{code, sprms};

/// Properties over the stream bytes `fc..fc_end`.
#[derive(Clone, Debug)]
pub struct FcRun {
    pub fc: u32,
    pub fc_end: u32,
    /// Paragraph style; zero for character runs.
    pub istd: u16,
    pub grpprl: Vec<u8>,
}

pub const PAGE: usize = 512;

/// The pages a bin table points to, its page numbers `pn_bytes` wide.
pub fn pages<'a>(wd: &'a [u8], bte: &[u8], pn_bytes: usize) -> Vec<&'a [u8]> {
    let plc = Plc::new(bte, pn_bytes);
    (0..plc.n)
        .filter_map(|i| {
            let pn = match pn_bytes {
                2 => u16_at(plc.data(i), 0)? as u32,
                _ => u32_at(plc.data(i), 0)? & 0x3F_FFFF,
            };
            slice(wd, pn as usize * PAGE, PAGE)
        })
        .collect()
}

pub fn chpx(wd: &[u8], bte: &[u8]) -> Vec<FcRun> {
    chpx_pages(pages(wd, bte, 4))
}

pub fn chpx_pages(pages: Vec<&[u8]>) -> Vec<FcRun> {
    let mut runs = Vec::new();
    for page in pages {
        let crun = page[PAGE - 1] as usize;
        if 4 * (crun + 1) + crun > PAGE - 1 {
            continue;
        }
        for i in 0..crun {
            let (Some(fc), Some(fc_end)) = (u32_at(page, 4 * i), u32_at(page, 4 * (i + 1))) else {
                break;
            };
            let offset = page[4 * (crun + 1) + i] as usize * 2;
            let grpprl = if offset == 0 {
                Vec::new()
            } else {
                let cb = page[offset] as usize;
                page.get(offset + 1..offset + 1 + cb)
                    .unwrap_or(&[])
                    .to_vec()
            };
            runs.push(FcRun {
                fc,
                fc_end,
                istd: 0,
                grpprl,
            });
        }
    }
    runs.sort_by_key(|r| r.fc);
    runs
}

pub fn papx(wd: &[u8], data: &[u8], bte: &[u8]) -> Vec<FcRun> {
    let mut runs = Vec::new();
    for page in pages(wd, bte, 4) {
        let crun = page[PAGE - 1] as usize;
        if 4 * (crun + 1) + 13 * crun > PAGE - 1 {
            continue;
        }
        for i in 0..crun {
            let (Some(fc), Some(fc_end)) = (u32_at(page, 4 * i), u32_at(page, 4 * (i + 1))) else {
                break;
            };
            let offset = page[4 * (crun + 1) + 13 * i] as usize * 2;
            let body = match page.get(offset).copied() {
                None => &[][..],
                Some(0) => {
                    let cb = page.get(offset + 1).copied().unwrap_or(0) as usize;
                    page.get(offset + 2..offset + 2 + 2 * cb).unwrap_or(&[])
                }
                Some(cb) => page
                    .get(offset + 1..offset + 2 * cb as usize)
                    .unwrap_or(&[]),
            };
            let istd = u16_at(body, 0).unwrap_or(0);
            let mut grpprl = body.get(2..).unwrap_or(&[]).to_vec();
            if let Some((code::P_HUGE_PAPX, op)) = sprms(&grpprl).next() {
                if let Some(at) = u32_at(op, 0) {
                    let at = at as usize;
                    let cb = u16_at(data, at).unwrap_or(0) as usize;
                    grpprl = data.get(at + 2..at + 2 + cb).unwrap_or(&[]).to_vec();
                }
            }
            runs.push(FcRun {
                fc,
                fc_end,
                istd,
                grpprl,
            });
        }
    }
    runs.sort_by_key(|r| r.fc);
    runs
}

/// The run covering stream offset `fc`.
pub fn find(runs: &[FcRun], fc: u32) -> Option<usize> {
    let i = runs.partition_point(|r| r.fc <= fc);
    (i > 0 && fc < runs[i - 1].fc_end).then(|| i - 1)
}
