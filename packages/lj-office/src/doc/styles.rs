//! The stylesheet (STSH), each style resolved through its `istdBase` chain.

use super::bytes::{slice, u16_at, utf16};
use super::props::{Chp, Pap, Tap};

#[derive(Clone, Debug, Default)]
pub struct Style {
    pub name: String,
    /// 1 paragraph, 2 character, 3 table, 4 numbering.
    pub kind: u8,
    pub sti: u16,
    pub base: u16,
    pub next: u16,
    papx: Vec<u8>,
    chpx: Vec<u8>,
    tapx: Vec<u8>,
}

#[derive(Default)]
pub struct Styles {
    pub styles: Vec<Option<Style>>,
    /// Default fonts (ASCII, East Asian, other) the stylesheet declares.
    pub fonts: [u16; 3],
    paps: Vec<Pap>,
    chps: Vec<Chp>,
    taps: Vec<Tap>,
}

const NONE: u16 = 0x0FFF;

impl Style {
    pub fn new(
        name: String,
        kind: u8,
        sti: u16,
        base: u16,
        next: u16,
        papx: Vec<u8>,
        chpx: Vec<u8>,
    ) -> Self {
        Self {
            name,
            kind,
            sti,
            base,
            next,
            papx,
            chpx,
            tapx: Vec::new(),
        }
    }
}

impl Styles {
    pub fn from_stds(styles: Vec<Option<Style>>, fonts: [u16; 3]) -> Self {
        Styles {
            styles,
            fonts,
            ..Styles::default()
        }
        .resolved()
    }

    pub fn parse(stsh: &[u8]) -> Self {
        let mut out = Styles::default();
        let Some(cb_stshi) = u16_at(stsh, 0) else {
            return out.resolved();
        };
        let stshi = slice(stsh, 2, cb_stshi as usize).unwrap_or(&[]);
        let cstd = u16_at(stshi, 0).unwrap_or(0) as usize;
        let cb_base = u16_at(stshi, 2).unwrap_or(10) as usize;
        for (i, f) in out.fonts.iter_mut().enumerate() {
            *f = u16_at(stshi, 12 + 2 * i).unwrap_or(0);
        }
        let mut at = 2 + cb_stshi as usize;
        for _ in 0..cstd {
            let Some(cb) = u16_at(stsh, at) else { break };
            let std = slice(stsh, at + 2, cb as usize).unwrap_or(&[]);
            at += 2 + cb as usize;
            out.styles.push(parse_std(std, cb_base));
        }
        out.resolved()
    }

    fn resolved(mut self) -> Self {
        let n = self.styles.len();
        let root_chp = Chp {
            fonts: self.fonts,
            ..Chp::default()
        };
        self.paps = vec![Pap::default(); n];
        self.chps = vec![root_chp.clone(); n];
        self.taps = vec![Tap::default(); n];
        let mut done = vec![false; n];
        for i in 0..n {
            self.resolve(i, &mut done, 0, &root_chp);
        }
        self
    }

    fn resolve(&mut self, i: usize, done: &mut [bool], depth: usize, root: &Chp) {
        if done[i] || depth > 16 {
            return;
        }
        done[i] = true;
        let Some(style) = self.styles[i].clone() else {
            return;
        };
        let base = style.base as usize;
        let (mut pap, mut chp, mut tap) =
            if style.base != NONE && base < self.styles.len() && base != i {
                self.resolve(base, done, depth + 1, root);
                (
                    self.paps[base].clone(),
                    self.chps[base].clone(),
                    self.taps[base].clone(),
                )
            } else {
                (base_pap(), root.clone(), Tap::default())
            };
        pap.apply(&style.papx);
        let before = chp.clone();
        chp.apply(&style.chpx, &before, &|_, s| s.clone());
        tap.apply(&style.tapx);
        pap.istd = i as u16;
        chp.istd = if style.kind == 2 { i as u16 } else { 10 };
        self.paps[i] = pap;
        self.chps[i] = chp;
        self.taps[i] = tap;
    }

    pub fn get(&self, istd: u16) -> Option<&Style> {
        self.styles.get(istd as usize).and_then(|s| s.as_ref())
    }

    /// The paragraph and character properties a paragraph style gives, the
    /// Normal style's when `istd` names nothing.
    pub fn paragraph(&self, istd: u16) -> (Pap, Chp) {
        let i = match self.get(istd) {
            Some(s) if s.kind == 1 => istd as usize,
            _ => 0,
        };
        match (self.paps.get(i), self.chps.get(i)) {
            (Some(p), Some(c)) => (p.clone(), c.clone()),
            _ => (
                base_pap(),
                Chp {
                    fonts: self.fonts,
                    ..Chp::default()
                },
            ),
        }
    }

    /// `under` with the character style `istd` applied: its whole chain of
    /// character grpprls, each toggle read against `under`.
    pub fn character(&self, istd: u16, under: &Chp) -> Chp {
        let mut chain = Vec::new();
        let mut at = istd;
        while let Some(style) = self.get(at) {
            if style.kind != 2 || chain.len() > 16 || style.sti == 65 {
                break;
            }
            chain.push(at);
            at = style.base;
        }
        let mut chp = under.clone();
        for i in chain.iter().rev() {
            if let Some(style) = self.get(*i) {
                chp.apply(&style.chpx, under, &|_, s| s.clone());
            }
        }
        chp.istd = istd;
        chp
    }
}

fn parse_std(std: &[u8], cb_base: usize) -> Option<Style> {
    if std.len() < 10 {
        return None;
    }
    let w1 = u16_at(std, 0)?;
    let w2 = u16_at(std, 2)?;
    let w3 = u16_at(std, 4)?;
    let kind = (w2 & 0xF) as u8;
    let cupx = (w3 & 0xF) as usize;
    let mut at = cb_base.min(std.len());
    let cch = u16_at(std, at)? as usize;
    let name = utf16(slice(std, at + 2, 2 * cch).unwrap_or(&[]));
    at += 2 + 2 * cch + 2;
    let mut upx: Vec<Vec<u8>> = Vec::new();
    for _ in 0..cupx {
        if at % 2 == 1 {
            at += 1;
        }
        let Some(cb) = u16_at(std, at) else { break };
        upx.push(slice(std, at + 2, cb as usize).unwrap_or(&[]).to_vec());
        at += 2 + cb as usize;
    }
    let take = |i: usize| upx.get(i).cloned().unwrap_or_default();
    let (papx, chpx, tapx) = match kind {
        1 => (
            take(0).get(2..).unwrap_or(&[]).to_vec(),
            take(1),
            Vec::new(),
        ),
        2 => (Vec::new(), take(0), Vec::new()),
        3 => (take(1).get(2..).unwrap_or(&[]).to_vec(), take(2), take(0)),
        4 => (
            take(0).get(2..).unwrap_or(&[]).to_vec(),
            Vec::new(),
            Vec::new(),
        ),
        _ => (Vec::new(), Vec::new(), Vec::new()),
    };
    Some(Style {
        name,
        kind,
        sti: w1 & 0x0FFF,
        base: w2 >> 4,
        next: w3 >> 4,
        papx,
        chpx,
        tapx,
    })
}

/// The paragraph properties under a style based on nothing: Word's widow
/// and orphan control is on unless a style turns it off.
fn base_pap() -> Pap {
    Pap {
        widow: true,
        ..Pap::default()
    }
}
