//! Character, paragraph, section and table properties, and how a grpprl
//! changes each of them.

use super::bytes::{i16_at, i32_at, u16_at, u32_at, u8_at};
use super::sprm::{code, sprms};

/// An RGB colour; `None` in a property means automatic.
pub type Rgb = [u8; 3];

/// The 17 colours an `Ico` indexes.
pub fn ico(index: u8) -> Option<Rgb> {
    Some(match index {
        1 => [0, 0, 0],
        2 => [0, 0, 255],
        3 => [0, 255, 255],
        4 => [0, 255, 0],
        5 => [255, 0, 255],
        6 => [255, 0, 0],
        7 => [255, 255, 0],
        8 => [255, 255, 255],
        9 => [0, 0, 128],
        10 => [0, 128, 128],
        11 => [0, 128, 0],
        12 => [128, 0, 128],
        13 => [128, 0, 0],
        14 => [128, 128, 0],
        15 => [128, 128, 128],
        16 => [192, 192, 192],
        _ => return None,
    })
}

/// A COLORREF: red, green, blue, then 0xFF for automatic.
fn colorref(b: &[u8]) -> Option<Rgb> {
    match b {
        [r, g, bl, 0, ..] => Some([*r, *g, *bl]),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Border {
    /// Width in eighths of a point.
    pub width: u8,
    /// `brcType`: 0 none, 1 single, 3 double, 6 dotted, 7 dashed…
    pub kind: u8,
    pub color: Option<Rgb>,
    /// Distance from the text, in points.
    pub space: u8,
    pub shadow: bool,
}

impl Border {
    fn visible(self) -> Option<Self> {
        (self.kind != 0 && self.kind != 0xFF).then_some(self)
    }

    fn from80(b: &[u8]) -> Option<Self> {
        if b.len() < 4 || u32_at(b, 0) == Some(0xFFFF_FFFF) {
            return None;
        }
        Border {
            width: b[0],
            kind: b[1],
            color: ico(b[2]),
            space: b[3] & 0x1F,
            shadow: b[3] & 0x20 != 0,
        }
        .visible()
    }

    fn from97(b: &[u8]) -> Option<Self> {
        if b.len() < 8 || u32_at(b, 4) == Some(0xFFFF_FFFF) {
            return None;
        }
        Border {
            width: b[4],
            kind: b[5],
            color: colorref(&b[0..4]),
            space: b[6] & 0x1F,
            shadow: b[6] & 0x20 != 0,
        }
        .visible()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shading {
    pub fore: Option<Rgb>,
    pub back: Option<Rgb>,
    /// `ipat`: 0 clear, 1 solid, 2..=13 percentages, others hatches.
    pub pattern: u16,
}

impl Shading {
    fn from80(v: u16) -> Option<Self> {
        let s = Shading {
            fore: ico((v & 0x1F) as u8),
            back: ico(((v >> 5) & 0x1F) as u8),
            pattern: v >> 10,
        };
        s.visible()
    }

    fn from97(b: &[u8]) -> Option<Self> {
        if b.len() < 10 {
            return None;
        }
        Shading {
            fore: colorref(&b[0..4]),
            back: colorref(&b[4..8]),
            pattern: u16_at(b, 8).unwrap_or(0),
        }
        .visible()
    }

    fn visible(self) -> Option<Self> {
        (self.pattern != 0 || self.back.is_some())
            .then_some(self)
            .filter(|s| s.pattern != 0xFFFF)
    }

    /// The colour a renderer fills with, a pattern mixed into one colour.
    pub fn fill(&self) -> Option<Rgb> {
        let share = crate::model::pattern_share(self.pattern);
        if share == 0 {
            return self.back;
        }
        let back = self.back.unwrap_or([255, 255, 255]);
        let fore = self.fore.unwrap_or([0, 0, 0]);
        Some(std::array::from_fn(|i| {
            crate::model::mix(back[i], fore[i], share)
        }))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chp {
    pub istd: u16,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub dstrike: bool,
    pub outline: bool,
    pub shadow: bool,
    pub emboss: bool,
    pub imprint: bool,
    pub small_caps: bool,
    pub caps: bool,
    pub vanish: bool,
    /// `kul`: 0 none, 1 single, 2 words, 3 double, 4 dotted…
    pub underline: u8,
    /// Font indices into the font table: ASCII, East Asian, other.
    pub fonts: [u16; 3],
    pub font_bi: u16,
    /// Size in half-points.
    pub size: u16,
    pub color: Option<Rgb>,
    pub highlight: Option<Rgb>,
    /// 0 normal, 1 superscript, 2 subscript.
    pub iss: u8,
    /// Raised or lowered, in half-points.
    pub position: i16,
    /// Extra space after each character, in twips.
    pub spacing: i16,
    /// Horizontal scale, in percent.
    pub scale: u16,
    pub shading: Option<Shading>,
    pub border: Option<Border>,
    pub lang: u16,
    /// The character is a special one (picture, footnote reference, field
    /// mark) rather than text.
    pub special: bool,
    pub ole2: bool,
    pub obj: bool,
    pub data: bool,
    pub deleted: bool,
    pub inserted: bool,
    pub field_vanish: bool,
    pub pic_location: Option<u32>,
    /// A symbol drawn from font `.0`, character `.1`.
    pub symbol: Option<(u16, u16)>,
}

impl Default for Chp {
    fn default() -> Self {
        Chp {
            istd: 10,
            bold: false,
            italic: false,
            strike: false,
            dstrike: false,
            outline: false,
            shadow: false,
            emboss: false,
            imprint: false,
            small_caps: false,
            caps: false,
            vanish: false,
            underline: 0,
            fonts: [0; 3],
            font_bi: 0,
            size: 20,
            color: None,
            highlight: None,
            iss: 0,
            position: 0,
            spacing: 0,
            scale: 100,
            shading: None,
            border: None,
            lang: 0x0400,
            special: false,
            ole2: false,
            obj: false,
            data: false,
            deleted: false,
            inserted: false,
            field_vanish: false,
            pic_location: None,
            symbol: None,
        }
    }
}

/// A toggle operand: 0 off, 1 on, 0x80 the style's value, 0x81 its opposite.
fn toggle(op: u8, style: bool) -> bool {
    match op {
        0 => false,
        1 => true,
        0x80 => style,
        0x81 => !style,
        _ => style,
    }
}

impl Chp {
    /// Apply a character grpprl. `style` is the CHP of the styles in force,
    /// which toggles 0x80 and 0x81 refer to; `char_style` resolves an istd to
    /// the CHP a character style gives on top of `style`.
    pub fn apply(&mut self, grpprl: &[u8], style: &Chp, char_style: &dyn Fn(u16, &Chp) -> Chp) {
        // The character style is the base the other sprms change, wherever
        // the grpprl places it.
        if let Some(istd) = sprms(grpprl)
            .find(|(s, _)| *s == code::C_ISTD)
            .and_then(|(_, op)| u16_at(op, 0))
        {
            let keep = self.clone();
            *self = char_style(istd, style);
            self.special = keep.special;
            self.pic_location = keep.pic_location;
            self.deleted = keep.deleted;
            self.inserted = keep.inserted;
        }
        for (sprm, op) in sprms(grpprl) {
            let b = op.first().copied().unwrap_or(0);
            let w = u16_at(op, 0).unwrap_or(0);
            match sprm {
                code::C_ISTD => {}
                code::C_PLAIN => {
                    let keep = (
                        self.special,
                        self.pic_location,
                        self.deleted,
                        self.inserted,
                        self.istd,
                    );
                    *self = style.clone();
                    (
                        self.special,
                        self.pic_location,
                        self.deleted,
                        self.inserted,
                        self.istd,
                    ) = keep;
                }
                code::C_BOLD => self.bold = toggle(b, style.bold),
                code::C_ITALIC => self.italic = toggle(b, style.italic),
                code::C_STRIKE => self.strike = toggle(b, style.strike),
                code::C_DSTRIKE => self.dstrike = toggle(b, style.dstrike),
                code::C_OUTLINE => self.outline = toggle(b, style.outline),
                code::C_SHADOW => self.shadow = toggle(b, style.shadow),
                code::C_EMBOSS => self.emboss = toggle(b, style.emboss),
                code::C_IMPRINT => self.imprint = toggle(b, style.imprint),
                code::C_SMALL_CAPS => self.small_caps = toggle(b, style.small_caps),
                code::C_CAPS => self.caps = toggle(b, style.caps),
                code::C_VANISH => self.vanish = toggle(b, style.vanish),
                code::C_KUL => self.underline = b,
                code::C_FTC_DEFAULT | code::C_FTC0 => self.fonts[0] = w,
                code::C_FTC1 => self.fonts[1] = w,
                code::C_FTC2 => self.fonts[2] = w,
                code::C_FTC_BI => self.font_bi = w,
                code::C_HPS => self.size = w.clamp(2, 3276),
                code::C_ICO => self.color = ico(b),
                code::C_CV => self.color = colorref(op),
                code::C_HIGHLIGHT => self.highlight = ico(b),
                code::C_ISS => self.iss = b,
                code::C_HPS_POS => self.position = w as i16,
                code::C_DXA_SPACE => self.spacing = w as i16,
                code::C_CHAR_SCALE => self.scale = w,
                code::C_SHD80 => self.shading = Shading::from80(w),
                code::C_SHD => self.shading = Shading::from97(op.get(1..).unwrap_or(&[])),
                code::C_BRC80 => self.border = Border::from80(op),
                code::C_BRC => self.border = Border::from97(op.get(1..).unwrap_or(&[])),
                code::C_LID0 | code::C_LID_OLD => self.lang = w,
                code::C_SPEC => self.special = b != 0,
                code::C_OLE2 => self.ole2 = b != 0,
                code::C_OBJ => self.obj = b != 0,
                code::C_DATA => self.data = b != 0,
                code::C_RMARK_DEL => self.deleted = toggle(b, style.deleted),
                code::C_RMARK_INS => self.inserted = toggle(b, style.inserted),
                code::C_FLD_VANISH => self.field_vanish = toggle(b, style.field_vanish),
                code::C_PIC_LOCATION => self.pic_location = u32_at(op, 0),
                code::C_SYMBOL => {
                    if let (Some(font), Some(ch)) = (u16_at(op, 0), u16_at(op, 2)) {
                        self.symbol = Some((font, ch));
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
    Distribute,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tab {
    /// Position in twips from the paragraph's left indent origin.
    pub pos: i16,
    /// 0 left, 1 centre, 2 right, 3 decimal, 4 bar.
    pub align: u8,
    /// 0 none, 1 dots, 2 hyphens, 3 underscores, 4 heavy, 5 middle dots.
    pub leader: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineSpacing {
    /// Twips when `multiple` is false (negative: exactly, positive: at least),
    /// else 240ths of a line.
    pub value: i16,
    pub multiple: bool,
}

impl Default for LineSpacing {
    fn default() -> Self {
        LineSpacing {
            value: 240,
            multiple: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pap {
    pub istd: u16,
    pub align: Align,
    /// Indents in twips; `first_line` is relative to `left`.
    pub left: i32,
    pub right: i32,
    pub first_line: i32,
    pub before: u16,
    pub after: u16,
    /// The space above is the auto space, until a later sprm sets it.
    pub before_auto: bool,
    pub after_auto: bool,
    pub contextual: bool,
    pub line: LineSpacing,
    pub keep: bool,
    pub keep_next: bool,
    pub page_break_before: bool,
    pub widow: bool,
    pub no_line_numbers: bool,
    pub tabs: Vec<Tab>,
    /// Top, left, bottom, right, between.
    pub borders: [Option<Border>; 5],
    pub shading: Option<Shading>,
    pub ilfo: u16,
    pub ilvl: u8,
    pub outline_level: u8,
    pub in_table: bool,
    pub row_end: bool,
    /// Table nesting depth; 1 for a plain table.
    pub depth: i32,
    pub inner_cell: bool,
    pub inner_row_end: bool,
    pub bidi: bool,
    /// Set when the paragraph is positioned in a frame (an APO).
    pub frame: Option<Frame>,
    /// A dropped capital: 1 in the text, 2 in the margin; and how many lines
    /// it spans. The letter is its own paragraph, the one it opens follows.
    pub drop_cap: Option<(u8, u8)>,
}

/// A positioned paragraph's frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    /// Vertical anchor (0 margin, 1 page, 2 paragraph) and horizontal anchor
    /// (0 column, 1 margin, 2 page), from sprmPPc.
    pub anchor_v: u8,
    pub anchor_h: u8,
    /// Position in twips; small negative values are Word's symbolic
    /// positions (-4 centre, -8 right, -12 inside, -16 outside).
    pub x: i16,
    pub y: i16,
    pub width: u16,
    /// Height in twips; the top bit set means at least, else exactly, zero auto.
    pub height: u16,
    /// Distance kept from surrounding text.
    pub dx_text: u16,
    pub dy_text: u16,
    /// 0 around, 1 none (text above and below), 2 around.
    pub wrap: u8,
}

impl Frame {
    /// No position of its own: the paragraph stays in the text.
    pub fn is_empty(&self) -> bool {
        matches!(self.wrap, 0 | 2)
            && Frame {
                wrap: 0,
                height: self.height & 0x7FFF,
                ..*self
            } == Frame::default()
    }
}

/// A table's absolute position: sprmTPc, without which the others place
/// nothing, and the offsets and distances that go with it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TablePosition {
    pub pc: Option<u8>,
    pub x: i16,
    pub y: i16,
    /// Distance kept from the text, left, right, top, bottom, in twips.
    pub distance: [u16; 4],
}

impl TablePosition {
    /// The frame the table floats in, none when it sits at the origin.
    pub fn frame(&self) -> Option<Frame> {
        let pc = self.pc?;
        let frame = Frame {
            anchor_v: (pc >> 4) & 3,
            anchor_h: (pc >> 6) & 3,
            x: self.x,
            y: self.y,
            ..Frame::default()
        };
        (pc != 0 || self.x != 0 || self.y != 0 || self.distance != [0; 4]).then_some(frame)
    }
}

fn change_tabs(tabs: &mut Vec<Tab>, op: &[u8], with_close: bool) {
    let mut at = 1;
    let Some(n_del) = u8_at(op, at) else { return };
    at += 1;
    let dels: Vec<i16> = (0..n_del as usize)
        .filter_map(|i| i16_at(op, at + 2 * i))
        .collect();
    at += 2 * n_del as usize;
    let closes: Vec<i16> = if with_close {
        let c = (0..n_del as usize)
            .filter_map(|i| i16_at(op, at + 2 * i))
            .collect();
        at += 2 * n_del as usize;
        c
    } else {
        vec![0; n_del as usize]
    };
    tabs.retain(|t| {
        !dels
            .iter()
            .zip(closes.iter())
            .any(|(d, c)| (t.pos as i32 - *d as i32).abs() <= (*c as i32).max(0))
    });
    let Some(n_add) = u8_at(op, at) else { return };
    at += 1;
    for i in 0..n_add as usize {
        let (Some(pos), Some(tbd)) = (
            i16_at(op, at + 2 * i),
            u8_at(op, at + 2 * n_add as usize + i),
        ) else {
            break;
        };
        tabs.retain(|t| t.pos != pos);
        tabs.push(Tab {
            pos,
            align: tbd & 7,
            leader: (tbd >> 3) & 7,
        });
    }
    tabs.sort_by_key(|t| t.pos);
}

impl Pap {
    pub fn apply(&mut self, grpprl: &[u8]) {
        for (sprm, op) in sprms(grpprl) {
            let b = op.first().copied().unwrap_or(0);
            let w = u16_at(op, 0).unwrap_or(0);
            match sprm {
                code::P_ISTD => self.istd = w,
                code::P_JC80 | code::P_JC => {
                    self.align = match b {
                        1 => Align::Center,
                        2 => Align::Right,
                        3 => Align::Justify,
                        4 | 5 => Align::Distribute,
                        _ => Align::Left,
                    }
                }
                code::P_DXA_LEFT80 | code::P_DXA_LEFT => self.left = w as i16 as i32,
                code::P_DXA_RIGHT80 | code::P_DXA_RIGHT => self.right = w as i16 as i32,
                code::P_DXA_LEFT1_80 | code::P_DXA_LEFT1 => self.first_line = w as i16 as i32,
                code::P_DYA_BEFORE => {
                    self.before = w;
                    self.before_auto = false;
                }
                code::P_DYA_AFTER => {
                    self.after = w;
                    self.after_auto = false;
                }
                // Turning auto spacing off keeps the space it set.
                code::P_BEFORE_AUTO => self.before_auto |= b != 0,
                code::P_AFTER_AUTO => self.after_auto |= b != 0,
                code::P_CONTEXTUAL => self.contextual = b != 0,
                code::P_DYA_LINE => {
                    self.line = LineSpacing {
                        value: w as i16,
                        multiple: i16_at(op, 2).unwrap_or(1) != 0,
                    }
                }
                code::P_KEEP => self.keep = b != 0,
                code::P_FNO_LINE_NUMB => self.no_line_numbers = b != 0,
                code::P_KEEP_FOLLOW => self.keep_next = b != 0,
                code::P_PAGE_BREAK_BEFORE => self.page_break_before = b != 0,
                code::P_WIDOW => self.widow = b != 0,
                code::P_CHG_TABS_PAPX => change_tabs(&mut self.tabs, op, false),
                code::P_CHG_TABS => change_tabs(&mut self.tabs, op, true),
                code::P_BRC_TOP80 => self.borders[0] = Border::from80(op),
                code::P_BRC_LEFT80 => self.borders[1] = Border::from80(op),
                code::P_BRC_BOTTOM80 => self.borders[2] = Border::from80(op),
                code::P_BRC_RIGHT80 => self.borders[3] = Border::from80(op),
                code::P_BRC_BETWEEN80 => self.borders[4] = Border::from80(op),
                code::P_BRC_TOP => self.borders[0] = Border::from97(op.get(1..).unwrap_or(&[])),
                code::P_BRC_LEFT => self.borders[1] = Border::from97(op.get(1..).unwrap_or(&[])),
                code::P_BRC_BOTTOM => self.borders[2] = Border::from97(op.get(1..).unwrap_or(&[])),
                code::P_BRC_RIGHT => self.borders[3] = Border::from97(op.get(1..).unwrap_or(&[])),
                code::P_BRC_BETWEEN => self.borders[4] = Border::from97(op.get(1..).unwrap_or(&[])),
                code::P_SHD80 => self.shading = Shading::from80(w),
                code::P_SHD => self.shading = Shading::from97(op.get(1..).unwrap_or(&[])),
                code::P_ILFO => self.ilfo = w,
                code::P_ILVL => self.ilvl = b.min(8),
                code::P_OUT_LVL => self.outline_level = b,
                code::P_IN_TABLE => {
                    self.in_table = b != 0;
                    if self.in_table && self.depth == 0 {
                        self.depth = 1;
                    }
                }
                code::P_TTP => self.row_end = b != 0,
                code::P_ITAP => {
                    self.depth = i32_at(op, 0).unwrap_or(0);
                    self.in_table = self.depth > 0;
                }
                code::P_DTAP => self.depth = self.depth.saturating_add(i32_at(op, 0).unwrap_or(0)),
                code::P_INNER_CELL => self.inner_cell = b != 0,
                code::P_INNER_TTP => self.inner_row_end = b != 0,
                code::P_BIDI => self.bidi = b != 0,
                code::P_PC => {
                    let f = self.frame.get_or_insert_with(Frame::default);
                    f.anchor_v = (b >> 4) & 3;
                    f.anchor_h = (b >> 6) & 3;
                }
                code::P_DXA_ABS => self.frame.get_or_insert_with(Frame::default).x = w as i16,
                code::P_DYA_ABS => self.frame.get_or_insert_with(Frame::default).y = w as i16,
                code::P_DXA_WIDTH => self.frame.get_or_insert_with(Frame::default).width = w,
                code::P_WHEIGHT_ABS => self.frame.get_or_insert_with(Frame::default).height = w,
                code::P_DXA_FROM_TEXT => self.frame.get_or_insert_with(Frame::default).dx_text = w,
                code::P_DYA_FROM_TEXT => self.frame.get_or_insert_with(Frame::default).dy_text = w,
                code::P_WR => self.frame.get_or_insert_with(Frame::default).wrap = b,
                code::P_DCS => {
                    self.drop_cap = (b & 7 != 0).then_some((b & 7, b >> 3));
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sep {
    /// `bkc`: 0 continuous, 1 new column, 2 new page, 3 even page, 4 odd page.
    pub break_kind: u8,
    pub title_page: bool,
    /// Page size and margins, in twips.
    pub page_width: u32,
    pub page_height: u32,
    pub left: u32,
    pub right: u32,
    pub top: i32,
    pub bottom: i32,
    pub gutter: u32,
    /// Distance of the header from the top edge, and of the footer from the
    /// bottom edge.
    pub header: u32,
    pub footer: u32,
    pub landscape: bool,
    pub columns: u16,
    pub column_space: u32,
    pub column_line: bool,
    pub page_number_restart: bool,
    pub page_number_start: u32,
    pub page_number_format: u8,
    /// Number formats of footnote and endnote marks, when the section sets
    /// them.
    pub footnote_format: Option<u8>,
    pub endnote_format: Option<u8>,
    /// Vertical alignment of the text on the page: 0 top, 1 centre, 2 both, 3 bottom.
    pub vertical_align: u8,
    /// Line numbers: shown on every `line_count_by`th line when non-zero,
    /// restarting per page (0), per section (1) or never (2), the first
    /// numbered `line_start + 1`, `line_distance` twips from the text.
    pub line_count_by: u16,
    pub line_restart: u8,
    pub line_start: u16,
    pub line_distance: i16,
}

impl Default for Sep {
    fn default() -> Self {
        Sep {
            break_kind: 2,
            title_page: false,
            page_width: 12240,
            page_height: 15840,
            left: 1800,
            right: 1800,
            top: 1440,
            bottom: 1440,
            gutter: 0,
            header: 720,
            footer: 720,
            landscape: false,
            columns: 1,
            column_space: 720,
            column_line: false,
            page_number_restart: false,
            page_number_start: 1,
            page_number_format: 0,
            footnote_format: None,
            endnote_format: None,
            vertical_align: 0,
            line_count_by: 0,
            line_restart: 0,
            line_start: 0,
            line_distance: 0,
        }
    }
}

impl Sep {
    pub fn apply(&mut self, grpprl: &[u8]) {
        for (sprm, op) in sprms(grpprl) {
            let b = op.first().copied().unwrap_or(0);
            let w = u16_at(op, 0).unwrap_or(0);
            match sprm {
                code::S_BKC => self.break_kind = b,
                code::S_TITLE_PAGE => self.title_page = b != 0,
                code::S_XA_PAGE if w > 0 => self.page_width = w as u32,
                code::S_YA_PAGE if w > 0 => self.page_height = w as u32,
                code::S_DXA_LEFT => self.left = w as u32,
                code::S_DXA_RIGHT => self.right = w as u32,
                code::S_DYA_TOP => self.top = w as i16 as i32,
                code::S_DYA_BOTTOM => self.bottom = w as i16 as i32,
                code::S_GUTTER => self.gutter = w as u32,
                code::S_HDR_TOP => self.header = w as u32,
                code::S_HDR_BOTTOM => self.footer = w as u32,
                code::S_ORIENTATION => self.landscape = b == 2,
                code::S_CCOLUMNS => self.columns = w.saturating_add(1),
                code::S_DXA_COLUMNS => self.column_space = w as u32,
                code::S_LBETWEEN => self.column_line = b != 0,
                code::S_PGN_RESTART => self.page_number_restart = b != 0,
                code::S_PGN_START97 => self.page_number_start = w as u32,
                code::S_PGN_START => self.page_number_start = u32_at(op, 0).unwrap_or(1),
                code::S_NFC_PGN => self.page_number_format = b,
                code::S_NFC_FTN_REF => self.footnote_format = Some(b),
                code::S_NFC_EDN_REF => self.endnote_format = Some(b),
                code::S_VJC => self.vertical_align = b,
                code::S_LNC => self.line_restart = b,
                code::S_NLNN_MOD => self.line_count_by = w,
                code::S_LNN_MIN => self.line_start = w,
                code::S_DXA_LNN => self.line_distance = w as i16,
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HMerge {
    #[default]
    None,
    First,
    Merged,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VMerge {
    #[default]
    None,
    Restart,
    Continue,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    /// Right edge, in twips from the same origin as `Tap::edges[0]`.
    pub width: i32,
    pub hmerge: HMerge,
    pub vmerge: VMerge,
    /// 0 top, 1 centre, 2 bottom.
    pub valign: u8,
    /// Top, left, bottom, right.
    pub borders: [Option<Border>; 4],
    pub shading: Option<Shading>,
    /// Margins in twips, top, left, bottom, right, over the row's.
    pub padding: [Option<u16>; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tap {
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    /// Half the space between cells, in twips.
    pub gap_half: i32,
    /// Row height in twips: positive at least, negative exactly, zero auto.
    pub height: i32,
    pub cant_split: bool,
    pub header: bool,
    /// Cell edges: `cells.len() + 1` positions.
    pub edges: Vec<i32>,
    pub cells: Vec<Cell>,
    /// Top, left, bottom, right, inside horizontal, inside vertical.
    pub borders: [Option<Border>; 6],
    /// The absolute position the table floats at.
    pub position: TablePosition,
    /// Cell margins in twips, top, left, bottom, right; left and right
    /// default to `gap_half`.
    pub padding: [Option<u16>; 4],
    /// Preferred width in fiftieths of a percent of the text column.
    pub percent: Option<u16>,
}

fn set_cells<F: FnMut(&mut Cell)>(cells: &mut [Cell], first: u8, lim: u8, mut f: F) {
    let lim = (lim as usize).min(cells.len());
    for cell in cells.iter_mut().take(lim).skip(first as usize) {
        f(cell);
    }
}

impl Tap {
    pub fn apply(&mut self, grpprl: &[u8]) {
        for (sprm, op) in sprms(grpprl) {
            let b = op.first().copied().unwrap_or(0);
            let w = u16_at(op, 0).unwrap_or(0);
            match sprm {
                code::T_JC90 | code::T_JC => self.align = b,
                code::T_DXA_GAP_HALF => self.gap_half = w as i16 as i32,
                code::T_DXA_LEFT => {
                    if let Some(first) = self.edges.first().copied() {
                        let shift = w as i16 as i32 - (first + self.gap_half);
                        self.edges.iter_mut().for_each(|e| *e += shift);
                    }
                }
                code::T_ROW_HEIGHT => self.height = w as i16 as i32,
                code::T_CANT_SPLIT90 | code::T_CANT_SPLIT => self.cant_split = b != 0,
                code::T_HEADER => self.header = b != 0,
                code::T_TABLE_WIDTH if b == 2 => self.percent = u16_at(op, 1),
                code::T_PC => self.position.pc = Some(b),
                code::T_DXA_ABS => self.position.x = w as i16,
                code::T_DYA_ABS => self.position.y = w as i16,
                code::T_DXA_FROM_TEXT => self.position.distance[0] = w,
                code::T_DXA_FROM_TEXT_RIGHT => self.position.distance[1] = w,
                code::T_DYA_FROM_TEXT => self.position.distance[2] = w,
                code::T_DYA_FROM_TEXT_BOTTOM => self.position.distance[3] = w,
                code::T_BORDERS80 => {
                    for i in 0..6 {
                        self.borders[i] =
                            Border::from80(op.get(1 + 4 * i..5 + 4 * i).unwrap_or(&[]));
                    }
                }
                code::T_BORDERS => {
                    for i in 0..6 {
                        self.borders[i] =
                            Border::from97(op.get(1 + 8 * i..9 + 8 * i).unwrap_or(&[]));
                    }
                }
                code::T_DEF_TABLE => self.def_table(op),
                code::T_DEF_SHD80 => {
                    let n = b as usize / 2;
                    for i in 0..n.min(self.cells.len()) {
                        self.cells[i].shading = Shading::from80(u16_at(op, 1 + 2 * i).unwrap_or(0));
                    }
                }
                code::T_DEF_SHD | code::T_DEF_SHD2 | code::T_DEF_SHD3 => {
                    let base = match sprm {
                        code::T_DEF_SHD2 => 22,
                        code::T_DEF_SHD3 => 44,
                        _ => 0,
                    };
                    let n = b as usize / 10;
                    for i in 0..n {
                        if let Some(cell) = self.cells.get_mut(base + i) {
                            cell.shading =
                                Shading::from97(op.get(1 + 10 * i..11 + 10 * i).unwrap_or(&[]));
                        }
                    }
                }
                code::T_SET_BRC80 | code::T_SET_BRC => {
                    let (first, lim, which) = (
                        op.get(1).copied().unwrap_or(0),
                        op.get(2).copied().unwrap_or(0),
                        op.get(3).copied().unwrap_or(0),
                    );
                    let brc = if sprm == code::T_SET_BRC80 {
                        Border::from80(op.get(4..8).unwrap_or(&[]))
                    } else {
                        Border::from97(op.get(4..12).unwrap_or(&[]))
                    };
                    set_cells(&mut self.cells, first, lim, |c| {
                        for (bit, side) in [(1, 0), (2, 1), (4, 2), (8, 3)] {
                            if which & bit != 0 {
                                c.borders[side] = brc;
                            }
                        }
                    });
                }
                code::T_VERT_MERGE => {
                    let (itc, flags) = (
                        op.get(1).copied().unwrap_or(0),
                        op.get(2).copied().unwrap_or(0),
                    );
                    if let Some(c) = self.cells.get_mut(itc as usize) {
                        c.vmerge = match flags {
                            1 => VMerge::Continue,
                            3 => VMerge::Restart,
                            _ => VMerge::None,
                        };
                    }
                }
                code::T_CELL_PADDING_DEFAULT | code::T_CELL_PADDING => {
                    let (first, lim, sides) = (
                        op.get(1).copied().unwrap_or(0),
                        op.get(2).copied().unwrap_or(0),
                        op.get(3).copied().unwrap_or(0),
                    );
                    let value = u16_at(op, 5).unwrap_or(0);
                    let set = |padding: &mut [Option<u16>; 4]| {
                        for (side, slot) in padding.iter_mut().enumerate() {
                            if sides & (1 << side) != 0 {
                                *slot = Some(value);
                            }
                        }
                    };
                    if sprm == code::T_CELL_PADDING {
                        set_cells(&mut self.cells, first, lim, |c| set(&mut c.padding));
                    } else {
                        set(&mut self.padding);
                    }
                }
                code::T_VERT_ALIGN => {
                    let (first, lim, v) = (
                        op.get(1).copied().unwrap_or(0),
                        op.get(2).copied().unwrap_or(0),
                        op.get(3).copied().unwrap_or(0),
                    );
                    set_cells(&mut self.cells, first, lim, |c| c.valign = v);
                }
                code::T_MERGE => {
                    let (first, lim) = (b, op.get(1).copied().unwrap_or(0));
                    let mut i = 0u8;
                    set_cells(&mut self.cells, first, lim, |c| {
                        c.hmerge = if i == 0 {
                            HMerge::First
                        } else {
                            HMerge::Merged
                        };
                        i += 1;
                    });
                }
                code::T_SPLIT => {
                    let lim = op.get(1).copied().unwrap_or(0);
                    set_cells(&mut self.cells, b, lim, |c| c.hmerge = HMerge::None);
                }
                code::T_INSERT => {
                    let (at, count, width) = (
                        b as usize,
                        op.get(1).copied().unwrap_or(0) as usize,
                        i16_at(op, 2).unwrap_or(0) as i32,
                    );
                    if self.edges.is_empty() {
                        self.edges.push(0);
                    }
                    let at = at.min(self.cells.len());
                    for k in 0..count {
                        let left = self.edges[at + k];
                        self.edges.insert(at + k + 1, left + width);
                        self.cells.insert(at + k, Cell::default());
                    }
                    for e in self.edges.iter_mut().skip(at + count + 1) {
                        *e += width * count as i32;
                    }
                }
                code::T_DELETE => {
                    let (first, lim) = (
                        b as usize,
                        (op.get(1).copied().unwrap_or(0) as usize).min(self.cells.len()),
                    );
                    if first < lim {
                        self.cells.drain(first..lim);
                        self.edges.drain(first + 1..lim + 1);
                    }
                }
                code::T_DXA_COL => {
                    let (first, lim, width) = (
                        b as usize,
                        (op.get(1).copied().unwrap_or(0) as usize).min(self.cells.len()),
                        i16_at(op, 2).unwrap_or(0) as i32,
                    );
                    for i in first..lim {
                        let old = self.edges[i + 1] - self.edges[i];
                        let delta = width - old;
                        for e in self.edges.iter_mut().skip(i + 1) {
                            *e += delta;
                        }
                    }
                }
                _ => {}
            }
        }
        for (i, c) in self.cells.iter_mut().enumerate() {
            c.width = self.edges.get(i + 1).copied().unwrap_or(0)
                - self.edges.get(i).copied().unwrap_or(0);
        }
    }

    fn def_table(&mut self, op: &[u8]) {
        let Some(n) = u8_at(op, 2) else { return };
        let n = n as usize;
        self.edges = (0..=n)
            .map(|i| i16_at(op, 3 + 2 * i).unwrap_or(0) as i32)
            .collect();
        let tc = 3 + 2 * (n + 1);
        self.cells = (0..n)
            .map(|i| {
                let at = tc + 20 * i;
                let Some(flags) = u16_at(op, at) else {
                    return Cell::default();
                };
                let brc = |k: usize| {
                    Border::from80(op.get(at + 4 + 4 * k..at + 8 + 4 * k).unwrap_or(&[]))
                };
                Cell {
                    width: 0,
                    hmerge: match flags & 3 {
                        1 | 3 => HMerge::First,
                        2 => HMerge::Merged,
                        _ => HMerge::None,
                    },
                    vmerge: match (flags >> 5) & 3 {
                        1 => VMerge::Continue,
                        3 => VMerge::Restart,
                        _ => VMerge::None,
                    },
                    valign: ((flags >> 7) & 3) as u8,
                    borders: [brc(0), brc(1), brc(2), brc(3)],
                    shading: None,
                    padding: [None; 4],
                }
            })
            .collect();
    }
}
