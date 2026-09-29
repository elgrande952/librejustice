//! Formatting as the layout reads it: every property resolved, nothing left to
//! look up in a style.

use std::sync::Arc;

use super::{Float, Margins};

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);

impl Color {
    pub const BLACK: Color = Color(0, 0, 0);
    pub const WHITE: Color = Color(255, 255, 255);

    /// Dark enough for automatic text to turn white on it, by LibreOffice's
    /// luminance threshold, lower for its default shape blue.
    pub fn is_dark(self) -> bool {
        let Color(r, g, b) = self;
        let luminance = (u32::from(b) * 29 + u32::from(g) * 151 + u32::from(r) * 76) >> 8;
        luminance
            <= if self == Color(0x72, 0x9f, 0xcf) {
                62
            } else {
                156
            }
    }

    /// Parse `RRGGBB`.
    pub fn hex(value: &str) -> Option<Color> {
        if value.len() != 6 {
            return None;
        }
        let channel = |i: usize| u8::from_str_radix(value.get(i..i + 2)?, 16).ok();
        Some(Color(channel(0)?, channel(2)?, channel(4)?))
    }
}

/// The per mille of foreground a Word shading pattern (`ipat`) shows, the
/// hatches counted as a third, as LibreOffice mixes them into one colour.
pub(crate) fn pattern_share(ipat: u16) -> u32 {
    const SHARES: [u32; 63] = [
        0, 1000, 50, 100, 200, 250, 300, 400, 500, 600, 700, 750, 800, 900, 333, 333, 333, 333,
        333, 333, 333, 333, 333, 333, 333, 333, 500, 500, 500, 500, 500, 500, 500, 500, 500, 25,
        75, 125, 150, 175, 225, 275, 325, 350, 375, 425, 450, 475, 525, 550, 575, 625, 650, 675,
        725, 775, 825, 850, 875, 925, 950, 975, 970,
    ];
    SHARES.get(usize::from(ipat)).copied().unwrap_or(0)
}

/// One channel of `fore` laid over `back` at `share` per mille, truncated.
pub(crate) fn mix(back: u8, fore: u8, share: u32) -> u8 {
    ((u32::from(fore) * share + u32::from(back) * (1000 - share)) / 1000) as u8
}

/// The family a run names for each class of character.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSet {
    /// Characters below U+0080.
    pub ascii: Arc<str>,
    /// Other characters of a Latin, Greek or Cyrillic script.
    pub h_ansi: Arc<str>,
    pub east_asia: Arc<str>,
    /// Complex scripts, and every character of a run marked right-to-left or
    /// complex.
    pub complex: Arc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Underline {
    #[default]
    None,
    Single,
    Words,
    Double,
    Thick,
    Dotted,
    Dashed,
    Wave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strike {
    #[default]
    None,
    Single,
    Double,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VerticalAlign {
    #[default]
    Baseline,
    Superscript,
    Subscript,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunProps {
    pub fonts: FontSet,
    /// Size in points; `size_complex` applies to complex-script characters.
    pub size: f64,
    pub size_complex: f64,
    pub bold: bool,
    pub italic: bool,
    pub bold_complex: bool,
    pub italic_complex: bool,
    pub underline: Underline,
    pub strike: Strike,
    pub caps: bool,
    pub small_caps: bool,
    pub vertical: VerticalAlign,
    /// Baseline raise, negative to lower.
    pub position: f64,
    /// `None` is automatic, black on a light background.
    pub color: Option<Color>,
    pub highlight: Option<Color>,
    pub shading: Option<Color>,
    /// Added after every character.
    pub spacing: f64,
    /// Horizontal scale, 1 is none.
    pub scale: f64,
    /// Pair kerning from the font.
    pub kerning: bool,
    pub hidden: bool,
    /// Whether LibreOffice exports the text.
    pub export: Export,
    /// Every character takes the complex-script font and size.
    pub complex: bool,
    pub rtl: bool,
    /// Spaces and general punctuation take the East Asian font.
    pub east_asian_hint: bool,
}

/// Whether a run's text reaches LibreOffice's text export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Export {
    /// When it is not hidden.
    Shown,
    /// Even hidden: a tracked deletion, the cached text of a field.
    Always,
    /// Never: a value computed on load.
    Never,
}

impl Default for RunProps {
    fn default() -> Self {
        let times: Arc<str> = Arc::from("Times New Roman");
        Self {
            fonts: FontSet {
                ascii: times.clone(),
                h_ansi: times.clone(),
                east_asia: times.clone(),
                complex: times,
            },
            size: 10.0,
            size_complex: 10.0,
            bold: false,
            italic: false,
            bold_complex: false,
            italic_complex: false,
            underline: Underline::None,
            strike: Strike::None,
            caps: false,
            small_caps: false,
            vertical: VerticalAlign::Baseline,
            position: 0.0,
            color: None,
            highlight: None,
            shading: None,
            spacing: 0.0,
            scale: 1.0,
            kerning: false,
            hidden: false,
            export: Export::Shown,
            complex: false,
            rtl: false,
            east_asian_hint: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
    /// Stretched to both edges, the last line left alone.
    Justify,
    /// Stretched to both edges, the last line included.
    Distribute,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Indent {
    pub left: f64,
    pub right: f64,
    /// Offset of the first line from `left`; negative for a hanging indent.
    pub first_line: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineSpacing {
    /// A percentage of the line the fonts ask for, 100 being single.
    Proportional(f64),
    Exact(f64),
    AtLeast(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spacing {
    pub before: f64,
    pub after: f64,
    pub line: LineSpacing,
    /// Word's automatic spacing, which a table cell drops at its start and end.
    pub before_auto: bool,
    pub after_auto: bool,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            before: 0.0,
            after: 0.0,
            line: LineSpacing::Proportional(100.0),
            before_auto: false,
            after_auto: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Left,
    Center,
    Right,
    Decimal,
    /// Draws a rule and moves nothing.
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Leader {
    #[default]
    None,
    Dot,
    Hyphen,
    Underscore,
    MiddleDot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabStop {
    /// From the left edge of the text column.
    pub position: f64,
    pub kind: TabKind,
    pub leader: Leader,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width: f64,
    pub color: Option<Color>,
    /// Gap between the border and the content.
    pub space: f64,
    pub double: bool,
}

impl Border {
    /// Room the rule takes: LibreOffice gives a double rule three lines.
    pub fn weight(&self) -> f64 {
        if self.double {
            3.0 * self.width
        } else {
            self.width
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Borders {
    pub top: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    pub right: Option<Border>,
    /// Between paragraphs of one bordered group, or between cells.
    pub between: Option<Border>,
    /// Between the columns of a table.
    pub vertical: Option<Border>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ParagraphProps {
    pub alignment: Alignment,
    pub indent: Indent,
    pub spacing: Spacing,
    /// No spacing against a neighbour of the same style.
    pub contextual_spacing: bool,
    pub keep_next: bool,
    pub keep_lines: bool,
    pub widow_control: bool,
    pub page_break_before: bool,
    /// The paragraph starts the next column, and the next page past the last.
    pub column_break_before: bool,
    /// Sorted by position.
    pub tabs: Vec<TabStop>,
    pub borders: Borders,
    pub shading: Option<Color>,
    pub rtl: bool,
    /// The lines keep their own height on a page whose lines snap to a grid.
    pub off_grid: bool,
    /// The lines take no number when the section numbers its lines.
    pub suppress_line_numbers: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TableAlignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TableWidth {
    #[default]
    Auto,
    Points(f64),
    /// A fraction of the text column.
    Fraction(f64),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TableProps {
    pub width: TableWidth,
    pub alignment: TableAlignment,
    /// Offset of the table from the left edge of the text column.
    pub indent: f64,
    /// The rules a cell takes on the sides it leaves unset.
    pub borders: Borders,
    /// Default margins of every cell.
    pub cell_margins: Margins,
    /// Gap between cells.
    pub cell_spacing: f64,
    /// Set apart from the text, which runs around it.
    pub float: Option<Box<Float>>,
    /// Opens a new page.
    pub page_break_before: bool,
}
