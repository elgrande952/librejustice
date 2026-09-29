//! The reader's document in the model `lj-office` lays out: twips and
//! half-points become points, indices into the font table become family
//! names, and every character keeps its CP as its anchor.

use std::collections::HashMap;
use std::sync::Arc;

use crate::model as m;
use crate::Anchor;

use super::bytes::{u16_at, u32_at};
use super::fib::lcb;
use super::model::{Block, Content, Document, Note, Paragraph, Picture, Row, Table};
use super::pictures;
use super::props::{Align, HMerge, VMerge};
use super::props::{Border, Chp, Frame, Pap, Sep, Shading, Tap};
use super::shapes::{self, Fspa, Shapes, Sp, FLIP_H, FLIP_V};
use super::text::Story;
use super::Doc;

const TWIP: f64 = 1.0 / 20.0;

pub fn anchor(story: Story, cp: u32) -> Anchor {
    Anchor::DocChar {
        story: story.index(),
        cp,
    }
}

pub fn to_model(doc: &Doc, d: &Document) -> m::Document {
    let dop = doc.fib.table(doc.table_stream(), lcb::DOP);
    let fonts = |i: u16| -> Arc<str> {
        Arc::from(
            d.fonts
                .get(i as usize)
                .map(|f| f.name.as_str())
                .unwrap_or("Times New Roman"),
        )
    };
    let notes: HashMap<u32, (m::NoteKind, &Note)> = d
        .footnotes
        .iter()
        .map(|n| (n.reference, (m::NoteKind::Foot, n)))
        .chain(
            d.endnotes
                .iter()
                .map(|n| (n.reference, (m::NoteKind::End, n))),
        )
        .collect();
    let shapes = shapes::read(doc);
    let cx = Cx {
        doc,
        fonts: &fonts,
        notes: &notes,
        pictures: &d.pictures,
        mark: None,
        shapes: &shapes,
        textboxes: [&d.textboxes, &d.header_textboxes],
        header_start: doc.text.story(Story::Headers).map_or(0, |r| r.start),
        // DOP fRMView: with revision marks off, a deletion is not drawn.
        show_deletions: dop.get(7).is_none_or(|b| b & 0x08 != 0),
        // DOP fDontUseHTMLAutoSpacing, which an older DOP leaves set.
        html_spacing: dop.len() >= 516 && u32_at(dop, 512).is_some_and(|v| v & 0x04 == 0),
    };
    let mut sections = Vec::new();
    let mut headers = m::HeaderFooters::default();
    let mut footers = m::HeaderFooters::default();
    let mut main = d.main.iter().peekable();
    for (i, s) in d.sections.iter().enumerate() {
        let story = |k: usize| -> Option<Arc<Vec<m::Block>>> {
            let blocks = d.headers.get(6 + 6 * i + k).filter(|b| !b.is_empty())?;
            Some(Arc::new(cx.blocks(blocks)))
        };
        let inherit = |slot: &mut Option<Arc<Vec<m::Block>>>, k: usize| {
            if let Some(story) = story(k) {
                *slot = Some(story);
            }
        };
        inherit(&mut headers.even, 0);
        inherit(&mut headers.default, 1);
        inherit(&mut footers.even, 2);
        inherit(&mut footers.default, 3);
        inherit(&mut headers.first, 4);
        inherit(&mut footers.first, 5);
        let mut list = Vec::new();
        while let Some(b) = main.peek() {
            let last = i + 1 == d.sections.len();
            if !last && first_cp(b).is_some_and(|cp| cp >= s.cp.end) {
                break;
            }
            list.push(main.next().expect("peeked"));
        }
        // A page's section break alone after a paragraph end opens no
        // paragraph in LibreOffice: the next section's text takes its place.
        if list.len() > 1
            && d.sections
                .get(i + 1)
                .is_some_and(|next| next.sep.break_kind >= 2)
            && matches!(list.last(), Some(Block::Paragraph(p))
                if p.runs.is_empty()
                    && p.cp.len() == 1
                    && doc.text.chars.get(p.cp.start as usize) == Some(&0x0C))
        {
            list.pop();
        }
        let blocks = cx.flow(&list, true);
        sections.push(section(&s.sep, headers.clone(), footers.clone(), blocks));
    }
    if cx.html_spacing {
        auto_spacing(sections.iter_mut().flat_map(|s| s.blocks.iter_mut()));
    }
    // LibreOffice balances the columns of a section a continuous one on a
    // page of the same size follows, unless the DOP's fNoColumnBalance.
    if u32_at(dop, 84).is_none_or(|v| v & 0x20 == 0) {
        for (k, next) in d.sections.iter().enumerate().skip(1) {
            let this = &d.sections[k - 1].sep;
            let next = &next.sep;
            sections[k - 1].balanced = this.columns > 1
                && next.break_kind == 0
                && (this.page_width, this.page_height) == (next.page_width, next.page_height);
        }
    }
    m::Document {
        sections,
        settings: m::Settings {
            default_tab_stop: u16_at(dop, 10).filter(|t| *t > 0).unwrap_or(720) as f64 * TWIP,
            even_and_odd_headers: dop.first().is_some_and(|b| b & 1 != 0),
            // A binary document is always laid out as Word 2003 lays it out.
            justify_shrinks: false,
            break_keeps_spacing: true,
            mark_follows_page_break: true,
            spacing_adds_up: false,
            tab_over_margin: false,
            note_separator: m::RunProps::default(),
            line_number_font: m::RunProps::default(),
            external_leading: true,
            endnotes_own_page: false,
            top_spacing: true,
        },
        fonts: d
            .fonts
            .iter()
            .map(|f| m::FontDecl {
                name: f.name.clone(),
                generic: match f.family() {
                    1 => m::Generic::Roman,
                    2 => m::Generic::Swiss,
                    3 => m::Generic::Modern,
                    4 => m::Generic::Script,
                    5 => m::Generic::Decorative,
                    _ => m::Generic::Unknown,
                },
            })
            .collect(),
        embedded_fonts: Vec::new(),
    }
}

/// An inline picture at its displayed size.
fn picture(pic: &Picture, props: m::RunProps) -> m::Drawing {
    let data: Arc<[u8]> = Arc::from(pic.bytes.as_slice());
    let shape = match pic.format {
        Some(_) => m::Shape::Picture(m::Picture {
            format: m::ImageFormat::sniff(&data),
            data,
            crop: m::Margins::default(),
            outline: None,
            recolor: recolor(&|id| pic.props.iter().find(|(p, _)| *p == id).map(|(_, v)| *v)),
        }),
        _ => m::Shape::Opaque,
    };
    m::Drawing {
        width: pic.width as f64 * TWIP,
        height: pic.height as f64 * TWIP,
        float: None,
        shape,
        transform: m::Transform::default(),
        props,
    }
}

fn first_cp(b: &Block) -> Option<u32> {
    let mut paras = Vec::new();
    b.paragraphs(&mut paras);
    paras.first().map(|p| p.cp.start)
}

/// The spacing LibreOffice gives HTML auto spacing along a story: none
/// above the first paragraph of the story or of a cell, none below the last
/// of a cell, none between the items of a list, and the auto space below the
/// last item of a list.
fn auto_spacing<'a>(blocks: impl Iterator<Item = &'a mut m::Block>) {
    let mut paragraphs = Vec::new();
    let mut first = true;
    for block in blocks {
        collect(block, &mut paragraphs, &mut first);
    }
    let mut open: Option<usize> = None;
    let mut numbered = false;
    for i in 0..paragraphs.len() {
        let (ref mut p, first, last) = paragraphs[i];
        let spacing = &mut p.props.spacing;
        let listed = p.label.is_some();
        if listed && (spacing.before_auto || spacing.after_auto) {
            if spacing.after_auto {
                spacing.after = 0.0;
            }
            if numbered && spacing.before_auto {
                spacing.before = 0.0;
            }
            open = Some(i);
        } else if let (false, Some(j)) = (listed, open.take()) {
            paragraphs[j].0.props.spacing.after = 14.0;
        }
        numbered = listed;
        let spacing = &mut paragraphs[i].0.props.spacing;
        if first && spacing.before_auto {
            spacing.before = 0.0;
        }
        if last && spacing.after_auto {
            spacing.after = 0.0;
        }
    }
}

/// The paragraphs under `block` in order, each with whether it opens its
/// story or cell and whether it closes its cell.
fn collect<'a>(
    block: &'a mut m::Block,
    out: &mut Vec<(&'a mut m::Paragraph, bool, bool)>,
    first: &mut bool,
) {
    match block {
        m::Block::Paragraph(p) => {
            out.push((p, *first, false));
            *first = false;
        }
        m::Block::Table(t) => {
            for cell in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                let mut first = true;
                let start = out.len();
                for b in cell.blocks.iter_mut() {
                    collect(b, out, &mut first);
                }
                if out.len() > start {
                    out.last_mut().expect("pushed").2 = true;
                }
            }
            *first = false;
        }
    }
}

fn section(
    sep: &Sep,
    headers: m::HeaderFooters,
    footers: m::HeaderFooters,
    blocks: Vec<m::Block>,
) -> m::Section {
    m::Section {
        page: m::PageSetup {
            width: sep.page_width as f64 * TWIP,
            height: sep.page_height as f64 * TWIP,
            margin: m::Margins {
                top: sep.top.unsigned_abs() as f64 * TWIP,
                right: sep.right as f64 * TWIP,
                bottom: sep.bottom.unsigned_abs() as f64 * TWIP,
                left: sep.left as f64 * TWIP,
            },
            header: sep.header as f64 * TWIP,
            footer: sep.footer as f64 * TWIP,
            texture: None,
            footer_min_height: 0.0,
            gutter: sep.gutter as f64 * TWIP,
            background: None,
            borders: None,
        },
        first_page: None,
        start: match sep.break_kind {
            0 => m::SectionStart::Continuous,
            1 => m::SectionStart::NextColumn,
            3 => m::SectionStart::EvenPage,
            4 => m::SectionStart::OddPage,
            _ => m::SectionStart::NextPage,
        },
        columns: m::Columns {
            count: sep.columns.max(1) as usize,
            gap: sep.column_space as f64 * TWIP,
            separator: sep.column_line,
            widths: Vec::new(),
        },
        headers,
        footers,
        title_page: sep.title_page,
        page_number_start: sep.page_number_restart.then_some(sep.page_number_start),
        page_number_format: crate::numbering::NumFormat::DECIMAL,
        line_grid: None,
        line_numbers: (sep.line_count_by > 0).then(|| m::LineNumbers {
            count_by: u32::from(sep.line_count_by),
            start: u32::from(sep.line_start),
            restart: match sep.line_restart {
                1 => m::LineRestart::NewSection,
                2 => m::LineRestart::Continuous,
                _ => m::LineRestart::NewPage,
            },
            distance: (sep.line_distance > 0).then(|| f64::from(sep.line_distance) * TWIP),
        }),
        dropped_after: None,
        own_stories: true,
        balanced: false,
        blocks,
    }
}

struct Cx<'a> {
    doc: &'a Doc,
    fonts: &'a dyn Fn(u16) -> Arc<str>,
    /// The footnotes and endnotes by the CP of their reference.
    notes: &'a HashMap<u32, (m::NoteKind, &'a Note)>,
    pictures: &'a [Picture],
    /// Inside a note, the mark its own number character draws.
    mark: Option<String>,
    shapes: &'a Shapes,
    /// The main and header text box stories.
    textboxes: [&'a [Vec<Block>]; 2],
    header_start: u32,
    show_deletions: bool,
    html_spacing: bool,
}

impl Cx<'_> {
    fn blocks(&self, list: &[Block]) -> Vec<m::Block> {
        let mut out = self.flow(&list.iter().collect::<Vec<_>>(), true);
        if self.html_spacing {
            auto_spacing(out.iter_mut());
        }
        out
    }

    /// The blocks of a story, each run of paragraphs sharing a frame floated
    /// at the paragraph after it when `framed`.
    fn flow(&self, list: &[&Block], framed: bool) -> Vec<m::Block> {
        let mut out = Vec::with_capacity(list.len());
        let mut pending: Vec<(Frame, Vec<m::Block>)> = Vec::new();
        for b in list {
            let frame = match b {
                Block::Paragraph(p) if framed && p.pap.drop_cap.is_none() => {
                    p.pap.frame.filter(|f| !f.is_empty())
                }
                _ => None,
            };
            let block = self.block(b);
            if let Some(frame) = frame {
                match pending.last_mut() {
                    Some((last, blocks)) if *last == frame => blocks.push(block),
                    _ => pending.push((frame, vec![block])),
                }
                continue;
            }
            match block {
                m::Block::Paragraph(mut p) => {
                    for (frame, blocks) in pending.drain(..).rev() {
                        p.inlines.insert(
                            0,
                            m::Inline::Drawing(Box::new(paragraph_frame(&frame, blocks))),
                        );
                    }
                    out.push(m::Block::Paragraph(p));
                }
                table => {
                    out.extend(pending.drain(..).flat_map(|(_, blocks)| blocks));
                    out.push(table);
                }
            }
        }
        out.extend(pending.into_iter().flat_map(|(_, blocks)| blocks));
        m::join_hidden_marks(&mut out);
        out
    }

    fn block(&self, b: &Block) -> m::Block {
        match b {
            Block::Paragraph(p) => m::Block::Paragraph(Box::new(self.paragraph(p))),
            Block::Table(t) => m::Block::Table(Box::new(self.table(t))),
        }
    }

    fn run(&self, chp: &Chp) -> m::RunProps {
        let size = if chp.size > 0 {
            chp.size as f64 / 2.0
        } else {
            10.0
        };
        m::RunProps {
            fonts: m::FontSet {
                ascii: (self.fonts)(chp.fonts[0]),
                east_asia: (self.fonts)(chp.fonts[1]),
                h_ansi: (self.fonts)(chp.fonts[2]),
                complex: (self.fonts)(chp.font_bi),
            },
            size,
            size_complex: size,
            bold: chp.bold,
            italic: chp.italic,
            bold_complex: chp.bold,
            italic_complex: chp.italic,
            underline: match chp.underline {
                0 => m::Underline::None,
                2 => m::Underline::Words,
                3 => m::Underline::Double,
                4 | 20 | 23 => m::Underline::Dotted,
                6 => m::Underline::Thick,
                7 | 9 | 10 | 39 | 41 | 42 => m::Underline::Dashed,
                11 | 43 | 55 => m::Underline::Wave,
                _ => m::Underline::Single,
            },
            strike: match (
                chp.strike || chp.deleted && self.show_deletions,
                chp.dstrike,
            ) {
                (_, true) => m::Strike::Double,
                (true, _) => m::Strike::Single,
                _ => m::Strike::None,
            },
            caps: chp.caps,
            small_caps: chp.small_caps,
            vertical: match chp.iss {
                1 => m::VerticalAlign::Superscript,
                2 => m::VerticalAlign::Subscript,
                _ => m::VerticalAlign::Baseline,
            },
            position: chp.position as f64 / 2.0,
            color: chp.color.map(color),
            highlight: chp.highlight.map(color),
            shading: chp.shading.as_ref().and_then(Shading::fill).map(color),
            spacing: chp.spacing as f64 * TWIP,
            scale: if chp.scale > 0 {
                chp.scale as f64 / 100.0
            } else {
                1.0
            },
            kerning: false,
            hidden: chp.vanish || chp.field_vanish || chp.deleted && !self.show_deletions,
            export: match chp.deleted && !self.show_deletions {
                true => m::Export::Always,
                false => m::Export::Shown,
            },
            complex: false,
            rtl: false,
            east_asian_hint: false,
        }
    }

    fn paragraph(&self, p: &Paragraph) -> m::Paragraph {
        let mut inlines = Vec::new();
        let mut skip = 0usize;
        let mut depth = 0usize;
        for run in &p.runs {
            let props = || self.run(&run.chp);
            match &run.content {
                Content::FieldStart(instr) => {
                    depth += 1;
                    if skip == 0 {
                        let field = match instr
                            .split_whitespace()
                            .next()
                            .map(str::to_ascii_uppercase)
                            .as_deref()
                        {
                            Some("PAGE") => Some(m::Field::Page),
                            Some("NUMPAGES") => Some(m::Field::NumPages),
                            Some("SECTIONPAGES") => Some(m::Field::SectionPages),
                            _ => None,
                        };
                        if let Some(field) = field {
                            inlines.push(m::Inline::Field(field, props()));
                            skip = depth;
                        }
                    }
                }
                Content::FieldEnd => {
                    if skip == depth {
                        skip = 0;
                    }
                    depth = depth.saturating_sub(1);
                }
                _ if skip > 0 => {}
                Content::Choice(text) => {
                    let props = props();
                    inlines.extend(text.chars().map(|c| m::Inline::Char(c, props.clone())));
                }
                Content::Text(text) if run.chp.symbol.is_some() => {
                    let props = props();
                    inlines.extend(text.chars().map(|c| m::Inline::Char(c, props.clone())));
                }
                Content::Text(text) => {
                    let mut cp = run.cp;
                    let anchors = text
                        .chars()
                        .map(|c| {
                            let at = anchor(p.story, cp);
                            cp += c.len_utf16() as u32;
                            at
                        })
                        .collect();
                    inlines.push(m::Inline::Text(m::Text {
                        text: text.clone(),
                        props: props(),
                        anchors,
                    }));
                }
                Content::Tab => inlines.push(m::Inline::Tab(props())),
                Content::LineBreak | Content::PageBreak | Content::ColumnBreak
                    if props().hidden => {}
                Content::LineBreak => inlines.push(m::Inline::Break(m::Break::Line)),
                Content::PageBreak => inlines.push(m::Inline::Break(m::Break::Page)),
                Content::ColumnBreak => inlines.push(m::Inline::Break(m::Break::Column)),
                Content::NoteReference(n) => match self.notes.get(&run.cp) {
                    Some((kind, note)) if self.mark.is_none() => {
                        let inner = Cx {
                            mark: Some(n.clone()),
                            ..*self
                        };
                        inlines.push(m::Inline::Note(
                            Arc::new(m::Note {
                                kind: *kind,
                                label: n.clone(),
                                blocks: inner.blocks(&note.blocks),
                            }),
                            props(),
                        ));
                    }
                    _ => {
                        let props = props();
                        let mark = self.mark.as_deref().unwrap_or(n);
                        inlines.extend(mark.chars().map(|c| m::Inline::Char(c, props.clone())));
                    }
                },
                Content::Picture(i) => {
                    if let Some(d) = self.pictures.get(*i).map(|pic| picture(pic, props())) {
                        inlines.push(m::Inline::Drawing(Box::new(d)));
                    }
                }
                Content::Shape => {
                    if let Some(d) = self.floating(p.story, run.cp, props()) {
                        inlines.push(m::Inline::Drawing(Box::new(d)));
                    }
                }
                Content::Object(id) => {
                    if let Some(pic) = pictures::object(&self.doc.cfb, *id) {
                        inlines.push(m::Inline::Drawing(Box::new(picture(&pic, props()))));
                    }
                }
                Content::CommentReference => {}
            }
        }
        let label = p.label.as_ref().map(|l| {
            let (text, suffix) = match l.text.chars().last() {
                Some('\t') => (&l.text[..l.text.len() - 1], m::LabelSuffix::Tab),
                Some(' ') => (&l.text[..l.text.len() - 1], m::LabelSuffix::Space),
                _ => (l.text.as_str(), m::LabelSuffix::Nothing),
            };
            m::Label {
                text: text.to_string(),
                props: self.run(&l.chp),
                suffix,
                alignment: m::Alignment::Left,
                tab_stop: None,
            }
        });
        let mut props = paragraph_props(&p.pap);
        let auto = if self.html_spacing { 14.0 } else { 5.0 };
        if p.pap.before_auto {
            props.spacing.before = auto;
            props.spacing.before_auto = true;
        }
        if p.pap.after_auto {
            props.spacing.after = auto;
            props.spacing.after_auto = true;
        }
        m::Paragraph {
            props,
            // LibreOffice joins a paragraph to the next when its mark is a
            // hidden deletion, not when its mark is hidden text.
            mark: m::RunProps {
                hidden: p.mark.deleted && !self.show_deletions,
                ..self.run(&p.mark)
            },
            style: self.doc.styles.get(p.pap.istd).map(|s| s.name.clone()),
            label,
            inlines,
            hidden_section: false,
        }
    }

    /// The floating shape whose anchor character is at `cp` of `story`.
    fn floating(&self, story: Story, cp: u32, props: m::RunProps) -> Option<m::Drawing> {
        let (fspa, header): (&Fspa, bool) = match story {
            Story::Main => (self.shapes.main.get(&cp)?, false),
            Story::Headers => (
                self.shapes
                    .header
                    .get(&cp.checked_sub(self.header_start)?)?,
                true,
            ),
            _ => return None,
        };
        let sp = self.shapes.sps.get(&fspa.spid)?;
        if sp.flag(0x03BF, 1) == Some(true) {
            return None;
        }
        let [l, t, r, b] = fspa.rect.map(|v| v as f64 * TWIP);
        let (mut x, mut y, mut width, mut height) = (l, t, r - l, b - t);
        let transform = transform(sp);
        if (45.0..135.0).contains(&transform.rotation.rem_euclid(180.0)) {
            x += (width - height) / 2.0;
            y += (height - width) / 2.0;
            std::mem::swap(&mut width, &mut height);
        }
        let place = |align: Option<u32>, offset: f64| match align {
            Some(1) => m::Place::Start,
            Some(2) => m::Place::Center,
            Some(3) => m::Place::End,
            Some(4) => m::Place::Inside,
            Some(5) => m::Place::Outside,
            _ => m::Place::Offset(offset),
        };
        let horizontal = m::Position {
            from: match sp.prop(0x0390).unwrap_or(u32::from(fspa.bx)) {
                1 => m::Relative::Page,
                2 => m::Relative::Column,
                3 => m::Relative::Character,
                _ => m::Relative::Margin,
            },
            place: place(sp.prop(0x038F), x),
        };
        let vertical = m::Position {
            from: match sp.prop(0x0392).unwrap_or(u32::from(fspa.by)) {
                1 => m::Relative::Page,
                2 => m::Relative::Paragraph,
                3 => m::Relative::Line,
                _ => m::Relative::Margin,
            },
            place: place(sp.prop(0x0391), y),
        };
        let emu = |id: u16, default: u32| f64::from(sp.prop(id).unwrap_or(default)) / 12700.0;
        let float = m::Float {
            horizontal,
            vertical,
            wrap: match fspa.wrap {
                1 => m::Wrap::TopAndBottom,
                3 => m::Wrap::None,
                _ => m::Wrap::Around,
            },
            side: match fspa.side {
                1 => m::WrapSide::Left,
                2 => m::WrapSide::Right,
                3 => m::WrapSide::Largest,
                _ => m::WrapSide::Both,
            },
            behind: fspa.below || sp.flag(0x03BF, 5) == Some(true),
            distance: m::Margins {
                top: emu(0x0385, 0),
                right: emu(0x0386, 114_300),
                bottom: emu(0x0387, 0),
                left: emu(0x0384, 114_300),
            },
            z: sp.z as i64,
            contour: Vec::new(),
            in_cell: true,
        };
        Some(m::Drawing {
            width,
            height,
            float: Some(Box::new(float)),
            shape: self.shape(sp, header, width, height),
            transform,
            props,
        })
    }

    /// An OfficeArt shape filling a `width` by `height` frame.
    fn shape(&self, sp: &Sp, header: bool, width: f64, height: f64) -> m::Shape {
        if let Some(space) = sp.space.filter(|_| !sp.children.is_empty()) {
            let [gl, gt, gr, gb] = space.map(f64::from);
            let sx = if gr != gl { width / (gr - gl) } else { 0.0 };
            let sy = if gb != gt { height / (gb - gt) } else { 0.0 };
            let children = sp
                .children
                .iter()
                .filter(|c| c.flag(0x03BF, 1) != Some(true))
                .filter_map(|c| {
                    let [l, t, r, b] = c.child?.map(f64::from);
                    let (w, h) = ((r - l) * sx, (b - t) * sy);
                    Some(m::Child {
                        x: (l - gl) * sx,
                        y: (t - gt) * sy,
                        width: w,
                        height: h,
                        shape: self.shape(c, header, w, h),
                        transform: transform(c),
                    })
                })
                .collect();
            return m::Shape::Group(children);
        }
        let picture_fill = matches!(sp.prop(0x0180), Some(2 | 3));
        let pib = sp
            .prop(0x0104)
            .or_else(|| sp.prop(0x0186).filter(|_| picture_fill));
        let stroke = |explicit: bool| {
            let on = match sp.flag(0x01FF, 3) {
                Some(on) => on,
                None => !explicit,
            };
            on.then(|| m::Stroke {
                width: f64::from(sp.prop(0x01CB).unwrap_or(9525)) / 12700.0,
                paint: m::Paint {
                    color: office_color(sp.prop(0x01C0)).unwrap_or(m::Color::BLACK),
                    alpha: 1.0,
                },
            })
        };
        if let Some((_, data)) = pib
            .and_then(|i| self.shapes.blips.get((i as usize).checked_sub(1)?))
            .and_then(Option::as_ref)
        {
            let crop = |id: u16| f64::from(sp.prop(id).unwrap_or(0) as i32) / 65536.0;
            return m::Shape::Picture(m::Picture {
                format: m::ImageFormat::sniff(data),
                data: data.clone(),
                crop: m::Margins {
                    top: crop(0x0100),
                    right: crop(0x0103),
                    bottom: crop(0x0101),
                    left: crop(0x0102),
                },
                outline: stroke(true),
                recolor: recolor(&|id| sp.prop(id)),
            });
        }
        if let Some(text) = sp
            .text(0x00C0)
            .filter(|t| sp.kind == 136 && !t.trim().is_empty())
        {
            let family: std::sync::Arc<str> =
                std::sync::Arc::from(sp.text(0x00C5).unwrap_or("Arial"));
            return m::Shape::WordArt(Box::new(m::WordArt {
                text: text.to_string(),
                props: m::RunProps {
                    fonts: m::FontSet {
                        ascii: family.clone(),
                        h_ansi: family.clone(),
                        east_asia: family.clone(),
                        complex: family,
                    },
                    size: 36.0,
                    size_complex: 36.0,
                    color: Some(wordart_color(sp)),
                    ..m::RunProps::default()
                },
                alpha: f64::from(sp.prop(0x0182).unwrap_or(0x10000)) / 65536.0,
            }));
        }
        let geometry = match sp.kind {
            1 | 75 | 202 => m::Geometry::Rect,
            2 => m::Geometry::RoundRect(f64::from(sp.prop(0x0147).unwrap_or(3600)) / 21600.0),
            3 => m::Geometry::Ellipse,
            20 | 32 => m::Geometry::Line,
            _ => {
                return m::Shape::Figure(Box::new(m::Figure {
                    geometry: m::Geometry::Rect,
                    fill: None,
                    stroke: None,
                    text: self.text_box(sp, header),
                }))
            }
        };
        let filled = geometry != m::Geometry::Line && sp.flag(0x01BF, 4) != Some(false);
        let fill = filled.then(|| {
            let opacity = |id: u16| f64::from(sp.prop(id).unwrap_or(0x10000)) / 65536.0;
            let front = m::Paint {
                color: office_color(sp.prop(0x0181)).unwrap_or(m::Color::WHITE),
                alpha: opacity(0x0182),
            };
            let kind = sp.prop(0x0180);
            if !matches!(kind, Some(4..=8)) {
                return m::Fill::Solid(front);
            }
            let back = m::Paint {
                color: office_color(sp.prop(0x0183)).unwrap_or(m::Color::WHITE),
                alpha: opacity(0x0184),
            };
            let fixed = |id: u16| f64::from(sp.prop(id).unwrap_or(0) as i32) / 65536.0;
            let center = matches!(kind, Some(5 | 6)).then(|| {
                [
                    (fixed(0x018F) + fixed(0x0191)) / 2.0,
                    (fixed(0x0190) + fixed(0x0192)) / 2.0,
                ]
            });
            let focus = f64::from(sp.prop(0x018C).unwrap_or(0) as i32) / 100.0;
            m::Fill::Gradient(std::sync::Arc::new(m::Gradient::office(
                front,
                back,
                fixed(0x018B),
                focus,
                center,
            )))
        });
        m::Shape::Figure(Box::new(m::Figure {
            geometry,
            fill,
            stroke: stroke(false),
            text: self.text_box(sp, header),
        }))
    }

    fn text_box(&self, sp: &Sp, header: bool) -> Option<m::TextBox> {
        let ids = if header {
            &self.shapes.header_text
        } else {
            &self.shapes.text
        };
        let blocks = self.textboxes[usize::from(header)].get(*ids.get(&sp.spid)?)?;
        // LibreOffice makes a text frame of a text box or rectangle, its text at the top.
        let frame = matches!(sp.kind, 1 | 202);
        let emu = |id: u16, default: u32| f64::from(sp.prop(id).unwrap_or(default)) / 12700.0;
        Some(m::TextBox {
            blocks: self.blocks(blocks),
            inset: m::Margins {
                top: emu(0x0082, 45_720),
                right: emu(0x0083, 91_440),
                bottom: emu(0x0084, 45_720),
                left: emu(0x0081, 91_440),
            },
            valign: match sp.prop(0x0087).map(|a| a % 3) {
                Some(1) if !frame => m::VAlign::Center,
                Some(2) if !frame => m::VAlign::Bottom,
                _ => m::VAlign::Top,
            },
            grow: sp.flag(0x00BF, 1) == Some(true),
            fit: false,
        })
    }

    fn cell(&self, list: &[Block]) -> Vec<m::Block> {
        self.flow(&list.iter().collect::<Vec<_>>(), false)
    }

    fn table(&self, t: &Table) -> m::Table {
        let mut grid: Vec<i32> = t
            .rows
            .iter()
            .flat_map(|r| r.tap.edges.iter().copied())
            .collect();
        grid.sort_unstable();
        grid.dedup();
        let column = |edge: i32| grid.partition_point(|g| *g < edge);
        let first = t.rows.first().map(|r| &r.tap);
        m::Table {
            props: m::TableProps {
                // LibreOffice keeps a whole percentage, and 100 past 600.
                width: match t.rows.iter().rev().find_map(|r| r.tap.percent) {
                    Some(p) if p > 30_000 => m::TableWidth::Fraction(1.0),
                    Some(p) if p >= 50 => m::TableWidth::Fraction(f64::from(p / 50) / 100.0),
                    _ => m::TableWidth::Auto,
                },
                alignment: match first.map(|tap| tap.align) {
                    Some(1) => m::TableAlignment::Center,
                    Some(2) => m::TableAlignment::Right,
                    _ => m::TableAlignment::Left,
                },
                indent: first
                    .and_then(|tap| tap.edges.first())
                    .copied()
                    .unwrap_or(0) as f64
                    * TWIP,
                borders: first
                    .map(|tap| table_borders(&tap.borders))
                    .unwrap_or_default(),
                cell_margins: first
                    .map(|tap| margins(tap, &[None; 4]))
                    .unwrap_or_default(),
                cell_spacing: 0.0,
                float: first.and_then(table_float).map(Box::new),
                page_break_before: matches!(
                    t.rows.first().and_then(|r| r.cells.first()).and_then(|c| c.first()),
                    Some(Block::Paragraph(p)) if p.pap.page_break_before
                ),
            },
            grid: grid
                .windows(2)
                .map(|w| (w[1] - w[0]) as f64 * TWIP)
                .collect(),
            rows: t.rows.iter().map(|r| self.row(r, &column)).collect(),
        }
    }

    fn row(&self, r: &Row, column: &dyn Fn(i32) -> usize) -> m::Row {
        let tap: &Tap = &r.tap;
        let mut cells: Vec<m::Cell> = Vec::new();
        for (i, blocks) in r.cells.iter().enumerate() {
            let props = tap.cells.get(i).cloned().unwrap_or_default();
            let span = match (tap.edges.get(i), tap.edges.get(i + 1)) {
                (Some(a), Some(b)) => column(*b).saturating_sub(column(*a)).max(1),
                _ => 1,
            };
            if props.hmerge == HMerge::Merged {
                if let Some(last) = cells.last_mut() {
                    last.span += span;
                    last.blocks.extend(self.cell(blocks));
                    continue;
                }
            }
            let [top, left, bottom, right] = props.borders;
            cells.push(m::Cell {
                span,
                vmerge: match props.vmerge {
                    VMerge::None => m::VMerge::None,
                    VMerge::Restart => m::VMerge::Restart,
                    VMerge::Continue => m::VMerge::Continue,
                },
                borders: m::Borders {
                    top: top.map(border),
                    bottom: bottom.map(border),
                    left: left.map(border),
                    right: right.map(border),
                    between: None,
                    vertical: None,
                },
                shading: props.shading.as_ref().and_then(Shading::fill).map(color),
                margins: margins(tap, &props.padding),
                valign: match props.valign {
                    1 => m::VAlign::Center,
                    2 => m::VAlign::Bottom,
                    _ => m::VAlign::Top,
                },
                blocks: self.cell(blocks),
            });
        }
        m::Row {
            grid_before: tap.edges.first().map(|e| column(*e)).unwrap_or(0),
            cells,
            height: (tap.height != 0).then(|| m::RowHeight {
                value: tap.height.unsigned_abs() as f64 * TWIP,
                exact: tap.height < 0,
            }),
            cant_split: tap.cant_split,
            header: tap.header,
        }
    }
}

/// A picture's colour mode, brightness and contrast, as LibreOffice reads them.
fn recolor(prop: &dyn Fn(u16) -> Option<u32>) -> m::Recolor {
    let contrast = match prop(0x0108).unwrap_or(0x10000) {
        0x10000 => 0,
        c if c < 0x10000 => (i64::from(c) * 101 / 0x10000 - 100) as i32,
        c => (100 - 100 * 0x10000 / i64::from(c)) as i32,
    };
    let luminance = prop(0x0109).unwrap_or(0) as i32 / 327;
    let (mode, luminance, contrast) = match prop(0x013F).unwrap_or(0) & 6 {
        4 => (m::ColorMode::Greys, luminance, contrast),
        6 => (m::ColorMode::Mono, luminance, contrast),
        0 if contrast == -70 && luminance == 70 => (m::ColorMode::Watermark, 0, 0),
        _ => (m::ColorMode::Standard, luminance, contrast),
    };
    m::Recolor {
        mode,
        luminance: f64::from(luminance),
        contrast: f64::from(contrast),
        office: luminance != 0 && contrast != 0,
    }
}

/// A shape's rotation and mirrors.
fn transform(sp: &Sp) -> m::Transform {
    m::Transform {
        rotation: f64::from(sp.prop(0x0004).unwrap_or(0) as i32) / 65536.0,
        flip_h: sp.flags & FLIP_H != 0,
        flip_v: sp.flags & FLIP_V != 0,
    }
}

/// A WordArt's fill colour, the middle of a gradient.
fn wordart_color(sp: &Sp) -> m::Color {
    let front = office_color(sp.prop(0x0181)).unwrap_or(m::Color::WHITE);
    if !matches!(sp.prop(0x0180), Some(4..=8)) {
        return front;
    }
    let back = office_color(sp.prop(0x0183)).unwrap_or(m::Color::WHITE);
    let mid = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
    m::Color(
        mid(front.0, back.0),
        mid(front.1, back.1),
        mid(front.2, back.2),
    )
}

/// An OfficeArt colour, `None` for one a palette or the system names.
fn office_color(value: Option<u32>) -> Option<m::Color> {
    let [r, g, b, flags] = value?.to_le_bytes();
    (flags & 0x1F == 0).then_some(m::Color(r, g, b))
}

fn color(rgb: [u8; 3]) -> m::Color {
    m::Color(rgb[0], rgb[1], rgb[2])
}

fn border(b: Border) -> m::Border {
    m::Border {
        width: b.width as f64 / 8.0,
        color: b.color.map(color),
        space: b.space as f64,
        double: b.kind == 3,
    }
}

/// Where LibreOffice puts a frame positioned by `f`: symbolic places
/// among the small negative offsets, and a vertical offset of zero keeping
/// to the paragraph whatever the anchor.
fn frame_position(f: &Frame) -> (m::Position, m::Position) {
    let horizontal = m::Position {
        from: match f.anchor_h {
            1 => m::Relative::Margin,
            2 => m::Relative::Page,
            _ => m::Relative::Column,
        },
        place: match f.x {
            -4 => m::Place::Center,
            -8 => m::Place::End,
            -12 => m::Place::Inside,
            -16 => m::Place::Outside,
            x => m::Place::Offset(f64::from(x) * TWIP),
        },
    };
    let vertical = m::Position {
        from: match (f.anchor_v, f.y) {
            (_, 0) | (2.., _) => m::Relative::Paragraph,
            (0, _) => m::Relative::Margin,
            _ => m::Relative::Page,
        },
        place: match f.y {
            -4 => m::Place::Start,
            -8 => m::Place::Center,
            -12 => m::Place::End,
            y => m::Place::Offset(f64::from(y) * TWIP),
        },
    };
    (horizontal, vertical)
}

/// A positioned paragraph's frame, as LibreOffice converts it.
fn paragraph_frame(f: &Frame, blocks: Vec<m::Block>) -> m::Drawing {
    let (horizontal, vertical) = frame_position(f);
    let (wrap, side) = match f.wrap {
        1 => (m::Wrap::TopAndBottom, m::WrapSide::Both),
        3 => (m::Wrap::None, m::WrapSide::Both),
        0 => (m::Wrap::Around, m::WrapSide::Largest),
        _ => (m::Wrap::Around, m::WrapSide::Both),
    };
    let (across, down) = (f64::from(f.dx_text) * TWIP, f64::from(f.dy_text) * TWIP);
    let float = m::Float {
        horizontal,
        vertical,
        wrap,
        side,
        behind: false,
        distance: m::Margins {
            top: down,
            right: across,
            bottom: down,
            left: across,
        },
        z: 0,
        contour: Vec::new(),
        in_cell: true,
    };
    let height = f64::from(f.height & 0x7FFF) * TWIP;
    let exact = f.height != 0 && f.height & 0x8000 == 0;
    let width = (f.width > 0).then(|| f64::from(f.width) * TWIP);
    m::Drawing::paragraph_frame(float, width, height, exact, blocks)
}

/// The frame LibreOffice floats a positioned table in, the text running
/// around it; a side placed against its reference keeps no distance there.
fn table_float(tap: &Tap) -> Option<m::Float> {
    let position = &tap.position;
    let frame = position.frame()?;
    let (horizontal, vertical) = frame_position(&frame);
    let [mut left, mut right, mut top, mut bottom] = position.distance.map(|d| f64::from(d) * TWIP);
    match frame.x {
        0 => left = 0.0,
        -8 => right = 0.0,
        _ => {}
    }
    match frame.y {
        -4 if frame.anchor_v < 2 => top = 0.0,
        -12 if frame.anchor_v < 2 => bottom = 0.0,
        _ => {}
    }
    Some(m::Float {
        horizontal,
        vertical,
        wrap: m::Wrap::Around,
        side: m::WrapSide::Both,
        behind: false,
        distance: m::Margins {
            top,
            right,
            bottom,
            left,
        },
        z: 0,
        contour: Vec::new(),
        in_cell: true,
    })
}

/// A cell's margins, its own over its row's, left and right falling back
/// on the row's half gap.
fn margins(tap: &Tap, own: &[Option<u16>; 4]) -> m::Margins {
    let side = |k: usize| own[k].or(tap.padding[k]).map(|v| f64::from(v) * TWIP);
    let gap = tap.gap_half as f64 * TWIP;
    m::Margins {
        top: side(0).unwrap_or(0.0),
        left: side(1).unwrap_or(gap),
        bottom: side(2).unwrap_or(0.0),
        right: side(3).unwrap_or(gap),
    }
}

fn table_borders(b: &[Option<Border>; 6]) -> m::Borders {
    m::Borders {
        top: b[0].map(border),
        left: b[1].map(border),
        bottom: b[2].map(border),
        right: b[3].map(border),
        between: b[4].map(border),
        vertical: b[5].map(border),
    }
}

fn paragraph_props(pap: &Pap) -> m::ParagraphProps {
    let [top, left, bottom, right, between] = pap.borders;
    m::ParagraphProps {
        alignment: match pap.align {
            Align::Left => m::Alignment::Left,
            Align::Center => m::Alignment::Center,
            Align::Right => m::Alignment::Right,
            Align::Justify => m::Alignment::Justify,
            Align::Distribute => m::Alignment::Distribute,
        },
        indent: m::Indent {
            left: pap.left as f64 * TWIP,
            right: pap.right as f64 * TWIP,
            first_line: pap.first_line as f64 * TWIP,
        },
        spacing: m::Spacing {
            before: pap.before as f64 * TWIP,
            after: pap.after as f64 * TWIP,
            line: match (pap.line.multiple, pap.line.value) {
                (true, v) => m::LineSpacing::Proportional(v as f64 / 240.0 * 100.0),
                (false, v) if v < 0 => m::LineSpacing::Exact(-(v as f64) * TWIP),
                (false, v) => m::LineSpacing::AtLeast(v as f64 * TWIP),
            },
            ..Default::default()
        },
        contextual_spacing: pap.contextual,
        keep_next: pap.keep_next,
        keep_lines: pap.keep,
        widow_control: pap.widow,
        page_break_before: pap.page_break_before,
        column_break_before: false,
        tabs: {
            let mut tabs: Vec<m::TabStop> = pap
                .tabs
                .iter()
                .map(|t| m::TabStop {
                    position: t.pos as f64 * TWIP,
                    kind: match t.align {
                        1 => m::TabKind::Center,
                        2 => m::TabKind::Right,
                        3 => m::TabKind::Decimal,
                        4 => m::TabKind::Bar,
                        _ => m::TabKind::Left,
                    },
                    leader: match t.leader {
                        1 => m::Leader::Dot,
                        2 => m::Leader::Hyphen,
                        3 | 4 => m::Leader::Underscore,
                        5 => m::Leader::MiddleDot,
                        _ => m::Leader::None,
                    },
                })
                .collect();
            tabs.sort_by(|a, b| a.position.total_cmp(&b.position));
            tabs
        },
        borders: m::Borders {
            top: top.map(border),
            bottom: bottom.map(border),
            left: left.map(border),
            right: right.map(border),
            between: between.map(border),
            vertical: None,
        },
        shading: pap.shading.as_ref().and_then(Shading::fill).map(color),
        rtl: pap.bidi,
        off_grid: false,
        suppress_line_numbers: pap.no_line_numbers,
    }
}
