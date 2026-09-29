//! Single property modifiers (SPRMs): a 16-bit code whose top bits give the
//! operand size, walked out of a grpprl.

use super::bytes::u16_at;

pub mod code {
    pub const C_RMARK_DEL: u16 = 0x0800;
    pub const C_RMARK_INS: u16 = 0x0801;
    pub const C_FLD_VANISH: u16 = 0x0802;
    pub const C_PIC_LOCATION: u16 = 0x6A03;
    pub const C_DATA: u16 = 0x0806;
    pub const C_CHS: u16 = 0xEA08;
    pub const C_SYMBOL: u16 = 0x6A09;
    pub const C_OLE2: u16 = 0x080A;
    pub const C_HIGHLIGHT: u16 = 0x2A0C;
    pub const C_WEB_HIDDEN: u16 = 0x0811;
    pub const C_ISTD: u16 = 0x4A30;
    pub const C_PLAIN: u16 = 0x2A33;
    pub const C_BOLD: u16 = 0x0835;
    pub const C_ITALIC: u16 = 0x0836;
    pub const C_STRIKE: u16 = 0x0837;
    pub const C_OUTLINE: u16 = 0x0838;
    pub const C_SHADOW: u16 = 0x0839;
    pub const C_SMALL_CAPS: u16 = 0x083A;
    pub const C_CAPS: u16 = 0x083B;
    pub const C_VANISH: u16 = 0x083C;
    pub const C_FTC_DEFAULT: u16 = 0x4A3D;
    pub const C_KUL: u16 = 0x2A3E;
    pub const C_DXA_SPACE: u16 = 0x8840;
    pub const C_ICO: u16 = 0x2A42;
    pub const C_HPS: u16 = 0x4A43;
    pub const C_HPS_POS: u16 = 0x4845;
    pub const C_ISS: u16 = 0x2A48;
    pub const C_FTC0: u16 = 0x4A4F;
    pub const C_FTC1: u16 = 0x4A50;
    pub const C_FTC2: u16 = 0x4A51;
    pub const C_CHAR_SCALE: u16 = 0x4852;
    pub const C_DSTRIKE: u16 = 0x2A53;
    pub const C_IMPRINT: u16 = 0x0854;
    pub const C_SPEC: u16 = 0x0855;
    pub const C_OBJ: u16 = 0x0856;
    pub const C_EMBOSS: u16 = 0x0858;
    pub const C_BOLD_BI: u16 = 0x085C;
    pub const C_ITALIC_BI: u16 = 0x085D;
    pub const C_FTC_BI: u16 = 0x4A5E;
    pub const C_HPS_BI: u16 = 0x4A61;
    pub const C_BRC80: u16 = 0x6865;
    pub const C_SHD80: u16 = 0x4866;
    pub const C_CV: u16 = 0x6870;
    pub const C_SHD: u16 = 0xCA71;
    pub const C_BRC: u16 = 0xCA72;
    pub const C_LID0: u16 = 0x4873;
    pub const C_LID1: u16 = 0x4874;
    pub const C_LID_OLD: u16 = 0x486D;
    pub const C_CV_UL: u16 = 0x6877;

    pub const P_ISTD: u16 = 0x4600;
    pub const P_JC80: u16 = 0x2403;
    pub const P_JC: u16 = 0x2461;
    pub const P_KEEP: u16 = 0x2405;
    pub const P_FNO_LINE_NUMB: u16 = 0x240C;
    pub const P_KEEP_FOLLOW: u16 = 0x2406;
    pub const P_PAGE_BREAK_BEFORE: u16 = 0x2407;
    pub const P_ILVL: u16 = 0x260A;
    pub const P_ILFO: u16 = 0x460B;
    pub const P_CHG_TABS_PAPX: u16 = 0xC60D;
    pub const P_CHG_TABS: u16 = 0xC615;
    pub const P_DXA_RIGHT80: u16 = 0x840E;
    pub const P_DXA_LEFT80: u16 = 0x840F;
    pub const P_DXA_LEFT1_80: u16 = 0x8411;
    pub const P_DXA_RIGHT: u16 = 0x845D;
    pub const P_DXA_LEFT: u16 = 0x845E;
    pub const P_DXA_LEFT1: u16 = 0x8460;
    pub const P_DYA_LINE: u16 = 0x6412;
    pub const P_DYA_BEFORE: u16 = 0xA413;
    pub const P_DYA_AFTER: u16 = 0xA414;
    pub const P_BEFORE_AUTO: u16 = 0x245B;
    pub const P_AFTER_AUTO: u16 = 0x245C;
    pub const P_IN_TABLE: u16 = 0x2416;
    pub const P_TTP: u16 = 0x2417;
    pub const P_WIDOW: u16 = 0x2431;
    pub const P_BRC_TOP80: u16 = 0x6424;
    pub const P_BRC_LEFT80: u16 = 0x6425;
    pub const P_BRC_BOTTOM80: u16 = 0x6426;
    pub const P_BRC_RIGHT80: u16 = 0x6427;
    pub const P_BRC_BETWEEN80: u16 = 0x6428;
    pub const P_SHD80: u16 = 0x442D;
    pub const P_SHD: u16 = 0xC64D;
    pub const P_BRC_TOP: u16 = 0xC64E;
    pub const P_BRC_LEFT: u16 = 0xC64F;
    pub const P_BRC_BOTTOM: u16 = 0xC650;
    pub const P_BRC_RIGHT: u16 = 0xC651;
    pub const P_BRC_BETWEEN: u16 = 0xC652;
    pub const P_OUT_LVL: u16 = 0x2640;
    pub const P_ITAP: u16 = 0x6649;
    pub const P_DTAP: u16 = 0x664A;
    pub const P_INNER_CELL: u16 = 0x244B;
    pub const P_INNER_TTP: u16 = 0x244C;
    pub const P_CONTEXTUAL: u16 = 0x246D;
    pub const P_HUGE_PAPX: u16 = 0x6646;
    pub const P_BIDI: u16 = 0x2441;
    pub const P_PC: u16 = 0x261B;
    pub const P_DXA_ABS: u16 = 0x8418;
    pub const P_DYA_ABS: u16 = 0x8419;
    pub const P_DXA_WIDTH: u16 = 0x841A;
    pub const P_WR: u16 = 0x2423;
    pub const P_WHEIGHT_ABS: u16 = 0x442B;
    pub const P_DYA_FROM_TEXT: u16 = 0x842E;
    pub const P_DXA_FROM_TEXT: u16 = 0x842F;
    pub const P_DCS: u16 = 0x442C;

    pub const T_JC90: u16 = 0x5400;
    pub const T_JC: u16 = 0x548A;
    pub const T_DXA_LEFT: u16 = 0x9601;
    pub const T_DXA_GAP_HALF: u16 = 0x9602;
    pub const T_CANT_SPLIT90: u16 = 0x3403;
    pub const T_CANT_SPLIT: u16 = 0x3466;
    pub const T_HEADER: u16 = 0x3404;
    pub const T_BORDERS80: u16 = 0xD605;
    pub const T_BORDERS: u16 = 0xD613;
    pub const T_ROW_HEIGHT: u16 = 0x9407;
    pub const T_DEF_TABLE: u16 = 0xD608;
    pub const T_DEF_SHD80: u16 = 0xD609;
    pub const T_DEF_SHD: u16 = 0xD612;
    pub const T_DEF_SHD2: u16 = 0xD616;
    pub const T_DEF_SHD3: u16 = 0xD60C;
    pub const T_SET_BRC80: u16 = 0xD620;
    pub const T_SET_BRC: u16 = 0xD62F;
    pub const T_INSERT: u16 = 0x7621;
    pub const T_DELETE: u16 = 0x5622;
    pub const T_DXA_COL: u16 = 0x7623;
    pub const T_MERGE: u16 = 0x5624;
    pub const T_SPLIT: u16 = 0x5625;
    pub const T_VERT_MERGE: u16 = 0xD62B;
    pub const T_VERT_ALIGN: u16 = 0xD62C;
    pub const T_SET_SHD80: u16 = 0x7627;
    pub const T_TABLE_WIDTH: u16 = 0xF614;
    pub const T_CELL_PADDING: u16 = 0xD632;
    pub const T_CELL_PADDING_DEFAULT: u16 = 0xD634;
    pub const T_CELL_WIDTH: u16 = 0xD635;
    pub const T_PC: u16 = 0x360D;
    pub const T_DXA_ABS: u16 = 0x940E;
    pub const T_DYA_ABS: u16 = 0x940F;
    pub const T_DXA_FROM_TEXT: u16 = 0x9410;
    pub const T_DYA_FROM_TEXT: u16 = 0x9411;
    pub const T_DXA_FROM_TEXT_RIGHT: u16 = 0x941E;
    pub const T_DYA_FROM_TEXT_BOTTOM: u16 = 0x941F;

    pub const S_BKC: u16 = 0x3009;
    pub const S_LNC: u16 = 0x3013;
    pub const S_NLNN_MOD: u16 = 0x5015;
    pub const S_DXA_LNN: u16 = 0x9016;
    pub const S_LNN_MIN: u16 = 0x501B;
    pub const S_TITLE_PAGE: u16 = 0x300A;
    pub const S_CCOLUMNS: u16 = 0x500B;
    pub const S_DXA_COLUMNS: u16 = 0x900C;
    pub const S_NFC_PGN: u16 = 0x300E;
    pub const S_NFC_FTN_REF: u16 = 0x5040;
    pub const S_NFC_EDN_REF: u16 = 0x5042;
    pub const S_PGN_RESTART: u16 = 0x3011;
    pub const S_PGN_START97: u16 = 0x501C;
    pub const S_PGN_START: u16 = 0x7044;
    pub const S_VJC: u16 = 0x301A;
    pub const S_ORIENTATION: u16 = 0x301D;
    pub const S_XA_PAGE: u16 = 0xB01F;
    pub const S_YA_PAGE: u16 = 0xB020;
    pub const S_DXA_LEFT: u16 = 0xB021;
    pub const S_DXA_RIGHT: u16 = 0xB022;
    pub const S_DYA_TOP: u16 = 0x9023;
    pub const S_DYA_BOTTOM: u16 = 0x9024;
    pub const S_GUTTER: u16 = 0xB025;
    pub const S_HDR_TOP: u16 = 0xB017;
    pub const S_HDR_BOTTOM: u16 = 0xB018;
    pub const S_LBETWEEN: u16 = 0x3019;
}

/// Walk the `(sprm, operand)` pairs of a Word 97 grpprl, stopping at the
/// first one whose operand would leave the buffer.
pub fn sprms(grpprl: &[u8]) -> Sprms<'_> {
    Sprms {
        data: grpprl,
        at: 0,
    }
}

pub struct Sprms<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Iterator for Sprms<'a> {
    type Item = (u16, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let sprm = u16_at(self.data, self.at)?;
        let at = self.at + 2;
        let len = operand_len(sprm, self.data, at)?;
        let operand = self.data.get(at..at + len)?;
        self.at = at + len;
        Some((sprm, operand))
    }
}

fn operand_len(sprm: u16, data: &[u8], at: usize) -> Option<usize> {
    Some(match sprm >> 13 {
        0 | 1 => 1,
        2 | 4 | 5 => 2,
        3 => 4,
        7 => 3,
        _ => match sprm {
            code::T_DEF_TABLE => u16_at(data, at)? as usize + 1,
            code::P_CHG_TABS if *data.get(at)? == 255 => {
                let del = *data.get(at + 1)? as usize;
                let add = *data.get(at + 2 + 4 * del)? as usize;
                1 + (1 + 4 * del) + (1 + 3 * add)
            }
            _ => *data.get(at)? as usize + 1,
        },
    })
}

/// What a sprm modifies, from its `sgc` bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Paragraph,
    Character,
    Picture,
    Section,
    Table,
    Other,
}

pub fn group(sprm: u16) -> Group {
    match (sprm >> 10) & 7 {
        1 => Group::Paragraph,
        2 => Group::Character,
        3 => Group::Picture,
        4 => Group::Section,
        5 => Group::Table,
        _ => Group::Other,
    }
}

/// The sprm a non-complex PRM stands for, indexed by its 7-bit `isprm`.
pub fn prm0_sprm(isprm: u8) -> Option<u16> {
    Some(match isprm {
        0x00 => 0x2879,
        0x04 => 0x2602,
        0x05 => 0x2403,
        0x07 => 0x2405,
        0x08 => 0x2406,
        0x09 => 0x2407,
        0x0C => 0x260A,
        0x0D => 0x2470,
        0x0E => 0x240C,
        0x0F => 0x2471,
        0x18 => 0x2416,
        0x19 => 0x2417,
        0x1D => 0x261B,
        0x25 => 0x2423,
        0x2C => 0x242A,
        0x32 => 0x2430,
        0x33 => 0x2431,
        0x35 => 0x2433,
        0x36 => 0x2434,
        0x37 => 0x2435,
        0x38 => 0x2436,
        0x39 => 0x2437,
        0x3A => 0x2438,
        0x41 => 0x0800,
        0x42 => 0x0801,
        0x43 => 0x0802,
        0x47 => 0x0806,
        0x4B => 0x080A,
        0x4D => 0x2A0C,
        0x4E => 0x0858,
        0x4F => 0x2859,
        0x50 => 0x0811,
        0x51 => 0x0818,
        0x53 => 0x2A33,
        0x55 => 0x0835,
        0x56 => 0x0836,
        0x57 => 0x0837,
        0x58 => 0x0838,
        0x59 => 0x0839,
        0x5A => 0x083A,
        0x5B => 0x083B,
        0x5C => 0x083C,
        0x5E => 0x2A3E,
        0x62 => 0x2A42,
        0x68 => 0x2A48,
        0x73 => 0x2A53,
        0x74 => 0x0854,
        0x75 => 0x0855,
        0x76 => 0x0856,
        0x78 => 0x2640,
        0x7B => 0x2A90,
        0x7C => 0x2A86,
        _ => return None,
    })
}
