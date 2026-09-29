//! The document every reader hands to the layout: sections of blocks, blocks of
//! inlines, and every character anchored in the file it came from, so what is
//! drawn, searched and removed is one enumeration. Lengths are points.

mod chart;
mod drawing;
mod hidden;
mod props;

pub use chart::*;
pub use drawing::*;
pub(crate) use hidden::join_hidden_marks;
pub use props::*;

use std::collections::HashSet;
use std::sync::Arc;

use crate::Anchor;

use crate::numbering::NumFormat;

pub struct Document {
    pub sections: Vec<Section>,
    pub settings: Settings,
    /// The fonts the document declares, which say what an unknown family
    /// falls back to.
    pub fonts: Vec<FontDecl>,
    /// Font files the document carries, known by the names inside them.
    pub embedded_fonts: Vec<Arc<[u8]>>,
}

pub struct Settings {
    /// Distance between the default tab stops.
    pub default_tab_stop: f64,
    /// Even pages take the `even` header and footer.
    pub even_and_odd_headers: bool,
    /// A justified line may squeeze its spaces to take one more word, as
    /// Word 2013 and later lay it out.
    pub justify_shrinks: bool,
    /// A paragraph that breaks the page itself keeps its space before at
    /// the top of the new page, as Word 2010 and earlier lay it out.
    pub break_keeps_spacing: bool,
    /// A page break that ends a paragraph leaves its mark, an empty line, at
    /// the top of the next page.
    pub mark_follows_page_break: bool,
    /// The space after a paragraph and the space before the next add up, as
    /// Writer lays them out; otherwise the larger stands for both.
    pub spacing_adds_up: bool,
    /// A tab that ends a line may run past its right edge rather than wrap,
    /// as Word 2010 and earlier lay it out.
    pub tab_over_margin: bool,
    /// The font of the default paragraph style, whose height is the space
    /// kept above the notes for their separator.
    pub note_separator: RunProps,
    /// The font line numbers are drawn in.
    pub line_number_font: RunProps,
    /// A line's height takes its font's line gap.
    pub external_leading: bool,
    /// The endnotes open a page of their own after the body, as Writer lays
    /// them out; otherwise they follow the body below a separator rule.
    pub endnotes_own_page: bool,
    /// A paragraph that opens a page, a cell, a header or a frame may keep
    /// its space above; otherwise it never does.
    pub top_spacing: bool,
}

pub struct FontDecl {
    pub name: String,
    pub generic: Generic,
}

/// The broad class of a font, which picks the fallback when the family is not
/// installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Generic {
    Swiss,
    Roman,
    Modern,
    Script,
    Decorative,
    /// Declared fixed-pitch, whatever its class.
    Fixed,
    #[default]
    Unknown,
}

pub struct Section {
    pub page: PageSetup,
    /// The setup of the section's first page, when it is not `page`.
    pub first_page: Option<PageSetup>,
    pub start: SectionStart,
    pub columns: Columns,
    pub headers: HeaderFooters,
    pub footers: HeaderFooters,
    /// The first page takes the `first` header and footer.
    pub title_page: bool,
    /// Page number of the section's first page, when it restarts.
    pub page_number_start: Option<u32>,
    /// How the page number and the page count are written on its pages.
    pub(crate) page_number_format: NumFormat,
    /// The pitch of the grid its body lines snap to.
    pub line_grid: Option<f64>,
    pub line_numbers: Option<LineNumbers>,
    /// The space after the empty paragraph that closed the section, which
    /// LibreOffice drops yet still counts against the next page's first.
    pub(crate) dropped_after: Option<f64>,
    /// It names a header or footer of its own: a page break within it, when
    /// it starts on no new page, brings in its page setup.
    pub(crate) own_stories: bool,
    /// Its columns share the text of its last page evenly.
    pub(crate) balanced: bool,
    pub blocks: Vec<Block>,
}

impl Section {
    pub(crate) fn setup(&self, first: bool) -> &PageSetup {
        match &self.first_page {
            Some(page) if first => page,
            _ => &self.page,
        }
    }
}

/// Numbers in the margin beside the lines of a section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineNumbers {
    /// Only the lines whose number is a multiple of it show it.
    pub count_by: u32,
    /// The number of the section's first line, less one.
    pub start: u32,
    pub restart: LineRestart,
    /// From the right edge of the numbers to the text.
    pub distance: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineRestart {
    NewPage,
    NewSection,
    Continuous,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageSetup {
    pub width: f64,
    pub height: f64,
    pub margin: Margins,
    /// Distance from the top edge to the header.
    pub header: f64,
    /// Distance from the bottom edge to the footer.
    pub footer: f64,
    /// The footer's text starts this high above `footer` at least, as in a
    /// box of that height growing upward.
    pub footer_min_height: f64,
    pub gutter: f64,
    /// The colour of the whole sheet, white when `None`.
    pub background: Option<Color>,
    /// Painted over `background`.
    pub texture: Option<Fill>,
    pub borders: Option<Box<PageBorders>>,
}

/// Lines around the page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageBorders {
    pub top: Option<Border>,
    pub right: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    /// From each edge of the page to the middle of its line.
    pub inset: Margins,
    pub on_first: bool,
    pub on_others: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Margins {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SectionStart {
    #[default]
    NextPage,
    Continuous,
    EvenPage,
    OddPage,
    NextColumn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    pub count: usize,
    /// Gap between equal columns.
    pub gap: f64,
    /// A rule is drawn between columns.
    pub separator: bool,
    /// Width and following gap of each column, when they are not equal.
    pub widths: Vec<(f64, f64)>,
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            count: 1,
            gap: 36.0,
            separator: false,
            widths: Vec::new(),
        }
    }
}

/// A section that sets no header of a kind inherits the previous section's,
/// so one story can serve several sections.
#[derive(Clone, Default)]
pub struct HeaderFooters {
    pub default: Option<Arc<Vec<Block>>>,
    pub first: Option<Arc<Vec<Block>>>,
    pub even: Option<Arc<Vec<Block>>>,
}

pub enum Block {
    Paragraph(Box<Paragraph>),
    Table(Box<Table>),
}

pub struct Paragraph {
    pub props: ParagraphProps,
    /// Properties of the paragraph mark, which size an empty paragraph.
    pub mark: RunProps,
    /// Style identifier, which contextual spacing compares.
    pub style: Option<String>,
    /// The list number or bullet drawn before the first line.
    pub label: Option<Label>,
    pub inlines: Vec<Inline>,
    /// In a hidden section: drawn nowhere, its text still exported.
    pub hidden_section: bool,
}

pub struct Label {
    pub text: String,
    pub props: RunProps,
    pub suffix: LabelSuffix,
    /// Left starts the label at the first line's indent, right ends it there.
    pub alignment: Alignment,
    /// A stop the paragraph gains on every line, which the tab after the
    /// label reaches in place of the hanging indent.
    pub tab_stop: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelSuffix {
    Tab,
    Space,
    Nothing,
}

pub enum Inline {
    Text(Text),
    /// A character the markup writes rather than a text node, such as a
    /// symbol or a non-breaking hyphen: it has no anchor.
    Char(char, RunProps),
    Tab(RunProps),
    Break(Break),
    /// A value the layout computes, drawn with these properties.
    Field(Field, RunProps),
    Drawing(Box<Drawing>),
    /// A footnote or endnote reference: its label drawn with these
    /// properties, its text at the foot of the page or after the body.
    Note(Arc<Note>, RunProps),
}

pub struct Note {
    pub kind: NoteKind,
    /// The number or mark the reference and the text start with, empty when
    /// the reference writes its own mark.
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Foot,
    End,
}

pub struct Text {
    pub text: String,
    pub props: RunProps,
    /// One per character of `text`, in order.
    pub anchors: Vec<Anchor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Break {
    Line,
    Page,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Page,
    NumPages,
    SectionPages,
}

pub struct Table {
    pub props: TableProps,
    /// Width of each grid column.
    pub grid: Vec<f64>,
    pub rows: Vec<Row>,
}

pub struct Row {
    /// Grid columns left empty before the first cell.
    pub grid_before: usize,
    pub cells: Vec<Cell>,
    pub height: Option<RowHeight>,
    pub cant_split: bool,
    /// Repeated at the top of each page the table continues on.
    pub header: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowHeight {
    pub value: f64,
    pub exact: bool,
}

pub struct Cell {
    /// Grid columns the cell covers.
    pub span: usize,
    pub vmerge: VMerge,
    pub borders: Borders,
    pub shading: Option<Color>,
    pub margins: Margins,
    pub valign: VAlign,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VMerge {
    #[default]
    None,
    Restart,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

impl Document {
    /// Every character of the body, of the headers and footers, then of the
    /// notes, each with its anchor, hidden and deleted text included since
    /// the file carries it. A newline holding the previous anchor closes each
    /// paragraph, as in the package feed, so no match straddles two
    /// paragraphs.
    pub fn characters(&self) -> Vec<(char, Anchor)> {
        let mut out = Vec::new();
        self.visit_stories(&mut |paragraph| {
            let before = out.len();
            for inline in &paragraph.inlines {
                if let Inline::Text(text) = inline {
                    out.extend(text.text.chars().zip(text.anchors.iter().copied()));
                }
            }
            if out.len() > before {
                let last = out[out.len() - 1].1;
                out.push(('\n', last));
            }
        });
        out
    }

    /// The characters of [`Self::characters`] that are hidden text.
    pub(crate) fn hidden(&self) -> HashSet<Anchor> {
        let mut out = HashSet::new();
        self.visit_stories(&mut |paragraph| {
            for inline in &paragraph.inlines {
                match inline {
                    Inline::Text(text) if text.props.hidden => out.extend(&text.anchors),
                    _ => {}
                }
            }
        });
        out
    }

    /// Each paragraph of the body, of the headers and footers, of the text
    /// boxes a paragraph draws, then of the notes.
    fn visit_stories(&self, visit: &mut dyn FnMut(&Paragraph)) {
        fn collect(paragraph: &Paragraph, visit: &mut dyn FnMut(&Paragraph)) {
            visit(paragraph);
            for inline in &paragraph.inlines {
                if let Inline::Drawing(drawing) = inline {
                    drawing
                        .shape
                        .stories(&mut |story| visit_blocks(story, &mut |p| collect(p, visit)));
                }
            }
        }
        self.visit_paragraphs(&mut |p| collect(p, visit));
        for kind in [NoteKind::Foot, NoteKind::End] {
            for note in self.notes(kind) {
                visit_blocks(&note.blocks, &mut |p| collect(p, visit));
            }
        }
    }

    /// The text LibreOffice exports, one paragraph per line: hidden text goes
    /// except a tracked deletion or a hidden section, the page number fields
    /// go, and of the notes only their references stay. A body of one empty paragraph exports the
    /// text of its first text box instead.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut write = |paragraph: &Paragraph| {
            if let Some(label) = &paragraph.label {
                out.push_str(&label.text);
                out.push(match label.suffix {
                    LabelSuffix::Tab => '\t',
                    _ => ' ',
                });
            }
            let shown = |props: &RunProps| match props.export {
                Export::Shown => !props.hidden || paragraph.hidden_section,
                Export::Always => true,
                Export::Never => false,
            };
            for inline in &paragraph.inlines {
                match inline {
                    Inline::Text(text) if shown(&text.props) => out.push_str(&text.text),
                    Inline::Char(c, props) if shown(props) => out.push(*c),
                    Inline::Note(note, props) if shown(props) => out.push_str(&note.label),
                    Inline::Tab(_) => out.push('\t'),
                    Inline::Break(Break::Line) => out.push('\n'),
                    _ => {}
                }
            }
            out.push('\n');
        };
        match self.lone_text_box() {
            Some(story) => visit_blocks(story, &mut write),
            None => self.visit_paragraphs(&mut write),
        }
        out
    }

    /// The first text box of a body that is one paragraph without text.
    fn lone_text_box(&self) -> Option<&[Block]> {
        let mut blocks = self.sections.iter().flat_map(|s| &s.blocks);
        let (Some(Block::Paragraph(only)), None) = (blocks.next(), blocks.next()) else {
            return None;
        };
        let mut first = None;
        for inline in &only.inlines {
            match inline {
                Inline::Text(text) if !text.text.is_empty() => return None,
                Inline::Drawing(drawing) if first.is_none() => first = drawing.shape.first_story(),
                _ => {}
            }
        }
        first
    }

    /// The notes of `kind` the body references, in reading order.
    pub(crate) fn notes(&self, kind: NoteKind) -> Vec<&Arc<Note>> {
        fn collect<'d>(list: &'d [Block], kind: NoteKind, out: &mut Vec<&'d Arc<Note>>) {
            for block in list {
                match block {
                    Block::Paragraph(p) => out.extend(p.inlines.iter().filter_map(|i| match i {
                        Inline::Note(note, _) if note.kind == kind => Some(note),
                        _ => None,
                    })),
                    Block::Table(t) => {
                        for cell in t.rows.iter().flat_map(|r| &r.cells) {
                            collect(&cell.blocks, kind, out);
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for section in &self.sections {
            collect(&section.blocks, kind, &mut out);
        }
        out
    }

    /// Body paragraphs in reading order, then each header and footer once.
    fn visit_paragraphs(&self, visit: &mut dyn FnMut(&Paragraph)) {
        fn blocks(list: &[Block], visit: &mut dyn FnMut(&Paragraph)) {
            for block in list {
                match block {
                    Block::Paragraph(p) => visit(p),
                    Block::Table(t) => {
                        for row in &t.rows {
                            for cell in &row.cells {
                                blocks(&cell.blocks, visit);
                            }
                        }
                    }
                }
            }
        }
        for section in &self.sections {
            blocks(&section.blocks, visit);
        }
        let mut seen: Vec<*const Vec<Block>> = Vec::new();
        for section in &self.sections {
            for set in [&section.headers, &section.footers] {
                for story in [&set.default, &set.first, &set.even].into_iter().flatten() {
                    if !seen.contains(&Arc::as_ptr(story)) {
                        seen.push(Arc::as_ptr(story));
                        blocks(story, visit);
                    }
                }
            }
        }
    }
}

/// Paragraphs of `list` in reading order, those of its tables' cells included.
pub(crate) fn visit_blocks(list: &[Block], visit: &mut dyn FnMut(&Paragraph)) {
    for block in list {
        match block {
            Block::Paragraph(p) => visit(p),
            Block::Table(t) => {
                for row in &t.rows {
                    for cell in &row.cells {
                        visit_blocks(&cell.blocks, visit);
                    }
                }
            }
        }
    }
}
