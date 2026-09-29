//! A web page as the shared model: blocks, runs, lists and tables styled as
//! a browser styles them, on the page an `@page` rule sets, else on A4.

use std::sync::Arc;

use super::css::{self, Paint, Sheet};
use super::token::{Element, Node};
use super::HtmlPackage;
use crate::model as m;
use crate::Anchor;

/// What a browser gives each element before any style of the page.
const DEFAULTS: &str = "
p { margin-top: 1em; margin-bottom: 1em }
h1 { font-size: 2em; font-weight: bold; margin-top: .67em; margin-bottom: .67em }
h2 { font-size: 1.5em; font-weight: bold; margin-top: .83em; margin-bottom: .83em }
h3 { font-size: 1.17em; font-weight: bold; margin-top: 1em; margin-bottom: 1em }
h4 { font-weight: bold; margin-top: 1.33em; margin-bottom: 1.33em }
h5 { font-size: .83em; font-weight: bold; margin-top: 1.67em; margin-bottom: 1.67em }
h6 { font-size: .67em; font-weight: bold; margin-top: 2.33em; margin-bottom: 2.33em }
ul, ol { margin-top: 1em; margin-bottom: 1em; padding-left: 40px }
blockquote { margin: 1em 40px }
dd { margin-left: 40px }
pre { font-family: monospace; white-space: pre; margin-top: 1em; margin-bottom: 1em }
b, strong { font-weight: bold }
i, em, cite, var, dfn, address { font-style: italic }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }
code, tt, kbd, samp { font-family: monospace }
sup { vertical-align: super }
sub { vertical-align: sub }
small { font-size: smaller }
big { font-size: larger }
th { font-weight: bold; text-align: center }
center { text-align: center }
";

/// Never drawn, whatever their style.
const UNSHOWN: &[&str] = &[
    "head", "title", "style", "script", "xml", "template", "noscript", "meta", "link", "img",
    "object", "embed", "iframe", "select", "option", "textarea",
];

const BLOCKS: &[&str] = &[
    "html",
    "body",
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "dd",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figure",
    "figcaption",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "ul",
    "caption",
];

/// Blocks that are a paragraph even when they hold no text.
const PARAGRAPHS: &[&str] = &[
    "p", "h1", "h2", "h3", "h4", "h5", "h6", "li", "pre", "dt", "dd",
];

#[derive(Clone, Copy, PartialEq)]
enum Line {
    Normal,
    Factor(f64),
    Points(f64),
}

/// What an element passes down to what it holds.
#[derive(Clone)]
struct Inherited {
    run: m::RunProps,
    align: m::Alignment,
    line: Line,
    text_indent: f64,
    pre: bool,
    list_type: Option<String>,
}

#[derive(Clone, Copy, Default)]
struct Side {
    width: Option<f64>,
    style: Option<bool>,
    double: bool,
    color: Option<Option<m::Color>>,
}

impl Side {
    fn border(self) -> Option<m::Border> {
        match self.style {
            Some(true) => Some(m::Border {
                width: self.width.unwrap_or(2.25),
                color: self.color.flatten(),
                space: 0.0,
                double: self.double,
            }),
            _ => None,
        }
    }
}

/// What an element keeps to itself.
#[derive(Default)]
struct Boxed {
    margin: [f64; 4],
    auto: [bool; 4],
    padding: [Option<f64>; 4],
    sides: [Side; 4],
    background: Option<m::Color>,
    width: Option<f64>,
    height: Option<f64>,
    block: bool,
    valign: Option<m::VAlign>,
    page_break_before: bool,
}

impl Boxed {
    fn borders(&self) -> m::Borders {
        m::Borders {
            top: self.sides[0].border(),
            right: self.sides[1].border(),
            bottom: self.sides[2].border(),
            left: self.sides[3].border(),
            between: None,
            vertical: None,
        }
    }

    fn pad(&self, side: usize) -> f64 {
        self.padding[side].unwrap_or(0.0)
    }
}

/// Where the blocks being read sit: from the left edge of the text column,
/// and how wide they may be.
#[derive(Clone, Copy)]
struct Ctx {
    left: f64,
    right: f64,
    width: f64,
}

struct List {
    ordered: bool,
    kind: Option<String>,
    next: i64,
}

struct Para {
    props: m::ParagraphProps,
    mark: m::RunProps,
    label: Option<m::Label>,
    inlines: Vec<m::Inline>,
    /// The last character is a space collapsed from white space, or there is
    /// none yet: white space that follows adds nothing.
    space: bool,
    /// The last character is such a space, dropped at the end of the line.
    trailing: bool,
}

struct Builder<'a> {
    package: &'a HtmlPackage,
    defaults: Sheet,
    sheet: Sheet,
    lists: Vec<List>,
    /// The label of a list item whose text starts in a block it holds.
    pending_label: Option<m::Label>,
    fonts: Vec<m::FontDecl>,
}

fn family(value: &str) -> Option<(String, m::Generic)> {
    let names: Vec<&str> = value
        .split(',')
        .map(|n| n.trim().trim_matches(['"', '\'']).trim())
        .filter(|n| !n.is_empty())
        .collect();
    let generic = names
        .iter()
        .find_map(|n| match n.to_ascii_lowercase().as_str() {
            "serif" => Some(m::Generic::Roman),
            "sans-serif" => Some(m::Generic::Swiss),
            "monospace" => Some(m::Generic::Fixed),
            "cursive" => Some(m::Generic::Script),
            "fantasy" => Some(m::Generic::Decorative),
            _ => None,
        })
        .unwrap_or_default();
    let first = *names.first()?;
    let name = match first.to_ascii_lowercase().as_str() {
        "inherit" | "initial" => return None,
        "serif" => "Times New Roman".to_string(),
        _ => first.to_string(),
    };
    Some((name, generic))
}

fn set_family(run: &mut m::RunProps, name: &str) {
    let name: Arc<str> = Arc::from(name);
    run.fonts = m::FontSet {
        ascii: name.clone(),
        h_ansi: name.clone(),
        east_asia: name.clone(),
        complex: name,
    };
}

fn list_label(kind: &str, n: i64, level: usize) -> Option<String> {
    fn alpha(mut n: i64) -> String {
        let mut out = Vec::new();
        while n > 0 {
            n -= 1;
            out.push((b'a' + (n % 26) as u8) as char);
            n /= 26;
        }
        out.iter().rev().collect()
    }
    fn roman(mut n: i64) -> String {
        const NUMERALS: [(i64, &str); 13] = [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ];
        let mut out = String::new();
        for (value, numeral) in NUMERALS {
            while n >= value {
                out.push_str(numeral);
                n -= value;
            }
        }
        out
    }
    Some(match kind {
        "none" => return None,
        "disc" => "\u{2022}".into(),
        "circle" => "\u{25E6}".into(),
        "square" => "\u{25AA}".into(),
        "bullet" => ["\u{2022}", "\u{25E6}", "\u{25AA}"][level.clamp(1, 3) - 1].into(),
        "lower-alpha" | "lower-latin" | "a" => format!("{}.", alpha(n)),
        "upper-alpha" | "upper-latin" | "A" => format!("{}.", alpha(n).to_uppercase()),
        "lower-roman" | "i" => format!("{}.", roman(n)),
        "upper-roman" | "I" => format!("{}.", roman(n).to_uppercase()),
        _ => format!("{n}."),
    })
}

impl<'a> Builder<'a> {
    fn attribute(&self, element: &Element, name: &str) -> Option<&'a str> {
        self.package
            .attributes(element)
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    /// The declarations an element takes, weakest first: the browser's, its
    /// presentational attributes, the page's rules, its own `style`.
    fn declarations(&self, element: &Element) -> Vec<(String, String)> {
        let tag = element.name.as_str();
        let classes: Vec<&str> = self
            .attribute(element, "class")
            .map(|c| c.split_whitespace().collect())
            .unwrap_or_default();
        let mut out: Vec<(String, String)> = self
            .defaults
            .matching(tag, &[])
            .into_iter()
            .cloned()
            .collect();
        for attribute in self.package.attributes(element) {
            let v = attribute.value.trim();
            let pixels = |v: &str| match v.ends_with('%') {
                true => v.to_string(),
                false => format!("{v}px"),
            };
            let declaration = match (attribute.name.as_str(), tag) {
                ("align", "table") => match v.to_ascii_lowercase().as_str() {
                    "center" => ("margin-left".into(), "auto".into()),
                    "right" => ("margin-left".into(), "auto".into()),
                    _ => continue,
                },
                ("align", _) => ("text-align".into(), v.into()),
                ("bgcolor", _) => ("background-color".into(), v.into()),
                ("color", "font") => ("color".into(), v.into()),
                ("face", "font") => ("font-family".into(), v.into()),
                ("size", "font") => match css::font_tag_size(v) {
                    Some(size) => ("font-size".into(), format!("{size}pt")),
                    None => continue,
                },
                ("width", _) => ("width".into(), pixels(v)),
                ("height", _) => ("height".into(), pixels(v)),
                ("valign", _) => ("vertical-align".into(), v.into()),
                ("hidden", _) => ("display".into(), "none".into()),
                ("type", "ol" | "ul" | "li") => ("list-style-type".into(), v.into()),
                _ => continue,
            };
            out.push(declaration);
        }
        if tag == "table"
            && self
                .attribute(element, "align")
                .is_some_and(|a| a.eq_ignore_ascii_case("center"))
        {
            out.push(("margin-right".into(), "auto".into()));
        }
        out.extend(self.sheet.matching(tag, &classes).into_iter().cloned());
        if let Some(style) = self.attribute(element, "style") {
            out.extend(css::declarations(style));
        }
        out
    }

    fn compute(&mut self, element: &Element, parent: &Inherited, whole: f64) -> (Inherited, Boxed) {
        let declarations = self.declarations(element);
        let mut inh = parent.clone();
        let mut b = Boxed {
            block: BLOCKS.contains(&element.name.as_str()),
            ..Boxed::default()
        };
        if element.name == "a" && self.attribute(element, "href").is_some() {
            inh.run.color = Some(m::Color(0, 0, 0xEE));
            inh.run.underline = m::Underline::Single;
        }
        let parent_size = parent.run.size;
        for (name, value) in &declarations {
            let size = match name.as_str() {
                "font-size" => css::font_size(value, parent_size),
                "font" => value
                    .split_whitespace()
                    .find_map(|t| css::font_size(t.split('/').next().unwrap_or(t), parent_size)),
                _ => None,
            };
            if let Some(size) = size {
                inh.run.size = size;
                inh.run.size_complex = size;
            }
        }
        let font = inh.run.size;
        let len = |v: &str| css::length(v, font, whole);
        for (name, value) in &declarations {
            let v = value.trim();
            let lower = v.to_ascii_lowercase();
            match name.as_str() {
                "font-family" => {
                    if let Some((name, generic)) = family(v) {
                        self.declare(&name, generic);
                        set_family(&mut inh.run, &name);
                    }
                }
                "font" => {
                    let tokens: Vec<&str> = v.split_whitespace().collect();
                    if let Some(at) = tokens.iter().position(|t| {
                        css::font_size(t.split('/').next().unwrap_or(t), parent_size).is_some()
                    }) {
                        if let Some((name, generic)) = family(&tokens[at + 1..].join(" ")) {
                            self.declare(&name, generic);
                            set_family(&mut inh.run, &name);
                        }
                        for t in &tokens[..at] {
                            match t.to_ascii_lowercase().as_str() {
                                "bold" | "bolder" => inh.run.bold = true,
                                "italic" | "oblique" => inh.run.italic = true,
                                _ => {}
                            }
                        }
                    }
                }
                "font-weight" => match lower.as_str() {
                    "bold" | "bolder" => inh.run.bold = true,
                    "normal" | "lighter" => inh.run.bold = false,
                    "inherit" => {}
                    n => {
                        if let Ok(n) = n.parse::<u32>() {
                            inh.run.bold = n >= 600;
                        }
                    }
                },
                "font-style" => match lower.as_str() {
                    "italic" | "oblique" => inh.run.italic = true,
                    "normal" => inh.run.italic = false,
                    _ => {}
                },
                "text-decoration" | "text-decoration-line" => {
                    if lower.contains("underline") {
                        inh.run.underline = m::Underline::Single;
                    }
                    if lower.contains("line-through") {
                        inh.run.strike = m::Strike::Single;
                    }
                    if lower.starts_with("none") {
                        inh.run.underline = m::Underline::None;
                        inh.run.strike = m::Strike::None;
                    }
                }
                "color" => match css::color(v) {
                    Some(Paint::Color(c)) => inh.run.color = Some(c),
                    Some(Paint::None) => inh.run.color = None,
                    _ => {}
                },
                "background" | "background-color" | "mso-highlight" => {
                    for token in css::words(v) {
                        match css::color(token) {
                            Some(Paint::Color(c)) => b.background = Some(c),
                            Some(Paint::None) => b.background = None,
                            _ => continue,
                        }
                        break;
                    }
                }
                "vertical-align" => match lower.as_str() {
                    "super" => inh.run.vertical = m::VerticalAlign::Superscript,
                    "sub" => inh.run.vertical = m::VerticalAlign::Subscript,
                    "baseline" => inh.run.vertical = m::VerticalAlign::Baseline,
                    "top" => b.valign = Some(m::VAlign::Top),
                    "middle" | "center" => b.valign = Some(m::VAlign::Center),
                    "bottom" => b.valign = Some(m::VAlign::Bottom),
                    _ => {}
                },
                "text-transform" => inh.run.caps = lower == "uppercase",
                "font-variant" => inh.run.small_caps = lower.contains("small-caps"),
                "letter-spacing" => inh.run.spacing = len(v).unwrap_or(0.0),
                "display" => match lower.as_str() {
                    "none" => inh.run.hidden = true,
                    "block" | "list-item" | "flex" | "grid" => b.block = true,
                    "inline" | "inline-block" => b.block = false,
                    _ => {}
                },
                "mso-hide" if lower == "all" => inh.run.hidden = true,
                "text-align" => {
                    inh.align = match lower.trim_start_matches("-webkit-") {
                        "left" | "start" => m::Alignment::Left,
                        "center" => m::Alignment::Center,
                        "right" | "end" => m::Alignment::Right,
                        "justify" => m::Alignment::Justify,
                        _ => inh.align,
                    }
                }
                "line-height" => {
                    inh.line = match lower.as_str() {
                        "normal" => Line::Normal,
                        l => match l.parse::<f64>() {
                            Ok(f) => Line::Factor(f),
                            Err(_) => match l.strip_suffix('%').and_then(|p| p.parse::<f64>().ok())
                            {
                                Some(p) => Line::Factor(p / 100.0),
                                None => len(l).map_or(inh.line, Line::Points),
                            },
                        },
                    }
                }
                "text-indent" => inh.text_indent = len(v).unwrap_or(inh.text_indent),
                "white-space" => inh.pre = lower.starts_with("pre"),
                "list-style-type" | "list-style" => {
                    if let Some(kind) = v
                        .split_whitespace()
                        .find(|t| !matches!(t.to_ascii_lowercase().as_str(), "inside" | "outside"))
                    {
                        inh.list_type = Some(kind.to_string());
                    }
                }
                "margin" | "padding" => {
                    if let Some(values) = css::sides(v) {
                        for (i, value) in values.iter().enumerate() {
                            self::side_length(name, i, value, &len, &mut b);
                        }
                    }
                }
                "border" | "border-width" | "border-style" | "border-color" => {
                    let parts = css::words(v);
                    match name.as_str() {
                        "border" => {
                            for side in &mut b.sides {
                                border(&parts, &len, side);
                            }
                        }
                        _ => {
                            if let Some(values) = css::sides(v) {
                                for (side, value) in b.sides.iter_mut().zip(values) {
                                    border(&[value], &len, side);
                                }
                            }
                        }
                    }
                }
                "width" => b.width = len(v),
                "height" => b.height = len(v),
                "page-break-before" | "break-before" => {
                    b.page_break_before =
                        matches!(lower.as_str(), "always" | "page" | "left" | "right")
                }
                n => {
                    let sides = ["top", "right", "bottom", "left"];
                    if let Some((kind, side)) = n.split_once('-') {
                        let at = sides
                            .iter()
                            .position(|s| side == *s || side.starts_with(&format!("{s}-")));
                        match (kind, at) {
                            ("margin" | "padding", Some(i)) if !side.contains('-') => {
                                self::side_length(kind, i, v, &len, &mut b)
                            }
                            ("border", Some(i)) => {
                                let parts = css::words(v);
                                border(&parts, &len, &mut b.sides[i]);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if !b.block {
            if let Some(background) = b.background {
                inh.run.shading = Some(background);
            }
        }
        (inh, b)
    }

    fn declare(&mut self, name: &str, generic: m::Generic) {
        if !self.fonts.iter().any(|f| f.name.eq_ignore_ascii_case(name)) {
            self.fonts.push(m::FontDecl {
                name: name.to_string(),
                generic,
            });
        }
    }

    fn para(&self, inh: &Inherited, ctx: &Ctx, keep_next: bool, shading: Option<m::Color>) -> Para {
        Para {
            props: m::ParagraphProps {
                alignment: inh.align,
                indent: m::Indent {
                    left: ctx.left,
                    right: ctx.right,
                    first_line: inh.text_indent,
                },
                spacing: m::Spacing {
                    line: match inh.line {
                        Line::Normal => m::LineSpacing::Proportional(100.0),
                        Line::Factor(f) => m::LineSpacing::AtLeast(f * inh.run.size),
                        Line::Points(p) => m::LineSpacing::AtLeast(p),
                    },
                    ..m::Spacing::default()
                },
                keep_next,
                widow_control: true,
                shading,
                ..m::ParagraphProps::default()
            },
            mark: inh.run.clone(),
            label: None,
            inlines: Vec::new(),
            space: true,
            trailing: false,
        }
    }

    /// Close `para` into `out` when it holds anything, and leave it open for
    /// what follows, without its label.
    fn flush(&mut self, para: &mut Para, out: &mut Vec<m::Block>) {
        if para.inlines.is_empty() {
            if let Some(label) = para.label.take() {
                self.pending_label = Some(label);
            }
            return;
        }
        if para.trailing {
            if let Some(m::Inline::Text(text)) = para.inlines.last_mut() {
                text.text.pop();
                text.anchors.pop();
            }
        }
        let label = para.label.take().or_else(|| self.pending_label.take());
        out.push(self.emit(para, label));
        para.props.indent.first_line = para.props.indent.first_line.max(0.0);
        para.space = true;
        para.trailing = false;
    }

    fn emit(&self, para: &mut Para, label: Option<m::Label>) -> m::Block {
        m::Block::Paragraph(Box::new(m::Paragraph {
            props: para.props.clone(),
            mark: para.mark.clone(),
            style: None,
            label,
            inlines: std::mem::take(&mut para.inlines),
            hidden_section: false,
        }))
    }

    fn text(&self, node: u32, inh: &Inherited, para: &mut Para) {
        let mut text = String::new();
        let mut anchors = Vec::new();
        let flush_text = |para: &mut Para, text: &mut String, anchors: &mut Vec<Anchor>| {
            if !text.is_empty() {
                para.inlines.push(m::Inline::Text(m::Text {
                    text: std::mem::take(text),
                    props: inh.run.clone(),
                    anchors: std::mem::take(anchors),
                }));
            }
        };
        for (offset, c) in self.package.text(node).chars().enumerate() {
            let anchor = Anchor::XmlText {
                part: 0,
                node,
                offset: offset as u32,
            };
            match c {
                '\r' if inh.pre => {}
                '\n' if inh.pre => {
                    flush_text(para, &mut text, &mut anchors);
                    para.inlines.push(m::Inline::Break(m::Break::Line));
                    para.trailing = false;
                }
                '\t' if inh.pre => {
                    flush_text(para, &mut text, &mut anchors);
                    para.inlines.push(m::Inline::Tab(inh.run.clone()));
                    para.trailing = false;
                }
                ' ' | '\t' | '\n' | '\r' | '\x0C' if !inh.pre => {
                    if !para.space {
                        text.push(' ');
                        anchors.push(anchor);
                        para.space = true;
                        para.trailing = true;
                    }
                }
                c => {
                    text.push(c);
                    anchors.push(anchor);
                    para.space = false;
                    para.trailing = false;
                }
            }
        }
        flush_text(para, &mut text, &mut anchors);
    }

    fn flow(
        &mut self,
        nodes: &[Node],
        inh: &Inherited,
        ctx: &Ctx,
        out: &mut Vec<m::Block>,
        para: &mut Para,
    ) {
        for node in nodes {
            let element = match node {
                Node::Text(n) => {
                    self.text(*n, inh, para);
                    continue;
                }
                Node::Element(e) => e,
            };
            let name = element.name.as_str();
            if UNSHOWN.contains(&name) {
                continue;
            }
            let (child, b) = self.compute(element, inh, ctx.width);
            match name {
                "br" => {
                    para.inlines
                        .push(m::Inline::Break(match b.page_break_before {
                            true => m::Break::Page,
                            false => m::Break::Line,
                        }));
                    para.space = true;
                    para.trailing = false;
                }
                "table" => {
                    self.flush(para, out);
                    let table = self.table(element, &child, &b, ctx);
                    out.push(m::Block::Table(Box::new(table)));
                }
                "hr" => {
                    self.flush(para, out);
                    let mut rule = self.para(&child, ctx, false, None);
                    rule.props.borders.bottom = Some(m::Border {
                        width: 0.75,
                        color: Some(m::Color(128, 128, 128)),
                        space: 0.0,
                        double: false,
                    });
                    rule.mark.size = 3.0;
                    out.push(self.emit(&mut rule, None));
                }
                _ if b.block => self.block(element, &child, &b, ctx, out, para),
                _ => self.flow(&element.children, &child, ctx, out, para),
            }
        }
    }

    fn block(
        &mut self,
        element: &Element,
        inh: &Inherited,
        b: &Boxed,
        ctx: &Ctx,
        out: &mut Vec<m::Block>,
        outer: &mut Para,
    ) {
        self.flush(outer, out);
        let name = element.name.as_str();
        let list = matches!(name, "ul" | "ol");
        let nested = !self.lists.is_empty();
        let left = b.margin[3] + b.pad(3) + b.sides[3].border().map_or(0.0, |s| s.width);
        let right = b.margin[1] + b.pad(1) + b.sides[1].border().map_or(0.0, |s| s.width);
        let inner = Ctx {
            left: ctx.left + left,
            right: ctx.right + right,
            width: (ctx.width - left - right).max(36.0),
        };
        if list {
            let ordered = name == "ol";
            self.lists.push(List {
                ordered,
                kind: inh.list_type.clone(),
                next: self
                    .attribute(element, "start")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(1),
            });
        }
        let shading = b.background;
        let mut para = self.para(
            inh,
            &inner,
            name.starts_with('h') && name.len() == 2,
            shading,
        );
        if name == "li" {
            if let Some(value) = self
                .attribute(element, "value")
                .and_then(|v| v.trim().parse().ok())
            {
                if let Some(list) = self.lists.last_mut() {
                    list.next = value;
                }
            }
            let level = self.lists.len();
            let (kind, n) = match self.lists.last_mut() {
                Some(list) => {
                    let n = list.next;
                    list.next += 1;
                    let kind = inh
                        .list_type
                        .clone()
                        .or_else(|| list.kind.clone())
                        .unwrap_or_else(|| match list.ordered {
                            true => "decimal".into(),
                            false => "bullet".into(),
                        });
                    (kind, n)
                }
                None => ("bullet".into(), 1),
            };
            if let Some(text) = list_label(&kind, n, level) {
                let mut props = inh.run.clone();
                props.underline = m::Underline::None;
                para.label = Some(m::Label {
                    text,
                    props,
                    suffix: m::LabelSuffix::Tab,
                    alignment: m::Alignment::Left,
                    tab_stop: None,
                });
                para.props.indent.first_line = -(18.0f64.min(inner.left));
            }
        }
        let before = out.len();
        self.flow(&element.children, inh, &inner, out, &mut para);
        if !para.inlines.is_empty() {
            self.flush(&mut para, out);
        } else if PARAGRAPHS.contains(&name) && out.len() == before {
            let label = para.label.take().or_else(|| self.pending_label.take());
            out.push(self.emit(&mut para, label));
        } else if let Some(label) = para.label.take() {
            self.pending_label = Some(label);
        }
        if list {
            self.lists.pop();
        }
        let (top, bottom) = match list && nested {
            true => (0.0, 0.0),
            false => (b.margin[0] + b.pad(0), b.margin[2] + b.pad(2)),
        };
        let mut paragraphs = out[before..].iter_mut().filter_map(|block| match block {
            m::Block::Paragraph(p) => Some(p),
            m::Block::Table(_) => None,
        });
        let first = paragraphs.next();
        let last = paragraphs.last();
        if let Some(first) = first {
            first.props.spacing.before = first.props.spacing.before.max(top);
            first.props.page_break_before |= b.page_break_before;
            if last.is_none() {
                first.props.spacing.after = first.props.spacing.after.max(bottom);
            }
        }
        if let Some(last) = last {
            last.props.spacing.after = last.props.spacing.after.max(bottom);
        }
    }

    fn table(&mut self, element: &Element, inh: &Inherited, b: &Boxed, ctx: &Ctx) -> m::Table {
        struct Slot<'e> {
            row: usize,
            col: usize,
            colspan: usize,
            rowspan: usize,
            element: &'e Element,
            inh: Inherited,
            b: Boxed,
        }
        let pixels = |v: Option<&str>| {
            v.and_then(|v| v.trim().parse::<f64>().ok())
                .map(|p| p * 0.75)
        };
        let frame = pixels(self.attribute(element, "border")).filter(|w| *w > 0.0);
        let padding = pixels(self.attribute(element, "cellpadding")).unwrap_or(0.75);
        let spacing = pixels(self.attribute(element, "cellspacing")).unwrap_or(0.0);
        let width = b.width;
        let whole = width.unwrap_or(ctx.width);

        let mut rows: Vec<(&Element, Inherited, Boxed, bool)> = Vec::new();
        let mut columns: Vec<Option<f64>> = Vec::new();
        fn rows_of<'e>(
            builder: &mut Builder<'_>,
            nodes: &'e [Node],
            inh: &Inherited,
            whole: f64,
            header: bool,
            rows: &mut Vec<(&'e Element, Inherited, Boxed, bool)>,
            columns: &mut Vec<Option<f64>>,
        ) {
            for node in nodes {
                let Node::Element(e) = node else { continue };
                match e.name.as_str() {
                    "tr" => {
                        let (i, b) = builder.compute(e, inh, whole);
                        rows.push((e, i, b, header));
                    }
                    "col" => {
                        let (_, b) = builder.compute(e, inh, whole);
                        let span = builder
                            .attribute(e, "span")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                            .unwrap_or(1)
                            .clamp(1, 1000);
                        columns.extend(std::iter::repeat_n(b.width, span));
                    }
                    "table" => {}
                    name => {
                        let (i, _) = builder.compute(e, inh, whole);
                        rows_of(
                            builder,
                            &e.children,
                            &i,
                            whole,
                            header || name == "thead",
                            rows,
                            columns,
                        );
                    }
                }
            }
        }
        rows_of(
            self,
            &element.children,
            inh,
            whole,
            false,
            &mut rows,
            &mut columns,
        );

        let mut slots: Vec<Slot> = Vec::new();
        let mut taken: Vec<Vec<bool>> = vec![Vec::new(); rows.len()];
        let count = rows.len();
        for (r, (tr, row_inh, _, _)) in rows.iter().enumerate() {
            let mut col = 0;
            for node in &tr.children {
                let Node::Element(td) = node else { continue };
                if !matches!(td.name.as_str(), "td" | "th") {
                    continue;
                }
                while taken[r].get(col).copied().unwrap_or(false) {
                    col += 1;
                }
                let span = |name: &str| {
                    self.attribute(td, name)
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                };
                let colspan = span("colspan").clamp(1, 1000);
                let rowspan = match span("rowspan") {
                    0 => count - r,
                    n => n.min(count - r),
                };
                for row in taken.iter_mut().skip(r).take(rowspan) {
                    if row.len() < col + colspan {
                        row.resize(col + colspan, false);
                    }
                    row[col..col + colspan].iter_mut().for_each(|t| *t = true);
                }
                let (inh, b) = self.compute(td, row_inh, whole);
                slots.push(Slot {
                    row: r,
                    col,
                    colspan,
                    rowspan,
                    element: td,
                    inh,
                    b,
                });
                col += colspan;
            }
        }
        let ncols = taken
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(columns.len())
            .max(1);
        columns.resize(ncols, None);
        for slot in slots.iter().filter(|s| s.colspan == 1) {
            if columns[slot.col].is_none() {
                columns[slot.col] = slot.b.width;
            }
        }
        let known: f64 = columns.iter().flatten().sum();
        let unknown = columns.iter().filter(|c| c.is_none()).count();
        let share = match unknown {
            0 => 0.0,
            n => ((whole - known) / n as f64).max(18.0),
        };
        let mut grid: Vec<f64> = columns.iter().map(|c| c.unwrap_or(share)).collect();
        let total: f64 = grid.iter().sum();
        let target = match width {
            Some(w) => w,
            None if total > ctx.width => ctx.width,
            None => total,
        };
        if total > 0.0 {
            grid.iter_mut().for_each(|g| *g *= target / total);
        }

        let default_border = frame.map(|_| m::Border {
            width: 0.75,
            color: Some(m::Color(128, 128, 128)),
            space: 0.0,
            double: false,
        });
        let mut model_rows: Vec<Vec<(usize, m::Cell)>> = (0..count).map(|_| Vec::new()).collect();
        for slot in &slots {
            let cell_width: f64 = grid[slot.col..slot.col + slot.colspan].iter().sum();
            let pad = |i: usize| slot.b.padding[i].unwrap_or(padding);
            let margins = m::Margins {
                top: pad(0),
                right: pad(1),
                bottom: pad(2),
                left: pad(3),
            };
            let cctx = Ctx {
                left: 0.0,
                right: 0.0,
                width: (cell_width - margins.left - margins.right).max(12.0),
            };
            let mut blocks = Vec::new();
            let mut para = self.para(&slot.inh, &cctx, false, None);
            let pending = self.pending_label.take();
            self.flow(
                &slot.element.children,
                &slot.inh,
                &cctx,
                &mut blocks,
                &mut para,
            );
            self.flush(&mut para, &mut blocks);
            self.pending_label = pending;
            if blocks.is_empty() {
                blocks.push(self.emit(&mut para, None));
            }
            let mut borders = slot.b.borders();
            for side in [
                &mut borders.top,
                &mut borders.right,
                &mut borders.bottom,
                &mut borders.left,
            ] {
                if side.is_none() {
                    *side = default_border;
                }
            }
            let shading = slot.b.background.or(rows[slot.row].2.background);
            let valign = slot
                .b
                .valign
                .or(rows[slot.row].2.valign)
                .unwrap_or(m::VAlign::Center);
            let cell = |vmerge, blocks| m::Cell {
                span: slot.colspan,
                vmerge,
                borders,
                shading,
                margins,
                valign,
                blocks,
            };
            let merged = slot.rowspan > 1;
            model_rows[slot.row].push((
                slot.col,
                cell(
                    match merged {
                        true => m::VMerge::Restart,
                        false => m::VMerge::None,
                    },
                    blocks,
                ),
            ));
            for row in model_rows
                .iter_mut()
                .skip(slot.row + 1)
                .take(slot.rowspan - 1)
            {
                let mut filler = self.para(&slot.inh, &cctx, false, None);
                let empty = vec![self.emit(&mut filler, None)];
                row.push((slot.col, cell(m::VMerge::Continue, empty)));
            }
        }
        let empty_cell = |builder: &Builder, width: f64| m::Cell {
            span: 1,
            vmerge: m::VMerge::None,
            borders: m::Borders::default(),
            shading: None,
            margins: m::Margins::default(),
            valign: m::VAlign::Top,
            blocks: vec![builder.emit(
                &mut builder.para(
                    inh,
                    &Ctx {
                        left: 0.0,
                        right: 0.0,
                        width,
                    },
                    false,
                    None,
                ),
                None,
            )],
        };
        let rows: Vec<m::Row> = model_rows
            .into_iter()
            .zip(&rows)
            .map(|(mut cells, (_, _, rb, header))| {
                cells.sort_by_key(|(col, _)| *col);
                let mut out = Vec::new();
                let mut at = 0;
                for (col, cell) in cells {
                    while at < col {
                        out.push(empty_cell(self, grid[at]));
                        at += 1;
                    }
                    at = col + cell.span;
                    out.push(cell);
                }
                if out.is_empty() {
                    out.push(empty_cell(self, grid[0]));
                }
                m::Row {
                    grid_before: 0,
                    cells: out,
                    height: rb.height.map(|value| m::RowHeight {
                        value,
                        exact: false,
                    }),
                    cant_split: false,
                    header: *header,
                }
            })
            .collect();
        let rows = match rows.is_empty() {
            true => vec![m::Row {
                grid_before: 0,
                cells: vec![empty_cell(self, grid[0])],
                height: None,
                cant_split: false,
                header: false,
            }],
            false => rows,
        };
        let mut borders = b.borders();
        if let Some(width) = frame {
            for side in [
                &mut borders.top,
                &mut borders.right,
                &mut borders.bottom,
                &mut borders.left,
            ] {
                side.get_or_insert(m::Border {
                    width,
                    color: Some(m::Color(128, 128, 128)),
                    space: 0.0,
                    double: false,
                });
            }
        }
        m::Table {
            props: m::TableProps {
                width: m::TableWidth::Points(grid.iter().sum()),
                alignment: match (b.auto[3], b.auto[1]) {
                    (true, true) => m::TableAlignment::Center,
                    (true, false) => m::TableAlignment::Right,
                    _ => m::TableAlignment::Left,
                },
                indent: ctx.left + b.margin[3],
                borders,
                cell_margins: m::Margins {
                    top: padding,
                    right: padding,
                    bottom: padding,
                    left: padding,
                },
                cell_spacing: spacing,
                float: None,
                page_break_before: b.page_break_before,
            },
            grid,
            rows,
        }
    }
}

fn side_length(
    kind: &str,
    i: usize,
    value: &str,
    len: &dyn Fn(&str) -> Option<f64>,
    b: &mut Boxed,
) {
    let auto = value.eq_ignore_ascii_case("auto");
    let length = if auto { Some(0.0) } else { len(value) };
    let Some(length) = length else { return };
    match kind {
        "margin" => {
            b.margin[i] = length;
            b.auto[i] = auto;
        }
        _ => b.padding[i] = Some(length.max(0.0)),
    }
}

fn border(parts: &[&str], len: &dyn Fn(&str) -> Option<f64>, side: &mut Side) {
    for part in parts {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "none" | "hidden" => side.style = Some(false),
            "solid" | "dotted" | "dashed" | "groove" | "ridge" | "inset" | "outset"
            | "windowtext" => side.style = Some(true),
            "double" => {
                side.style = Some(true);
                side.double = true;
            }
            "thin" => side.width = Some(0.75),
            "medium" => side.width = Some(2.25),
            "thick" => side.width = Some(3.75),
            _ => {
                if let Some(width) = len(part) {
                    side.width = Some(width);
                    if width <= 0.0 {
                        side.style = Some(false);
                    }
                } else if let Some(paint) = css::color(part) {
                    side.color = Some(match paint {
                        Paint::Color(c) => Some(c),
                        _ => None,
                    });
                }
            }
        }
    }
}

fn page(declarations: &[(String, String)]) -> m::PageSetup {
    let (mut width, mut height) = (595.3, 841.9);
    let mut margin = m::Margins {
        top: 70.85,
        right: 70.85,
        bottom: 70.85,
        left: 70.85,
    };
    let (mut header, mut footer) = (35.4, 35.4);
    let len = |v: &str| css::length(v, 12.0, 0.0);
    for (name, value) in declarations {
        let v = value.trim();
        match name.as_str() {
            "size" => {
                let tokens: Vec<String> =
                    v.split_whitespace().map(str::to_ascii_lowercase).collect();
                let lengths: Vec<f64> = tokens.iter().filter_map(|t| len(t)).collect();
                match lengths[..] {
                    [w, h] => (width, height) = (w, h),
                    [s] => (width, height) = (s, s),
                    _ => {}
                }
                for t in &tokens {
                    match t.as_str() {
                        "a4" => (width, height) = (595.3, 841.9),
                        "a5" => (width, height) = (419.5, 595.3),
                        "a3" => (width, height) = (841.9, 1190.6),
                        "letter" => (width, height) = (612.0, 792.0),
                        "legal" => (width, height) = (612.0, 1008.0),
                        _ => {}
                    }
                }
                if tokens.iter().any(|t| t == "landscape") && width < height {
                    (width, height) = (height, width);
                }
            }
            "margin" => {
                if let Some([t, r, b, l]) = css::sides(v) {
                    for (slot, value) in [
                        (&mut margin.top, t),
                        (&mut margin.right, r),
                        (&mut margin.bottom, b),
                        (&mut margin.left, l),
                    ] {
                        if let Some(v) = len(value) {
                            *slot = v;
                        }
                    }
                }
            }
            "margin-top" => margin.top = len(v).unwrap_or(margin.top),
            "margin-right" => margin.right = len(v).unwrap_or(margin.right),
            "margin-bottom" => margin.bottom = len(v).unwrap_or(margin.bottom),
            "margin-left" => margin.left = len(v).unwrap_or(margin.left),
            "mso-header-margin" => header = len(v).unwrap_or(header),
            "mso-footer-margin" => footer = len(v).unwrap_or(footer),
            _ => {}
        }
    }
    if !(width > 72.0 && height > 72.0) {
        (width, height) = (595.3, 841.9);
    }
    margin.left = margin.left.clamp(0.0, width / 3.0);
    margin.right = margin.right.clamp(0.0, width / 3.0);
    margin.top = margin.top.clamp(0.0, height / 3.0);
    margin.bottom = margin.bottom.clamp(0.0, height / 3.0);
    m::PageSetup {
        width,
        height,
        margin,
        header,
        footer,
        footer_min_height: 0.0,
        gutter: 0.0,
        background: None,
        texture: None,
        borders: None,
    }
}

pub(super) fn document(package: &HtmlPackage) -> m::Document {
    let sheet = Sheet::parse(&package.tokens.styles);
    let page = page(&sheet.page);
    let width = page.width - page.margin.left - page.margin.right;
    let mut builder = Builder {
        package,
        defaults: Sheet::parse(DEFAULTS),
        sheet,
        lists: Vec::new(),
        pending_label: None,
        fonts: vec![m::FontDecl {
            name: "Times New Roman".into(),
            generic: m::Generic::Roman,
        }],
    };
    let run = m::RunProps {
        size: 12.0,
        size_complex: 12.0,
        ..m::RunProps::default()
    };
    let root = Inherited {
        run,
        align: m::Alignment::Left,
        line: Line::Normal,
        text_indent: 0.0,
        pre: false,
        list_type: None,
    };
    let ctx = Ctx {
        left: 0.0,
        right: 0.0,
        width,
    };
    let mut blocks = Vec::new();
    let mut para = builder.para(&root, &ctx, false, None);
    builder.flow(&package.tree, &root, &ctx, &mut blocks, &mut para);
    builder.flush(&mut para, &mut blocks);
    if blocks.is_empty() {
        blocks.push(builder.emit(&mut para, None));
    }
    m::Document {
        sections: vec![m::Section {
            page,
            first_page: None,
            start: m::SectionStart::NextPage,
            columns: m::Columns::default(),
            headers: m::HeaderFooters::default(),
            footers: m::HeaderFooters::default(),
            title_page: false,
            page_number_start: None,
            page_number_format: crate::numbering::NumFormat::DECIMAL,
            line_grid: None,
            line_numbers: None,
            dropped_after: None,
            own_stories: false,
            balanced: false,
            blocks,
        }],
        settings: m::Settings {
            default_tab_stop: 36.0,
            even_and_odd_headers: false,
            justify_shrinks: false,
            break_keeps_spacing: false,
            mark_follows_page_break: false,
            spacing_adds_up: false,
            tab_over_margin: false,
            note_separator: m::RunProps::default(),
            line_number_font: m::RunProps::default(),
            external_leading: false,
            endnotes_own_page: false,
            top_spacing: true,
        },
        fonts: builder.fonts,
        embedded_fonts: Vec::new(),
    }
}
