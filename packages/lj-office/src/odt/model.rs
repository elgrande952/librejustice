//! The document an `.odt` describes, every property resolved through its style
//! chain, every character anchored in the part it came from.

use std::sync::Arc;

use crate::Anchor;

/// Points.
pub type Pt = f32;

/// Where one character sits, as an [`Anchor::XmlText`] says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Loc {
    pub part: u16,
    pub node: u32,
    pub offset: u32,
}

impl From<Loc> for Anchor {
    fn from(l: Loc) -> Self {
        Anchor::XmlText {
            part: l.part,
            node: l.node,
            offset: l.offset,
        }
    }
}

/// `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sides<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStyle {
    Solid,
    Double,
    Dotted,
    Dashed,
    DotDash,
    DotDotDash,
    LongDash,
    Wave,
    Groove,
    Ridge,
    Inset,
    Outset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width: Pt,
    pub style: LineStyle,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextLine {
    pub style: LineStyle,
    pub bold: bool,
    /// `None` follows the text colour.
    pub color: Option<Color>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

/// One face of a script: Latin, Asian or complex text each carry their own.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub family: String,
    /// `roman`, `swiss`, `modern`, `decorative`, `script`, `system`.
    pub generic: Option<String>,
    pub fixed_pitch: bool,
    pub size: Pt,
    pub weight: u16,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextProps {
    pub latin: Face,
    pub asian: Face,
    pub complex: Face,
    pub underline: Option<TextLine>,
    pub strike: Option<TextLine>,
    pub overline: Option<TextLine>,
    /// `None` is the automatic colour, black on a light background.
    pub color: Option<Color>,
    pub highlight: Option<Color>,
    pub transform: Transform,
    pub small_caps: bool,
    /// Baseline shift and size, both as a share of the font size: `super`
    /// is `(0.33, 0.58)`.
    pub position: Option<(f32, f32)>,
    pub letter_spacing: Pt,
    /// Horizontal scale, 1.0 for none.
    pub scale: f32,
    pub hidden: bool,
    /// Exported even when hidden: a tracked deletion, the cached text of a
    /// field Writer computes again.
    pub export_hidden: bool,
    pub outline: bool,
    pub shadow: bool,
    pub kerning: bool,
    pub hyphenate: bool,
    pub language: Option<String>,
    pub country: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    /// A multiple of the font's line height.
    Proportional(f32),
    Fixed(Pt),
    AtLeast(Pt),
    /// Extra space added to the font's line height.
    Leading(Pt),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Break {
    None,
    Column,
    Page,
    EvenPage,
    OddPage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Left,
    Center,
    Right,
    Char(char),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabStop {
    /// From the paragraph's left margin.
    pub position: Pt,
    pub kind: TabKind,
    pub leader: Option<char>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropCap {
    pub lines: u8,
    /// Characters, zero for the first word.
    pub length: u16,
    pub distance: Pt,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParaProps {
    pub align: Align,
    pub align_last: Align,
    pub rtl: bool,
    pub margin: Sides<Pt>,
    pub text_indent: Pt,
    pub auto_text_indent: bool,
    pub line_height: LineHeight,
    pub contextual_spacing: bool,
    pub keep_with_next: bool,
    pub keep_together: bool,
    pub widows: u8,
    pub orphans: u8,
    pub break_before: Break,
    pub break_after: Break,
    pub tab_stops: Vec<TabStop>,
    pub default_tab: Pt,
    pub borders: Sides<Option<Border>>,
    pub padding: Sides<Pt>,
    pub background: Option<Color>,
    pub drop_cap: Option<DropCap>,
    pub page_number: Option<u32>,
}

/// How a list label sits before its paragraph.
#[derive(Clone, Debug, PartialEq)]
pub enum LabelPlacement {
    /// ODF 1.2 `label-alignment`: the paragraph's own indents place the label,
    /// followed by a tab to `tab`, a space, or nothing.
    Aligned { follow: LabelFollow },
    /// The older `label-width-and-position`.
    Positioned {
        space_before: Pt,
        min_width: Pt,
        min_distance: Pt,
        align: Align,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LabelFollow {
    Tab(Option<Pt>),
    Space,
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    /// 1 for the outermost list.
    pub level: u8,
    pub bullet: bool,
    pub props: Arc<TextProps>,
    pub placement: LabelPlacement,
    /// An image bullet, as an index into [`Document::images`].
    pub image: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    /// One per character of `text`.
    pub locs: Vec<Loc>,
    pub props: Arc<TextProps>,
    pub link: Option<Arc<str>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    PageNumber,
    PageCount,
    /// Text Writer computes on load, which the file does not hold: `run`
    /// carries it without anchors.
    Computed,
    /// Anything else: the text the producer last computed is shown.
    Cached,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(Run),
    Tab {
        loc: Loc,
        props: Arc<TextProps>,
    },
    LineBreak {
        loc: Loc,
    },
    /// A field; `run` holds its cached text.
    Field {
        kind: FieldKind,
        run: Run,
    },
    /// A note's citation, numbered in document order as a reader numbers it,
    /// the note itself in [`Document::notes`].
    NoteRef {
        note: usize,
        citation: String,
        props: Arc<TextProps>,
    },
    Frame(Box<Frame>),
    /// Where the producer broke the page last time it laid the text out.
    SoftPageBreak,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    /// The paragraph style's name, which contextual spacing compares.
    pub style: Option<String>,
    pub props: Arc<ParaProps>,
    /// The paragraph's own character properties, those of an empty line.
    pub text: Arc<TextProps>,
    pub heading: Option<u8>,
    pub label: Option<Label>,
    /// Starts a page with this page style, an index into [`Document::pages`].
    pub page_style: Option<usize>,
    pub hidden: bool,
    pub inlines: Vec<Inline>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAlign {
    Left,
    Center,
    Right,
    Margins,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableProps {
    pub width: Option<Pt>,
    pub rel_width: Option<f32>,
    pub align: TableAlign,
    pub margin: Sides<Pt>,
    pub break_before: Break,
    pub break_after: Break,
    pub keep_with_next: bool,
    pub may_break_rows: bool,
    pub background: Option<Color>,
    pub rtl: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub width: Option<Pt>,
    /// Relative weight, `style:rel-column-width`.
    pub rel_width: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellProps {
    pub background: Option<Color>,
    pub borders: Sides<Option<Border>>,
    pub padding: Sides<Pt>,
    pub valign: VAlign,
    pub vertical_text: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub col_span: u32,
    pub row_span: u32,
    /// Hidden under a neighbour's span.
    pub covered: bool,
    pub props: CellProps,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub height: Option<Pt>,
    pub min_height: Option<Pt>,
    pub keep_together: bool,
    pub background: Option<Color>,
    pub header: bool,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    pub props: TableProps,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    pub page_style: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    pub count: u16,
    pub gap: Pt,
    /// Relative widths, one per column, when they are not equal.
    pub widths: Vec<f32>,
    /// Start and end indents of each column, which its relative width counts.
    pub indents: Vec<(Pt, Pt)>,
    pub separator: Option<Border>,
}

impl Default for Columns {
    fn default() -> Self {
        Columns {
            count: 1,
            gap: 0.0,
            widths: Vec::new(),
            indents: Vec::new(),
            separator: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    pub columns: Columns,
    pub margin_left: Pt,
    pub margin_right: Pt,
    pub background: Option<Color>,
    pub hidden: bool,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    Section(Section),
    SoftPageBreak,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameAnchor {
    AsChar,
    Char,
    Paragraph,
    Page(u32),
    Frame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    None,
    Left,
    Right,
    Parallel,
    Dynamic,
    RunThrough,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrameContent {
    /// An index into [`Document::images`].
    Image(usize),
    TextBox(Vec<Block>),
    /// An embedded object, drawn from its preview image when it has one.
    Object {
        path: String,
        preview: Option<usize>,
    },
    Chart(Box<super::chart::Chart>),
    /// A drawing shape, with the text it carries.
    Shape {
        kind: String,
        /// A custom shape's `draw:type`.
        geometry: Option<String>,
        path: Option<Box<super::geometry::ShapePath>>,
        blocks: Vec<Block>,
    },
    Group(Vec<Frame>),
    Empty,
}

/// A `draw:gradient`, its colours at their intensity.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub style: GradientStyle,
    /// Of a radial style, as shares of the frame.
    pub center: (f32, f32),
    pub start: Color,
    pub end: Color,
    /// Counter-clockwise, in degrees; at 0 the start colour is at the top.
    pub angle: f32,
    /// The share of the gradient held at the start colour.
    pub border: f32,
    /// Bands of plain colour, 0 for a smooth change.
    pub steps: u32,
}

/// A `draw:hatch`.
#[derive(Clone, Debug, PartialEq)]
pub struct Hatch {
    pub color: Color,
    pub distance: Pt,
    /// Counter-clockwise, in degrees.
    pub angle: f32,
    /// 1, 2 or 3 sets of lines.
    pub lines: u8,
    /// The fill colour under the lines.
    pub back: Option<Color>,
}

/// A `draw:fill-image`: the picture, stretched over the frame or repeated
/// at `tile`, its own size when `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct FillImage {
    pub image: usize,
    pub stretch: bool,
    pub tile: Option<(Pt, Pt)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradientStyle {
    Linear,
    /// The start colour at both edges, the end colour on the middle line.
    Axial,
    /// The start colour outside, the end colour at the centre.
    Radial,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub name: String,
    pub anchor: FrameAnchor,
    pub x: Pt,
    pub y: Pt,
    pub width: Option<Pt>,
    pub height: Option<Pt>,
    pub min_height: Option<Pt>,
    pub rel_width: Option<f32>,
    pub rel_height: Option<f32>,
    pub wrap: Wrap,
    /// A run-through frame painted under the text.
    pub behind_text: bool,
    pub horizontal: (String, String),
    pub vertical: (String, String),
    pub margin: Sides<Pt>,
    pub borders: Sides<Option<Border>>,
    pub padding: Sides<Pt>,
    pub background: Option<Color>,
    /// A drawing shape's fill and outline, a frame's being its background and borders.
    pub fill: Option<Color>,
    /// Painted in place of the fill or the background.
    pub gradient: Option<Gradient>,
    pub hatch: Option<Hatch>,
    pub fill_image: Option<FillImage>,
    /// The fill's opacity changing across the frame, as a gradient whose grey
    /// levels are the opacities.
    pub transparency: Option<Gradient>,
    /// Of the fill or the background, 1 opaque.
    pub opacity: f32,
    pub stroke: Option<(Pt, Color)>,
    pub valign: VAlign,
    /// Clockwise, in degrees, about the frame's centre.
    pub rotation: f32,
    pub mirror: (bool, bool),
    pub z_index: i32,
    pub title: String,
    pub description: String,
    pub link: Option<Arc<str>>,
    pub content: FrameContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteClass {
    Footnote,
    Endnote,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub class: NoteClass,
    /// The mark in the text.
    pub citation: String,
    /// The mark before the note's own text, with the configured prefix and suffix.
    pub label: String,
    pub label_props: Arc<TextProps>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeaderFooter {
    pub min_height: Pt,
    pub height: Option<Pt>,
    /// Between it and the body.
    pub spacing: Pt,
    pub margin_left: Pt,
    pub margin_right: Pt,
    pub dynamic_spacing: bool,
    pub borders: Sides<Option<Border>>,
    pub padding: Sides<Pt>,
    pub background: Option<Color>,
    pub blocks: Vec<Block>,
    /// Left pages, when they differ.
    pub left: Option<Vec<Block>>,
    /// The first page, when it differs.
    pub first: Option<Vec<Block>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumFormat {
    Decimal,
    /// Decimal padded with zeros to this many digits: `001`.
    Padded(u8),
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
    /// Chinese numerals: `一`, `二`... `十一`.
    Chinese,
    /// The ten heavenly stems: `甲`, `乙`...
    Stems,
    /// `①` to `㊿`, decimal past fifty.
    Circled,
    Greek {
        upper: bool,
    },
    /// The Russian alphabet; `upper` capitalises only the first letter of `Аа`.
    Cyrillic {
        upper: bool,
    },
    /// Hebrew numerals: `א`, `ב`... `י`, `יא`, with `טו` and `טז` for 15 and 16.
    Hebrew,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageStyle {
    pub name: String,
    pub width: Pt,
    pub height: Pt,
    pub margin: Sides<Pt>,
    pub landscape: bool,
    pub num_format: NumFormat,
    pub background: Option<Color>,
    pub borders: Sides<Option<Border>>,
    pub padding: Sides<Pt>,
    pub columns: Columns,
    pub rtl: bool,
    /// `all`, `left`, `right` or `mirrored`.
    pub usage: String,
    pub header: Option<HeaderFooter>,
    pub footer: Option<HeaderFooter>,
    pub footnote_max_height: Option<Pt>,
    /// The page style of the page after, an index into [`Document::pages`].
    pub next: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub name: String,
    pub family: String,
    pub generic: Option<String>,
    pub fixed_pitch: bool,
    /// Font files the document embeds that read as a font.
    pub embedded: Vec<Arc<[u8]>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub path: String,
    pub mime: String,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    pub subject: String,
    pub description: String,
    pub creator: String,
    pub initial_creator: String,
    pub keywords: Vec<String>,
    pub language: String,
    pub generator: String,
    pub created: String,
    pub modified: String,
    pub page_count: Option<u32>,
    pub user_defined: Vec<(String, String)>,
}

/// The layout options of `settings.xml`, with the values Writer gives a
/// file that predates them.
#[derive(Clone, Debug, PartialEq)]
pub struct Compat {
    /// A numbered paragraph's own first line indent is dropped.
    pub ignore_first_line_indent_in_numbering: bool,
    /// The space between two paragraphs is the sum of theirs, not the larger.
    pub add_para_table_spacing: bool,
    /// The last paragraph of a cell keeps its space below.
    pub add_para_spacing_to_table_cells: bool,
    /// A justified line may squeeze its spaces to take one more word.
    pub justify_lines_with_shrinking: bool,
    /// Tracked deletions are drawn, struck through, where they were made.
    pub show_changes: bool,
    /// A line's height takes its font's line gap.
    pub add_external_leading: bool,
    /// The endnotes follow the body rather than open a page of their own.
    pub continuous_endnotes: bool,
    /// A paragraph that opens a page, a cell or a frame may keep its space
    /// above.
    pub add_para_table_spacing_at_start: bool,
}

impl Default for Compat {
    fn default() -> Self {
        Self {
            ignore_first_line_indent_in_numbering: false,
            add_para_table_spacing: true,
            add_para_spacing_to_table_cells: true,
            justify_lines_with_shrinking: false,
            show_changes: true,
            add_external_leading: true,
            continuous_endnotes: false,
            add_para_table_spacing_at_start: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub meta: Meta,
    pub compat: Compat,
    pub pages: Vec<PageStyle>,
    /// The page style the document starts with.
    pub first_page: usize,
    pub body: Vec<Block>,
    pub notes: Vec<Note>,
    /// Frames and shapes anchored to a page rather than to the text.
    pub page_frames: Vec<Frame>,
    pub fonts: Vec<FontFace>,
    pub images: Vec<Image>,
}
