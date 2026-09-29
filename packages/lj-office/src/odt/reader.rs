//! From the parts of a package to a [`Document`]: styles resolved, lists
//! numbered, whitespace collapsed the way ODF prescribes, every character
//! anchored in the leaf it came from.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use super::chart;
use super::container::{deletions, OdtPackage};
use super::geometry::{Enhanced, ShapePath};
use super::lists::{self, Counters, ListStyle};
use super::model::*;
use super::styles::{self, apply_borders, apply_sides, background, brk, Def, Scope, Styles};
use super::units;
use super::xml::{Child, Element, Kind, Ns, Xml};

/// Nesting past this is a hostile or broken file; what lies deeper is dropped.
const MAX_DEPTH: usize = 64;
/// Repeated rows, columns and cells are expanded up to this count.
const MAX_REPEAT: u32 = 1024;

#[derive(Clone, Copy)]
struct Ctx<'a> {
    part: u16,
    xml: &'a Xml,
    scope: Scope,
    depth: usize,
}

impl<'a> Ctx<'a> {
    fn deeper(self) -> Self {
        Ctx {
            depth: self.depth + 1,
            ..self
        }
    }
}

/// One list, possibly continued by later `text:list` elements.
struct Instance {
    counters: Counters,
}

/// Where a paragraph sits in a list.
#[derive(Clone)]
struct Item {
    instance: usize,
    style: Option<String>,
    level: usize,
    /// The next paragraph of the item carries its label.
    pending: bool,
    header: bool,
    restart: Option<u32>,
    /// The next paragraph of the list restarts its level: Writer restarts a
    /// nested list that follows another in the same item.
    block_restart: bool,
    /// A nested list naming a style of its own: Writer labels its paragraphs
    /// from their paragraph style's list, or with bullets.
    foreign: bool,
}

struct NotesConfig {
    /// The mark before the note's own text.
    citation_style: Option<String>,
    /// The mark in the text.
    body_style: Option<String>,
    format: NumFormat,
    start: u32,
    prefix: String,
    suffix: String,
}

/// A paragraph's list: its style, counters instance, level, whether it shows a
/// label, and the value it restarts at.
type ListSource<'a> = (Option<Rc<ListStyle<'a>>>, usize, usize, bool, Option<u32>);

#[derive(Default)]
struct Grid<'a> {
    default_cells: Vec<Option<&'a str>>,
    /// Rows each column stays covered by a cell spanning down from above.
    rows_left: Vec<u32>,
}

pub struct Reader<'a> {
    pkg: &'a OdtPackage,
    styles: Styles<'a>,
    page_index: HashMap<String, usize>,
    notes: Vec<Note>,
    page_frames: Vec<Frame>,
    images: Vec<Image>,
    image_index: HashMap<(usize, String), usize>,
    list_cache: HashMap<(Scope, String), Rc<ListStyle<'a>>>,
    outline: Option<Rc<ListStyle<'a>>>,
    outline_name: Option<&'a str>,
    default_list: Rc<ListStyle<'a>>,
    outline_counters: Counters,
    instances: Vec<Instance>,
    list_ids: HashMap<String, usize>,
    last_by_style: HashMap<String, usize>,
    notes_config: HashMap<String, NotesConfig>,
    footnotes: u32,
    endnotes: u32,
    compat: Compat,
    /// The deletion each change identifier names, by part.
    deletions: HashMap<u16, HashMap<&'a str, u32>>,
    /// Deletions already drawn at a mark.
    inlined: HashSet<(u16, u32)>,
    /// The names a reference field can point to, read on the first one.
    targets: Option<HashSet<&'a str>>,
}

pub fn read(pkg: &OdtPackage) -> Result<Document> {
    read_as(pkg, false)
}

/// [`read`], each tracked deletion drawn where it was made whatever the file
/// says of showing changes.
pub(crate) fn read_showing_changes(pkg: &OdtPackage) -> Result<Document> {
    read_as(pkg, true)
}

fn read_as(pkg: &OdtPackage, show_changes: bool) -> Result<Document> {
    let content_part = pkg
        .part(0, "content.xml")
        .ok_or_else(|| anyhow!("no content.xml"))?;
    let styles_part = pkg.part(0, "styles.xml");
    let content = &pkg.parts[content_part].xml;
    let styles_xml = styles_part.map(|i| &pkg.parts[i].xml);
    let styles = Styles::new(content, styles_xml);
    let outline = styles.outline.map(|d| Rc::new(lists::parse(d.xml, d.el)));
    let outline_name = styles.outline.and_then(|d| d.el.attr(Ns::Style, "name"));
    let mut r = Reader {
        pkg,
        styles,
        page_index: HashMap::new(),
        notes: Vec::new(),
        page_frames: Vec::new(),
        images: Vec::new(),
        image_index: HashMap::new(),
        list_cache: HashMap::new(),
        outline,
        outline_name,
        default_list: Rc::new(lists::default_bullets()),
        outline_counters: Counters::default(),
        instances: Vec::new(),
        list_ids: HashMap::new(),
        last_by_style: HashMap::new(),
        notes_config: HashMap::new(),
        footnotes: 0,
        endnotes: 0,
        compat: {
            let mut compat: Compat = pkg
                .part(0, "settings.xml")
                .map(|i| read_compat(&pkg.parts[i].xml))
                .unwrap_or_default();
            compat.show_changes |= show_changes;
            compat
        },
        deletions: HashMap::new(),
        inlined: HashSet::new(),
        targets: None,
    };
    for xml in styles_xml.into_iter().chain([content]) {
        r.read_notes_config(xml);
    }

    for (i, m) in r.styles.master_pages.iter().enumerate() {
        if let Some(name) = m.attr(Ns::Style, "name") {
            r.page_index.entry(name.to_string()).or_insert(i);
        }
    }
    let masters: Vec<Def> = r.styles.master_pages.clone();
    let mut pages = Vec::with_capacity(masters.len());
    for m in &masters {
        let ctx = Ctx {
            part: styles_part.unwrap_or(content_part) as u16,
            xml: m.xml,
            scope: Scope::Styles,
            depth: 0,
        };
        pages.push(r.page_style(ctx, *m));
    }

    let ctx = Ctx {
        part: content_part as u16,
        xml: content,
        scope: Scope::Content,
        depth: 0,
    };
    let mut body = Vec::new();
    let text = content
        .root()
        .and_then(|root| content.child(root, Ns::Office, "body"))
        .and_then(|b| content.child(b, Ns::Office, "text"))
        .ok_or_else(|| anyhow!("content.xml has no office:text"))?;
    r.blocks(ctx, text, &mut body);

    // Nothing naming a page style, Writer opens on its built-in default,
    // bare of the headers a master page of another name carries.
    let first_page = match first_page_style(&body).or_else(|| r.page_index.get("Standard").copied())
    {
        Some(i) => i,
        None => {
            pages.push(default_page());
            pages.len() - 1
        }
    };
    let meta = pkg
        .part(0, "meta.xml")
        .map(|i| read_meta(&pkg.parts[i].xml))
        .unwrap_or_default();
    let fonts = r
        .styles
        .font_order
        .iter()
        .filter_map(|name| {
            let decl = r.styles.face(name)?;
            Some(FontFace {
                name: name.clone(),
                family: decl.family.clone(),
                generic: decl.generic.clone(),
                fixed_pitch: decl.fixed_pitch,
                embedded: decl
                    .embedded
                    .iter()
                    .filter_map(|href| pkg.entry(0, href.strip_prefix("./").unwrap_or(href)))
                    .filter(|file| crate::fonts::readable(file))
                    .map(Arc::from)
                    .collect(),
            })
        })
        .collect();
    Ok(Document {
        meta,
        compat: r.compat,
        pages,
        first_page,
        body,
        notes: r.notes,
        page_frames: r.page_frames,
        fonts,
        images: r.images,
    })
}

fn first_page_style(body: &[Block]) -> Option<usize> {
    for block in body {
        match block {
            Block::Paragraph(p) => return p.page_style,
            Block::Table(t) => return t.page_style,
            Block::Section(s) => {
                if let Some(p) = first_page_style(&s.blocks) {
                    return Some(p);
                }
                return None;
            }
            Block::SoftPageBreak => {}
        }
    }
    None
}

/// A page style that names no size is Letter, as LibreOffice sets it under
/// the en-US locale the bench reference runs in.
fn default_page() -> PageStyle {
    PageStyle {
        name: "Standard".to_string(),
        width: 612.0,
        height: 792.0,
        margin: Sides {
            top: 56.7,
            right: 56.7,
            bottom: 56.7,
            left: 56.7,
        },
        landscape: false,
        num_format: NumFormat::Decimal,
        background: None,
        borders: Sides::default(),
        padding: Sides::default(),
        columns: Columns::default(),
        rtl: false,
        usage: "all".to_string(),
        header: None,
        footer: None,
        footnote_max_height: None,
        next: None,
    }
}

/// The paragraphs under `e` in document order, lists, sections and tables
/// walked through.
fn collect_paragraphs<'x>(xml: &'x Xml, e: &'x Element, depth: usize, out: &mut Vec<&'x Element>) {
    if depth > MAX_DEPTH {
        return;
    }
    for c in xml.children(e) {
        match (c.ns, c.local.as_str()) {
            (Ns::Text, "p" | "h") => out.push(c),
            (Ns::Office, "change-info") => {}
            _ => collect_paragraphs(xml, c, depth + 1, out),
        }
    }
}

fn read_compat(xml: &Xml) -> Compat {
    let mut items = HashMap::new();
    let mut found = false;
    if let Some(root) = xml.root() {
        let sets = xml
            .children(root)
            .filter(|e| e.is(Ns::Office, "settings"))
            .flat_map(|s| xml.children(s))
            .filter(|e| {
                matches!(
                    e.attr(Ns::Config, "name"),
                    Some("ooo:configuration-settings" | "ooo:view-settings")
                )
            });
        for set in sets {
            found |= set.attr(Ns::Config, "name") == Some("ooo:configuration-settings");
            for item in xml
                .children(set)
                .filter(|e| e.is(Ns::Config, "config-item"))
            {
                if let Some(name) = item.attr(Ns::Config, "name") {
                    items.insert(name, xml.text_of(item));
                }
            }
        }
    }
    let flag = |name: &str| items.get(name).map(|v| v == "true");
    // A file that predates StarOffice 8 lacks this item.
    let prior_so8 = found && !items.contains_key("ConsiderTextWrapOnObjPos");
    Compat {
        ignore_first_line_indent_in_numbering: flag("IgnoreFirstLineIndentInNumbering")
            .unwrap_or(prior_so8),
        add_para_table_spacing: flag("AddParaTableSpacing").unwrap_or(true),
        add_para_spacing_to_table_cells: flag("AddParaSpacingToTableCells").unwrap_or(!found),
        justify_lines_with_shrinking: flag("JustifyLinesWithShrinking").unwrap_or(false),
        show_changes: flag("ShowRedlineChanges").unwrap_or(true),
        add_external_leading: flag("AddExternalLeading").unwrap_or(true),
        continuous_endnotes: flag("ContinuousEndnotes").unwrap_or(false),
        add_para_table_spacing_at_start: flag("AddParaTableSpacingAtStart").unwrap_or(true),
    }
}

fn read_meta(xml: &Xml) -> Meta {
    let mut meta = Meta::default();
    let Some(root) = xml.root() else {
        return meta;
    };
    let Some(office) = xml.child(root, Ns::Office, "meta") else {
        return meta;
    };
    for e in xml.children(office) {
        let text = || xml.text_of(e);
        match (e.ns, e.local.as_str()) {
            (Ns::Dc, "title") => meta.title = text(),
            (Ns::Dc, "subject") => meta.subject = text(),
            (Ns::Dc, "description") => meta.description = text(),
            (Ns::Dc, "creator") => meta.creator = text(),
            (Ns::Dc, "language") => meta.language = text(),
            (Ns::Dc, "date") => meta.modified = text(),
            (Ns::Meta, "initial-creator") => meta.initial_creator = text(),
            (Ns::Meta, "keyword") => meta.keywords.push(text()),
            (Ns::Meta, "generator") => meta.generator = text(),
            (Ns::Meta, "creation-date") => meta.created = text(),
            (Ns::Meta, "document-statistic") => {
                meta.page_count = e.attr(Ns::Meta, "page-count").and_then(|p| p.parse().ok())
            }
            (Ns::Meta, "user-defined") => meta.user_defined.push((
                e.attr(Ns::Meta, "name").unwrap_or_default().to_string(),
                text(),
            )),
            _ => {}
        }
    }
    meta
}

fn base64(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn sniff_mime(path: &str, bytes: &[u8]) -> String {
    let lower = path.to_ascii_lowercase();
    let by_magic = if bytes.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"\xd7\xcd\xc6\x9a") || bytes.starts_with(b"\x01\x00\x09\x00") {
        Some("image/x-wmf")
    } else if bytes.get(40..44) == Some(b" EMF") {
        Some("image/x-emf")
    } else if bytes.starts_with(b"VCLMTF") {
        Some("image/x-svm")
    } else {
        None
    };
    if let Some(m) = by_magic {
        return m.to_string();
    }
    if lower.ends_with(".svg") || bytes.windows(4).take(512).any(|w| w == b"<svg") {
        return "image/svg+xml".to_string();
    }
    "application/octet-stream".to_string()
}

fn displayable(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/bmp" | "image/webp" | "image/svg+xml"
    )
}

impl<'a> Reader<'a> {
    fn read_notes_config(&mut self, xml: &'a Xml) {
        for e in &xml.elements {
            if e.is(Ns::Text, "notes-configuration") {
                let class = e
                    .attr(Ns::Text, "note-class")
                    .unwrap_or("footnote")
                    .to_string();
                self.notes_config.insert(
                    class,
                    NotesConfig {
                        citation_style: e.attr(Ns::Text, "citation-style-name").map(str::to_string),
                        body_style: e
                            .attr(Ns::Text, "citation-body-style-name")
                            .map(str::to_string),
                        format: lists::num_format(e.attr(Ns::Style, "num-format")),
                        start: e
                            .attr(Ns::Text, "start-value")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0),
                        prefix: e
                            .attr(Ns::Style, "num-prefix")
                            .unwrap_or_default()
                            .to_string(),
                        suffix: e
                            .attr(Ns::Style, "num-suffix")
                            .unwrap_or_default()
                            .to_string(),
                    },
                );
            }
        }
    }

    fn page_style(&mut self, ctx: Ctx<'a>, master: Def<'a>) -> PageStyle {
        let mut page = default_page();
        page.name = master
            .attr(Ns::Style, "name")
            .unwrap_or_default()
            .to_string();
        let layout = master
            .attr(Ns::Style, "page-layout-name")
            .and_then(|n| self.styles.page_layouts.get(n).copied());
        let mut header_props = None;
        let mut footer_props = None;
        if let Some(layout) = layout {
            if let Some(usage) = layout.attr(Ns::Style, "page-usage") {
                page.usage = usage.to_string();
            }
            if let Some(p) = layout.props("page-layout-properties") {
                let positive = |v: &f32| *v > 0.0;
                if let Some(w) = p
                    .attr(Ns::Fo, "page-width")
                    .and_then(units::length)
                    .filter(positive)
                {
                    page.width = w;
                }
                if let Some(h) = p
                    .attr(Ns::Fo, "page-height")
                    .and_then(units::length)
                    .filter(positive)
                {
                    page.height = h;
                }
                page.margin = Sides::default();
                apply_sides(&mut page.margin, p, "margin");
                page.landscape = p.attr(Ns::Style, "print-orientation") == Some("landscape");
                page.num_format = lists::num_format(p.attr(Ns::Style, "num-format"));
                if let Some(bg) = background(p) {
                    page.background = bg;
                }
                apply_borders(&mut page.borders, p);
                apply_sides(&mut page.padding, p, "padding");
                if let Some(c) = styles::columns(layout.xml, p) {
                    page.columns = c;
                }
                page.rtl = p
                    .attr(Ns::Style, "writing-mode")
                    .is_some_and(|w| w.starts_with("rl"));
                page.footnote_max_height = p
                    .attr(Ns::Style, "footnote-max-height")
                    .and_then(units::length)
                    .filter(|h| *h > 0.0);
            }
            header_props = layout
                .props("header-style")
                .map(|h| layout.xml.child(h, Ns::Style, "header-footer-properties"));
            footer_props = layout
                .props("footer-style")
                .map(|h| layout.xml.child(h, Ns::Style, "header-footer-properties"));
        }
        page.header = self.header_footer(ctx, master, "header", header_props.flatten());
        page.footer = self.header_footer(ctx, master, "footer", footer_props.flatten());
        page.next = master
            .attr(Ns::Style, "next-style-name")
            .and_then(|n| self.page_index.get(n).copied());
        page
    }

    fn header_footer(
        &mut self,
        ctx: Ctx<'a>,
        master: Def<'a>,
        which: &str,
        props: Option<&'a Element>,
    ) -> Option<HeaderFooter> {
        let xml = master.xml;
        let shown = |e: &&Element| e.attr(Ns::Style, "display") != Some("false");
        let main = xml.child(master.el, Ns::Style, which).filter(shown)?;
        let mut hf = HeaderFooter {
            min_height: 0.0,
            height: None,
            spacing: 0.0,
            margin_left: 0.0,
            margin_right: 0.0,
            dynamic_spacing: false,
            borders: Sides::default(),
            padding: Sides::default(),
            background: None,
            blocks: Vec::new(),
            left: None,
            first: None,
        };
        if let Some(p) = props {
            hf.min_height = p
                .attr(Ns::Fo, "min-height")
                .and_then(units::length)
                .unwrap_or(0.0);
            hf.height = p.attr(Ns::Svg, "height").and_then(units::length);
            let spacing = if which == "header" {
                "margin-bottom"
            } else {
                "margin-top"
            };
            hf.spacing = p
                .attr(Ns::Fo, spacing)
                .and_then(units::length)
                .unwrap_or(0.0);
            hf.margin_left = p
                .attr(Ns::Fo, "margin-left")
                .and_then(units::length)
                .unwrap_or(0.0);
            hf.margin_right = p
                .attr(Ns::Fo, "margin-right")
                .and_then(units::length)
                .unwrap_or(0.0);
            hf.dynamic_spacing = p.attr(Ns::Style, "dynamic-spacing") == Some("true");
            apply_borders(&mut hf.borders, p);
            apply_sides(&mut hf.padding, p, "padding");
            if let Some(bg) = background(p) {
                hf.background = bg;
            }
        }
        self.blocks(ctx, main, &mut hf.blocks);
        if let Some(left) = xml
            .child(master.el, Ns::Style, &format!("{which}-left"))
            .filter(shown)
        {
            let mut blocks = Vec::new();
            self.blocks(ctx, left, &mut blocks);
            hf.left = Some(blocks);
        }
        let first = xml
            .child(master.el, Ns::Style, &format!("{which}-first"))
            .or_else(|| xml.child(master.el, Ns::Loext, &format!("{which}-first")))
            .filter(shown);
        if let Some(first) = first {
            let mut blocks = Vec::new();
            self.blocks(ctx, first, &mut blocks);
            hf.first = Some(blocks);
        }
        Some(hf)
    }

    fn blocks(&mut self, ctx: Ctx<'a>, parent: &'a Element, out: &mut Vec<Block>) {
        if ctx.depth > MAX_DEPTH {
            return;
        }
        for child in ctx.xml.children(parent) {
            self.block(ctx, child, out);
        }
    }

    fn block(&mut self, ctx: Ctx<'a>, e: &'a Element, out: &mut Vec<Block>) {
        let deeper = ctx.deeper();
        match (e.ns, e.local.as_str()) {
            (Ns::Text, "p") | (Ns::Text, "h") => {
                let p = self.paragraph(ctx, e, None);
                out.push(Block::Paragraph(p));
            }
            (Ns::Text, "list") => self.list(deeper, e, None, false, out),
            (Ns::Text, "numbered-paragraph") => self.numbered_paragraph(deeper, e, out),
            (Ns::Table, "table") => {
                let t = self.table(deeper, e);
                out.push(Block::Table(t));
            }
            (Ns::Text, "section") => {
                let s = self.section(deeper, e);
                out.push(Block::Section(s));
            }
            (Ns::Text, "soft-page-break") => out.push(Block::SoftPageBreak),
            (Ns::Text, "table-of-content")
            | (Ns::Text, "illustration-index")
            | (Ns::Text, "table-index")
            | (Ns::Text, "object-index")
            | (Ns::Text, "user-index")
            | (Ns::Text, "alphabetical-index")
            | (Ns::Text, "bibliography") => {
                if let Some(body) = ctx.xml.child(e, Ns::Text, "index-body") {
                    self.blocks(deeper, body, out);
                }
            }
            (Ns::Text, "index-title") => self.blocks(deeper, e, out),
            (Ns::Draw, _) => {
                if let Some(frame) = self.frame(deeper, e, None) {
                    self.page_frames.push(frame);
                }
            }
            (Ns::Text, _) | (Ns::Office, _) | (Ns::Table, _) => {}
            _ => self.blocks(deeper, e, out),
        }
    }

    fn list_style(&mut self, scope: Scope, name: &str) -> Option<Rc<ListStyle<'a>>> {
        let key = (scope, name.to_string());
        if let Some(hit) = self.list_cache.get(&key) {
            return Some(hit.clone());
        }
        let def = self.styles.list_style(scope, name)?;
        let parsed = Rc::new(lists::parse(def.xml, def.el));
        self.list_cache.insert(key, parsed.clone());
        Some(parsed)
    }

    fn new_instance(&mut self) -> usize {
        self.instances.push(Instance {
            counters: Counters::default(),
        });
        self.instances.len() - 1
    }

    fn list(
        &mut self,
        ctx: Ctx<'a>,
        e: &'a Element,
        parent: Option<&Item>,
        follows_list: bool,
        out: &mut Vec<Block>,
    ) {
        if ctx.depth > MAX_DEPTH {
            return;
        }
        let own_style = e.attr(Ns::Text, "style-name").map(str::to_string);
        let foreign =
            parent.is_some_and(|p| p.foreign || (own_style.is_some() && own_style != p.style));
        let style = own_style
            .clone()
            .or_else(|| parent.and_then(|p| p.style.clone()));
        let (instance, level) = match parent {
            Some(p) => (p.instance, p.level + 1),
            None => {
                let continued = e
                    .attr(Ns::Text, "continue-list")
                    .and_then(|id| self.list_ids.get(id).copied())
                    .or_else(|| {
                        (e.attr(Ns::Text, "continue-numbering") == Some("true"))
                            .then(|| {
                                style
                                    .as_ref()
                                    .and_then(|s| self.last_by_style.get(s).copied())
                            })
                            .flatten()
                    });
                (continued.unwrap_or_else(|| self.new_instance()), 1)
            }
        };
        if let Some(id) = e.attr(Ns::Xml, "id") {
            self.list_ids.insert(id.to_string(), instance);
        }
        if parent.is_none() {
            if let Some(s) = &style {
                self.last_by_style.insert(s.clone(), instance);
            }
        }
        let mut block_restart = parent.is_some_and(|p| p.block_restart) || follows_list;
        for item in ctx.xml.children(e) {
            let header = match (item.ns, item.local.as_str()) {
                (Ns::Text, "list-item") => false,
                (Ns::Text, "list-header") => true,
                _ => continue,
            };
            let style_override = item.attr(Ns::Text, "style-override").map(str::to_string);
            let mut state = Item {
                instance,
                foreign: foreign && style_override.is_none(),
                style: style_override.or_else(|| style.clone()),
                level,
                pending: !header,
                header,
                restart: lists::start_value(item),
                block_restart,
            };
            let mut lists_seen = false;
            for child in ctx.xml.children(item) {
                match (child.ns, child.local.as_str()) {
                    (Ns::Text, "p") | (Ns::Text, "h") => {
                        let p = self.paragraph(ctx, child, Some(&mut state));
                        out.push(Block::Paragraph(p));
                    }
                    (Ns::Text, "list") => {
                        let nested = Item {
                            style: style.clone(),
                            foreign,
                            ..state.clone()
                        };
                        self.list(ctx.deeper(), child, Some(&nested), lists_seen, out);
                        lists_seen = true;
                        state.pending = false;
                    }
                    _ => self.block(ctx, child, out),
                }
            }
            block_restart = state.block_restart;
        }
    }

    fn numbered_paragraph(&mut self, ctx: Ctx<'a>, e: &'a Element, out: &mut Vec<Block>) {
        let id = e.attr(Ns::Text, "list-id").unwrap_or_default().to_string();
        let instance = match self.list_ids.get(&id) {
            Some(i) => *i,
            None => {
                let i = self.new_instance();
                self.list_ids.insert(id, i);
                i
            }
        };
        let level = e
            .attr(Ns::Text, "level")
            .and_then(|l| l.parse().ok())
            .unwrap_or(1);
        let mut state = Item {
            instance,
            style: e.attr(Ns::Text, "style-name").map(str::to_string),
            level,
            pending: true,
            header: false,
            restart: lists::start_value(e),
            block_restart: false,
            foreign: false,
        };
        for child in ctx.xml.children(e) {
            if child.ns == Ns::Text && matches!(child.local.as_str(), "p" | "h") {
                let p = self.paragraph(ctx, child, Some(&mut state));
                out.push(Block::Paragraph(p));
            }
        }
    }

    fn paragraph(&mut self, ctx: Ctx<'a>, e: &'a Element, item: Option<&mut Item>) -> Paragraph {
        let style_name = e.attr(Ns::Text, "style-name");
        let ps = self.styles.paragraph(ctx.scope, style_name);
        let is_heading = e.local == "h";
        let heading = is_heading.then(|| {
            e.attr(Ns::Text, "outline-level")
                .and_then(|l| l.parse().ok())
                .or(ps.outline_level)
                .unwrap_or(1)
                .clamp(1, 10)
        });
        let mut para = ps.para.clone();
        let mut label = None;

        let ignore_indent = self.compat.ignore_first_line_indent_in_numbering;
        let apply_level = |para: &mut Arc<ParaProps>, lvl: &lists::Level, labelled: bool| {
            if let LabelPlacement::Positioned {
                space_before,
                min_width,
                ..
            } = lvl.placement
            {
                // The level's indents add to the paragraph's own.
                let mut p = (**para).clone();
                p.margin.left += space_before + min_width;
                if labelled {
                    p.text_indent = if ignore_indent { 0.0 } else { p.text_indent } - min_width;
                }
                if p != **para {
                    *para = Arc::new(p);
                }
            } else if lvl.margin_left.is_some() || lvl.text_indent.is_some() {
                let mut p = (**para).clone();
                if let (Some(m), false) = (lvl.margin_left, ps.own_margin) {
                    p.margin.left = m;
                }
                if let (Some(t), false) = (lvl.text_indent, ps.own_indent) {
                    p.text_indent = t;
                }
                *para = Arc::new(p);
            }
        };

        let list_source: Option<ListSource> = match item {
            Some(item) => {
                let style = if item.foreign {
                    match ps
                        .list_style
                        .clone()
                        .and_then(|s| self.list_style(ctx.scope, &s))
                    {
                        Some(s) => Some(s),
                        None => Some(self.default_list.clone()),
                    }
                } else {
                    item.style
                        .clone()
                        .or_else(|| ps.list_style.clone())
                        .and_then(|s| self.list_style(ctx.scope, &s))
                };
                let style = match (style, heading) {
                    (None, Some(_)) => self.outline.clone(),
                    (None, None) => Some(self.default_list.clone()),
                    (s, _) => s,
                };
                let labelled = item.pending && !item.header;
                item.pending = false;
                let level = item.level.clamp(1, 10);
                let restart = item.restart.take().or_else(|| {
                    std::mem::take(&mut item.block_restart)
                        .then(|| style.as_ref().map_or(1, |s| s.levels[level - 1].start))
                });
                Some((style, item.instance, item.level, labelled, restart))
            }
            None => match heading {
                Some(level)
                    if ps.list_style.is_none() || ps.list_style.as_deref() == self.outline_name =>
                {
                    let numbered = e.attr(Ns::Text, "is-list-header") != Some("true");
                    // Out of range, the start value falls back to the level's.
                    let restart =
                        (e.attr(Ns::Text, "restart-numbering") == Some("true")).then(|| {
                            lists::start_value(e).unwrap_or_else(|| {
                                self.outline
                                    .as_ref()
                                    .map_or(1, |o| o.levels[level.clamp(1, 10) as usize - 1].start)
                            })
                        });
                    Some((
                        self.outline.clone(),
                        usize::MAX,
                        level as usize,
                        numbered,
                        restart,
                    ))
                }
                _ => None,
            },
        };
        if let Some((Some(style), instance, level, labelled, restart)) = list_source {
            let lvl = &style.levels[level.clamp(1, 10) - 1];
            apply_level(&mut para, lvl, labelled);
            if labelled {
                let start = |k: usize| style.levels[k].start;
                let numbered = matches!(lvl.kind, lists::Kind::Number { format, .. } if format != NumFormat::None);
                let values = if instance == usize::MAX {
                    self.outline_counters.bump(level, &start, restart, numbered)
                } else {
                    let counters = &mut self.instances[instance].counters;
                    counters.consecutive = style.consecutive;
                    counters.bump(level, &start, restart, numbered)
                };
                let text = lists::label_text(&style, level, &values);
                let image = match &lvl.kind {
                    lists::Kind::Image(href) => self.image_at(ctx, href),
                    _ => None,
                };
                let is_empty_number = matches!(
                    lvl.kind,
                    lists::Kind::Number {
                        format: NumFormat::None,
                        ..
                    }
                ) && text.is_empty();
                if !is_empty_number || image.is_some() {
                    label = Some(Label {
                        text,
                        level: level.clamp(1, 10) as u8,
                        bullet: matches!(lvl.kind, lists::Kind::Bullet(_) | lists::Kind::Image(_)),
                        props: lists::label_props(&self.styles, ctx.scope, &style, level, &ps.text),
                        placement: lvl.placement.clone(),
                        image,
                    });
                }
            }
        }

        let page_style = ps
            .master_page
            .as_ref()
            .and_then(|m| self.page_index.get(m).copied());
        let mut inlines = Vec::new();
        let mut ws = true;
        let mut hidden = false;
        self.inlines(ctx, e, &ps.text, None, &mut ws, &mut hidden, &mut inlines);
        Paragraph {
            style: style_name.map(str::to_string),
            props: para,
            text: ps.text.clone(),
            heading,
            label,
            page_style,
            hidden,
            inlines,
        }
    }

    fn push_char(
        out: &mut Vec<Inline>,
        c: char,
        loc: Loc,
        props: &Arc<TextProps>,
        link: &Option<Arc<str>>,
    ) {
        if let Some(Inline::Text(run)) = out.last_mut() {
            if Arc::ptr_eq(&run.props, props) && run.link == *link {
                run.text.push(c);
                run.locs.push(loc);
                return;
            }
        }
        out.push(Inline::Text(Run {
            text: c.to_string(),
            locs: vec![loc],
            props: props.clone(),
            link: link.clone(),
        }));
    }

    fn leaf(
        &mut self,
        ctx: Ctx<'a>,
        node: u32,
        props: &Arc<TextProps>,
        link: &Option<Arc<str>>,
        ws: &mut bool,
        out: &mut Vec<Inline>,
    ) {
        let n = &ctx.xml.nodes[node as usize];
        let loc = |offset: usize| Loc {
            part: ctx.part,
            node,
            offset: offset as u32,
        };
        match n.kind {
            Kind::Chars | Kind::Ref | Kind::CData => {
                for (i, c) in n.text.chars().enumerate() {
                    let c = if matches!(c, ' ' | '\t' | '\r' | '\n') {
                        if *ws {
                            continue;
                        }
                        *ws = true;
                        ' '
                    } else {
                        *ws = false;
                        c
                    };
                    Self::push_char(out, c, loc(i), props, link);
                }
            }
            Kind::Spaces => {
                for i in 0..n.text.chars().count() {
                    Self::push_char(out, ' ', loc(i), props, link);
                }
                *ws = false;
            }
            Kind::Tab => {
                out.push(Inline::Tab {
                    loc: loc(0),
                    props: props.clone(),
                });
                *ws = false;
            }
            Kind::LineBreak => {
                out.push(Inline::LineBreak { loc: loc(0) });
                *ws = false;
            }
            Kind::Attr | Kind::Comment => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn inlines(
        &mut self,
        ctx: Ctx<'a>,
        e: &'a Element,
        props: &Arc<TextProps>,
        link: Option<Arc<str>>,
        ws: &mut bool,
        hidden: &mut bool,
        out: &mut Vec<Inline>,
    ) {
        if ctx.depth > MAX_DEPTH {
            return;
        }
        let deeper = ctx.deeper();
        for child in &e.children {
            let c = match child {
                Child::Node(n) => {
                    self.leaf(ctx, *n, props, &link, ws, out);
                    continue;
                }
                Child::Element(id) => ctx.xml.element(*id),
            };
            match (c.ns, c.local.as_str()) {
                (Ns::Text, "span") => {
                    let p = self
                        .styles
                        .text(ctx.scope, props, c.attr(Ns::Text, "style-name"));
                    self.inlines(deeper, c, &p, link.clone(), ws, hidden, out);
                }
                (Ns::Text, "a") => {
                    let href: Option<Arc<str>> = c.attr(Ns::Xlink, "href").map(Arc::from);
                    let p = self
                        .styles
                        .text(ctx.scope, props, c.attr(Ns::Text, "style-name"));
                    self.inlines(deeper, c, &p, href.or(link.clone()), ws, hidden, out);
                }
                (Ns::Text, "note") => self.note(deeper, c, props, ws, out),
                (Ns::Text, "change") => self.deletion(deeper, c, props, link.clone(), ws, out),
                (Ns::Text, "ruby") => {
                    if let Some(base) = ctx.xml.child(c, Ns::Text, "ruby-base") {
                        self.inlines(deeper, base, props, link.clone(), ws, hidden, out);
                    }
                }
                (Ns::Text, "page-number") | (Ns::Text, "page-count") => {
                    let mut cached = Vec::new();
                    self.inlines(deeper, c, props, link.clone(), ws, hidden, &mut cached);
                    let mut run = Run {
                        text: String::new(),
                        locs: Vec::new(),
                        props: props.clone(),
                        link: link.clone(),
                    };
                    for i in cached {
                        if let Inline::Text(r) = i {
                            run.text.push_str(&r.text);
                            run.locs.extend(r.locs);
                        }
                    }
                    let kind = if c.local == "page-number" {
                        FieldKind::PageNumber
                    } else {
                        FieldKind::PageCount
                    };
                    out.push(Inline::Field { kind, run });
                }
                (Ns::Text, "hidden-text") => {
                    if c.attr(Ns::Text, "is-hidden") != Some("true") {
                        self.inlines(deeper, c, props, link.clone(), ws, hidden, out);
                    }
                }
                (Ns::Text, "hidden-paragraph") => {
                    if c.attr(Ns::Text, "is-hidden") == Some("true") {
                        *hidden = true;
                    }
                }
                (Ns::Text, "soft-page-break") => out.push(Inline::SoftPageBreak),
                (Ns::Text, "drop-down") => {
                    for part in &c.children {
                        match part {
                            Child::Node(n) => self.leaf(deeper, *n, props, &link, ws, out),
                            Child::Element(_) => {}
                        }
                    }
                }
                (Ns::Office, "annotation")
                | (Ns::Office, "annotation-end")
                | (Ns::Text, "bookmark")
                | (Ns::Text, "bookmark-start")
                | (Ns::Text, "bookmark-end")
                | (Ns::Text, "reference-mark")
                | (Ns::Text, "reference-mark-start")
                | (Ns::Text, "reference-mark-end")
                | (Ns::Text, "toc-mark")
                | (Ns::Text, "toc-mark-start")
                | (Ns::Text, "toc-mark-end")
                | (Ns::Text, "alphabetical-index-mark")
                | (Ns::Text, "alphabetical-index-mark-start")
                | (Ns::Text, "alphabetical-index-mark-end")
                | (Ns::Text, "user-index-mark")
                | (Ns::Text, "user-index-mark-start")
                | (Ns::Text, "user-index-mark-end")
                | (Ns::Text, "number")
                | (Ns::Text, "change-start")
                | (Ns::Text, "change-end")
                | (Ns::Text, "sequence-decls")
                | (Ns::Text, "variable-decls")
                | (Ns::Text, "user-field-decls") => {}
                (Ns::Draw, _) => {
                    if let Some(frame) = self.frame(deeper, c, link.clone()) {
                        if let FrameAnchor::Page(_) = frame.anchor {
                            self.page_frames.push(frame);
                        } else {
                            out.push(Inline::Frame(Box::new(frame)));
                        }
                    }
                }
                (Ns::Text, "reference-ref" | "bookmark-ref" | "sequence-ref")
                    if !self
                        .targets()
                        .contains(c.attr(Ns::Text, "ref-name").unwrap_or("")) =>
                {
                    let cached = Arc::new(TextProps {
                        hidden: true,
                        export_hidden: true,
                        ..(**props).clone()
                    });
                    self.inlines(deeper, c, &cached, link.clone(), ws, hidden, out);
                    let text = "Error: Reference source not found".to_string();
                    Self::computed(out, text, props, &link, ws);
                }
                (Ns::Field, "fieldmark")
                    if c.attr(Ns::Field, "type")
                        == Some("vnd.oasis.opendocument.field.FORMDROPDOWN") =>
                {
                    let params: Vec<(&str, &str)> = ctx
                        .xml
                        .children(c)
                        .filter(|p| p.is(Ns::Field, "param"))
                        .filter_map(|p| {
                            Some((p.attr(Ns::Field, "name")?, p.attr(Ns::Field, "value")?))
                        })
                        .collect();
                    let selected = params
                        .iter()
                        .find(|(name, _)| *name == "Dropdown_Selected")
                        .and_then(|(_, v)| v.parse::<usize>().ok());
                    let entry = selected.and_then(|i| {
                        params
                            .iter()
                            .filter(|(name, _)| *name == "Dropdown_ListEntry")
                            .nth(i)
                            .map(|(_, v)| *v)
                    });
                    if let Some(entry) = entry {
                        Self::computed(out, entry.to_string(), props, &link, ws);
                    }
                }
                _ => self.inlines(deeper, c, props, link.clone(), ws, hidden, out),
            }
        }
    }

    fn note(
        &mut self,
        ctx: Ctx<'a>,
        e: &'a Element,
        props: &Arc<TextProps>,
        ws: &mut bool,
        out: &mut Vec<Inline>,
    ) {
        let class = match e.attr(Ns::Text, "note-class") {
            Some("endnote") => NoteClass::Endnote,
            _ => NoteClass::Footnote,
        };
        let key = if class == NoteClass::Endnote {
            "endnote"
        } else {
            "footnote"
        };
        let config = self.notes_config.get(key);
        // Writer's own anchor style when the file names none: superscript at 58 %.
        let cprops = match config.and_then(|c| c.body_style.as_deref()) {
            Some(name) => self.styles.text(ctx.scope, props, Some(name)),
            None => Arc::new(TextProps {
                position: Some((0.33, 0.58)),
                ..(**props).clone()
            }),
        };
        let label_style = config.and_then(|c| c.citation_style.clone());
        let custom = ctx
            .xml
            .child(e, Ns::Text, "note-citation")
            .and_then(|c| c.attr(Ns::Text, "label"))
            .map(str::to_string);
        let (citation, label) = match custom {
            Some(label) => (label.clone(), label),
            None => {
                let count = match class {
                    NoteClass::Footnote => &mut self.footnotes,
                    NoteClass::Endnote => &mut self.endnotes,
                };
                *count += 1;
                let n = *count;
                let default_format = if class == NoteClass::Endnote {
                    NumFormat::LowerRoman
                } else {
                    NumFormat::Decimal
                };
                match config {
                    Some(c) => {
                        let number = lists::format_number(c.start + n, c.format, false);
                        let label = format!("{}{}{}", c.prefix, number, c.suffix);
                        (number, label)
                    }
                    None => {
                        let number = lists::format_number(n, default_format, false);
                        (number.clone(), number)
                    }
                }
            }
        };
        let mut blocks = Vec::new();
        if let Some(body) = ctx.xml.child(e, Ns::Text, "note-body") {
            self.blocks(ctx, body, &mut blocks);
        }
        let first = blocks.iter().find_map(|b| match b {
            Block::Paragraph(p) => Some(p.text.clone()),
            _ => None,
        });
        let label_props = self.styles.text(
            ctx.scope,
            first.as_ref().unwrap_or(props),
            label_style.as_deref(),
        );
        self.notes.push(Note {
            class,
            citation: citation.clone(),
            label,
            label_props,
            blocks,
        });
        *ws = false;
        out.push(Inline::NoteRef {
            note: self.notes.len() - 1,
            citation,
            props: cprops,
        });
    }

    /// The text a tracked deletion removed, where its mark stands: struck
    /// through as Writer shows it, or left out of the drawing when the file
    /// hides its changes. Each deleted paragraph after the first starts a line.
    fn deletion(
        &mut self,
        ctx: Ctx<'a>,
        e: &'a Element,
        props: &Arc<TextProps>,
        link: Option<Arc<str>>,
        ws: &mut bool,
        out: &mut Vec<Inline>,
    ) {
        let Some(id) = e.attr(Ns::Text, "change-id") else {
            return;
        };
        let found = self
            .deletions
            .entry(ctx.part)
            .or_insert_with(|| deletions(ctx.xml))
            .get(id)
            .copied();
        let Some(deletion) = found.filter(|d| self.inlined.insert((ctx.part, *d))) else {
            return;
        };
        let shown = self.compat.show_changes;
        let props = Arc::new(TextProps {
            export_hidden: true,
            hidden: props.hidden || !shown,
            strike: match shown {
                true => Some(TextLine {
                    style: LineStyle::Solid,
                    bold: false,
                    color: None,
                }),
                false => props.strike,
            },
            ..(**props).clone()
        });
        let mut paragraphs = Vec::new();
        collect_paragraphs(
            ctx.xml,
            ctx.xml.element(deletion),
            ctx.depth,
            &mut paragraphs,
        );
        for (k, p) in paragraphs.into_iter().enumerate() {
            let loc = Loc {
                part: ctx.part,
                node: deletion,
                offset: 0,
            };
            match (k, shown) {
                (0, _) => {}
                (_, true) => out.push(Inline::LineBreak { loc }),
                (_, false) => Self::push_char(out, ' ', loc, &props, &link),
            }
            let (footnotes, endnotes) = (self.footnotes, self.endnotes);
            let mut deleted = Vec::new();
            self.inlines(ctx, p, &props, link.clone(), ws, &mut false, &mut deleted);
            if !shown {
                (self.footnotes, self.endnotes) = (footnotes, endnotes);
                deleted.retain(|i| matches!(i, Inline::Text(_) | Inline::Tab { .. }));
            }
            out.append(&mut deleted);
        }
    }

    fn computed(
        out: &mut Vec<Inline>,
        text: String,
        props: &Arc<TextProps>,
        link: &Option<Arc<str>>,
        ws: &mut bool,
    ) {
        let run = Run {
            text,
            locs: Vec::new(),
            props: props.clone(),
            link: link.clone(),
        };
        out.push(Inline::Field {
            kind: FieldKind::Computed,
            run,
        });
        *ws = false;
    }

    fn targets(&mut self) -> &HashSet<&'a str> {
        let pkg = self.pkg;
        self.targets.get_or_insert_with(|| {
            let mut names = HashSet::new();
            for part in &pkg.parts {
                for e in &part.xml.elements {
                    let name = match (e.ns, e.local.as_str()) {
                        (
                            Ns::Text,
                            "reference-mark"
                            | "reference-mark-start"
                            | "bookmark"
                            | "bookmark-start",
                        ) => e.attr(Ns::Text, "name"),
                        (Ns::Text, "sequence") => e.attr(Ns::Text, "ref-name"),
                        _ => None,
                    };
                    names.extend(name);
                }
            }
            names
        })
    }

    fn image_at(&mut self, ctx: Ctx<'a>, href: &str) -> Option<usize> {
        let path = href.strip_prefix("./").unwrap_or(href);
        let package = self.pkg.parts[ctx.part as usize].package;
        let key = (package, path.to_string());
        if let Some(i) = self.image_index.get(&key) {
            return Some(*i);
        }
        let bytes = self.pkg.entry(package, path)?;
        let mime = sniff_mime(path, bytes);
        self.images.push(Image {
            path: path.to_string(),
            mime,
            bytes: Arc::from(bytes),
        });
        self.image_index.insert(key, self.images.len() - 1);
        Some(self.images.len() - 1)
    }

    fn image(&mut self, ctx: Ctx<'a>, e: &'a Element) -> Option<usize> {
        if let Some(href) = e.attr(Ns::Xlink, "href") {
            return self.image_at(ctx, href);
        }
        let data = ctx.xml.child(e, Ns::Office, "binary-data")?;
        let bytes = base64(&ctx.xml.text_of(data));
        let mime = sniff_mime("", &bytes);
        self.images.push(Image {
            path: String::new(),
            mime,
            bytes: Arc::from(bytes),
        });
        Some(self.images.len() - 1)
    }

    fn frame(&mut self, ctx: Ctx<'a>, e: &'a Element, link: Option<Arc<str>>) -> Option<Frame> {
        if ctx.depth > MAX_DEPTH {
            return None;
        }
        if e.is(Ns::Draw, "a") {
            let href: Option<Arc<str>> = e.attr(Ns::Xlink, "href").map(Arc::from);
            let inner = ctx.xml.children(e).find(|c| c.ns == Ns::Draw)?;
            return self.frame(ctx.deeper(), inner, href.or(link));
        }
        let anchor = match e.attr(Ns::Text, "anchor-type") {
            Some("as-char") => FrameAnchor::AsChar,
            Some("char") => FrameAnchor::Char,
            Some("page") => FrameAnchor::Page(
                e.attr(Ns::Text, "anchor-page-number")
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(1),
            ),
            Some("frame") => FrameAnchor::Frame,
            _ => FrameAnchor::Paragraph,
        };
        let len = |ns, local| e.attr(ns, local).and_then(units::length);
        let rel = |local| {
            e.attr(Ns::Style, local)
                .and_then(units::percent)
                .map(|p| p / 100.0)
        };
        let mut frame = Frame {
            name: e.attr(Ns::Draw, "name").unwrap_or_default().to_string(),
            anchor,
            x: len(Ns::Svg, "x").unwrap_or(0.0),
            y: len(Ns::Svg, "y").unwrap_or(0.0),
            width: len(Ns::Svg, "width"),
            height: len(Ns::Svg, "height"),
            min_height: len(Ns::Fo, "min-height"),
            rel_width: rel("rel-width"),
            rel_height: rel("rel-height"),
            wrap: Wrap::Parallel,
            behind_text: false,
            horizontal: ("center".into(), "paragraph".into()),
            vertical: ("top".into(), "paragraph".into()),
            margin: Sides::default(),
            borders: Sides::default(),
            padding: Sides::default(),
            background: None,
            fill: None,
            gradient: None,
            hatch: None,
            fill_image: None,
            transparency: None,
            opacity: 1.0,
            stroke: None,
            valign: VAlign::Top,
            rotation: 0.0,
            mirror: (false, false),
            z_index: e
                .attr(Ns::Draw, "z-index")
                .and_then(|z| z.parse().ok())
                .unwrap_or(0),
            title: ctx
                .xml
                .child(e, Ns::Svg, "title")
                .map(|t| ctx.xml.text_of(t))
                .unwrap_or_default(),
            description: ctx
                .xml
                .child(e, Ns::Svg, "desc")
                .map(|t| ctx.xml.text_of(t))
                .unwrap_or_default(),
            link,
            content: FrameContent::Empty,
        };
        if e.local == "line" {
            let (x1, y1) = (
                len(Ns::Svg, "x1").unwrap_or(0.0),
                len(Ns::Svg, "y1").unwrap_or(0.0),
            );
            let (x2, y2) = (
                len(Ns::Svg, "x2").unwrap_or(0.0),
                len(Ns::Svg, "y2").unwrap_or(0.0),
            );
            frame.x = x1.min(x2);
            frame.y = y1.min(y2);
            frame.width = Some((x2 - x1).abs());
            frame.height = Some((y2 - y1).abs());
            frame.mirror.0 = (x2 < x1) != (y2 < y1);
        }
        if let Some(t) = e.attr(Ns::Draw, "transform") {
            place_transformed(&mut frame, t);
        }
        let style = e.attr(Ns::Draw, "style-name");
        let (mut fill, mut fill_color, mut stroke, mut stroke_color, mut stroke_width) =
            (None, None, None, None, 0.0);
        let mut valign = None;
        let (mut gradient, mut steps) = (None, 0);
        let (mut hatch, mut hatch_solid, mut picture, mut repeat, mut transparency) =
            (None, false, None, None, None);
        let (mut tile_w, mut tile_h) = (None, None);
        for (p, _) in self
            .styles
            .layers(ctx.scope, "graphic", style, "graphic-properties")
        {
            if let Some(v) = p.attr(Ns::Draw, "fill") {
                fill = Some(v);
            }
            if let Some(v) = p.attr(Ns::Draw, "fill-gradient-name") {
                gradient = Some(v);
            }
            if let Some(v) = p.attr(Ns::Draw, "fill-hatch-name") {
                hatch = Some(v);
            }
            if let Some(v) = p.attr(Ns::Draw, "fill-hatch-solid") {
                hatch_solid = v == "true";
            }
            if let Some(v) = p.attr(Ns::Draw, "fill-image-name") {
                picture = Some(v);
            }
            if let Some(v) = p.attr(Ns::Style, "repeat") {
                repeat = Some(v);
            }
            if let Some(v) = p.attr(Ns::Draw, "fill-image-width") {
                tile_w = units::length(v);
            }
            if let Some(v) = p.attr(Ns::Draw, "fill-image-height") {
                tile_h = units::length(v);
            }
            if let Some(v) = p.attr(Ns::Draw, "opacity-name") {
                transparency = Some(v);
            }
            if let Some(n) = p.attr(Ns::Draw, "gradient-step-count") {
                // More bands than an 8-bit channel has levels draw nothing more.
                steps = n.parse::<u32>().unwrap_or(0).min(256);
            }
            if let Some(o) = p.attr(Ns::Draw, "opacity").and_then(units::percent) {
                frame.opacity = (o / 100.0).clamp(0.0, 1.0);
            }
            if let Some(c) = p.attr(Ns::Draw, "fill-color").and_then(units::color) {
                fill_color = Some(c);
            }
            if let Some(v) = p.attr(Ns::Draw, "stroke") {
                stroke = Some(v);
            }
            if let Some(c) = p.attr(Ns::Svg, "stroke-color").and_then(units::color) {
                stroke_color = Some(c);
            }
            if let Some(w) = p.attr(Ns::Svg, "stroke-width").and_then(units::length) {
                stroke_width = w;
            }
            if let Some(v) = p.attr(Ns::Draw, "textarea-vertical-align") {
                valign = Some(match v {
                    "middle" => VAlign::Middle,
                    "bottom" => VAlign::Bottom,
                    _ => VAlign::Top,
                });
            }
            if let Some(v) = p.attr(Ns::Style, "mirror") {
                frame.mirror = (v.contains("horizontal"), v.contains("vertical"));
            }
            if let Some(w) = p.attr(Ns::Style, "wrap") {
                frame.wrap = match w {
                    "left" => Wrap::Left,
                    "right" => Wrap::Right,
                    "parallel" => Wrap::Parallel,
                    "dynamic" | "biggest" => Wrap::Dynamic,
                    "run-through" => Wrap::RunThrough,
                    _ => Wrap::None,
                };
            }
            if let Some(r) = p.attr(Ns::Style, "run-through") {
                frame.behind_text = r == "background";
            }
            if let Some(v) = p.attr(Ns::Style, "horizontal-pos") {
                frame.horizontal.0 = v.to_string();
            }
            if let Some(v) = p.attr(Ns::Style, "horizontal-rel") {
                frame.horizontal.1 = v.to_string();
            }
            if let Some(v) = p.attr(Ns::Style, "vertical-pos") {
                frame.vertical.0 = v.to_string();
            }
            if let Some(v) = p.attr(Ns::Style, "vertical-rel") {
                frame.vertical.1 = v.to_string();
            }
            apply_sides(&mut frame.margin, p, "margin");
            apply_borders(&mut frame.borders, p);
            apply_sides(&mut frame.padding, p, "padding");
            if let Some(bg) = background(p) {
                frame.background = bg;
            }
        }

        let fills = &self.styles.fills;
        let definition = |kind: &'static str, name: Option<&str>| {
            name.and_then(|name| fills.get(&(kind, name.to_string())).copied())
        };
        frame.transparency = definition("opacity", transparency).map(read_opacity);
        let mut href = None;
        match fill {
            Some("gradient") => {
                frame.gradient = definition("gradient", gradient).map(|g| read_gradient(g, steps));
            }
            Some("hatch") => {
                frame.hatch = definition("hatch", hatch)
                    .map(|h| read_hatch(h, hatch_solid.then_some(fill_color).flatten()));
            }
            Some("bitmap") => {
                href = definition("fill-image", picture).and_then(|i| i.attr(Ns::Xlink, "href"));
            }
            _ => {}
        }
        if let Some(href) = href {
            let tile = match (tile_w, tile_h) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some((w, h)),
                _ => None,
            };
            frame.fill_image = self.image_at(ctx, href).map(|image| FillImage {
                image,
                stretch: repeat == Some("stretch"),
                tile,
            });
        }
        frame.valign = valign.unwrap_or(VAlign::Top);
        frame.content = match e.local.as_str() {
            "frame" => {
                let children: Vec<&Element> = ctx.xml.children(e).collect();
                let images: Vec<&Element> = children
                    .iter()
                    .copied()
                    .filter(|c| c.is(Ns::Draw, "image"))
                    .collect();
                if let Some(obj) = children
                    .iter()
                    .find(|c| c.is(Ns::Draw, "object") || c.is(Ns::Draw, "object-ole"))
                {
                    let path = obj
                        .attr(Ns::Xlink, "href")
                        .unwrap_or_default()
                        .trim_start_matches("./")
                        .trim_end_matches('/')
                        .to_string();
                    let package = self.pkg.parts[ctx.part as usize].package;
                    let preview = images.first().and_then(|i| self.image(ctx, i));
                    // Writer's own preview draws a chart closer to Writer than the chart model does.
                    let chart = preview
                        .is_none()
                        .then(|| chart::read(self.pkg, package, &path))
                        .flatten();
                    match chart {
                        Some(chart) => FrameContent::Chart(Box::new(chart)),
                        None => FrameContent::Object { path, preview },
                    }
                } else if let Some(tb) = children.iter().find(|c| c.is(Ns::Draw, "text-box")) {
                    if frame.min_height.is_none() {
                        frame.min_height = tb.attr(Ns::Fo, "min-height").and_then(units::length);
                    }
                    let mut blocks = Vec::new();
                    self.blocks(ctx.deeper(), tb, &mut blocks);
                    FrameContent::TextBox(blocks)
                } else if !images.is_empty() {
                    let mut chosen = None;
                    for img in &images {
                        if let Some(i) = self.image(ctx, img) {
                            if chosen.is_none() || displayable(&self.images[i].mime) {
                                let better = chosen
                                    .is_none_or(|c: usize| !displayable(&self.images[c].mime));
                                if better {
                                    chosen = Some(i);
                                }
                            }
                        }
                    }
                    chosen
                        .map(FrameContent::Image)
                        .unwrap_or(FrameContent::Empty)
                } else {
                    FrameContent::Empty
                }
            }
            "g" => {
                let mut members = Vec::new();
                for c in ctx.xml.children(e) {
                    if c.ns == Ns::Draw {
                        if let Some(f) = self.frame(ctx.deeper(), c, None) {
                            members.push(f);
                        }
                    }
                }
                FrameContent::Group(members)
            }
            kind => {
                let mut blocks = Vec::new();
                for c in ctx.xml.children(e) {
                    if c.ns == Ns::Text {
                        self.block(ctx.deeper(), c, &mut blocks);
                    }
                }
                // A shape centres its text, and is filled and outlined, unless its style says not.
                frame.valign = valign.unwrap_or(VAlign::Middle);
                let color = |c: Option<Color>, default| c.unwrap_or(Color(default));
                frame.fill = (!matches!(kind, "line" | "polyline") && fill != Some("none"))
                    .then(|| color(fill_color, 0x729fcf));
                frame.stroke =
                    (stroke != Some("none")).then(|| (stroke_width, color(stroke_color, 0x3465a4)));
                let enhanced = ctx.xml.child(e, Ns::Draw, "enhanced-geometry");
                let geometry = enhanced
                    .and_then(|g| g.attr(Ns::Draw, "type"))
                    .map(str::to_string);
                let view_box = |v: Option<&str>| {
                    let n: Vec<f64> = v
                        .unwrap_or_default()
                        .split([' ', ','])
                        .filter_map(|n| n.parse().ok())
                        .collect();
                    <[f64; 4]>::try_from(n).unwrap_or_default()
                };
                let path = match kind {
                    "custom-shape" => enhanced.and_then(|g| {
                        Some(ShapePath::Enhanced(Enhanced {
                            view_box: view_box(g.attr(Ns::Svg, "viewBox")),
                            path: g.attr(Ns::Draw, "enhanced-path")?.to_string(),
                            equations: ctx
                                .xml
                                .children(g)
                                .filter(|q| q.is(Ns::Draw, "equation"))
                                .filter_map(|q| {
                                    Some((
                                        q.attr(Ns::Draw, "name")?.to_string(),
                                        q.attr(Ns::Draw, "formula")?.to_string(),
                                    ))
                                })
                                .collect(),
                            modifiers: g
                                .attr(Ns::Draw, "modifiers")
                                .unwrap_or_default()
                                .split_whitespace()
                                .map(|m| m.parse().unwrap_or(0.0))
                                .collect(),
                            mirror: (
                                g.attr(Ns::Draw, "mirror-horizontal") == Some("true"),
                                g.attr(Ns::Draw, "mirror-vertical") == Some("true"),
                            ),
                        }))
                    }),
                    "polygon" | "polyline" => e.attr(Ns::Draw, "points").map(|points| {
                        let mut d = String::new();
                        for (i, p) in points.split_whitespace().enumerate() {
                            d.push_str(if i == 0 { "M " } else { " L " });
                            d.push_str(&p.replace(',', " "));
                        }
                        if kind == "polygon" {
                            d.push_str(" Z");
                        }
                        ShapePath::Svg {
                            d,
                            view_box: view_box(e.attr(Ns::Svg, "viewBox")),
                            fill: kind == "polygon",
                        }
                    }),
                    "path" => e.attr(Ns::Svg, "d").map(|d| ShapePath::Svg {
                        d: d.to_string(),
                        view_box: view_box(e.attr(Ns::Svg, "viewBox")),
                        fill: true,
                    }),
                    _ => None,
                };
                FrameContent::Shape {
                    kind: kind.to_string(),
                    geometry,
                    path: path.map(Box::new),
                    blocks,
                }
            }
        };
        Some(frame)
    }

    fn section(&mut self, ctx: Ctx<'a>, e: &'a Element) -> Section {
        let mut s = Section {
            name: e.attr(Ns::Text, "name").unwrap_or_default().to_string(),
            columns: Columns::default(),
            margin_left: 0.0,
            margin_right: 0.0,
            background: None,
            hidden: e.attr(Ns::Text, "display") == Some("none")
                || (e.attr(Ns::Text, "display") == Some("condition")
                    && e.attr(Ns::Text, "is-hidden") == Some("true")),
            blocks: Vec::new(),
        };
        let style = e.attr(Ns::Text, "style-name");
        for (p, xml) in self
            .styles
            .layers(ctx.scope, "section", style, "section-properties")
        {
            if let Some(c) = styles::columns(xml, p) {
                s.columns = c;
            }
            if let Some(m) = p.attr(Ns::Fo, "margin-left").and_then(units::length) {
                s.margin_left = m;
            }
            if let Some(m) = p.attr(Ns::Fo, "margin-right").and_then(units::length) {
                s.margin_right = m;
            }
            if let Some(bg) = background(p) {
                s.background = bg;
            }
        }
        self.blocks(ctx, e, &mut s.blocks);
        s
    }

    fn table(&mut self, ctx: Ctx<'a>, e: &'a Element) -> Table {
        let style = e.attr(Ns::Table, "style-name");
        let mut props = TableProps {
            width: None,
            rel_width: None,
            align: TableAlign::Margins,
            margin: Sides::default(),
            break_before: Break::None,
            break_after: Break::None,
            keep_with_next: false,
            may_break_rows: true,
            background: None,
            rtl: false,
        };
        for (p, _) in self
            .styles
            .layers(ctx.scope, "table", style, "table-properties")
        {
            if let Some(w) = p.attr(Ns::Style, "width").and_then(units::length) {
                props.width = Some(w);
            }
            if let Some(w) = p.attr(Ns::Style, "rel-width").and_then(units::percent) {
                props.rel_width = Some(w / 100.0);
            }
            if let Some(a) = p.attr(Ns::Table, "align") {
                props.align = match a {
                    "left" => TableAlign::Left,
                    "center" => TableAlign::Center,
                    "right" => TableAlign::Right,
                    _ => TableAlign::Margins,
                };
            }
            apply_sides(&mut props.margin, p, "margin");
            if let Some(v) = p.attr(Ns::Fo, "break-before") {
                props.break_before = brk(v);
            }
            if let Some(v) = p.attr(Ns::Fo, "break-after") {
                props.break_after = brk(v);
            }
            if let Some(v) = p.attr(Ns::Fo, "keep-with-next") {
                props.keep_with_next = v == "always";
            }
            if let Some(v) = p.attr(Ns::Style, "may-break-between-rows") {
                props.may_break_rows = units::boolean(v);
            }
            if let Some(bg) = background(p) {
                props.background = bg;
            }
            if let Some(w) = p.attr(Ns::Style, "writing-mode") {
                props.rtl = w.starts_with("rl");
            }
        }
        let page_style = style
            .and_then(|s| self.styles.find(ctx.scope, "table", s))
            .and_then(|d| d.attr(Ns::Style, "master-page-name"))
            .filter(|m| !m.is_empty())
            .and_then(|m| self.page_index.get(m).copied());

        let mut table = Table {
            name: e.attr(Ns::Table, "name").unwrap_or_default().to_string(),
            props,
            columns: Vec::new(),
            rows: Vec::new(),
            page_style,
        };
        let mut grid = Grid::default();
        self.table_parts(ctx, e, false, &mut table, &mut grid);
        let rows = table.rows.len();
        for (r, row) in table.rows.iter_mut().enumerate() {
            for cell in &mut row.cells {
                cell.row_span = cell.row_span.min((rows - r) as u32);
            }
        }
        table
    }

    fn table_parts(
        &mut self,
        ctx: Ctx<'a>,
        e: &'a Element,
        header: bool,
        table: &mut Table,
        grid: &mut Grid<'a>,
    ) {
        if ctx.depth > MAX_DEPTH {
            return;
        }
        for c in ctx.xml.children(e) {
            match (c.ns, c.local.as_str()) {
                (Ns::Table, "table-column") => {
                    let repeat = c
                        .attr(Ns::Table, "number-columns-repeated")
                        .and_then(|r| r.parse::<u32>().ok())
                        .unwrap_or(1)
                        .clamp(1, MAX_REPEAT);
                    let mut col = Column {
                        width: None,
                        rel_width: None,
                    };
                    let style = c.attr(Ns::Table, "style-name");
                    for (p, _) in self.styles.layers(
                        ctx.scope,
                        "table-column",
                        style,
                        "table-column-properties",
                    ) {
                        if let Some(w) = p.attr(Ns::Style, "column-width").and_then(units::length) {
                            col.width = Some(w);
                        }
                        if let Some(w) = p
                            .attr(Ns::Style, "rel-column-width")
                            .and_then(|w| w.trim_end_matches('*').parse().ok())
                        {
                            col.rel_width = Some(w);
                        }
                    }
                    for _ in 0..repeat {
                        if table.columns.len() >= MAX_REPEAT as usize {
                            break;
                        }
                        table.columns.push(col.clone());
                        grid.default_cells
                            .push(c.attr(Ns::Table, "default-cell-style-name"));
                    }
                }
                (Ns::Table, "table-columns")
                | (Ns::Table, "table-column-group")
                | (Ns::Table, "table-header-columns") => {
                    self.table_parts(ctx.deeper(), c, header, table, grid)
                }
                (Ns::Table, "table-header-rows") => {
                    self.table_parts(ctx.deeper(), c, true, table, grid)
                }
                (Ns::Table, "table-rows") | (Ns::Table, "table-row-group") => {
                    self.table_parts(ctx.deeper(), c, header, table, grid)
                }
                (Ns::Table, "table-row") => {
                    // Writer ignores `number-rows-repeated`: the row appears once.
                    let width = table.columns.len().max(1);
                    let row = self.row(ctx, c, header, grid, width);
                    table.rows.push(row);
                }
                (Ns::Text, "soft-page-break") => {}
                _ => {}
            }
        }
    }

    /// A row laid on Writer's grid: every position holds a cell or a covered
    /// one. Writer ignores `covered-table-cell`, skips the positions a cell
    /// above spans into, cuts a span at the first of them, and drops the cells
    /// past the `width` the columns declare.
    fn row(
        &mut self,
        ctx: Ctx<'a>,
        e: &'a Element,
        header: bool,
        grid: &mut Grid<'a>,
        width: usize,
    ) -> Row {
        let mut row = Row {
            height: None,
            min_height: None,
            keep_together: false,
            background: None,
            header,
            cells: Vec::new(),
        };
        let style = e.attr(Ns::Table, "style-name");
        for (p, _) in self
            .styles
            .layers(ctx.scope, "table-row", style, "table-row-properties")
        {
            if let Some(h) = p.attr(Ns::Style, "row-height").and_then(units::length) {
                row.height = Some(h);
            }
            if let Some(h) = p.attr(Ns::Style, "min-row-height").and_then(units::length) {
                row.min_height = Some(h);
            }
            if let Some(k) = p.attr(Ns::Fo, "keep-together") {
                row.keep_together = k == "always";
            }
            if let Some(bg) = background(p) {
                row.background = bg;
            }
        }
        grid.rows_left.resize(width, 0);
        let mut used: Vec<bool> = grid.rows_left.iter().map(|&n| n > 0).collect();
        let mut placed: Vec<Option<Cell>> = vec![None; width];
        let skip = |used: &[bool], mut at: usize| {
            while at < width && used[at] {
                at += 1;
            }
            at
        };
        let mut at = skip(&used, 0);
        let row_default = e.attr(Ns::Table, "default-cell-style-name");
        for c in ctx.xml.children(e) {
            if (c.ns, c.local.as_str()) != (Ns::Table, "table-cell") || at >= width {
                continue;
            }
            let repeat = c
                .attr(Ns::Table, "number-columns-repeated")
                .and_then(|r| r.parse::<u32>().ok())
                .unwrap_or(1)
                .clamp(1, MAX_REPEAT);
            let style = c
                .attr(Ns::Table, "style-name")
                .or(row_default)
                .or_else(|| grid.default_cells.get(at).copied().flatten());
            let mut props = CellProps {
                background: None,
                borders: Sides::default(),
                padding: Sides::default(),
                valign: VAlign::Top,
                vertical_text: false,
            };
            for (p, _) in
                self.styles
                    .layers(ctx.scope, "table-cell", style, "table-cell-properties")
            {
                if let Some(bg) = background(p) {
                    props.background = bg;
                }
                apply_borders(&mut props.borders, p);
                apply_sides(&mut props.padding, p, "padding");
                if let Some(v) = p.attr(Ns::Style, "vertical-align") {
                    props.valign = match v {
                        "middle" => VAlign::Middle,
                        "bottom" => VAlign::Bottom,
                        _ => VAlign::Top,
                    };
                }
                if let Some(d) = p.attr(Ns::Style, "direction") {
                    props.vertical_text = d == "ttb";
                }
            }
            let mut blocks = Vec::new();
            self.blocks(ctx.deeper(), c, &mut blocks);
            let col_span = c
                .attr(Ns::Table, "number-columns-spanned")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let row_span = c
                .attr(Ns::Table, "number-rows-spanned")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1)
                .max(1);
            for _ in 0..repeat {
                if at >= width {
                    break;
                }
                let end = (at + 1..(at + col_span).min(width))
                    .find(|&k| used[k])
                    .unwrap_or((at + col_span).min(width));
                used[at..end].fill(true);
                grid.rows_left[at..end].fill(row_span);
                placed[at] = Some(Cell {
                    col_span: (end - at) as u32,
                    row_span,
                    covered: false,
                    props: props.clone(),
                    blocks: blocks.clone(),
                });
                at = skip(&used, end);
            }
        }
        let empty = CellProps {
            background: None,
            borders: Sides::default(),
            padding: Sides::default(),
            valign: VAlign::Top,
            vertical_text: false,
        };
        let covered = |props: CellProps| Cell {
            col_span: 1,
            row_span: 1,
            covered: true,
            props,
            blocks: Vec::new(),
        };
        let mut k = 0;
        while k < width {
            match placed[k].take() {
                Some(cell) => {
                    let span = cell.col_span as usize;
                    row.cells.push(cell);
                    row.cells.extend((1..span).map(|_| covered(empty.clone())));
                    k += span;
                }
                None if used[k] => {
                    row.cells.push(covered(empty.clone()));
                    k += 1;
                }
                None => {
                    // Writer fills a short row with one empty cell to the next taken position.
                    let end = skip(&used.iter().map(|u| !u).collect::<Vec<_>>(), k);
                    row.cells.push(Cell {
                        col_span: (end - k) as u32,
                        covered: false,
                        ..covered(empty.clone())
                    });
                    row.cells
                        .extend((k + 1..end).map(|_| covered(empty.clone())));
                    k = end;
                }
            }
        }
        for n in &mut grid.rows_left {
            *n = n.saturating_sub(1);
        }
        row
    }
}

/// `rotate(a) translate(x y)`: a shape turned about its top left corner by
/// `a` radians counterclockwise, then moved, as a frame turned about its centre.
fn place_transformed(frame: &mut Frame, transform: &str) {
    let (mut angle, mut at) = (0.0f32, None);
    for part in transform.split(')') {
        let Some((name, args)) = part.split_once('(') else {
            continue;
        };
        let args: Vec<&str> = args
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|a| !a.is_empty())
            .collect();
        match name.trim() {
            "rotate" => angle = args.first().and_then(|a| a.parse().ok()).unwrap_or(0.0),
            "translate" => {
                at = Some((
                    args.first().and_then(|a| units::length(a)).unwrap_or(0.0),
                    args.get(1).and_then(|a| units::length(a)).unwrap_or(0.0),
                ))
            }
            _ => {}
        }
    }
    let Some((tx, ty)) = at else { return };
    let (w, h) = (frame.width.unwrap_or(0.0), frame.height.unwrap_or(0.0));
    let (sin, cos) = angle.sin_cos();
    let (cx, cy) = (
        tx + w / 2.0 * cos + h / 2.0 * sin,
        ty - w / 2.0 * sin + h / 2.0 * cos,
    );
    frame.x = cx - w / 2.0;
    frame.y = cy - h / 2.0;
    frame.rotation = -angle.to_degrees();
}

fn read_hatch(h: &Element, back: Option<Color>) -> Hatch {
    Hatch {
        color: h
            .attr(Ns::Draw, "color")
            .and_then(units::color)
            .unwrap_or(Color(0)),
        distance: h
            .attr(Ns::Draw, "distance")
            .and_then(units::length)
            .unwrap_or(0.0),
        angle: h.attr(Ns::Draw, "rotation").map_or(0.0, angle),
        lines: match h.attr(Ns::Draw, "style") {
            Some("double") => 2,
            Some("triple") => 3,
            _ => 1,
        },
        back,
    }
}

/// A `draw:opacity` read as a gradient from the start opacity to the end one,
/// each as a grey level.
fn read_opacity(o: &Element) -> Gradient {
    let level = |local: &str, default: f32| {
        let share = o
            .attr(Ns::Draw, local)
            .and_then(units::percent)
            .map_or(default, |p| (p / 100.0).clamp(0.0, 1.0));
        let v = (share * 255.0).round() as u32;
        Color(v << 16 | v << 8 | v)
    };
    let mut g = read_gradient(o, 0);
    g.start = level("start", 1.0);
    g.end = level("end", 1.0);
    g
}

/// An angle in degrees, tenths of a degree when it has no unit.
fn angle(a: &str) -> f32 {
    let a = a.trim();
    let number = |s: &str| s.trim().parse::<f32>().unwrap_or(0.0);
    if let Some(d) = a.strip_suffix("deg") {
        number(d)
    } else if let Some(g) = a.strip_suffix("grad") {
        number(g) * 0.9
    } else if let Some(r) = a.strip_suffix("rad") {
        number(r).to_degrees()
    } else {
        number(a) / 10.0
    }
}

fn read_gradient(g: &Element, steps: u32) -> Gradient {
    let share = |local: &str, default: f32| {
        g.attr(Ns::Draw, local)
            .and_then(units::percent)
            .map_or(default, |p| (p / 100.0).clamp(0.0, 1.0))
    };
    let at = |local: &str, intensity: &str, default: u32| {
        let Color(c) = g
            .attr(Ns::Draw, local)
            .and_then(units::color)
            .unwrap_or(Color(default));
        let k = share(intensity, 1.0);
        let channel = |shift: u32| ((((c >> shift) & 0xff) as f32 * k).round() as u32) << shift;
        Color(channel(16) | channel(8) | channel(0))
    };
    let angle = g.attr(Ns::Draw, "angle").map_or(0.0, angle);
    Gradient {
        style: match g.attr(Ns::Draw, "style") {
            Some("axial") => GradientStyle::Axial,
            Some("radial" | "ellipsoid" | "square" | "rectangular") => GradientStyle::Radial,
            _ => GradientStyle::Linear,
        },
        center: (share("cx", 0.5), share("cy", 0.5)),
        start: at("start-color", "start-intensity", 0x000000),
        end: at("end-color", "end-intensity", 0xffffff),
        angle,
        border: share("border", 0.0),
        steps,
    }
}
