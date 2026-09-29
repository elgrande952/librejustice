//! The `.odt` document as the layout reads it. A page style change cuts a
//! section, as a Word section break does; a text section with columns of its
//! own becomes a continuous section. A frame becomes a drawing, in the line when
//! anchored as a character; one anchored to a page floats from the first
//! paragraph LibreOffice last laid out on that page. The redaction walks the
//! package and reads nothing here.

use std::sync::Arc;

use crate::Anchor;

use super::drawing;
use super::model as odt;
use crate::model as m;

pub fn to_model(doc: &odt::Document) -> m::Document {
    let mut sections = Vec::new();
    let first = doc.pages.get(doc.first_page);
    let mut current = Open::new(doc, first, m::SectionStart::NextPage);
    let mut pending_break = false;
    let mut pager = Pager::new(doc);
    blocks(
        doc,
        &doc.body,
        &mut current,
        &mut sections,
        &mut pending_break,
        false,
        &mut pager,
    );
    sections.push(current.close());
    let default_tab_stop = first_paragraph(&doc.body).map_or(35.43, |p| p.props.default_tab as f64);
    m::Document {
        sections,
        settings: m::Settings {
            default_tab_stop,
            even_and_odd_headers: doc.pages.iter().any(|p| {
                p.header.as_ref().is_some_and(|h| h.left.is_some())
                    || p.footer.as_ref().is_some_and(|f| f.left.is_some())
            }),
            justify_shrinks: doc.compat.justify_lines_with_shrinking,
            break_keeps_spacing: true,
            mark_follows_page_break: false,
            spacing_adds_up: doc.compat.add_para_table_spacing,
            tab_over_margin: false,
            note_separator: m::RunProps::default(),
            line_number_font: m::RunProps::default(),
            external_leading: doc.compat.add_external_leading,
            endnotes_own_page: !doc.compat.continuous_endnotes,
            top_spacing: doc.compat.add_para_table_spacing_at_start,
        },
        fonts: doc
            .fonts
            .iter()
            .map(|f| m::FontDecl {
                name: f.family.clone(),
                generic: match f.fixed_pitch {
                    true => m::Generic::Fixed,
                    false => generic(f.generic.as_deref(), &f.family),
                },
            })
            .collect(),
        embedded_fonts: doc
            .fonts
            .iter()
            .flat_map(|f| f.embedded.iter().cloned())
            .collect(),
    }
}

/// The section being filled: its page setup and the blocks so far.
struct Open {
    page: m::PageSetup,
    first_page: Option<m::PageSetup>,
    start: m::SectionStart,
    columns: m::Columns,
    headers: m::HeaderFooters,
    footers: m::HeaderFooters,
    title_page: bool,
    page_number_start: Option<u32>,
    blocks: Vec<m::Block>,
}

impl Open {
    fn new(doc: &odt::Document, page: Option<&odt::PageStyle>, start: m::SectionStart) -> Self {
        let Some(p) = page else {
            return Self {
                page: m::PageSetup {
                    width: 612.0,
                    height: 792.0,
                    margin: m::Margins {
                        top: 56.7,
                        right: 56.7,
                        bottom: 56.7,
                        left: 56.7,
                    },
                    header: 36.0,
                    footer: 36.0,
                    texture: None,
                    footer_min_height: 0.0,
                    gutter: 0.0,
                    background: None,
                    borders: None,
                },
                first_page: None,
                start,
                columns: m::Columns::default(),
                headers: m::HeaderFooters::default(),
                footers: m::HeaderFooters::default(),
                title_page: false,
                page_number_start: None,
                blocks: Vec::new(),
            };
        };
        // A first page style hands the pages after the first to its next style.
        let first = p;
        let p = match p.next.map(|i| &doc.pages[i]) {
            Some(next) if next.name != p.name => next,
            _ => p,
        };
        let text_width = (p.width - p.margin.left - p.margin.right) as f64;
        let stories = |hf: &Option<odt::HeaderFooter>| match hf {
            None => m::HeaderFooters::default(),
            Some(h) => m::HeaderFooters {
                default: Some(Arc::new(story(doc, &h.blocks, false))),
                first: h.first.as_ref().map(|b| Arc::new(story(doc, b, false))),
                even: h.left.as_ref().map(|b| Arc::new(story(doc, b, false))),
            },
        };
        let chained = !std::ptr::eq(first, p);
        let mut headers = stories(&p.header);
        let mut footers = stories(&p.footer);
        if chained {
            let opening = |hf: &Option<odt::HeaderFooter>| {
                hf.as_ref()
                    .map(|h| Arc::new(story(doc, h.first.as_ref().unwrap_or(&h.blocks), false)))
            };
            headers.first = opening(&first.header);
            footers.first = opening(&first.footer);
        }
        let page = setup(p);
        Self {
            first_page: Some(setup(first)).filter(|f| chained && *f != page),
            page,
            start,
            columns: columns(&p.columns, text_width),
            title_page: chained
                || p.header.as_ref().is_some_and(|h| h.first.is_some())
                || p.footer.as_ref().is_some_and(|f| f.first.is_some()),
            headers,
            footers,
            page_number_start: None,
            blocks: Vec::new(),
        }
    }

    fn close(self) -> m::Section {
        m::Section {
            page: self.page,
            first_page: self.first_page,
            start: self.start,
            columns: self.columns,
            headers: self.headers,
            footers: self.footers,
            title_page: self.title_page,
            page_number_start: self.page_number_start,
            page_number_format: crate::numbering::NumFormat::DECIMAL,
            line_grid: None,
            line_numbers: None,
            dropped_after: None,
            own_stories: true,
            balanced: false,
            blocks: self.blocks,
        }
    }

    /// A section with this one's page and stories, the columns replaced.
    fn continued(&self, columns: m::Columns) -> Self {
        Self {
            page: self.page.clone(),
            first_page: None,
            start: m::SectionStart::Continuous,
            columns,
            headers: self.headers.clone(),
            footers: self.footers.clone(),
            title_page: self.title_page,
            page_number_start: None,
            blocks: Vec::new(),
        }
    }
}

fn setup(p: &odt::PageStyle) -> m::PageSetup {
    // ODF measures the page margin to the header; Word measures it to the body.
    // A dynamic spacing lies inside the least height, and shrinks as the text grows.
    let band = |hf: &Option<odt::HeaderFooter>| {
        hf.as_ref().map_or(0.0, |h| {
            h.height.unwrap_or(h.min_height) + if h.dynamic_spacing { 0.0 } else { h.spacing }
        }) as f64
    };
    m::PageSetup {
        width: p.width as f64,
        height: p.height as f64,
        margin: m::Margins {
            top: p.margin.top as f64 + band(&p.header),
            right: p.margin.right as f64,
            bottom: p.margin.bottom as f64 + band(&p.footer),
            left: p.margin.left as f64,
        },
        header: p.margin.top as f64,
        footer: p.margin.bottom as f64,
        texture: None,
        // Writer sets the footer's text at the top of its box.
        footer_min_height: p.footer.as_ref().map_or(0.0, |h| {
            let least = h.height.unwrap_or(h.min_height);
            (if h.dynamic_spacing {
                least - h.spacing
            } else {
                least
            })
            .max(0.0) as f64
        }),
        gutter: 0.0,
        background: p.background.map(color),
        borders: None,
    }
}

fn first_paragraph(list: &[odt::Block]) -> Option<&odt::Paragraph> {
    list.iter().find_map(|b| match b {
        odt::Block::Paragraph(p) => Some(p),
        odt::Block::Section(s) => first_paragraph(&s.blocks),
        _ => None,
    })
}

/// The frames anchored to pages, waiting for the page their paragraph starts
/// on, which the soft page breaks LibreOffice saved count.
struct Pager<'d> {
    doc: &'d odt::Document,
    page: u32,
    waiting: Vec<(u32, &'d odt::Frame)>,
}

impl<'d> Pager<'d> {
    fn new(doc: &'d odt::Document) -> Self {
        let mut waiting: Vec<(u32, &odt::Frame)> = doc
            .page_frames
            .iter()
            .filter_map(|f| match f.anchor {
                odt::FrameAnchor::Page(n) => Some((n.max(1), f)),
                _ => None,
            })
            .collect();
        waiting.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        Self {
            doc,
            page: 1,
            waiting,
        }
    }

    /// Float the frames of the pages up to this one from `paragraph`.
    fn anchor(&mut self, paragraph: &mut m::Paragraph) {
        let mut due = Vec::new();
        while let Some(&(n, frame)) = self.waiting.last() {
            if n > self.page {
                break;
            }
            self.waiting.pop();
            due.extend(drawing::drawing(self.doc, frame, &paragraph.mark));
        }
        paragraph.inlines.splice(
            0..0,
            due.into_iter().map(|d| m::Inline::Drawing(Box::new(d))),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn blocks<'d>(
    doc: &'d odt::Document,
    list: &'d [odt::Block],
    current: &mut Open,
    sections: &mut Vec<m::Section>,
    pending_break: &mut bool,
    hidden: bool,
    pager: &mut Pager<'d>,
) {
    for block in list {
        let (page_style, number) = match block {
            odt::Block::Paragraph(p) => (p.page_style, p.props.page_number),
            odt::Block::Table(t) => (t.page_style, None),
            _ => (None, None),
        };
        if let Some(i) = page_style {
            let start = match block {
                odt::Block::Paragraph(p) => match p.props.break_before {
                    odt::Break::EvenPage => m::SectionStart::EvenPage,
                    odt::Break::OddPage => m::SectionStart::OddPage,
                    _ => m::SectionStart::NextPage,
                },
                _ => m::SectionStart::NextPage,
            };
            if !current.blocks.is_empty() {
                let next = Open::new(doc, doc.pages.get(i), start);
                sections.push(std::mem::replace(current, next).close());
            } else {
                let kept = current.start;
                // A text section's columns outlast the page style its first paragraph names.
                let columns = (kept == m::SectionStart::Continuous && current.columns.count > 1)
                    .then(|| current.columns.clone());
                *current = Open::new(doc, doc.pages.get(i), kept);
                if let Some(columns) = columns {
                    current.columns = columns;
                }
            }
            current.page_number_start = number;
            *pending_break = false;
        }
        match block {
            odt::Block::Paragraph(p) => {
                let mut para = paragraph(doc, p, hidden);
                let soft_breaks = p
                    .inlines
                    .iter()
                    .filter(|i| matches!(i, odt::Inline::SoftPageBreak))
                    .count() as u32;
                // A hidden section is never laid out: the page frames and a
                // pending break wait for the next shown paragraph.
                if hidden {
                    pager.page += soft_breaks;
                    current.blocks.push(m::Block::Paragraph(Box::new(para)));
                    continue;
                }
                pager.anchor(&mut para);
                pager.page += soft_breaks;
                para.props.page_break_before |=
                    std::mem::take(pending_break) && page_style.is_none();
                // Writer ignores a column break outside columns.
                if current.columns.count <= 1 {
                    para.props.column_break_before = false;
                    para.inlines
                        .retain(|i| !matches!(i, m::Inline::Break(m::Break::Column)));
                }
                if matches!(
                    p.props.break_after,
                    odt::Break::Page | odt::Break::EvenPage | odt::Break::OddPage
                ) {
                    *pending_break = true;
                }
                current.blocks.push(m::Block::Paragraph(Box::new(para)));
            }
            odt::Block::Table(t) => {
                let mut table = table(doc, t, hidden);
                if hidden {
                    current.blocks.push(m::Block::Table(Box::new(table)));
                    continue;
                }
                table.props.page_break_before |=
                    std::mem::take(pending_break) && page_style.is_none();
                if matches!(
                    t.props.break_after,
                    odt::Break::Page | odt::Break::EvenPage | odt::Break::OddPage
                ) {
                    *pending_break = true;
                }
                current.blocks.push(m::Block::Table(Box::new(table)));
            }
            odt::Block::Section(s) if s.columns.count > 1 => {
                let width =
                    (current.page.width - current.page.margin.left - current.page.margin.right)
                        - (s.margin_left + s.margin_right) as f64;
                let outer = current.columns.clone();
                let inner = current.continued(columns(&s.columns, width));
                sections.push(std::mem::replace(current, inner).close());
                blocks(
                    doc,
                    &s.blocks,
                    current,
                    sections,
                    pending_break,
                    hidden || s.hidden,
                    pager,
                );
                let after = current.continued(outer);
                sections.push(std::mem::replace(current, after).close());
            }
            odt::Block::Section(s) => blocks(
                doc,
                &s.blocks,
                current,
                sections,
                pending_break,
                hidden || s.hidden,
                pager,
            ),
            odt::Block::SoftPageBreak => pager.page += 1,
        }
    }
}

pub(super) fn story(doc: &odt::Document, list: &[odt::Block], hidden: bool) -> Vec<m::Block> {
    let mut out = Vec::new();
    for block in list {
        match block {
            odt::Block::Paragraph(p) => {
                out.push(m::Block::Paragraph(Box::new(paragraph(doc, p, hidden))))
            }
            odt::Block::Table(t) => out.push(m::Block::Table(Box::new(table(doc, t, hidden)))),
            odt::Block::Section(s) => out.extend(story(doc, &s.blocks, hidden || s.hidden)),
            odt::Block::SoftPageBreak => {}
        }
    }
    out
}

fn columns(c: &odt::Columns, text_width: f64) -> m::Columns {
    let count = c.count.max(1) as usize;
    let gap = c.gap as f64;
    let widths = if c.widths.len() == count && count > 1 {
        let total: f32 = c.widths.iter().sum();
        let indent = |i: usize| {
            c.indents
                .get(i)
                .map_or((0.0, 0.0), |&(s, e)| (s as f64, e as f64))
        };
        // Each relative width holds the column's indents, which make the gaps.
        let indented = c.indents.iter().any(|&(s, e)| s > 0.0 || e > 0.0);
        let room = if indented {
            text_width
        } else {
            text_width - gap * (count - 1) as f64
        };
        c.widths
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let share = if total > 0.0 {
                    room * (*w / total) as f64
                } else {
                    room / count as f64
                };
                let (start, end) = if indented { indent(i) } else { (0.0, 0.0) };
                let after = match (i + 1 < count, indented) {
                    (false, _) => 0.0,
                    (true, true) => end + indent(i + 1).0,
                    (true, false) => gap,
                };
                (share - start - end, after)
            })
            .collect()
    } else {
        Vec::new()
    };
    m::Columns {
        count,
        gap,
        separator: c.separator.is_some(),
        widths,
    }
}

/// A family declared without a class is sans-serif to Writer, unless its name
/// says serif.
fn generic(g: Option<&str>, family: &str) -> m::Generic {
    match g {
        Some("swiss") => m::Generic::Swiss,
        Some("roman") => m::Generic::Roman,
        Some("modern") => m::Generic::Modern,
        Some("script") => m::Generic::Script,
        Some("decorative") => m::Generic::Decorative,
        None | Some("system") => {
            let name = family.to_ascii_lowercase();
            if name.contains("serif") && !name.contains("sans") {
                m::Generic::Roman
            } else {
                m::Generic::Swiss
            }
        }
        _ => m::Generic::Unknown,
    }
}

pub(super) fn color(c: odt::Color) -> m::Color {
    m::Color((c.0 >> 16) as u8, (c.0 >> 8) as u8, c.0 as u8)
}

fn border(b: &Option<odt::Border>) -> Option<m::Border> {
    b.as_ref().map(|b| m::Border {
        width: b.width as f64,
        color: Some(color(b.color)),
        space: 0.0,
        double: b.style == odt::LineStyle::Double,
    })
}

fn borders(s: &odt::Sides<Option<odt::Border>>) -> m::Borders {
    m::Borders {
        top: border(&s.top),
        bottom: border(&s.bottom),
        left: border(&s.left),
        right: border(&s.right),
        between: None,
        vertical: None,
    }
}

/// Writer draws a paragraph's borders on its margins and sets its text inside
/// them and their padding, which applies on a side with no line as well.
fn paragraph_borders(p: &odt::ParaProps) -> m::Borders {
    let side = |b: &Option<odt::Border>, padding: odt::Pt| match border(b) {
        Some(b) => Some(m::Border {
            space: padding as f64,
            ..b
        }),
        None => (padding > 0.0).then_some(m::Border {
            width: 0.0,
            color: None,
            space: padding as f64,
            double: false,
        }),
    };
    m::Borders {
        top: side(&p.borders.top, p.padding.top),
        bottom: side(&p.borders.bottom, p.padding.bottom),
        left: side(&p.borders.left, p.padding.left),
        right: side(&p.borders.right, p.padding.right),
        between: None,
        vertical: None,
    }
}

fn run_props(t: &odt::TextProps, hidden: bool) -> m::RunProps {
    let underline = t.underline.map_or(m::Underline::None, |u| match u.style {
        odt::LineStyle::Double => m::Underline::Double,
        odt::LineStyle::Dotted => m::Underline::Dotted,
        odt::LineStyle::Dashed
        | odt::LineStyle::LongDash
        | odt::LineStyle::DotDash
        | odt::LineStyle::DotDotDash => m::Underline::Dashed,
        odt::LineStyle::Wave => m::Underline::Wave,
        _ if u.bold => m::Underline::Thick,
        _ => m::Underline::Single,
    });
    let strike = t.strike.map_or(m::Strike::None, |s| match s.style {
        odt::LineStyle::Double => m::Strike::Double,
        _ => m::Strike::Single,
    });
    let size = t.latin.size as f64;
    let (vertical, position) = match t.position {
        Some((shift, scale)) if scale < 0.99 && shift > 0.0 => (m::VerticalAlign::Superscript, 0.0),
        Some((shift, scale)) if scale < 0.99 && shift < 0.0 => (m::VerticalAlign::Subscript, 0.0),
        Some((shift, _)) => (m::VerticalAlign::Baseline, shift as f64 * size),
        None => (m::VerticalAlign::Baseline, 0.0),
    };
    let family = |f: &odt::Face| Arc::<str>::from(f.family.as_str());
    m::RunProps {
        fonts: m::FontSet {
            ascii: family(&t.latin),
            h_ansi: family(&t.latin),
            east_asia: family(&t.asian),
            complex: family(&t.complex),
        },
        size,
        size_complex: t.complex.size as f64,
        bold: t.latin.weight >= 600,
        italic: t.latin.italic,
        bold_complex: t.complex.weight >= 600,
        italic_complex: t.complex.italic,
        underline,
        strike,
        caps: t.transform == odt::Transform::Uppercase,
        small_caps: t.small_caps,
        vertical,
        position,
        color: t.color.map(color),
        highlight: t.highlight.map(color),
        shading: None,
        spacing: t.letter_spacing as f64,
        scale: t.scale as f64,
        kerning: t.kerning,
        hidden: t.hidden || hidden,
        export: match t.export_hidden {
            true => m::Export::Always,
            false => m::Export::Shown,
        },
        complex: false,
        rtl: false,
        east_asian_hint: false,
    }
}

fn paragraph_props(p: &odt::ParaProps) -> m::ParagraphProps {
    let alignment = match (p.align, p.align_last) {
        (odt::Align::Justify, odt::Align::Justify) => m::Alignment::Distribute,
        (odt::Align::Justify, _) => m::Alignment::Justify,
        (odt::Align::Center, _) => m::Alignment::Center,
        (odt::Align::Right, _) => m::Alignment::Right,
        (odt::Align::Left, _) => m::Alignment::Left,
    };
    let line = match p.line_height {
        odt::LineHeight::Proportional(f) => m::LineSpacing::Proportional(f as f64 * 100.0),
        odt::LineHeight::Fixed(v) => m::LineSpacing::Exact(v as f64),
        odt::LineHeight::AtLeast(v) => m::LineSpacing::AtLeast(v as f64),
        odt::LineHeight::Leading(_) => m::LineSpacing::Proportional(100.0),
    };
    let borders = paragraph_borders(p);
    let inset = |b: Option<m::Border>| b.map_or(0.0, |b| b.width + b.space);
    let left = p.margin.left as f64 + inset(borders.left);
    // Writer places tab stops from the paragraph's indent, the layout from the column edge.
    let mut tabs: Vec<m::TabStop> = p
        .tab_stops
        .iter()
        .map(|t| m::TabStop {
            position: t.position as f64 + left,
            kind: match t.kind {
                odt::TabKind::Left => m::TabKind::Left,
                odt::TabKind::Center => m::TabKind::Center,
                odt::TabKind::Right => m::TabKind::Right,
                odt::TabKind::Char(_) => m::TabKind::Decimal,
            },
            leader: match t.leader {
                Some('.') => m::Leader::Dot,
                Some('-') => m::Leader::Hyphen,
                Some('_') => m::Leader::Underscore,
                Some('\u{b7}') => m::Leader::MiddleDot,
                _ => m::Leader::None,
            },
        })
        .collect();
    tabs.sort_by(|a, b| a.position.total_cmp(&b.position));
    m::ParagraphProps {
        alignment,
        indent: m::Indent {
            left,
            right: p.margin.right as f64 + inset(borders.right),
            first_line: p.text_indent as f64,
        },
        spacing: m::Spacing {
            before: p.margin.top as f64,
            after: p.margin.bottom as f64,
            line,
            ..Default::default()
        },
        contextual_spacing: p.contextual_spacing,
        keep_next: p.keep_with_next,
        keep_lines: p.keep_together,
        widow_control: p.widows > 0 || p.orphans > 0,
        page_break_before: matches!(
            p.break_before,
            odt::Break::Page | odt::Break::EvenPage | odt::Break::OddPage
        ),
        column_break_before: p.break_before == odt::Break::Column,
        tabs,
        borders,
        shading: p.background.map(color),
        rtl: p.rtl,
        off_grid: false,
        suppress_line_numbers: false,
    }
}

fn text(run: &odt::Run, hidden: bool) -> m::Inline {
    m::Inline::Text(m::Text {
        text: run.text.clone(),
        props: run_props(&run.props, hidden),
        anchors: run.locs.iter().map(|l| Anchor::from(*l)).collect(),
    })
}

fn paragraph(doc: &odt::Document, p: &odt::Paragraph, hidden_section: bool) -> m::Paragraph {
    let hidden = hidden_section || p.hidden;
    let mut inlines = Vec::new();
    for inline in &p.inlines {
        match inline {
            odt::Inline::Text(run) => inlines.push(text(run, hidden)),
            odt::Inline::Tab { props, .. } => {
                inlines.push(m::Inline::Tab(run_props(props, hidden)))
            }
            odt::Inline::LineBreak { .. } => inlines.push(m::Inline::Break(m::Break::Line)),
            odt::Inline::Field { kind, run } => match kind {
                odt::FieldKind::PageNumber => inlines.push(m::Inline::Field(
                    m::Field::Page,
                    run_props(&run.props, hidden),
                )),
                odt::FieldKind::PageCount => inlines.push(m::Inline::Field(
                    m::Field::NumPages,
                    run_props(&run.props, hidden),
                )),
                odt::FieldKind::Cached => inlines.push(text(run, hidden)),
                odt::FieldKind::Computed => {
                    let props = m::RunProps {
                        export: m::Export::Never,
                        ..run_props(&run.props, hidden)
                    };
                    inlines.extend(run.text.chars().map(|c| m::Inline::Char(c, props.clone())));
                }
            },
            odt::Inline::NoteRef {
                note,
                citation,
                props,
            } => inlines.push(m::Inline::Note(
                Arc::new(self::note(doc, &doc.notes[*note], citation)),
                run_props(props, hidden),
            )),
            odt::Inline::Frame(frame) => {
                let mark = run_props(&p.text, hidden);
                if let Some(d) = drawing::drawing(doc, frame, &mark) {
                    inlines.push(m::Inline::Drawing(Box::new(d)));
                }
            }
            odt::Inline::SoftPageBreak => {}
        }
    }
    if p.props.break_after == odt::Break::Column {
        inlines.push(m::Inline::Break(m::Break::Column));
    }
    m::Paragraph {
        props: paragraph_props(&p.props),
        mark: run_props(&p.text, hidden),
        style: p.style.clone(),
        label: p.label.as_ref().map(|l| m::Label {
            text: l.text.clone(),
            props: run_props(&l.props, hidden),
            suffix: match l.placement {
                odt::LabelPlacement::Aligned {
                    follow: odt::LabelFollow::Space,
                } => m::LabelSuffix::Space,
                odt::LabelPlacement::Aligned {
                    follow: odt::LabelFollow::Nothing,
                } => m::LabelSuffix::Nothing,
                odt::LabelPlacement::Positioned { min_width, .. } if min_width <= 0.0 => {
                    m::LabelSuffix::Nothing
                }
                _ => m::LabelSuffix::Tab,
            },
            alignment: m::Alignment::Left,
            // The label takes at least its minimum width from the first line's start.
            tab_stop: match l.placement {
                odt::LabelPlacement::Positioned { min_width, .. } if min_width > 0.0 => {
                    Some((p.props.margin.left + p.props.text_indent + min_width) as f64)
                }
                _ => None,
            },
        }),
        inlines,
        hidden_section,
    }
}

/// The note's text, its label written before the first paragraph's, as
/// Writer draws it in the note area.
fn note(doc: &odt::Document, n: &odt::Note, citation: &str) -> m::Note {
    let mut blocks = story(doc, &n.blocks, false);
    let label = run_props(&n.label_props, false);
    let first = blocks.iter_mut().find_map(|b| match b {
        m::Block::Paragraph(p) => Some(p),
        _ => None,
    });
    if let Some(p) = first {
        // Writer starts the text at the left indent when the label sits in a hanging indent.
        let hanging = p.props.indent.first_line < 0.0;
        let chars = n.label.chars().map(|c| m::Inline::Char(c, label.clone()));
        let tab = hanging.then(|| m::Inline::Tab(label.clone()));
        p.inlines.splice(0..0, chars.chain(tab));
    }
    m::Note {
        kind: match n.class {
            odt::NoteClass::Footnote => m::NoteKind::Foot,
            odt::NoteClass::Endnote => m::NoteKind::End,
        },
        label: citation.to_string(),
        blocks,
    }
}

fn table(doc: &odt::Document, t: &odt::Table, hidden: bool) -> m::Table {
    let total_rel: f32 = t.columns.iter().filter_map(|c| c.rel_width).sum();
    let width = t.props.width.map(|w| w as f64);
    let grid: Vec<f64> = t
        .columns
        .iter()
        .map(|c| match (c.width, c.rel_width, width) {
            (Some(w), _, _) => w as f64,
            (None, Some(r), Some(total)) if total_rel > 0.0 => total * (r / total_rel) as f64,
            _ => width.map_or(72.0, |w| w / t.columns.len().max(1) as f64),
        })
        .collect();
    // Cells a row span covers, by column: rows left and the span's width.
    let mut below: Vec<Option<(u32, u32)>> = vec![None; t.columns.len().max(1)];
    let mut rows = Vec::new();
    for row in &t.rows {
        let mut cells = Vec::new();
        let mut k = 0;
        while k < row.cells.len() {
            let c = &row.cells[k];
            if below.len() < row.cells.len() {
                below.resize(row.cells.len(), None);
            }
            if !c.covered {
                let span = c.col_span.max(1);
                if c.row_span > 1 {
                    below[k] = Some((c.row_span - 1, span));
                }
                cells.push(cell(
                    doc,
                    c,
                    span as usize,
                    if c.row_span > 1 {
                        m::VMerge::Restart
                    } else {
                        m::VMerge::None
                    },
                    hidden,
                ));
                k += span as usize;
                continue;
            }
            match below[k] {
                Some((left, span)) => {
                    below[k] = (left > 1).then_some((left - 1, span));
                    cells.push(cell(doc, c, span as usize, m::VMerge::Continue, hidden));
                    k += span as usize;
                }
                None => k += 1,
            }
        }
        rows.push(m::Row {
            grid_before: 0,
            cells,
            height: match (row.height, row.min_height) {
                (Some(h), _) => Some(m::RowHeight {
                    value: h as f64,
                    exact: true,
                }),
                (None, Some(h)) => Some(m::RowHeight {
                    value: h as f64,
                    exact: false,
                }),
                (None, None) => None,
            },
            cant_split: row.keep_together,
            header: row.header,
        });
    }
    m::Table {
        props: m::TableProps {
            width: match (width, t.props.rel_width) {
                (_, Some(r)) if t.props.align != odt::TableAlign::Margins => {
                    m::TableWidth::Fraction(r as f64)
                }
                (Some(w), _) => m::TableWidth::Points(w),
                _ => m::TableWidth::Auto,
            },
            alignment: match t.props.align {
                odt::TableAlign::Center => m::TableAlignment::Center,
                odt::TableAlign::Right => m::TableAlignment::Right,
                _ => m::TableAlignment::Left,
            },
            indent: t.props.margin.left as f64,
            borders: m::Borders::default(),
            cell_margins: m::Margins::default(),
            cell_spacing: 0.0,
            float: None,
            page_break_before: matches!(
                t.props.break_before,
                odt::Break::Page | odt::Break::EvenPage | odt::Break::OddPage
            ),
        },
        grid,
        rows,
    }
}

fn cell(
    doc: &odt::Document,
    c: &odt::Cell,
    span: usize,
    vmerge: m::VMerge,
    hidden: bool,
) -> m::Cell {
    let mut blocks = Vec::new();
    if vmerge != m::VMerge::Continue {
        for b in &c.blocks {
            match b {
                odt::Block::Paragraph(p) => {
                    blocks.push(m::Block::Paragraph(Box::new(paragraph(doc, p, hidden))))
                }
                odt::Block::Table(t) => {
                    blocks.push(m::Block::Table(Box::new(table(doc, t, hidden))))
                }
                odt::Block::Section(s) => blocks.extend(story(doc, &s.blocks, hidden || s.hidden)),
                odt::Block::SoftPageBreak => {}
            }
        }
        if !doc.compat.add_para_spacing_to_table_cells {
            if let Some(m::Block::Paragraph(p)) = blocks.last_mut() {
                p.props.spacing.after = 0.0;
            }
        }
    }
    m::Cell {
        span,
        vmerge,
        borders: borders(&c.props.borders),
        shading: c.props.background.map(color),
        margins: m::Margins {
            top: c.props.padding.top as f64,
            right: c.props.padding.right as f64,
            bottom: c.props.padding.bottom as f64,
            left: c.props.padding.left as f64,
        },
        valign: match c.props.valign {
            odt::VAlign::Top => m::VAlign::Top,
            odt::VAlign::Middle => m::VAlign::Center,
            odt::VAlign::Bottom => m::VAlign::Bottom,
        },
        blocks,
    }
}
