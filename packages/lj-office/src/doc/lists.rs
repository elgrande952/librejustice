//! Lists: definitions (PlfLst and their LVLs), per-paragraph overrides
//! (PlfLfo), and the counters that turn them into labels.

use std::collections::HashMap;

use super::bytes::{i32_at, u16_at, u32_at};

#[derive(Clone, Debug, Default)]
pub struct Level {
    pub start: i32,
    /// Number format: 0 decimal, 1 and 2 upper and lower roman, 3 and 4
    /// upper and lower letter, 22 zero-padded decimal, 23 bullet, 255 none.
    pub nfc: u8,
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    pub legal: bool,
    pub no_restart: bool,
    /// The number text, level placeholders as the units 0 to 8.
    pub text: Vec<u16>,
    /// 0 tab, 1 space, 2 nothing.
    pub follow: u8,
    pub restart_limit: u8,
    pub papx: Vec<u8>,
    pub chpx: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct List {
    pub lsid: i32,
    pub levels: Vec<Level>,
}

#[derive(Clone, Debug, Default)]
pub struct Override {
    pub lsid: i32,
    /// Per level: a new start value, and possibly a whole new level.
    pub levels: Vec<(u8, Option<i32>, Option<Level>)>,
}

#[derive(Default)]
pub struct Lists {
    pub lists: Vec<List>,
    pub overrides: Vec<Override>,
}

fn level(data: &[u8], at: &mut usize) -> Option<Level> {
    let base = *at;
    let lvlf = data.get(base..base + 28)?;
    let flags = lvlf[5];
    let cb_chpx = lvlf[24] as usize;
    let cb_papx = lvlf[25] as usize;
    let mut p = base + 28;
    let papx = data.get(p..p + cb_papx)?.to_vec();
    p += cb_papx;
    let chpx = data.get(p..p + cb_chpx)?.to_vec();
    p += cb_chpx;
    let cch = u16_at(data, p)? as usize;
    let text: Vec<u16> = (0..cch)
        .filter_map(|i| u16_at(data, p + 2 + 2 * i))
        .collect();
    p += 2 + 2 * cch;
    *at = p;
    Some(Level {
        start: i32::from_le_bytes([lvlf[0], lvlf[1], lvlf[2], lvlf[3]]),
        nfc: lvlf[4],
        align: flags & 3,
        legal: flags & 4 != 0,
        no_restart: flags & 8 != 0,
        text,
        follow: lvlf[15],
        restart_limit: lvlf[26],
        papx,
        chpx,
    })
}

impl Lists {
    /// `table` is the whole table stream: the LVLs follow the PlfLst there,
    /// past the length the FIB gives it.
    pub fn parse(table: &[u8], plf_lst: (u32, u32), plf_lfo: (u32, u32)) -> Self {
        let mut out = Lists::default();
        if plf_lst.1 > 0 {
            let base = plf_lst.0 as usize;
            let count = u16_at(table, base).unwrap_or(0) as usize;
            let mut lvl_at = base + 2 + 28 * count;
            for i in 0..count {
                let at = base + 2 + 28 * i;
                let Some(lsid) = i32_at(table, at) else { break };
                let simple = table.get(at + 26).copied().unwrap_or(0) & 1 != 0;
                let n = if simple { 1 } else { 9 };
                let mut levels = Vec::new();
                for _ in 0..n {
                    match level(table, &mut lvl_at) {
                        Some(l) => levels.push(l),
                        None => break,
                    }
                }
                out.lists.push(List { lsid, levels });
            }
        }
        if plf_lfo.1 > 0 {
            let base = plf_lfo.0 as usize;
            let count = (i32_at(table, base).unwrap_or(0).max(0) as usize).min(0x7FFF);
            let mut data_at = base + 4 + 16 * count;
            for i in 0..count {
                let at = base + 4 + 16 * i;
                let Some(lsid) = i32_at(table, at) else { break };
                let clfolvl = table.get(at + 12).copied().unwrap_or(0) as usize;
                let mut o = Override {
                    lsid,
                    levels: Vec::new(),
                };
                if clfolvl > 0 {
                    data_at += 4;
                    for _ in 0..clfolvl.min(9) {
                        let start = i32_at(table, data_at);
                        let flags = u32_at(table, data_at + 4).unwrap_or(0);
                        data_at += 8;
                        let ilvl = (flags & 0xF) as u8;
                        let lvl = if flags & 0x20 != 0 {
                            level(table, &mut data_at)
                        } else {
                            None
                        };
                        o.levels
                            .push((ilvl, start.filter(|_| flags & 0x10 != 0), lvl));
                    }
                } else if u32_at(table, data_at) == Some(0xFFFF_FFFF)
                    || u32_at(table, data_at) == Some(0)
                {
                    data_at += 4;
                }
                out.overrides.push(o);
            }
        }
        out
    }

    /// The level a paragraph with `ilfo` and `ilvl` uses, and the list it
    /// counts in.
    pub fn level(&self, ilfo: u16, ilvl: u8) -> Option<(i32, &Level)> {
        let o = self.overrides.get((ilfo as usize).checked_sub(1)?)?;
        if let Some((_, _, Some(l))) = o.levels.iter().find(|(i, _, l)| *i == ilvl && l.is_some()) {
            return Some((o.lsid, l));
        }
        let list = self.lists.iter().find(|l| l.lsid == o.lsid)?;
        let l = list
            .levels
            .get(ilvl as usize)
            .or_else(|| list.levels.first())?;
        Some((o.lsid, l))
    }
}

/// Counters over one story, fed paragraph by paragraph in reading order.
#[derive(Default)]
pub struct Numbering {
    counters: HashMap<i32, [Option<i32>; 9]>,
    started: HashMap<u16, bool>,
}

impl Numbering {
    /// The label of the next paragraph numbered by `ilfo` at `ilvl`, the
    /// follow character included.
    pub fn next(&mut self, lists: &Lists, ilfo: u16, ilvl: u8) -> Option<String> {
        let ilvl = ilvl.min(8);
        let (lsid, lvl) = lists.level(ilfo, ilvl)?;
        let override_start = lists
            .overrides
            .get(ilfo as usize - 1)
            .and_then(|o| o.levels.iter().find(|(i, s, _)| *i == ilvl && s.is_some()))
            .and_then(|(_, s, _)| *s);
        let first_use = !self.started.contains_key(&ilfo);
        self.started.insert(ilfo, true);
        let counters = self.counters.entry(lsid).or_insert([None; 9]);
        let start_of = |l: u8| lists.level(ilfo, l).map(|(_, v)| v.start).unwrap_or(1);
        let value = match (counters[ilvl as usize], override_start) {
            (_, Some(s)) if first_use => s,
            (Some(v), _) => v.saturating_add(1),
            (None, _) => lvl.start,
        };
        counters[ilvl as usize] = Some(value);
        for (deeper, counter) in counters.iter_mut().enumerate().skip(ilvl as usize + 1) {
            let restart = lists
                .level(ilfo, deeper as u8)
                .map(|(_, l)| !l.no_restart)
                .unwrap_or(true);
            if restart {
                *counter = None;
            }
        }
        let mut label = String::new();
        for unit in &lvl.text {
            if *unit < 9 {
                let l = *unit as u8;
                let v = counters[l as usize].unwrap_or_else(|| start_of(l));
                let nfc = if lvl.legal && l != ilvl {
                    0
                } else {
                    lists.level(ilfo, l).map(|(_, x)| x.nfc).unwrap_or(0)
                };
                label.push_str(&format_number(v, nfc));
            } else {
                label.push(char::from_u32(*unit as u32).unwrap_or('\u{FFFD}'));
            }
        }
        match lvl.follow {
            0 => label.push('\t'),
            1 => label.push(' '),
            _ => {}
        }
        (lvl.nfc != 255 || !label.trim().is_empty()).then_some(label)
    }
}

/// A bullet drawn from a symbol font, as the Unicode character it shows.
pub fn bullet(unit: u16) -> char {
    match unit {
        0xF0B7 | 0xF09F | 0xF0A8 => '\u{2022}',
        0xF0A7 => '\u{25AA}',
        0xF0D8 => '\u{27A2}',
        0xF0FC => '\u{2714}',
        0xF076 => '\u{2756}',
        0xF06E => '\u{25A0}',
        0xF071 => '\u{25A1}',
        0xF0E0 => '\u{27A2}',
        0xF02D => '\u{2013}',
        _ => char::from_u32(unit as u32).unwrap_or('\u{FFFD}'),
    }
}

pub fn format_number(v: i32, nfc: u8) -> String {
    // Letters, roman and Chicago marks grow with the value; LibreOffice keeps
    // a list start in 16 bits.
    let v = match nfc {
        1..=4 | 9 => v.min(i32::from(u16::MAX)),
        _ => v,
    };
    match nfc {
        1 => roman(v).to_uppercase(),
        2 => roman(v),
        3 => letters(v).to_uppercase(),
        4 => letters(v),
        5 => format!("{v}."),
        9 if v > 0 => {
            let mark = ['*', '\u{2020}', '\u{2021}', '\u{a7}'][(v as usize - 1) % 4];
            std::iter::repeat_n(mark, (v as usize - 1) / 4 + 1).collect()
        }
        22 => format!("{v:02}"),
        23 | 255 => String::new(),
        _ => v.to_string(),
    }
}

fn roman(mut v: i32) -> String {
    if v <= 0 {
        return v.to_string();
    }
    let mut out = String::new();
    for (n, s) in [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ] {
        while v >= n {
            out.push_str(s);
            v -= n;
        }
    }
    out
}

/// Word's letter numbering: a to z, then aa to zz, and so on.
fn letters(v: i32) -> String {
    if v <= 0 {
        return v.to_string();
    }
    let v = v - 1;
    let c = (b'a' + (v % 26) as u8) as char;
    std::iter::repeat_n(c, (v / 26 + 1) as usize).collect()
}
