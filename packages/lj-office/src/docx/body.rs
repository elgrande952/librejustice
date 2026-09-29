//! One story of the package, the body or a header or a cell: paragraphs, runs
//! and fields, every character of a text node anchored where the splice will
//! look for it.

use std::sync::Arc;

use crate::Anchor;
use anyhow::Result;

use super::props::{ParaPatch, RunPatch};
use super::styles::TableLayers;
use super::xml::{Element, Node, Ns};
use super::{Context, Rels};
use crate::model::*;
use crate::numbering::{number, NumFormat};

/// A complex field between its `begin` and its `end`.
struct FieldFrame {
    instruction: String,
    /// Past `separate`: what follows is the result Word cached.
    result: bool,
    /// A field the layout computes, whose cached result is not shown.
    computed: Option<Field>,
    /// This frame and every one around it are past `separate`.
    resolved: bool,
    /// Text read in this frame is shown by it and by every one around it.
    shown: bool,
    /// This frame or one around it is past the `separate` of an index field.
    index: bool,
    /// The entry a drop-down form field shows.
    choice: Option<String>,
}

/// The entry a drop-down form field shows: its result, else its default,
/// else its first.
fn choice(begin: &Element) -> Option<String> {
    let list = begin.wc("ffData")?.wc("ddList")?;
    let index = |name: &str| list.wc(name).and_then(|e| e.val()?.parse::<usize>().ok());
    let entries: Vec<&str> = list
        .elements()
        .filter(|e| e.is(Ns::W, "listEntry"))
        .filter_map(|e| e.val())
        .collect();
    let at = index("result").or_else(|| index("default")).unwrap_or(0);
    entries.get(at).or(entries.first()).map(|e| e.to_string())
}

fn index(instruction: &str) -> bool {
    instruction
        .split_whitespace()
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("TOC") || name.eq_ignore_ascii_case("INDEX"))
}

fn computed(instruction: &str) -> Option<Field> {
    match instruction
        .split_whitespace()
        .next()?
        .to_ascii_uppercase()
        .as_str()
    {
        "PAGE" => Some(Field::Page),
        "NUMPAGES" => Some(Field::NumPages),
        // SECTIONPAGES keeps its cached result, which LibreOffice shows.
        _ => None,
    }
}

/// A paragraph frame: the paragraphs that share it float together in a text
/// frame anchored at the next paragraph, as LibreOffice converts them.
struct Frame {
    key: Vec<Option<String>>,
    float: Float,
    width: Option<f64>,
    height: f64,
    exact: bool,
}

/// What sets a frame apart, paragraphs alike in all of it sharing one.
const FRAME_KEYS: [&str; 12] = [
    "w", "h", "hRule", "hSpace", "vSpace", "wrap", "hAnchor", "vAnchor", "x", "xAlign", "y",
    "yAlign",
];

impl Frame {
    fn read(e: &Element) -> Option<Self> {
        if e.w("dropCap").is_some_and(|v| v != "none") {
            return None;
        }
        let twips = |name: &str| {
            e.w(name)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| v / 20.0)
        };
        let place = |align: Option<&str>, offset: &str| match align {
            Some("left" | "top") => Place::Start,
            Some("center") => Place::Center,
            Some("right" | "bottom") => Place::End,
            Some("inside") => Place::Inside,
            Some("outside") => Place::Outside,
            _ => Place::Offset(twips(offset).unwrap_or(0.0)),
        };
        let horizontal = Position {
            from: match e.w("hAnchor") {
                Some("page") => Relative::Page,
                Some("margin") => Relative::Margin,
                _ => Relative::Column,
            },
            place: place(e.w("xAlign"), "x"),
        };
        let vertical = Position {
            from: match e.w("vAnchor") {
                Some("page") => Relative::Page,
                Some("margin") => Relative::Margin,
                _ => Relative::Paragraph,
            },
            place: place(e.w("yAlign").filter(|a| *a != "inline"), "y"),
        };
        let (wrap, side) = match e.w("wrap") {
            Some("none") => (Wrap::None, WrapSide::Both),
            Some("notBeside") => (Wrap::TopAndBottom, WrapSide::Both),
            Some("around") => (Wrap::Around, WrapSide::Both),
            _ => (Wrap::Around, WrapSide::Largest),
        };
        let (across, down) = (
            twips("hSpace").unwrap_or(0.0),
            twips("vSpace").unwrap_or(0.0),
        );
        let key = FRAME_KEYS
            .iter()
            .map(|name| e.w(name).map(str::to_string))
            .collect();
        Some(Self {
            key,
            float: Float {
                horizontal,
                vertical,
                wrap,
                side,
                behind: false,
                distance: Margins {
                    top: down,
                    right: across,
                    bottom: down,
                    left: across,
                },
                z: 0,
                contour: Vec::new(),
                in_cell: true,
            },
            width: twips("w").filter(|w| *w > 0.0),
            height: twips("h").unwrap_or(0.0).max(0.0),
            exact: e.w("hRule") == Some("exact"),
        })
    }

    fn drawing(self, paragraphs: Vec<Paragraph>) -> Drawing {
        let blocks = paragraphs
            .into_iter()
            .map(|p| Block::Paragraph(Box::new(p)))
            .collect();
        Drawing::paragraph_frame(self.float, self.width, self.height, self.exact, blocks)
    }
}

pub(super) struct Story<'c, 'p> {
    pub ctx: &'c Context<'p>,
    part: u16,
    /// Open complex fields, which may span paragraphs.
    fields: Vec<FieldFrame>,
    /// The part's relationships, which name its pictures.
    pub rels: Option<&'c Rels>,
    /// The label of the note being read, which its reference mark writes.
    mark: Option<String>,
    /// Tables open around the current block.
    pub tables: usize,
    /// Tables and text boxes open around the block being read.
    nesting: usize,
    /// The body paragraphs within a table of contents, by index.
    pub contents: Vec<usize>,
    /// Paragraphs whose mark lies within a field code, joined to the next.
    held: Option<Paragraph>,
    /// Frames waiting for the paragraph that anchors them.
    framed: Vec<(Frame, Vec<Paragraph>)>,
    /// The part and relationships of the SmartArt drawing being read.
    pub diagram: Option<(Option<u16>, Rels)>,
}

/// Tables and text boxes one may nest in another; what lies deeper is
/// dropped, so the walks of the model and the layout fit a wasm stack.
const MAX_NESTING: usize = 64;

#[derive(Clone, Copy)]
struct Scope<'a> {
    table_style: Option<TableLayers<'a>>,
    paragraph_style: Option<&'a str>,
    deleted: bool,
}

impl<'c, 'p> Story<'c, 'p> {
    pub fn new(ctx: &'c Context<'p>, part: u16) -> Self {
        Self {
            ctx,
            part,
            fields: Vec::new(),
            rels: None,
            mark: None,
            tables: 0,
            nesting: 0,
            contents: Vec::new(),
            held: None,
            framed: Vec::new(),
            diagram: None,
        }
    }

    pub fn with_rels(mut self, rels: &'c Rels) -> Self {
        self.rels = Some(rels);
        self
    }

    /// The blocks of a text box, read apart from the fields open around it.
    pub fn text_box(&mut self, container: &Element) -> Result<Vec<Block>> {
        if self.nesting == MAX_NESTING {
            return Ok(Vec::new());
        }
        let open = std::mem::take(&mut self.fields);
        let held = self.held.take();
        let framed = std::mem::take(&mut self.framed);
        let mut blocks = Vec::new();
        self.nesting += 1;
        let read = self.blocks(container, None, &mut blocks, &mut Vec::new());
        self.nesting -= 1;
        self.fields = open;
        self.held = held;
        self.framed = framed;
        read.map(|()| blocks)
    }

    /// The blocks of a body, cell or header, each paragraph that closes a
    /// section noted in `ends` with its `w:sectPr`.
    pub fn blocks<'e>(
        &mut self,
        container: &'e Element,
        table_style: Option<TableLayers<'_>>,
        out: &mut Vec<Block>,
        ends: &mut Vec<(usize, &'e Element)>,
    ) -> Result<()> {
        self.each_block(container, table_style, out, ends)?;
        self.settle(out);
        Ok(())
    }

    /// Frames no paragraph follows to anchor them, set in the text as they
    /// are, then the paragraph a field code holds.
    fn settle(&mut self, out: &mut Vec<Block>) {
        for (_, paragraphs) in self.framed.drain(..) {
            out.extend(
                paragraphs
                    .into_iter()
                    .map(|p| Block::Paragraph(Box::new(p))),
            );
        }
        if let Some(paragraph) = self.held.take() {
            out.push(Block::Paragraph(Box::new(paragraph)));
        }
    }

    fn each_block<'e>(
        &mut self,
        container: &'e Element,
        table_style: Option<TableLayers<'_>>,
        out: &mut Vec<Block>,
        ends: &mut Vec<(usize, &'e Element)>,
    ) -> Result<()> {
        for child in container.elements() {
            if child.ns != Ns::W {
                continue;
            }
            match &*child.name {
                "p" => {
                    let within = self.in_contents();
                    let (mut paragraph, section) = self.paragraph(child, table_style);
                    if let Some(mut first) = self.held.take() {
                        first.inlines.append(&mut paragraph.inlines);
                        paragraph.inlines = first.inlines;
                    }
                    // A paragraph mark within a field code is hidden with it.
                    if section.is_none() && self.fields.last().is_some_and(|f| !f.result) {
                        self.held = Some(paragraph);
                        continue;
                    }
                    let frame = self.frame_of(child).and_then(Frame::read);
                    if let Some(frame) = frame.filter(|_| section.is_none() && self.tables == 0) {
                        match self.framed.last_mut() {
                            Some((last, paragraphs)) if last.key == frame.key => {
                                paragraphs.push(paragraph)
                            }
                            _ => self.framed.push((frame, vec![paragraph])),
                        }
                        continue;
                    }
                    for (frame, paragraphs) in self.framed.drain(..).rev() {
                        let drawing = frame.drawing(paragraphs);
                        paragraph
                            .inlines
                            .insert(0, Inline::Drawing(Box::new(drawing)));
                    }
                    if self.nesting == 0 && (within || self.in_contents()) {
                        self.contents.push(out.len());
                    }
                    out.push(Block::Paragraph(Box::new(paragraph)));
                    if let Some(section) = section {
                        ends.push((out.len() - 1, section));
                    }
                    // LibreOffice sets a table written inside a paragraph after it.
                    for table in child.elements().filter(|e| e.is(Ns::W, "tbl")) {
                        self.table_block(table, out)?;
                    }
                }
                "tbl" => {
                    self.settle(out);
                    self.table_block(child, out)?;
                }
                "sdt" => {
                    if let Some(content) = child.wc("sdtContent") {
                        self.each_block(content, table_style, out, ends)?;
                    }
                }
                "customXml" | "ins" | "moveTo" => self.each_block(child, table_style, out, ends)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// The `w:framePr` of a paragraph, its own or its style's.
    fn frame_of<'e>(&'e self, p: &'e Element) -> Option<&'e Element> {
        let ppr = p.wc("pPr");
        if let Some(frame) = ppr.and_then(|e| e.wc("framePr")) {
            return Some(frame);
        }
        let styles = &self.ctx.styles;
        let named = ppr.and_then(|e| e.wc("pStyle")).and_then(|e| e.val());
        let id = styles.paragraph_style(named)?;
        styles
            .chain(id)
            .into_iter()
            .rev()
            .find_map(|s| s.frame.as_ref())
    }

    fn table_block(&mut self, element: &Element, out: &mut Vec<Block>) -> Result<()> {
        if self.nesting == MAX_NESTING {
            return Ok(());
        }
        self.nesting += 1;
        let table = self.table(element);
        self.nesting -= 1;
        let mut table = table?;
        // A table glued to the previous one continues it, without a break.
        if matches!(out.last(), Some(Block::Table(_))) {
            table.props.page_break_before = false;
        }
        out.push(Block::Table(Box::new(table)));
        Ok(())
    }

    fn paragraph<'e>(
        &mut self,
        p: &'e Element,
        table_style: Option<TableLayers<'_>>,
    ) -> (Paragraph, Option<&'e Element>) {
        let ctx = self.ctx;
        let styles = &ctx.styles;
        let ppr = p.wc("pPr");
        let patch = ppr.map(ParaPatch::read).unwrap_or_default();
        let named = patch.style.as_deref();
        let numbered = ctx
            .numbering
            .paragraph(styles, &patch, styles.paragraph_style(named));
        let props = match &numbered {
            Some(n) => styles.paragraph_layered(table_style, named, &|p| n.indent(p), &patch),
            None => styles.paragraph(table_style, named, &patch),
        };
        let mark = ppr
            .and_then(|e| e.wc("rPr"))
            .map(RunPatch::read)
            .unwrap_or_default();
        let mark = styles.run(&ctx.theme, table_style, named, &mark);
        let label = numbered.map(|n| {
            let mut props = mark.clone();
            n.level.run.apply(&mut props, &ctx.theme);
            Label {
                tab_stop: n
                    .level
                    .tab
                    .filter(|_| n.visible() && n.level.suffix == LabelSuffix::Tab),
                text: n.text,
                props,
                suffix: n.level.suffix,
                alignment: n.level.alignment,
            }
        });
        let style = styles.paragraph_style(named).map(str::to_string);
        let scope = Scope {
            table_style,
            paragraph_style: style.as_deref(),
            deleted: false,
        };
        let mut inlines = Vec::new();
        self.inlines(p, scope, &mut inlines);
        let paragraph = Paragraph {
            props,
            mark,
            style,
            label,
            inlines,
            hidden_section: false,
        };
        (paragraph, ppr.and_then(|e| e.wc("sectPr")))
    }

    /// Whether text read now is shown: outside every field instruction, and
    /// not the cached result of a field the layout computes.
    fn shown(&self) -> bool {
        self.fields.last().is_none_or(|f| f.shown)
    }

    fn inlines(&mut self, container: &Element, scope: Scope<'_>, out: &mut Vec<Inline>) {
        for child in container.elements() {
            match (child.ns, &*child.name) {
                (Ns::W, "r") => self.run(child, scope, out),
                (Ns::W, "del" | "moveFrom") => self.inlines(
                    child,
                    Scope {
                        deleted: true,
                        ..scope
                    },
                    out,
                ),
                (
                    Ns::W,
                    "hyperlink" | "ins" | "moveTo" | "smartTag" | "customXml" | "bdo" | "dir",
                ) => self.inlines(child, scope, out),
                (Ns::W, "sdt") => {
                    if let Some(content) = child.wc("sdtContent") {
                        self.inlines(content, scope, out);
                    }
                }
                (Ns::W, "fldSimple") => match child.w("instr").and_then(computed) {
                    Some(field) if self.shown() => {
                        let props = self.run_props(None, scope);
                        out.push(Inline::Field(field, props));
                    }
                    Some(_) => {}
                    None => self.inlines(child, scope, out),
                },
                (Ns::M, _) => self.inlines(child, scope, out),
                _ => {}
            }
        }
    }

    fn in_contents(&self) -> bool {
        self.fields.iter().any(|f| {
            f.result
                && f.instruction
                    .split_whitespace()
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("TOC"))
        })
    }

    /// Within an index field's result, LibreOffice drops the character style.
    fn in_index(&self) -> bool {
        self.fields.last().is_some_and(|f| f.index)
    }

    fn run_props(&self, rpr: Option<&Element>, scope: Scope<'_>) -> RunProps {
        let mut patch = rpr.map(RunPatch::read).unwrap_or_default();
        if self.in_index() {
            patch.style = None;
        }
        self.ctx.styles.run(
            &self.ctx.theme,
            scope.table_style,
            scope.paragraph_style,
            &patch,
        )
    }

    fn run(&mut self, r: &Element, scope: Scope<'_>, out: &mut Vec<Inline>) {
        let mut props = self.run_props(r.wc("rPr"), scope);
        if scope.deleted && props.strike == Strike::None {
            props.strike = Strike::Single;
        }
        for child in r.elements() {
            if !matches!(child.ns, Ns::W | Ns::M) {
                continue;
            }
            match &*child.name {
                "t" | "delText" => {
                    if !self.shown() {
                        continue;
                    }
                    for node in &child.children {
                        let Node::Text(number, text) = node else {
                            continue;
                        };
                        let anchors = (0..text.chars().count() as u32)
                            .map(|offset| Anchor::XmlText {
                                part: self.part,
                                node: *number,
                                offset,
                            })
                            .collect();
                        out.push(Inline::Text(Text {
                            text: text.to_string(),
                            props: props.clone(),
                            anchors,
                        }));
                    }
                }
                "instrText" | "delInstrText" => {
                    if let Some(field) = self.fields.last_mut() {
                        for node in &child.children {
                            if let Node::Text(_, text) = node {
                                field.instruction.push_str(text);
                            }
                        }
                    }
                }
                "fldChar" => self.field_char(child, &props, out),
                _ if !self.shown() => {}
                "drawing" | "pict" | "object" => self.drawing(child, &props, out),
                "tab" | "ptab" => out.push(Inline::Tab(props.clone())),
                "br" => out.push(Inline::Break(match child.w("type") {
                    Some("page") => Break::Page,
                    Some("column") => Break::Column,
                    _ => Break::Line,
                })),
                "cr" => out.push(Inline::Break(Break::Line)),
                "noBreakHyphen" => out.push(Inline::Char('\u{2011}', props.clone())),
                "softHyphen" => out.push(Inline::Char('\u{ad}', props.clone())),
                "sym" => {
                    let Some(c) = child
                        .w("char")
                        .and_then(|v| u32::from_str_radix(v, 16).ok())
                        .and_then(char::from_u32)
                    else {
                        continue;
                    };
                    let mut props = props.clone();
                    if let Some(font) = child.w("font") {
                        let font: Arc<str> = Arc::from(font);
                        props.fonts = FontSet {
                            ascii: font.clone(),
                            h_ansi: font.clone(),
                            east_asia: font.clone(),
                            complex: font,
                        };
                    }
                    out.push(Inline::Char(c, props));
                }
                "pgNum" => out.push(Inline::Field(Field::Page, props.clone())),
                "footnoteReference" | "endnoteReference" => {
                    if let Some(note) = self.note(child) {
                        out.push(Inline::Note(note, props.clone()));
                    }
                }
                "footnoteRef" | "endnoteRef" => {
                    for c in self.mark.iter().flat_map(|m| m.chars()) {
                        out.push(Inline::Char(c, props.clone()));
                    }
                }
                _ => {}
            }
        }
    }

    /// The note a reference names, numbered in reading order unless the
    /// reference writes its own mark.
    fn note(&self, reference: &Element) -> Option<Arc<Note>> {
        let (kind, slot) = match &*reference.name {
            "footnoteReference" => (NoteKind::Foot, 0),
            _ => (NoteKind::End, 1),
        };
        let part = match kind {
            NoteKind::Foot => self.ctx.footnotes.as_ref()?,
            NoteKind::End => self.ctx.endnotes.as_ref()?,
        };
        let id = reference.w("id")?;
        let body = part.root.elements().find(|e| e.w("id") == Some(id))?;
        let label = if reference
            .w("customMarkFollows")
            .is_some_and(|v| v != "0" && v != "false")
        {
            String::new()
        } else {
            let mut numbered = self.ctx.numbered.get();
            numbered[slot] += 1;
            self.ctx.numbered.set(numbered);
            let format = match kind {
                NoteKind::Foot => NumFormat::DECIMAL,
                NoteKind::End => NumFormat::LowerRoman,
            };
            number(numbered[slot], format)
        };
        let mut story = Story::new(self.ctx, part.part).with_rels(&part.rels);
        story.mark = Some(label.clone());
        let mut blocks = Vec::new();
        story
            .blocks(body, None, &mut blocks, &mut Vec::new())
            .ok()?;
        join_hidden_marks(&mut blocks);
        Some(Arc::new(Note {
            kind,
            label,
            blocks,
        }))
    }

    fn field_char(&mut self, element: &Element, props: &RunProps, out: &mut Vec<Inline>) {
        match element.w("fldCharType") {
            Some("begin") => self.fields.push(FieldFrame {
                instruction: String::new(),
                result: false,
                computed: None,
                resolved: false,
                shown: false,
                index: self.in_index(),
                choice: choice(element),
            }),
            Some("separate") => {
                let outer = self.fields.len().saturating_sub(1);
                let (visible, shown) = self.fields[..outer]
                    .last()
                    .map_or((true, true), |f| (f.resolved, f.shown));
                if let Some(field) = self.fields.last_mut() {
                    field.result = true;
                    field.computed = computed(&field.instruction);
                    field.resolved = visible;
                    field.shown = shown && field.computed.is_none();
                    field.index |= index(&field.instruction);
                    if let (Some(kind), true) = (field.computed, visible) {
                        out.push(Inline::Field(kind, props.clone()));
                    }
                }
            }
            Some("end") => {
                if let Some(field) = self.fields.pop() {
                    let visible = self.fields.last().is_none_or(|f| f.resolved);
                    if !field.result && visible {
                        if let Some(kind) = computed(&field.instruction) {
                            out.push(Inline::Field(kind, props.clone()));
                        }
                        let dropdown =
                            field.instruction.split_whitespace().next() == Some("FORMDROPDOWN");
                        if let (true, Some(text)) = (dropdown, field.choice) {
                            out.extend(text.chars().map(|c| Inline::Char(c, props.clone())));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
