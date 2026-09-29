//! Blocks poured into columns and pages: paragraph spacing, widows and
//! orphans, kept paragraphs, breaks, sections, headers and footers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use super::float::{self, Area, Exclusion, Floats, PageFloats, Placed};
use super::metrics::font_box;
use super::text::{self, Line};
use super::{Env, Fields, Glyphs, Item, Outline, Page};
use crate::model::*;
use crate::numbering::NumFormat;

pub(super) const EPSILON: f64 = 1e-6;

#[derive(Clone, Copy, Debug)]
pub(super) struct Column {
    pub x: f64,
    pub width: f64,
}

/// What the spacing before a paragraph collapses with.
#[derive(Clone, Copy)]
pub(super) struct Above<'d> {
    pub after: f64,
    pub style: Option<&'d str>,
    pub contextual: bool,
    pub borders: Option<&'d Borders>,
}

impl<'d> Above<'d> {
    fn of(paragraph: &'d Paragraph) -> Self {
        Self {
            after: paragraph.props.spacing.after,
            style: paragraph.style.as_deref(),
            contextual: paragraph.props.contextual_spacing,
            borders: Some(&paragraph.props.borders),
        }
    }
}

/// The space after `above` and before `paragraph`, either dropped against a
/// neighbour of the same style when it asks for contextual spacing.
fn spacing(above: Above<'_>, paragraph: &Paragraph) -> (f64, f64) {
    let same = above.style == paragraph.style.as_deref();
    let after = if above.contextual && same {
        0.0
    } else {
        above.after
    };
    let before = if paragraph.props.contextual_spacing && same {
        0.0
    } else {
        paragraph.props.spacing.before
    };
    (after, before)
}

/// The space above `paragraph`: the larger of its own and of the paragraph
/// above's. At the top of a column reached by a break, none; at the top of a
/// page that keeps it, the paragraph broken from still takes its space after
/// out of it.
fn gap(
    above: Option<Above<'_>>,
    top_keeps_spacing: bool,
    broken: Option<Above<'_>>,
    paragraph: &Paragraph,
    adds_up: bool,
) -> f64 {
    match above {
        None if top_keeps_spacing => match broken {
            Some(broken) => {
                let (after, before) = spacing(broken, paragraph);
                (before - after).max(0.0)
            }
            None => paragraph.props.spacing.before,
        },
        None => 0.0,
        Some(above) => {
            let (after, before) = spacing(above, paragraph);
            if adds_up {
                after + before
            } else {
                after.max(before)
            }
        }
    }
}

/// A block of a hidden section, whose text is exported and takes no room.
fn in_hidden_section(block: &Block) -> bool {
    match block {
        Block::Paragraph(p) => p.hidden_section,
        Block::Table(t) => {
            let mut blocks = t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks);
            blocks.next().is_some_and(in_hidden_section) && blocks.all(in_hidden_section)
        }
    }
}

fn height(lines: &[Line]) -> f64 {
    lines.iter().map(|l| l.bounds.height).sum()
}

/// How many of `lines` fit in `room`, the last one's spacing below its text
/// allowed to hang past it.
fn fitting(lines: &[Line], room: f64) -> usize {
    let mut used = 0.0;
    lines
        .iter()
        .take_while(|l| {
            used += l.bounds.height;
            used - l.hang <= room + EPSILON
        })
        .count()
}

/// Lines to leave on this page when `fit` of `count` fit, with at least two
/// on either side of the break when widows and orphans are controlled.
pub(super) fn widow_orphan(count: usize, fit: usize, controlled: bool) -> usize {
    if !controlled || fit >= count {
        return fit;
    }
    let mut fit = fit;
    if count - fit < 2 {
        fit = count.saturating_sub(2);
    }
    if fit < 2 {
        fit = 0;
    }
    fit
}

#[derive(Clone)]
struct Sheet<'d> {
    width: f64,
    height: f64,
    items: Vec<Item>,
    section: &'d Section,
    page: &'d PageSetup,
    section_index: usize,
    number: u32,
    /// The first page of its section.
    first: bool,
    /// A page inserted to give the next one its parity: counted, never
    /// shown, as LibreOffice exports it.
    blank: bool,
    /// The footnotes set at its foot, in order, above `limit`.
    notes: Vec<NoteChunk>,
    limit: f64,
}

/// A footnote or the part of one a page holds: its items from its top left
/// corner, its height, and the line bottoms it may be cut at.
#[derive(Clone)]
struct NoteChunk {
    items: Vec<Item>,
    height: f64,
    cuts: Vec<f64>,
}

impl NoteChunk {
    fn first_cut(&self) -> f64 {
        self.cuts
            .iter()
            .copied()
            .find(|c| *c > EPSILON)
            .unwrap_or(self.height)
    }

    /// The part within `room`, cut below a line, and the rest.
    fn split(self, room: f64) -> (Option<NoteChunk>, Option<NoteChunk>) {
        if self.height <= room + EPSILON {
            return (Some(self), None);
        }
        let Some(cut) = self
            .cuts
            .iter()
            .copied()
            .rfind(|c| *c > EPSILON && *c <= room + EPSILON)
        else {
            return (None, Some(self));
        };
        let (above, below): (Vec<Item>, Vec<Item>) =
            self.items.into_iter().partition(|i| top(i) < cut);
        let rest = NoteChunk {
            items: below
                .into_iter()
                .map(|mut i| {
                    i.translate(0.0, -cut);
                    i
                })
                .collect(),
            height: self.height - cut,
            cuts: self
                .cuts
                .iter()
                .filter(|c| **c > cut + EPSILON)
                .map(|c| c - cut)
                .collect(),
        };
        let kept = NoteChunk {
            items: above,
            height: cut,
            cuts: self.cuts.into_iter().filter(|c| *c <= cut).collect(),
        };
        (Some(kept), Some(rest))
    }
}

fn top(item: &Item) -> f64 {
    match item {
        Item::Glyphs(g) => g.y,
        Item::Rect { y, .. } => *y,
        Item::Line { from, to, .. } => from[1].min(to[1]),
        Item::Image(image) => image.y,
        Item::Shape(shape) => shape.y,
        Item::Polygon { points, .. } => points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min),
        Item::Path(path) => path
            .rings
            .iter()
            .flatten()
            .map(|p| p[1])
            .fold(f64::INFINITY, f64::min),
        Item::Art(art) => art.frame[1],
    }
}

/// The height of the default paragraph font, which LibreOffice keeps above
/// the notes of a Word document, the separator rule at three fifths of it.
fn separator(env: &Env<'_>) -> f64 {
    let (family, size, bold, italic) = text::slot('x', &env.settings.note_separator);
    let face = env.resolve(family, bold, italic);
    font_box(
        &env.fonts.face(face.face).metrics,
        size,
        env.settings.external_leading,
    )
    .height
}

/// The lines of a page's border, a double one as two lines a width apart.
fn page_border(b: &PageBorders, width: f64, height: f64, out: &mut Vec<Item>) {
    let m = b.inset;
    let (left, right, top, bottom) = (m.left, width - m.right, m.top, height - m.bottom);
    for (border, from, to, across) in [
        (&b.top, [left, top], [right, top], [0.0, 1.0]),
        (&b.bottom, [left, bottom], [right, bottom], [0.0, 1.0]),
        (&b.left, [left, top], [left, bottom], [1.0, 0.0]),
        (&b.right, [right, top], [right, bottom], [1.0, 0.0]),
    ] {
        let Some(border) = border else {
            continue;
        };
        let color = border.color.unwrap_or(Color::BLACK);
        let offsets: &[f64] = if border.double { &[-1.0, 1.0] } else { &[0.0] };
        for k in offsets {
            let d = k * border.width;
            out.push(Item::Line {
                from: [from[0] + across[0] * d, from[1] + across[1] * d],
                to: [to[0] + across[0] * d, to[1] + across[1] * d],
                width: border.width,
                color,
            });
        }
    }
}

/// Word's separator rule: two inches long, half a point thick.
fn rule(x: f64, y: f64, width: f64) -> Item {
    Item::Rect {
        x,
        y,
        width: width.min(144.0),
        height: 0.5,
        color: Color::BLACK,
    }
}

pub(super) struct Flow<'e, 'a, 'd> {
    pub env: &'e Env<'a>,
    pub fields: Fields,
    total: Option<u32>,
    even_and_odd: bool,
    /// Pages, the last one being filled. Empty for a stack, which never
    /// breaks and writes into `items`.
    sheets: Vec<Sheet<'d>>,
    items: Vec<Item>,
    columns: Vec<Column>,
    column: usize,
    band_top: f64,
    bottom: f64,
    pub y: f64,
    /// Nothing is placed in this column yet.
    fresh: bool,
    /// The top of this column keeps the spacing before its first paragraph:
    /// the start of the document or of a section.
    keep_before: bool,
    above: Option<Above<'d>>,
    /// A break that ended the previous paragraph, taken before the next.
    pending: Option<Break>,
    /// Floating objects by page, a stack's under index 0.
    floats: Floats,
    /// The page a stack is set on, and where its top left corner lands.
    frame: Option<(&'d PageSetup, [f64; 2])>,
    /// The section being poured, which a continuous section changes without
    /// changing the page's.
    current: Option<(&'d Section, usize)>,
    /// The section whose page setup, headers and footers the pages take:
    /// the last to start a page, or to break one while naming headers or
    /// footers of its own, as LibreOffice keeps one page style across a
    /// continuous break.
    page_owner: Option<&'d Section>,
    /// The bottom of the body when no footnote takes room below it.
    limit: f64,
    /// Height the footnotes of the page take, their separator included.
    noted: f64,
    /// What did not fit of the footnotes cut at the foot of the page, set
    /// first on the next.
    carry: Vec<NoteChunk>,
    /// Line bottoms, recorded when the flow sets a note.
    cuts: Option<Vec<f64>>,
    separator: f64,
    /// The pitch of the grid the body lines of the section snap to.
    grid: Option<f64>,
    /// Notes as set at the width of the page, by address.
    set_notes: RefCell<HashMap<*const Note, NoteChunk>>,
    /// The last paragraph before the page or column break.
    broken: Option<Above<'d>>,
    /// The body lines placed so far, beside which line numbers stand.
    numbered: Vec<NumberedLine>,
    /// The number the next body line restarts at, set by its section.
    line_restart: Option<u32>,
    /// The lowest a column of the page ended at before the next.
    deepest: f64,
    /// The bottom the columns of the page with this index stop at.
    cap: Option<(usize, f64)>,
}

/// What pouring blocks changes of a flow, to pour them again.
struct Saved<'d> {
    fields: Fields,
    sheets: usize,
    sheet: Option<Sheet<'d>>,
    floats: usize,
    page_floats: Option<PageFloats>,
    numbered: usize,
    columns: Vec<Column>,
    column: usize,
    band_top: f64,
    bottom: f64,
    y: f64,
    fresh: bool,
    keep_before: bool,
    above: Option<Above<'d>>,
    pending: Option<Break>,
    current: Option<(&'d Section, usize)>,
    page_owner: Option<&'d Section>,
    limit: f64,
    noted: f64,
    carry: Vec<NoteChunk>,
    cuts: Option<Vec<f64>>,
    grid: Option<f64>,
    broken: Option<Above<'d>>,
    line_restart: Option<u32>,
}

/// A body line, which the section's line numbering may count and number.
struct NumberedLine {
    sheet: usize,
    /// The left edge of its column.
    x: f64,
    baseline: f64,
    counted: bool,
    restart: Option<u32>,
}

/// From the right edge of the line numbers to the text, when the document
/// sets none: LibreOffice's half centimetre.
const LINE_NUMBER_DISTANCE: f64 = 14.17;

/// The header or footer a page shows.
fn story<'d>(
    set: &'d HeaderFooters,
    section: &Section,
    first: bool,
    number: u32,
    even_and_odd: bool,
) -> Option<&'d Arc<Vec<Block>>> {
    if first && section.title_page {
        return set.first.as_ref();
    }
    if even_and_odd && number.is_multiple_of(2) {
        return set.even.as_ref();
    }
    set.default.as_ref()
}

/// Text columns a section may set; LibreOffice's dialog offers 20.
const MAX_COLUMNS: usize = 99;

fn columns(section: &Section, x: f64, width: f64) -> Vec<Column> {
    let c = &section.columns;
    if !c.widths.is_empty() {
        let mut at = x;
        return c
            .widths
            .iter()
            .take(MAX_COLUMNS)
            .map(|(w, gap)| {
                let column = Column { x: at, width: *w };
                at += w + gap;
                column
            })
            .collect();
    }
    let count = c.count.clamp(1, MAX_COLUMNS);
    let each = (width - c.gap * (count - 1) as f64) / count as f64;
    (0..count)
        .map(|k| Column {
            x: x + k as f64 * (each + c.gap),
            width: each,
        })
        .collect()
}

/// The text column of a page: its left edge and width.
fn body(page: &PageSetup) -> (f64, f64) {
    let x = page.margin.left + page.gutter;
    (x, page.width - x - page.margin.right)
}

/// Set `blocks` in a column `width` wide that never breaks, as a cell, a
/// header or a footer is: the items from the top left corner, and the height.
pub(super) fn stack(
    env: &Env<'_>,
    blocks: &[Block],
    width: f64,
    fields: Fields,
) -> (Vec<Item>, f64) {
    let (items, height, mut floats) = stack_on(env, blocks, width, fields, None);
    let mut all = float::layer(&mut floats, true);
    all.extend(items);
    all.extend(float::layer(&mut floats, false));
    (all, height)
}

/// A stack, and where a page may cut it: after any line, as LibreOffice drops
/// widows, orphans and kept lines in a table row it splits.
pub(super) struct Stacked {
    pub items: Vec<Item>,
    pub height: f64,
    pub cuts: Vec<f64>,
}

pub(super) fn stacked(env: &Env<'_>, blocks: &[Block], width: f64, fields: Fields) -> Stacked {
    let mut flow = stacking(env, width, fields, None);
    flow.cuts = Some(Vec::new());
    flow.blocks(blocks);
    let height = flow.y + flow.above.map_or(0.0, |a| a.after);
    let mut floats = std::mem::take(&mut flow.floats.page(0).placed);
    let mut items = float::layer(&mut floats, true);
    items.append(&mut flow.items);
    items.extend(float::layer(&mut floats, false));
    Stacked {
        items,
        height,
        cuts: flow.cuts.unwrap_or_default(),
    }
}

/// [`stack`] on a page, its top left corner at `origin`: the floating
/// objects apart, in page coordinates.
fn stack_on<'d>(
    env: &Env<'_>,
    blocks: &'d [Block],
    width: f64,
    fields: Fields,
    frame: Option<(&'d PageSetup, [f64; 2])>,
) -> (Vec<Item>, f64, Vec<Placed>) {
    let mut flow = stacking(env, width, fields, frame);
    flow.blocks(blocks);
    let height = flow.y + flow.above.map_or(0.0, |a| a.after);
    let mut floats = std::mem::take(&mut flow.floats.page(0).placed);
    if frame.is_none() {
        return (flow.items, height, floats);
    }
    let [dx, dy] = frame.map_or([0.0, 0.0], |f| f.1);
    for placed in &mut floats {
        for item in &mut placed.items {
            item.translate(dx, dy);
        }
    }
    (flow.items, height, floats)
}

/// A flow for a column `width` wide that never breaks.
fn stacking<'e, 'a, 'd>(
    env: &'e Env<'a>,
    width: f64,
    fields: Fields,
    frame: Option<(&'d PageSetup, [f64; 2])>,
) -> Flow<'e, 'a, 'd> {
    Flow {
        env,
        fields,
        total: None,
        even_and_odd: false,
        sheets: Vec::new(),
        items: Vec::new(),
        columns: vec![Column { x: 0.0, width }],
        column: 0,
        band_top: 0.0,
        bottom: f64::INFINITY,
        y: 0.0,
        fresh: true,
        keep_before: true,
        above: None,
        pending: None,
        floats: Floats::default(),
        frame,
        current: None,
        page_owner: None,
        limit: f64::INFINITY,
        noted: 0.0,
        carry: Vec::new(),
        cuts: None,
        separator: 0.0,
        grid: None,
        set_notes: RefCell::default(),
        broken: None,
        numbered: Vec::new(),
        line_restart: None,
        deepest: 0.0,
        cap: None,
    }
}

pub(super) fn paginate(document: &Document, env: &Env<'_>, total: Option<u32>) -> Vec<Page> {
    let mut flow = Flow {
        env,
        fields: Fields {
            page: 1,
            pages: total.unwrap_or(1),
            section_pages: 1,
            format: NumFormat::DECIMAL,
        },
        total,
        even_and_odd: document.settings.even_and_odd_headers,
        sheets: Vec::new(),
        items: Vec::new(),
        columns: Vec::new(),
        column: 0,
        band_top: 0.0,
        bottom: 0.0,
        y: 0.0,
        fresh: true,
        keep_before: true,
        above: None,
        pending: None,
        floats: Floats::default(),
        frame: None,
        current: None,
        page_owner: None,
        limit: 0.0,
        noted: 0.0,
        carry: Vec::new(),
        cuts: None,
        separator: separator(env),
        grid: None,
        set_notes: RefCell::default(),
        broken: None,
        numbered: Vec::new(),
        line_restart: None,
        deepest: 0.0,
        cap: None,
    };
    // LibreOffice numbers the lines of the whole document as the first
    // section to number them says, and restarts where a later one asks to.
    let mut numbering: Option<LineNumbers> = None;
    for (index, section) in document.sections.iter().enumerate() {
        env.backdrop.set(section.page.background);
        flow.section(document, index, section);
        if let Some(n) = section.line_numbers {
            if n.start > 0 || (numbering.is_some() && n.restart == LineRestart::NewSection) {
                flow.line_restart = Some(n.start + 1);
            }
            numbering.get_or_insert(n);
        }
        if section.balanced {
            flow.balanced(&section.blocks);
        } else {
            flow.blocks(&section.blocks);
        }
    }
    flow.endnotes(&document.notes(NoteKind::End));
    while !flow.carry.is_empty() {
        let Some((section, index)) = flow.current else {
            break;
        };
        flow.new_page(section, index, false);
    }
    flow.finish(numbering)
}

impl<'e, 'a, 'd> Flow<'e, 'a, 'd> {
    pub(super) fn paged(&self) -> bool {
        !self.sheets.is_empty()
    }

    /// Height left in this column.
    pub(super) fn room(&self) -> f64 {
        self.bottom - self.y
    }

    /// Height of an empty column.
    pub(super) fn column_height(&self) -> f64 {
        self.bottom - self.band_top
    }

    pub(super) fn column_top(&self) -> f64 {
        self.band_top
    }

    /// What a floating table set here is placed against: the area of the
    /// paragraph after it, which starts below the spacing of the one above.
    pub(super) fn table_area(&self) -> Option<Area<'d>> {
        let page = self.sheets.last()?.page;
        let top = self.y + self.above.map_or(0.0, |a| a.after);
        Some(Area {
            page,
            column: self.column(),
            paragraph: top,
            line: top,
            odd: self.fields.page % 2 == 1,
        })
    }

    /// Set `items` over the text of this page, which keeps out of `exclusion`.
    pub(super) fn float_items(&mut self, items: Vec<Item>, exclusion: Exclusion) {
        let floats = self.floats.page(self.sheets.len().saturating_sub(1));
        floats.placed.push(Placed {
            z: 0,
            behind: false,
            items,
        });
        floats.exclusions.push(exclusion);
    }

    fn out(&mut self) -> &mut Vec<Item> {
        match self.sheets.last_mut() {
            Some(sheet) => &mut sheet.items,
            None => &mut self.items,
        }
    }

    pub(super) fn column(&self) -> Column {
        self.columns[self.column]
    }

    fn section(&mut self, document: &'d Document, index: usize, section: &'d Section) {
        let previous = index.checked_sub(1).map(|i| &document.sections[i]);
        // Sizes a few twips apart, as rounding leaves them, are one size.
        let near = |a: f64, b: f64| (a - b).abs() < 1.0;
        let same_page = previous.is_some_and(|p| {
            near(p.page.width, section.page.width) && near(p.page.height, section.page.height)
        });
        if same_page && section.start == SectionStart::Continuous {
            let (x, width) = body(&section.page);
            self.columns = columns(section, x, width);
            self.column = 0;
            self.band_top = self.y;
            self.current = Some((section, index));
            self.grid = section.line_grid;
            return;
        }
        // A page that only hidden sections filled is the new section's.
        if self.fresh
            && self.carry.is_empty()
            && self
                .sheets
                .last()
                .is_some_and(|s| !s.blank && s.items.is_empty() && s.notes.is_empty())
        {
            self.sheets.pop();
        }
        let last = self.sheets.last().map(|s| s.number);
        let number = section
            .page_number_start
            .unwrap_or_else(|| last.map_or(1, |n| n + 1));
        let odd = number % 2 == 1;
        // A restarted number keeps its parity: LibreOffice puts an odd page
        // on the right, and inserts a blank one between two right pages.
        let wrong = match section.start {
            SectionStart::OddPage => !odd,
            SectionStart::EvenPage => odd,
            _ => false,
        } || (section.page_number_start.is_some()
            && last.is_some_and(|n| n % 2 == number % 2));
        if wrong {
            self.blank_page(section, index);
        }
        self.new_page(section, index, true);
        self.keep_before = true;
        if let (Some(after), Some(broken)) =
            (previous.and_then(|p| p.dropped_after), self.broken.as_mut())
        {
            broken.after = after;
        }
    }

    fn blank_page(&mut self, section: &'d Section, index: usize) {
        self.sheets.push(Sheet {
            width: section.page.width,
            height: section.page.height,
            items: Vec::new(),
            section,
            page: &section.page,
            section_index: index,
            number: self.sheets.last().map_or(1, |s| s.number + 1),
            first: false,
            blank: true,
            notes: Vec::new(),
            limit: 0.0,
        });
    }

    fn new_page(&mut self, section: &'d Section, index: usize, first: bool) {
        let number = match (first, section.page_number_start) {
            (true, Some(start)) => start,
            _ => self.sheets.last().map_or(1, |s| s.number + 1),
        };
        let owner = match self.page_owner {
            Some(owner) if !first => owner,
            _ => section,
        };
        self.page_owner = Some(owner);
        let page = owner.setup(first);
        let (x, width) = body(page);
        let fields = Fields {
            page: number,
            pages: self.total.unwrap_or(number),
            section_pages: 1,
            format: owner.page_number_format,
        };
        let measure = |set: &'d HeaderFooters| {
            story(set, owner, first, number, self.even_and_odd)
                .map(|blocks| stack(self.env, blocks, width, fields).1)
        };
        let top = match measure(&owner.headers) {
            Some(header) => page.margin.top.max(page.header + header),
            None => page.margin.top,
        };
        let bottom = match measure(&owner.footers) {
            Some(footer) => {
                (page.height - page.margin.bottom).min(page.height - page.footer - footer)
            }
            None => page.height - page.margin.bottom,
        };
        self.sheets.push(Sheet {
            width: page.width,
            height: page.height,
            items: Vec::new(),
            section: owner,
            page,
            section_index: index,
            number,
            first,
            blank: false,
            notes: Vec::new(),
            limit: bottom,
        });
        self.current = Some((section, index));
        self.grid = section.line_grid;
        self.fields.page = number;
        self.fields.format = owner.page_number_format;
        self.columns = match std::ptr::eq(owner, section) {
            true => columns(section, x, width),
            false => {
                let (x, width) = body(&section.page);
                columns(section, x, width)
            }
        };
        self.column = 0;
        self.band_top = top;
        self.bottom = match self.cap {
            Some((page, cap)) if page == self.sheets.len() => bottom.min(cap),
            _ => bottom,
        };
        self.y = top;
        self.deepest = top;
        self.fresh = true;
        self.keep_before = false;
        self.broken = self.above.take();
        self.limit = bottom;
        self.noted = 0.0;
        for chunk in std::mem::take(&mut self.carry) {
            self.add_note(chunk);
        }
    }

    pub(super) fn next_column(&mut self) {
        let Some((section, index)) = self.current else {
            return;
        };
        if self.column + 1 < self.columns.len() {
            self.deepest = self.deepest.max(self.y);
            self.column += 1;
            self.y = self.band_top;
            self.fresh = true;
            self.keep_before = false;
            self.broken = self.above.take();
        } else {
            self.new_page(section, index, false);
        }
    }

    /// Lays out `blocks`, and when they end on the page they start on, lays
    /// them out again in the shortest columns that hold them, as LibreOffice
    /// balances a section before a continuous one.
    fn balanced(&mut self, blocks: &'d [Block]) {
        if !self.paged() || self.columns.len() < 2 {
            return self.blocks(blocks);
        }
        let saved = self.save();
        self.blocks(blocks);
        let pages = self.sheets.len();
        let (top, bottom) = if pages == saved.sheets {
            (saved.band_top, saved.bottom)
        } else {
            (self.band_top, self.bottom)
        };
        let pour = |flow: &mut Self, height: f64| {
            flow.restore(&saved);
            flow.cap = Some((pages, top + height));
            if pages == saved.sheets {
                flow.bottom = top + height;
            }
            flow.deepest = top;
            flow.blocks(blocks);
            flow.cap = None;
            flow.sheets.len() == pages
        };
        let (mut low, mut high) = (0.0, bottom - top);
        while high - low > 0.5 {
            let mid = (low + high) / 2.0;
            if pour(self, mid) {
                high = mid;
            } else {
                low = mid;
            }
        }
        pour(self, high);
        self.y = self.deepest.max(self.y);
        self.bottom = bottom;
    }

    fn save(&self) -> Saved<'d> {
        Saved {
            fields: self.fields,
            sheets: self.sheets.len(),
            sheet: self.sheets.last().cloned(),
            floats: self.floats.pages.len(),
            page_floats: self
                .floats
                .pages
                .get(self.sheets.len().wrapping_sub(1))
                .cloned(),
            numbered: self.numbered.len(),
            columns: self.columns.clone(),
            column: self.column,
            band_top: self.band_top,
            bottom: self.bottom,
            y: self.y,
            fresh: self.fresh,
            keep_before: self.keep_before,
            above: self.above,
            pending: self.pending,
            current: self.current,
            page_owner: self.page_owner,
            limit: self.limit,
            noted: self.noted,
            carry: self.carry.clone(),
            cuts: self.cuts.clone(),
            grid: self.grid,
            broken: self.broken,
            line_restart: self.line_restart,
        }
    }

    fn restore(&mut self, saved: &Saved<'d>) {
        self.fields = saved.fields;
        self.sheets.truncate(saved.sheets);
        if let (Some(last), Some(sheet)) = (self.sheets.last_mut(), &saved.sheet) {
            *last = sheet.clone();
        }
        self.floats.pages.truncate(saved.floats);
        if let (Some(page), Some(floats)) = (
            self.floats.pages.get_mut(saved.sheets.wrapping_sub(1)),
            &saved.page_floats,
        ) {
            *page = floats.clone();
        }
        self.numbered.truncate(saved.numbered);
        self.columns.clone_from(&saved.columns);
        self.column = saved.column;
        self.band_top = saved.band_top;
        self.bottom = saved.bottom;
        self.y = saved.y;
        self.fresh = saved.fresh;
        self.keep_before = saved.keep_before;
        self.above = saved.above;
        self.pending = saved.pending;
        self.current = saved.current;
        self.page_owner = saved.page_owner;
        self.limit = saved.limit;
        self.noted = saved.noted;
        self.carry.clone_from(&saved.carry);
        self.cuts.clone_from(&saved.cuts);
        self.grid = saved.grid;
        self.broken = saved.broken;
        self.line_restart = saved.line_restart;
    }

    fn advance(&mut self, kind: Break) {
        match kind {
            Break::Column => self.next_column(),
            _ => {
                if let Some((section, index)) = self.current {
                    if section.own_stories {
                        self.page_owner = Some(section);
                    }
                    self.new_page(section, index, false);
                }
            }
        }
    }

    pub(super) fn blocks(&mut self, blocks: &'d [Block]) {
        for (i, block) in blocks.iter().enumerate() {
            if in_hidden_section(block) {
                continue;
            }
            match block {
                Block::Paragraph(p) => {
                    let continues_chain =
                        i > 0 && matches!(&blocks[i - 1], Block::Paragraph(q) if q.props.keep_next);
                    if p.props.keep_next && !continues_chain {
                        self.keep_chain(&blocks[i..]);
                    }
                    self.paragraph(p, blocks.get(i + 1));
                }
                Block::Table(t) => {
                    let own = t.props.page_break_before.then_some(Break::Page);
                    if let Some(kind) = own.or(self.pending.take()) {
                        if !self.fresh {
                            self.advance(kind);
                        }
                    }
                    self.table(t)
                }
            }
        }
    }

    /// Break before a run of paragraphs kept with the next when the run and
    /// the first lines of the paragraph it leads to do not fit below.
    fn keep_chain(&mut self, blocks: &'d [Block]) {
        if !self.paged() {
            return;
        }
        let width = self.column().width;
        let mut above = self.above;
        let mut needed = 0.0;
        let mut keep_before = !self.fresh || self.keep_before && self.env.settings.top_spacing;
        for block in blocks {
            let p = match block {
                Block::Paragraph(p) => p,
                Block::Table(t) => {
                    needed += above.map_or(0.0, |a| a.after) + self.lead(t);
                    break;
                }
            };
            let lines = text::set(self.env, p, &p.inlines, true, width, self.fields, self.grid);
            needed += gap(
                above,
                keep_before,
                self.broken,
                p,
                self.env.settings.spacing_adds_up,
            );
            keep_before = true;
            if !p.props.keep_next {
                let count = if p.props.keep_lines || (p.props.widow_control && lines.len() < 4) {
                    lines.len()
                } else if p.props.widow_control {
                    2
                } else {
                    1
                };
                needed += height(&lines[..count.min(lines.len())]);
                break;
            }
            needed += height(&lines);
            above = Some(Above::of(p));
        }
        let page = self.bottom - self.band_top;
        if !self.fresh && self.y + needed > self.bottom + EPSILON && needed <= page + EPSILON {
            self.next_column();
        }
    }

    fn paragraph(&mut self, paragraph: &'d Paragraph, next: Option<&'d Block>) {
        let settings = self.env.settings;
        let mut parts = split(&paragraph.inlines);
        let mut own = if paragraph.props.page_break_before {
            Some(Break::Page)
        } else {
            paragraph.props.column_break_before.then_some(Break::Column)
        };
        let opens = parts.len() > 1
            && !shows_anything(parts[0].0)
            && (settings.mark_follows_page_break || parts[1..].iter().any(|p| shows_anything(p.0)));
        if opens {
            parts.remove(0);
            own = own.or(parts[0].1);
        }
        let count = parts.len();
        for (k, (inlines, part_break)) in parts.into_iter().enumerate() {
            let mark_follows =
                part_break == Some(Break::Column) || settings.mark_follows_page_break;
            if k > 0 && k + 1 == count && !shows_anything(inlines) && !mark_follows {
                self.pending = part_break;
                break;
            }
            let pending = self.pending.take();
            let before = if k == 0 { own.or(pending) } else { part_break };
            if let Some(kind) = before {
                if !self.fresh {
                    self.advance(kind);
                }
            }
            if k == 0 && own.is_some() && settings.break_keeps_spacing {
                self.keep_before = true;
            }
            self.place(paragraph, inlines, k == 0, next);
        }
    }

    fn place(
        &mut self,
        paragraph: &'d Paragraph,
        inlines: &'d [Inline],
        first_part: bool,
        next: Option<&'d Block>,
    ) {
        let props = &paragraph.props;
        let mut width = self.column().width;
        let fields = self.fields;
        let borders = &props.borders;
        let joined_above = self
            .above
            .and_then(|a| a.borders)
            .is_some_and(|b| b == borders && borders.top.is_some());
        let joined_below = matches!(next, Some(Block::Paragraph(n)) if n.props.borders == *borders)
            && borders.bottom.is_some();
        let top_border = if joined_above {
            borders.between
        } else {
            borders.top
        };
        let bottom_border = if joined_below { None } else { borders.bottom };
        let top_extra = top_border.map_or(0.0, |b| b.space + b.width);
        let bottom_extra = bottom_border.map_or(0.0, |b| b.space + b.width);

        let mut gap = gap(
            self.above,
            !self.fresh || self.keep_before && self.env.settings.top_spacing,
            self.broken,
            paragraph,
            self.env.settings.spacing_adds_up,
        ) + top_extra;
        let mut lead = self.above.map_or(0.0, |a| a.after).min(gap).max(0.0);
        let anchors = first_part && has_floats(paragraph);
        let bands = self.bands(paragraph, anchors.then_some(self.y + lead), self.y + gap);
        let mut lines = if bands.is_empty() {
            text::set(
                self.env,
                paragraph,
                inlines,
                first_part,
                width,
                self.fields,
                self.grid,
            )
        } else {
            text::set_around(
                self.env,
                paragraph,
                inlines,
                first_part,
                width,
                self.fields,
                self.grid,
                &bands,
            )
        };
        if anchors && self.paged() && !self.fresh {
            let first = lines.first().map_or(0.0, |l| l.bounds.height);
            if self.y + gap + self.pushed_above(paragraph, self.y + lead, self.y + gap) + first
                > self.bottom
            {
                self.next_column();
                return self.place(paragraph, inlines, first_part, next);
            }
        }
        let mut at = 0;
        let mut first_chunk = true;
        while at < lines.len() {
            let rest = &lines[at..];
            let room = self.bottom - self.y - gap;
            let natural = if self.paged() && rest.iter().any(|l| !l.notes.is_empty()) {
                self.fitting_noted(rest, gap)
            } else {
                fitting(rest, room)
            };
            let mut fit = natural;
            if fit < rest.len() {
                fit = if props.keep_lines
                    && first_chunk
                    && !self.fresh
                    && height(rest) <= self.bottom - self.band_top
                {
                    0
                } else {
                    widow_orphan(rest.len(), fit, props.widow_control)
                };
                if fit == 0 && self.fresh {
                    fit = natural.max(1);
                }
            }
            if fit > 0 {
                if anchors && first_chunk {
                    self.anchor(paragraph, self.y + lead, self.y + gap, false);
                }
                let last = at + fit == lines.len();
                let chunk_top = self.y + gap - top_extra;
                let start = self.out().len();
                let column = self.column();
                let mut y = self.y + gap;
                let mut notes = Vec::new();
                for line in &mut lines[at..at + fit] {
                    let baseline = y + line.bounds.ascent;
                    let items = std::mem::take(&mut line.items);
                    let out = self.out();
                    for mut item in items {
                        item.translate(column.x, baseline);
                        out.push(item);
                    }
                    if self.paged() {
                        self.numbered.push(NumberedLine {
                            sheet: self.sheets.len() - 1,
                            x: column.x,
                            baseline,
                            counted: !props.suppress_line_numbers,
                            restart: self.line_restart.take(),
                        });
                    }
                    y += line.bounds.height;
                    if let Some(cuts) = &mut self.cuts {
                        cuts.push(y);
                    }
                    notes.append(&mut line.notes);
                }
                if last {
                    y += bottom_extra;
                    if let Some(cut) = self.cuts.as_mut().and_then(|c| c.last_mut()) {
                        *cut = y;
                    }
                }
                self.y = y;
                self.frame(
                    paragraph,
                    start,
                    chunk_top,
                    y,
                    first_chunk.then_some(top_border).flatten(),
                    last.then_some(bottom_border).flatten(),
                );
                self.fresh = false;
                self.keep_before = false;
                first_chunk = false;
                at += fit;
                self.add_notes(&notes);
            }
            if at < lines.len() {
                self.next_column();
                gap = if first_chunk { top_extra } else { 0.0 };
                lead = 0.0;
                let column = self.column().width;
                if (column - width).abs() > EPSILON {
                    let rest = match at.checked_sub(1) {
                        None => text::set(
                            self.env, paragraph, inlines, first_part, column, fields, self.grid,
                        ),
                        Some(last) => text::set_rest(
                            self.env,
                            paragraph,
                            inlines,
                            first_part,
                            column,
                            fields,
                            self.grid,
                            lines[last].end,
                        ),
                    };
                    lines.truncate(at);
                    lines.extend(rest);
                    width = column;
                }
                for line in &mut lines[at..] {
                    line.bounds.ascent -= line.skip;
                    line.bounds.height -= line.skip;
                    line.skip = 0.0;
                }
            }
        }
        self.above = Some(Above::of(paragraph));
    }

    /// Where the floating objects of `paragraph` land when its area starts
    /// at `top` and its first line at `line`: with `escaped`, only those of a
    /// table cell that the cell does not bound, which a cell leaves out.
    fn float_frames(
        &self,
        paragraph: &'d Paragraph,
        top: f64,
        line: f64,
        escaped: bool,
    ) -> Vec<(f64, f64, f64, f64, &'d Drawing)> {
        let cell = self.sheets.is_empty() && self.frame.is_none();
        let column = self.column();
        let (page, origin, fallback);
        match (self.sheets.last(), self.frame) {
            (Some(sheet), _) => (page, origin) = (sheet.page, [0.0, 0.0]),
            (None, Some((setup, at))) => (page, origin) = (setup, at),
            (None, None) => {
                fallback = PageSetup {
                    width: column.width,
                    height: self.y.max(line),
                    margin: Margins::default(),
                    header: 0.0,
                    footer: 0.0,
                    texture: None,
                    footer_min_height: 0.0,
                    gutter: 0.0,
                    background: None,
                    borders: None,
                };
                (page, origin) = (&fallback, [0.0, 0.0]);
            }
        }
        let area = Area {
            page,
            column: Column {
                x: column.x + origin[0],
                width: column.width,
            },
            paragraph: top + origin[1],
            line: line + origin[1],
            odd: self.fields.page % 2 == 1,
        };
        paragraph
            .inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Drawing(d) if !d.props.hidden => {
                    let f = d.float.as_deref()?;
                    let escapes =
                        !f.in_cell && pinned(f.horizontal.from) && pinned(f.vertical.from);
                    if (escaped && !escapes) || (!escaped && cell && escapes) {
                        return None;
                    }
                    let w = float::width(self.env, d, self.fields, column.width);
                    let h = float::height(self.env, d, w, self.fields);
                    let (x, y) = float::position(f, w, h, &area);
                    Some((x - origin[0], y - origin[1], w, h, &**d))
                }
                _ => None,
            })
            .collect()
    }

    /// Draw on this page the objects of the table cells in `blocks` that
    /// their cell does not bound.
    pub(super) fn escaped(&mut self, blocks: &'d [Block]) {
        for block in blocks {
            match block {
                Block::Paragraph(p) => self.anchor(p, self.y, self.y, true),
                Block::Table(t) => {
                    for cell in t.rows.iter().flat_map(|r| &r.cells) {
                        self.escaped(&cell.blocks);
                    }
                }
            }
        }
    }

    /// How far the text already set in this column would move down to clear
    /// the objects of `paragraph` that sit above it at a place fixed on the
    /// page, wrapped top and bottom.
    fn pushed_above(&self, paragraph: &'d Paragraph, top: f64, line: f64) -> f64 {
        let column = self.column();
        self.float_frames(paragraph, top, line, false)
            .into_iter()
            .filter(|(x, _, w, _, d)| {
                let f = d.float.as_deref().expect("a float frame is floating");
                f.wrap == Wrap::TopAndBottom
                    && !f.behind
                    && matches!(f.vertical.from, Relative::Page | Relative::Margin)
                    && *x < column.x + column.width
                    && x + w > column.x
            })
            .map(|(_, y, _, h, d)| {
                let f = d.float.as_deref().expect("a float frame is floating");
                let (from, to) = (y - f.distance.top, y + h + f.distance.bottom);
                (to.min(line) - from.max(self.band_top)).max(0.0)
            })
            .sum()
    }

    /// The bands the lines of `paragraph` from `line` run beside: the page's
    /// objects, and its own when `anchors` gives where its area starts.
    fn bands(&self, paragraph: &'d Paragraph, anchors: Option<f64>, line: f64) -> Vec<float::Band> {
        let page = self.sheets.len().saturating_sub(1);
        let mut exclusions: Vec<Exclusion> = self
            .floats
            .pages
            .get(page)
            .map(|p| p.exclusions.clone())
            .unwrap_or_default();
        if let Some(top) = anchors {
            exclusions.extend(
                self.float_frames(paragraph, top, line, false)
                    .into_iter()
                    .flat_map(|(x, y, w, h, d)| excluded(x, y, w, h, d)),
            );
        }
        let column = self.column();
        exclusions
            .iter()
            .filter(|e| e.bottom > line && e.top < self.bottom)
            .filter(|e| e.right > column.x && e.left < column.x + column.width)
            .map(|e| float::band(e, line, column))
            .collect()
    }

    /// Draw the floating objects of `paragraph` on the current page.
    fn anchor(&mut self, paragraph: &'d Paragraph, top: f64, line: f64, escaped: bool) {
        let frames = self.float_frames(paragraph, top, line, escaped);
        let index = self.sheets.len().saturating_sub(1);
        for (x, y, w, h, d) in frames {
            let f = d.float.as_deref().expect("a float frame is floating");
            let mut items = Vec::new();
            float::draw(
                self.env,
                &d.shape,
                x,
                y,
                w,
                h,
                d.transform,
                self.fields,
                &mut items,
            );
            let floats = self.floats.page(index);
            floats.placed.push(Placed {
                z: f.z,
                behind: f.behind,
                items,
            });
            floats.exclusions.extend(excluded(x, y, w, h, d));
        }
    }

    /// Shading and borders of a paragraph between `top` and `bottom`, the
    /// shading painted under the items placed from `start`.
    fn frame(
        &mut self,
        paragraph: &Paragraph,
        start: usize,
        top: f64,
        bottom: f64,
        top_border: Option<Border>,
        bottom_border: Option<Border>,
    ) {
        let props = &paragraph.props;
        let column = self.column();
        let left = column.x
            + props
                .indent
                .left
                .min(props.indent.left + props.indent.first_line);
        let right = column.x + column.width - props.indent.right;
        let borders = &props.borders;
        let x0 = left - borders.left.map_or(0.0, |b| b.space + b.width);
        let x1 = right + borders.right.map_or(0.0, |b| b.space + b.width);
        if let Some(color) = props.shading {
            self.out().insert(
                start,
                Item::Rect {
                    x: x0,
                    y: top,
                    width: x1 - x0,
                    height: bottom - top,
                    color,
                },
            );
        }
        let mut rule = |from: [f64; 2], to: [f64; 2], border: Border| {
            self.out().push(Item::Line {
                from,
                to,
                width: border.width,
                color: border.color.unwrap_or(Color::BLACK),
            });
        };
        if let Some(b) = top_border {
            let y = top + b.width / 2.0;
            rule([x0, y], [x1, y], b);
        }
        if let Some(b) = bottom_border {
            let y = bottom - b.width / 2.0;
            rule([x0, y], [x1, y], b);
        }
        if let Some(b) = borders.left {
            let x = x0 + b.width / 2.0;
            rule([x, top], [x, bottom], b);
        }
        if let Some(b) = borders.right {
            let x = x1 - b.width / 2.0;
            rule([x, top], [x, bottom], b);
        }
    }

    /// Before a block that is no paragraph: the spacing after the paragraph
    /// above.
    pub(super) fn open_block(&mut self) {
        if let Some(above) = self.above.take() {
            self.y += above.after;
        }
    }

    pub(super) fn close_block(&mut self) {
        if let Some(cuts) = &mut self.cuts {
            cuts.push(self.y);
        }
        self.fresh = false;
        self.keep_before = false;
        self.above = Some(Above {
            after: 0.0,
            style: None,
            contextual: false,
            borders: None,
        });
    }

    pub(super) fn fits(&self, height: f64) -> bool {
        self.fresh || self.y + height <= self.bottom + EPSILON
    }

    pub(super) fn push(&mut self, item: Item) {
        self.out().push(item);
    }

    /// How many of `rest` fit below `gap` with the footnotes they reference:
    /// a line keeps at least the first line of its last note on its page.
    fn fitting_noted(&self, rest: &[Line], gap: f64) -> usize {
        let mut y = self.y + gap;
        let mut noted = self.noted;
        let mut cut = !self.carry.is_empty();
        for (k, line) in rest.iter().enumerate() {
            y += line.bounds.height;
            let chunks: Vec<NoteChunk> = if cut {
                Vec::new()
            } else {
                line.notes
                    .iter()
                    .filter_map(|n| self.note_chunk(n))
                    .collect()
            };
            let mut need = noted;
            if let Some((last, others)) = chunks.split_last() {
                if need == 0.0 {
                    need = self.separator;
                }
                need += others.iter().map(|c| c.height).sum::<f64>() + last.first_cut();
            }
            if y + need > self.limit + EPSILON {
                return k;
            }
            if !chunks.is_empty() {
                noted = noted.max(self.separator) + chunks.iter().map(|c| c.height).sum::<f64>();
                if y + noted > self.limit + EPSILON {
                    cut = true;
                    noted = self.limit - y;
                }
            }
        }
        rest.len()
    }

    /// `note` set at the width of the page's body, when it is a footnote.
    fn note_chunk(&self, note: &Note) -> Option<NoteChunk> {
        if note.kind != NoteKind::Foot {
            return None;
        }
        let key = note as *const Note;
        if let Some(chunk) = self.set_notes.borrow().get(&key) {
            return Some(chunk.clone());
        }
        let (section, _) = self.current?;
        let (_, width) = body(self.sheets.last().map_or(&section.page, |s| s.page));
        let mut flow = stacking(self.env, width, self.fields, None);
        flow.cuts = Some(Vec::new());
        flow.blocks(&note.blocks);
        let height = flow.y + flow.above.map_or(0.0, |a| a.after);
        let mut floats = std::mem::take(&mut flow.floats.page(0).placed);
        let mut items = float::layer(&mut floats, true);
        items.append(&mut flow.items);
        items.extend(float::layer(&mut floats, false));
        let chunk = NoteChunk {
            items,
            height,
            cuts: flow.cuts.unwrap_or_default(),
        };
        self.set_notes.borrow_mut().insert(key, chunk.clone());
        Some(chunk)
    }

    /// Set the footnotes of `notes` at the foot of the page.
    fn add_notes(&mut self, notes: &[Arc<Note>]) {
        if !self.paged() {
            return;
        }
        for note in notes {
            if let Some(chunk) = self.note_chunk(note) {
                self.add_note(chunk);
            }
        }
    }

    /// The footnotes the cells of `row` reference, once it is placed.
    pub(super) fn row_notes(&mut self, row: &Row) {
        let mut notes = Vec::new();
        for cell in &row.cells {
            visit_blocks(&cell.blocks, &mut |p| {
                for inline in &p.inlines {
                    if let Inline::Note(note, props) = inline {
                        if !props.hidden {
                            notes.push(Arc::clone(note));
                        }
                    }
                }
            });
        }
        self.add_notes(&notes);
    }

    /// Set `chunk` in the note area, what does not fit carried to the next
    /// page. A page that holds nothing yet takes its first line whatever.
    fn add_note(&mut self, chunk: NoteChunk) {
        if !self.carry.is_empty() {
            self.carry.push(chunk);
            return;
        }
        let separator = if self.noted > 0.0 {
            0.0
        } else {
            self.separator
        };
        let room = self.limit - self.y - self.noted - separator;
        let (kept, rest) = match chunk.split(room) {
            (None, Some(rest)) if self.fresh && self.noted == 0.0 => {
                let first = rest.first_cut();
                rest.split(first)
            }
            split => split,
        };
        if let Some(kept) = kept {
            self.noted += separator + kept.height;
            self.bottom = self.limit - self.noted;
            if let Some(sheet) = self.sheets.last_mut() {
                sheet.notes.push(kept);
            }
        }
        self.carry.extend(rest);
    }

    /// The endnotes after the body, below a separator rule or on a page of
    /// their own.
    fn endnotes(&mut self, notes: &[&'d Arc<Note>]) {
        if notes.is_empty() || !self.paged() {
            return;
        }
        self.open_block();
        if self.env.settings.endnotes_own_page {
            self.pending = None;
            self.advance(Break::Page);
        } else {
            if self.y + self.separator > self.bottom + EPSILON {
                self.next_column();
            }
            let column = self.column();
            let item = rule(column.x, self.y + 0.6 * self.separator, column.width);
            self.push(item);
            self.y += self.separator;
            self.keep_before = true;
        }
        self.above = None;
        self.broken = None;
        for note in notes {
            self.blocks(&note.blocks);
        }
    }

    fn finish(mut self, numbering: Option<LineNumbers>) -> Vec<Page> {
        let total = self.sheets.len() as u32;
        let env = self.env;
        let mut numbers = self.line_numbers(numbering);
        let even_and_odd = self.even_and_odd;
        let separator = self.separator;
        let mut per_section = std::collections::HashMap::new();
        for sheet in &self.sheets {
            *per_section.entry(sheet.section_index).or_insert(0u32) += 1;
        }
        let mut floats = std::mem::take(&mut self.floats);
        self.sheets
            .into_iter()
            .enumerate()
            .filter(|(_, sheet)| !sheet.blank)
            .map(|(index, sheet)| {
                let mut placed = std::mem::take(&mut floats.page(index).placed);
                let section = sheet.section;
                let page = sheet.page;
                let (x, width) = body(page);
                let fields = Fields {
                    page: sheet.number,
                    pages: total,
                    section_pages: per_section[&sheet.section_index],
                    format: section.page_number_format,
                };
                let pick = |set| story(set, section, sheet.first, sheet.number, even_and_odd);
                env.backdrop.set(page.background);
                let mut items = Vec::new();
                if let Some(blocks) = pick(&section.headers) {
                    let frame = Some((page, [x, page.header]));
                    let (header, _, anchored) = stack_on(env, blocks, width, fields, frame);
                    placed.extend(anchored);
                    items.extend(header.into_iter().map(|mut i| {
                        i.translate(x, page.header);
                        i
                    }));
                }
                if let Some(blocks) = pick(&section.footers) {
                    let (mut footer, mut height, _) = stack_on(env, blocks, width, fields, None);
                    let boxed = |height: f64| height.max(page.footer_min_height);
                    if stories_float(blocks) {
                        let top = page.height - page.footer - boxed(height);
                        let frame = Some((page, [x, top]));
                        let anchored;
                        (footer, height, anchored) = stack_on(env, blocks, width, fields, frame);
                        placed.extend(anchored);
                    }
                    let top = page.height - page.footer - boxed(height);
                    items.extend(footer.into_iter().map(|mut i| {
                        i.translate(x, top);
                        i
                    }));
                }
                items.extend(sheet.items);
                if !sheet.notes.is_empty() {
                    let total: f64 = sheet.notes.iter().map(|n| n.height).sum();
                    let mut y = sheet.limit - total;
                    items.push(rule(x, y - separator + 0.6 * separator, width));
                    for chunk in sheet.notes {
                        items.extend(chunk.items.into_iter().map(|mut i| {
                            i.translate(x, y);
                            i
                        }));
                        y += chunk.height;
                    }
                }
                let sheet_fill = page.background.map(|color| Item::Rect {
                    x: 0.0,
                    y: 0.0,
                    width: sheet.width,
                    height: sheet.height,
                    color,
                });
                let mut behind: Vec<Item> = sheet_fill.into_iter().collect();
                if let Some(texture) = &page.texture {
                    behind.push(Item::Shape(Box::new(Outline {
                        x: 0.0,
                        y: 0.0,
                        width: sheet.width,
                        height: sheet.height,
                        geometry: Geometry::Rect,
                        fill: Some(texture.clone()),
                        stroke: None,
                        transform: Transform::default(),
                    })));
                }
                if let Some(b) = page.borders.as_deref().filter(|b| {
                    if sheet.first {
                        b.on_first
                    } else {
                        b.on_others
                    }
                }) {
                    page_border(b, sheet.width, sheet.height, &mut behind);
                }
                behind.extend(float::layer(&mut placed, true));
                behind.extend(items);
                behind.extend(float::layer(&mut placed, false));
                behind.append(&mut numbers[index]);
                let items = behind;
                Page {
                    width: sheet.width,
                    height: sheet.height,
                    items,
                }
            })
            .collect()
    }
}

impl Flow<'_, '_, '_> {
    /// The line numbers of each sheet, right-aligned left of the column.
    fn line_numbers(&self, numbering: Option<LineNumbers>) -> Vec<Vec<Item>> {
        let mut out: Vec<Vec<Item>> = self.sheets.iter().map(|_| Vec::new()).collect();
        let Some(n) = numbering else {
            return out;
        };
        let props = &self.env.settings.line_number_font;
        let distance = n.distance.unwrap_or(LINE_NUMBER_DISTANCE);
        let mut count = 0;
        let mut sheet = None;
        for line in &self.numbered {
            if n.restart == LineRestart::NewPage && sheet != Some(line.sheet) {
                count = 0;
            }
            sheet = Some(line.sheet);
            if let Some(restart) = line.restart {
                count = restart - 1;
            }
            if !line.counted {
                continue;
            }
            count += 1;
            if count % n.count_by == 0 {
                out[line.sheet].push(line_number(
                    self.env,
                    props,
                    count,
                    line.x - distance,
                    line.baseline,
                ));
            }
        }
        out
    }
}

/// A line number, its right edge at `right`, on `baseline`.
fn line_number(env: &Env<'_>, props: &RunProps, number: u32, right: f64, baseline: f64) -> Item {
    let text = number.to_string();
    let (family, size, bold, italic) = text::slot('0', props);
    let face = env.resolve(family, bold, italic);
    let font = env.fonts.face(face.face);
    let scale = size / f64::from(font.metrics.units_per_em);
    let advances: Vec<f64> = font
        .advances(&text, false)
        .into_iter()
        .map(|a| a * scale)
        .collect();
    Item::Glyphs(Glyphs {
        x: right - advances.iter().sum::<f64>(),
        y: baseline,
        face: face.face,
        size,
        color: props.color.unwrap_or(Color::BLACK),
        anchors: vec![None; advances.len()],
        text,
        advances,
        synthetic_bold: face.synthetic_bold,
        synthetic_italic: face.synthetic_italic,
        kerning: false,
        letter_spacing: 0.0,
    })
}

/// The inlines of a paragraph cut at its page and column breaks, each part
/// with the break before it.
fn split(inlines: &[Inline]) -> Vec<(&[Inline], Option<Break>)> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut before = None;
    for (i, inline) in inlines.iter().enumerate() {
        if let Inline::Break(kind @ (Break::Page | Break::Column)) = inline {
            parts.push((&inlines[start..i], before));
            before = Some(*kind);
            start = i + 1;
        }
    }
    parts.push((&inlines[start..], before));
    parts
}

fn stories_float(blocks: &[Block]) -> bool {
    blocks.iter().any(|b| match b {
        Block::Paragraph(p) => has_floats(p),
        Block::Table(t) => t
            .rows
            .iter()
            .flat_map(|r| &r.cells)
            .any(|c| stories_float(&c.blocks)),
    })
}

/// Measured from the page or its margins, not from the text.
fn pinned(from: Relative) -> bool {
    !matches!(
        from,
        Relative::Column | Relative::Paragraph | Relative::Line | Relative::Character
    )
}

fn has_floats(paragraph: &Paragraph) -> bool {
    paragraph
        .inlines
        .iter()
        .any(|i| matches!(i, Inline::Drawing(d) if d.float.is_some()))
}

/// Height of the strips a contour is cut in, each keeping the text out of
/// the outline's extent across it.
const CONTOUR_STRIP: f64 = 1.0;

/// The rectangles a floating object at `x`, `y`, `width` by `height`, keeps
/// the text out of: its frame, or strips along its contour.
fn excluded(x: f64, y: f64, width: f64, height: f64, drawing: &Drawing) -> Vec<Exclusion> {
    let Some(f) = drawing.float.as_deref().filter(|f| f.wrap != Wrap::None) else {
        return Vec::new();
    };
    let d = &f.distance;
    let rect = |left: f64, top: f64, right: f64, bottom: f64| Exclusion {
        left: left - d.left,
        top,
        right: right + d.right,
        bottom,
        wrap: f.wrap,
        side: f.side,
    };
    if f.contour.len() < 3 {
        return vec![rect(x, y - d.top, x + width, y + height + d.bottom)];
    }
    let points: Vec<[f64; 2]> = f
        .contour
        .iter()
        .map(|p| [x + p[0] * width, y + p[1] * height])
        .collect();
    let top = points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let bottom = points
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max);
    let strips = ((bottom - top) / CONTOUR_STRIP).ceil().max(1.0) as usize;
    (0..strips)
        .filter_map(|k| {
            let from = top + k as f64 * CONTOUR_STRIP;
            let to = (from + CONTOUR_STRIP).min(bottom);
            let (left, right) = across(&points, from, to)?;
            Some(rect(left, from, right, to))
        })
        .collect()
}

/// How far a closed outline reaches left and right between two heights.
fn across(points: &[[f64; 2]], from: f64, to: f64) -> Option<(f64, f64)> {
    let mut xs = Vec::new();
    for (k, a) in points.iter().enumerate() {
        let b = points[(k + 1) % points.len()];
        if a[1] >= from && a[1] <= to {
            xs.push(a[0]);
        }
        for level in [from, to] {
            if (a[1] - level) * (b[1] - level) < 0.0 {
                xs.push(a[0] + (b[0] - a[0]) * (level - a[1]) / (b[1] - a[1]));
            }
        }
    }
    let left = xs.iter().copied().reduce(f64::min)?;
    let right = xs.iter().copied().reduce(f64::max)?;
    Some((left, right))
}

fn shows_anything(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Text(t) => !t.props.hidden && !t.text.is_empty(),
        Inline::Char(_, props)
        | Inline::Tab(props)
        | Inline::Field(_, props)
        | Inline::Note(_, props) => !props.hidden,
        Inline::Break(_) | Inline::Drawing(_) => true,
    })
}

#[cfg(test)]
mod tests {
    use super::{across, fitting, widow_orphan};
    use crate::layout::metrics::LineBox;
    use crate::layout::text::Line;

    #[test]
    fn widows_and_orphans_keep_two_lines_on_each_side() {
        assert_eq!(widow_orphan(5, 4, true), 3);
        assert_eq!(widow_orphan(5, 1, true), 0);
        assert_eq!(widow_orphan(3, 2, true), 0);
        assert_eq!(widow_orphan(6, 3, true), 3);
        assert_eq!(widow_orphan(5, 4, false), 4);
        assert_eq!(widow_orphan(5, 1, false), 1);
        assert_eq!(widow_orphan(4, 4, true), 4);
    }

    #[test]
    fn a_line_fits_when_only_its_proportional_spacing_passes_the_bottom() {
        let line = |height: f64, hang: f64| Line {
            items: Vec::new(),
            bounds: LineBox {
                ascent: 10.0,
                height,
            },
            skip: 0.0,
            hang,
            notes: Vec::new(),
            end: 0,
        };
        let lines = [line(16.0, 0.0), line(16.0, 2.2), line(16.0, 2.2)];
        assert_eq!(fitting(&lines, 30.0), 2);
        assert_eq!(fitting(&lines, 29.0), 1);
        assert_eq!(fitting(&lines, 45.8), 3);
    }

    #[test]
    fn a_contour_keeps_the_text_out_of_its_extent_across_each_strip() {
        let diamond = [[50.0, 0.0], [100.0, 50.0], [50.0, 100.0], [0.0, 50.0]];
        assert_eq!(across(&diamond, 0.0, 10.0), Some((40.0, 60.0)));
        assert_eq!(across(&diamond, 40.0, 60.0), Some((0.0, 100.0)));
        assert_eq!(across(&diamond, 90.0, 100.0), Some((40.0, 60.0)));
        assert_eq!(across(&diamond, 120.0, 130.0), None);
    }
}
