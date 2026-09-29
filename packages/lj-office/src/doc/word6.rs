//! Word 6 and Word 95. Their sprms are one byte, their strings 8-bit, their
//! stylesheet and font table shorter, and a run's font picks the code page of
//! its text. What differs is read here and handed on in the Word 97 form the
//! rest of the reader knows.

use super::bytes::{slice, u16_at};
use super::codepage::Codepage;
use super::fkp::{self, FcRun};
use super::sprm::{code, sprms};
use super::sttb::Font;
use super::styles::Styles;
use super::text::Text;

enum Len {
    Fixed(usize),
    /// A length byte, then that many bytes.
    Var,
    /// A 16-bit count; the operand is that count plus one byte long.
    Var2,
}

/// Operand length of a Word 6 sprm, and the Word 97 sprm with the same
/// operand when there is one. `None` for a code this reader does not know,
/// which ends the walk: its length is unknown too.
fn sprm6(sprm: u8) -> Option<(Len, Option<u16>)> {
    use Len::*;
    let map = |n: usize, to: u16| Some((Fixed(n), Some(to)));
    let skip = |n: usize| Some((Fixed(n), None));
    match sprm {
        0 => skip(0),
        2 | 4 | 6 | 10 | 11 | 13 => skip(1),
        14 => map(1, code::P_FNO_LINE_NUMB),
        3 | 12 | 52 | 77 | 79 | 81 | 82 | 103 | 105 | 106 | 108 | 113 | 115 | 116 | 120 | 133
        | 191 => Some((Var, None)),
        5 => map(1, code::P_JC80),
        7 => map(1, code::P_KEEP),
        8 => map(1, code::P_KEEP_FOLLOW),
        9 => map(1, code::P_PAGE_BREAK_BEFORE),
        15 => Some((Var, Some(code::P_CHG_TABS_PAPX))),
        16 => map(2, code::P_DXA_RIGHT80),
        17 => map(2, code::P_DXA_LEFT80),
        18 => skip(2),
        19 => map(2, code::P_DXA_LEFT1_80),
        20 => map(4, code::P_DYA_LINE),
        21 => map(2, code::P_DYA_BEFORE),
        22 => map(2, code::P_DYA_AFTER),
        23 => Some((Var, Some(code::P_CHG_TABS))),
        24 => map(1, code::P_IN_TABLE),
        25 => map(1, code::P_TTP),
        26 => map(2, code::P_DXA_ABS),
        27 => map(2, code::P_DYA_ABS),
        28 => map(2, code::P_DXA_WIDTH),
        29 => map(1, code::P_PC),
        30..=36 | 38..=43 => skip(2),
        37 => map(1, code::P_WR),
        44 | 50 | 53..=58 => skip(1),
        45 => map(2, code::P_WHEIGHT_ABS),
        46 => map(2, code::P_DCS),
        47 => map(2, code::P_SHD80),
        48 => map(2, code::P_DYA_FROM_TEXT),
        49 => map(2, code::P_DXA_FROM_TEXT),
        51 => map(1, code::P_WIDOW),
        65 => map(1, code::C_RMARK_DEL),
        66 => map(1, code::C_RMARK_INS),
        67 => map(1, code::C_FLD_VANISH),
        68 => Some((Var, Some(code::C_PIC_LOCATION))),
        69 | 72 | 107 | 109..=112 => skip(2),
        70 => skip(4),
        71 => map(1, code::C_DATA),
        73 => map(3, code::C_CHS),
        74 => Some((Var, Some(code::C_SYMBOL))),
        75 => map(1, code::C_OLE2),
        80 => map(2, code::C_ISTD),
        83 => skip(0),
        85 => map(1, code::C_BOLD),
        86 => map(1, code::C_ITALIC),
        87 => map(1, code::C_STRIKE),
        88 => map(1, code::C_OUTLINE),
        89 => map(1, code::C_SHADOW),
        90 => map(1, code::C_SMALL_CAPS),
        91 => map(1, code::C_CAPS),
        92 => map(1, code::C_VANISH),
        93 => map(2, code::C_FTC0),
        94 => map(1, code::C_KUL),
        95 => skip(3),
        96 => map(2, code::C_DXA_SPACE),
        97 => map(2, code::C_LID_OLD),
        98 => map(1, code::C_ICO),
        99 => map(2, code::C_HPS),
        100 | 102 | 119 => skip(1),
        101 => map(2, code::C_HPS_POS),
        104 => map(1, code::C_ISS),
        117 => map(1, code::C_SPEC),
        118 => map(1, code::C_OBJ),
        121..=124 => skip(2),
        131 => map(1, 0x3000),
        132 => map(1, 0x3001),
        136 => map(3, 0xF203),
        137 => map(3, 0xF204),
        138 | 139 | 146 | 151 | 153 | 163 => skip(1),
        140 | 141 | 171 => skip(2),
        152 => map(1, code::S_LNC),
        154 => map(2, code::S_NLNN_MOD),
        155 => map(2, code::S_DXA_LNN),
        160 => map(2, code::S_LNN_MIN),
        142 => map(1, code::S_BKC),
        143 => map(1, code::S_TITLE_PAGE),
        144 => map(2, code::S_CCOLUMNS),
        145 => map(2, code::S_DXA_COLUMNS),
        147 => map(1, code::S_NFC_PGN),
        148 => map(2, 0xB00F),
        149 => map(2, 0xB010),
        150 => map(1, code::S_PGN_RESTART),
        156 => map(2, code::S_HDR_TOP),
        157 => map(2, code::S_HDR_BOTTOM),
        158 => map(1, code::S_LBETWEEN),
        159 => map(1, code::S_VJC),
        161 => map(2, code::S_PGN_START97),
        162 => map(1, code::S_ORIENTATION),
        164 => map(2, code::S_XA_PAGE),
        165 => map(2, code::S_YA_PAGE),
        166 => map(2, code::S_DXA_LEFT),
        167 => map(2, code::S_DXA_RIGHT),
        168 => map(2, code::S_DYA_TOP),
        169 => map(2, code::S_DYA_BOTTOM),
        170 => map(2, code::S_GUTTER),
        182 => map(2, code::T_JC90),
        183 => map(2, code::T_DXA_LEFT),
        184 => map(2, code::T_DXA_GAP_HALF),
        185 => map(1, code::T_CANT_SPLIT90),
        186 => map(1, code::T_HEADER),
        187 => skip(12),
        188 => Some((Var2, None)),
        189 => map(2, code::T_ROW_HEIGHT),
        190 => Some((Var2, Some(code::T_DEF_TABLE))),
        192 => map(4, 0x740A),
        193 | 199 => skip(5),
        194 => map(4, code::T_INSERT),
        195 => map(2, code::T_DELETE),
        196 => map(4, code::T_DXA_COL),
        197 => map(2, code::T_MERGE),
        198 => map(2, code::T_SPLIT),
        200 => skip(4),
        _ => None,
    }
}

/// A Word 6 grpprl in Word 97 form: the sprms whose meaning carries over,
/// the others dropped.
pub fn translate(grpprl: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((_, op, next, to)) = step(grpprl, at) {
        at = next;
        let Some(to) = to else { continue };
        let op: Vec<u8> = match to {
            code::C_PIC_LOCATION => op.get(1..5).map(<[u8]>::to_vec).unwrap_or_default(),
            code::C_SYMBOL => match op {
                [_, lo, hi, ch, ..] => vec![*lo, *hi, *ch, 0],
                _ => continue,
            },
            code::T_DEF_TABLE => match def_table(op) {
                Some(op) => op,
                None => continue,
            },
            _ => op.to_vec(),
        };
        out.extend_from_slice(&to.to_le_bytes());
        out.extend_from_slice(&op);
    }
    out
}

/// The sprm at `at`: code, operand, next offset, Word 97 equivalent.
fn step(grpprl: &[u8], at: usize) -> Option<(u8, &[u8], usize, Option<u16>)> {
    let sprm = *grpprl.get(at)?;
    let (len, to) = sprm6(sprm)?;
    let (body, next) = match len {
        Len::Fixed(n) => (at + 1..at + 1 + n, at + 1 + n),
        Len::Var => {
            let n = *grpprl.get(at + 1)? as usize;
            (at + 1..at + 2 + n, at + 2 + n)
        }
        Len::Var2 => {
            let n = u16_at(grpprl, at + 1)? as usize;
            (at + 1..at + 2 + n, at + 2 + n)
        }
    };
    Some((sprm, grpprl.get(body)?, next, to))
}

/// A Word 6 grpprl less the sprms that make a character stand for an object,
/// and with `charset` as its character set when set.
pub fn plain(grpprl: &[u8], charset: Option<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((sprm, _, next, _)) = step(grpprl, at) {
        let hard = charset.is_some() && sprm == 73;
        if !hard && !matches!(sprm, 68 | 71 | 74 | 75 | 117 | 118) {
            out.extend_from_slice(&grpprl[at..next]);
        }
        at = next;
    }
    if let Some(chs) = charset {
        out.extend_from_slice(&[73, 1, chs, 0]);
    }
    out
}

/// A Word 6 table definition with its 10-byte cell descriptors widened to the
/// 20-byte Word 97 ones, 16-bit borders to 32-bit.
fn def_table(op: &[u8]) -> Option<Vec<u8>> {
    let n = *op.get(2)? as usize;
    let centers = slice(op, 3, 2 * (n + 1))?;
    let tcs = op.get(3 + 2 * (n + 1)..)?;
    let mut out = vec![0, 0, n as u8];
    out.extend_from_slice(centers);
    for i in 0..n {
        let tc = tcs.get(10 * i..10 * i + 10).unwrap_or(&[0; 10]);
        out.extend_from_slice(&tc[0..2]);
        out.extend_from_slice(&[0, 0]);
        for k in 0..4 {
            let brc = u16_at(tc, 2 + 2 * k).unwrap_or(0);
            let width = (brc & 7) as u8;
            let kind = ((brc >> 3) & 3) as u8;
            let ico = ((brc >> 6) & 0x1F) as u8;
            let space = ((brc >> 11) & 0x1F) as u8;
            let shadow = ((brc >> 5) & 1) as u8;
            out.extend_from_slice(&[width * 6, kind, ico, space | (shadow << 5)]);
        }
    }
    let cb = (out.len() - 1) as u16;
    out[0..2].copy_from_slice(&cb.to_le_bytes());
    Some(out)
}

/// The font table: a byte count, then FFNs whose name is 8-bit.
pub fn fonts(data: &[u8]) -> Vec<Font> {
    let mut out = Vec::new();
    let end = (u16_at(data, 0).unwrap_or(0) as usize).min(data.len());
    let mut at = 2;
    while at < end {
        let cb = data[at] as usize;
        let Some(ffn) = data.get(at + 1..at + 1 + cb) else {
            break;
        };
        at += 1 + cb;
        if ffn.len() < 6 {
            continue;
        }
        let mut names = ffn[5..]
            .split(|b| *b == 0)
            .map(|s| s.iter().map(|b| char::from(*b)).collect::<String>());
        let name = names.next().unwrap_or_default();
        let alt = (ffn[4] > 0)
            .then(|| names.next())
            .flatten()
            .filter(|s| !s.is_empty());
        out.push(Font {
            name,
            alt,
            ffid: ffn[0],
            weight: i16::from_le_bytes([ffn[1], ffn[2]]),
            charset: ffn[3],
        });
    }
    out
}

/// Where the strings of a string table sit: a byte count, then strings each
/// led by its length byte.
pub fn sttb(data: &[u8]) -> Vec<std::ops::Range<usize>> {
    let end = (u16_at(data, 0).unwrap_or(0) as usize).min(data.len());
    let mut out = Vec::new();
    let mut at = 2;
    while at < end {
        let cch = data[at] as usize;
        out.push(at + 1..(at + 1 + cch).min(end));
        at += 1 + cch;
    }
    out
}

/// The stylesheet: 8-bit names, one-byte sprms.
pub fn styles(stsh: &[u8]) -> Styles {
    let Some(cb_stshi) = u16_at(stsh, 0) else {
        return Styles::from_stds(Vec::new(), [0; 3]);
    };
    let stshi = slice(stsh, 2, cb_stshi as usize).unwrap_or(&[]);
    let cstd = u16_at(stshi, 0).unwrap_or(0) as usize;
    let cb_base = u16_at(stshi, 2).unwrap_or(8) as usize;
    let ftc = u16_at(stshi, 12).unwrap_or(0);
    let mut stds = Vec::new();
    let mut at = 2 + cb_stshi as usize;
    for _ in 0..cstd {
        let Some(cb) = u16_at(stsh, at) else { break };
        let std = slice(stsh, at + 2, cb as usize).unwrap_or(&[]);
        at += 2 + cb as usize;
        stds.push(std6(std, cb_base));
    }
    Styles::from_stds(stds, [ftc; 3])
}

fn std6(std: &[u8], cb_base: usize) -> Option<super::styles::Style> {
    let (w1, w2, w3) = (u16_at(std, 0)?, u16_at(std, 2)?, u16_at(std, 4)?);
    let kind = (w2 & 0xF) as u8;
    let mut at = cb_base.min(std.len());
    let cch = *std.get(at)? as usize;
    let name: String = slice(std, at + 1, cch)
        .unwrap_or(&[])
        .iter()
        .map(|b| char::from(*b))
        .collect();
    at += 1 + cch + 1;
    let mut upx: Vec<&[u8]> = Vec::new();
    for _ in 0..(w3 & 0xF) {
        at += at % 2;
        let Some(cb) = u16_at(std, at) else { break };
        upx.push(slice(std, at + 2, cb as usize).unwrap_or(&[]));
        at += 2 + cb as usize;
    }
    let take = |i: usize| upx.get(i).copied().unwrap_or(&[]);
    let (papx, chpx) = match kind {
        1 => (
            translate(take(0).get(2..).unwrap_or(&[])),
            translate(take(1)),
        ),
        2 => (Vec::new(), translate(take(0))),
        _ => (Vec::new(), Vec::new()),
    };
    Some(super::styles::Style::new(
        name,
        kind,
        w1 & 0x0FFF,
        w2 >> 4,
        w3 >> 4,
        papx,
        chpx,
    ))
}

/// Character runs, from FKPs addressed by 16-bit page numbers.
pub fn chpx(wd: &[u8], bte: &[u8]) -> Vec<FcRun> {
    let mut runs = fkp::chpx_pages(fkp::pages(wd, bte, 2));
    for r in &mut runs {
        r.grpprl = translate(&r.grpprl);
    }
    runs
}

/// Paragraph runs: 7-byte BX entries, and a PAPX that is a word count, the
/// style, then the sprms.
pub fn papx(wd: &[u8], bte: &[u8]) -> Vec<FcRun> {
    let mut runs = Vec::new();
    for page in fkp::pages(wd, bte, 2) {
        let crun = page[fkp::PAGE - 1] as usize;
        if 4 * (crun + 1) + 7 * crun > fkp::PAGE - 1 {
            continue;
        }
        for i in 0..crun {
            let (Some(fc), Some(fc_end)) = (
                super::bytes::u32_at(page, 4 * i),
                super::bytes::u32_at(page, 4 * (i + 1)),
            ) else {
                break;
            };
            let offset = page[4 * (crun + 1) + 7 * i] as usize * 2;
            let body = match page.get(offset) {
                Some(&cw) if offset > 0 => page
                    .get(offset + 1..offset + 1 + 2 * cw as usize)
                    .unwrap_or(&[]),
                _ => &[],
            };
            let istd = u16_at(body, 0).unwrap_or(0);
            runs.push(FcRun {
                fc,
                fc_end,
                istd,
                grpprl: translate(body.get(2..).unwrap_or(&[])),
            });
        }
    }
    runs.sort_by_key(|r| r.fc);
    runs
}

/// The code page a Windows character set stands for, for the single-byte
/// sets; `None` for the others, which keep the language's.
pub fn charset_codepage(charset: u8) -> Option<u16> {
    Some(match charset {
        0 => 1252,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        _ => return None,
    })
}

/// The character set of the byte at `fc`: its run's, else its font's.
pub fn charset_at(
    fc: u32,
    chpx: &[FcRun],
    papx: &[FcRun],
    styles: &Styles,
    fonts: &[Font],
) -> Option<u8> {
    let istd = fkp::find(papx, fc).map(|i| papx[i].istd).unwrap_or(0);
    let base = styles.paragraph(istd).1;
    let mut ftc = base.fonts[0];
    let mut charset = None;
    if let Some(run) = fkp::find(chpx, fc) {
        for (sprm, op) in sprms(&chpx[run].grpprl) {
            match sprm {
                code::C_FTC0 => ftc = u16_at(op, 0).unwrap_or(ftc),
                code::C_ISTD => ftc = styles.character(u16_at(op, 0).unwrap_or(10), &base).fonts[0],
                code::C_CHS if op.first().is_some_and(|f| *f != 0) => {
                    charset = u16_at(op, 1).map(|c| c as u8)
                }
                _ => {}
            }
        }
    }
    charset.or_else(|| fonts.get(ftc as usize).map(|f| f.charset))
}

/// Decode again each 8-bit character whose run's font, or the font its
/// paragraph style gives, declares another single-byte character set than
/// the language's code page. A symbol font keeps its byte values.
pub fn recode(
    text: &mut Text,
    wd: &[u8],
    chpx: &[FcRun],
    papx: &[FcRun],
    styles: &Styles,
    fonts: &[Font],
    default: u16,
) {
    let mut pages: Vec<(u16, Codepage)> = Vec::new();
    let mut page = |number: u16| -> Codepage {
        if let Some((_, cp)) = pages.iter().find(|(n, _)| *n == number) {
            return *cp;
        }
        let cp = Codepage::windows(number);
        pages.push((number, cp));
        cp
    };
    let pieces = text.pieces.clone();
    for p in pieces.iter().filter(|p| p.compressed) {
        for cp in p.cp..p.cp_end.min(text.chars.len() as u32) {
            let fc = p.fc + (cp - p.cp);
            let Some(&byte) = wd.get(fc as usize) else {
                continue;
            };
            let unit = match charset_at(fc, chpx, papx, styles, fonts) {
                Some(2) => byte as u16,
                Some(c) => match charset_codepage(c) {
                    Some(n) if n != default => page(n).decode(byte),
                    _ => continue,
                },
                None => continue,
            };
            text.chars[cp as usize] = unit;
        }
    }
}
