//! Style lookup and cascade: default style, then the parent chain from the root
//! down, then the style itself. A percentage is taken on what the chain has
//! resolved so far, which is its parent's value.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use super::model::*;
use super::units::{self, Measure};
use super::xml::{Element, Ns, Xml};

/// Automatic styles are private to the part that declares them: the body reads
/// those of `content.xml`, headers and footers those of `styles.xml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Content,
    Styles,
}

#[derive(Clone, Copy)]
pub struct Def<'a> {
    pub el: &'a Element,
    pub xml: &'a Xml,
}

impl<'a> Def<'a> {
    pub fn attr(&self, ns: Ns, local: &str) -> Option<&'a str> {
        self.el.attr(ns, local)
    }

    pub fn props(&self, local: &str) -> Option<&'a Element> {
        self.xml.child(self.el, Ns::Style, local)
    }
}

pub struct FontDecl {
    pub family: String,
    pub generic: Option<String>,
    pub fixed_pitch: bool,
    pub embedded: Vec<String>,
}

pub struct ParaStyle {
    pub para: Arc<ParaProps>,
    pub text: Arc<TextProps>,
    pub list_style: Option<String>,
    pub master_page: Option<String>,
    pub outline_level: Option<u8>,
    /// The most specific style sets its own left margin, or its own first-line
    /// indent, which then win over a list level's.
    pub own_margin: bool,
    pub own_indent: bool,
}

type Key = (String, String);

pub struct Styles<'a> {
    common: HashMap<Key, Def<'a>>,
    auto: [HashMap<Key, Def<'a>>; 2],
    defaults: HashMap<String, Def<'a>>,
    lists_common: HashMap<String, Def<'a>>,
    lists_auto: [HashMap<String, Def<'a>>; 2],
    pub outline: Option<Def<'a>>,
    pub page_layouts: HashMap<String, Def<'a>>,
    /// `draw:gradient`, `draw:hatch`, `draw:fill-image` and `draw:opacity`
    /// definitions by element and name.
    pub fills: HashMap<(&'static str, String), &'a Element>,
    pub master_pages: Vec<Def<'a>>,
    pub fonts: HashMap<String, FontDecl>,
    pub font_order: Vec<String>,
    para_memo: RefCell<HashMap<(Scope, String), Arc<ParaStyle>>>,
    text_memo: RefCell<HashMap<(usize, Scope, String), Arc<TextProps>>>,
}

fn slot(scope: Scope) -> usize {
    match scope {
        Scope::Content => 0,
        Scope::Styles => 1,
    }
}

impl<'a> Styles<'a> {
    pub fn new(content: &'a Xml, styles: Option<&'a Xml>) -> Self {
        let mut this = Styles {
            common: HashMap::new(),
            auto: [HashMap::new(), HashMap::new()],
            defaults: HashMap::new(),
            lists_common: HashMap::new(),
            lists_auto: [HashMap::new(), HashMap::new()],
            outline: None,
            page_layouts: HashMap::new(),
            fills: HashMap::new(),
            master_pages: Vec::new(),
            fonts: HashMap::new(),
            font_order: Vec::new(),
            para_memo: RefCell::new(HashMap::new()),
            text_memo: RefCell::new(HashMap::new()),
        };
        // styles.xml first: a flat or hand-made content.xml may repeat a
        // common style, and the one next to the body wins.
        if let Some(xml) = styles {
            this.collect(xml, Scope::Styles);
        }
        this.collect(content, Scope::Content);
        this
    }

    fn collect(&mut self, xml: &'a Xml, scope: Scope) {
        let Some(root) = xml.root() else {
            return;
        };
        for section in xml.children(root) {
            if section.ns != Ns::Office {
                continue;
            }
            match section.local.as_str() {
                "font-face-decls" => self.collect_fonts(xml, section),
                "styles" => self.collect_styles(xml, section, None),
                "automatic-styles" => self.collect_styles(xml, section, Some(scope)),
                "master-styles" => {
                    for m in xml.children(section) {
                        if m.is(Ns::Style, "master-page") {
                            self.master_pages.push(Def { el: m, xml });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_fonts(&mut self, xml: &'a Xml, decls: &'a Element) {
        for f in xml.children(decls) {
            if !f.is(Ns::Style, "font-face") {
                continue;
            }
            let Some(name) = f.attr(Ns::Style, "name") else {
                continue;
            };
            let family = f
                .attr(Ns::Svg, "font-family")
                .map(units::family)
                .unwrap_or_else(|| name.to_string());
            let mut embedded = Vec::new();
            if let Some(src) = xml.child(f, Ns::Svg, "font-face-src") {
                for uri in xml.children(src) {
                    if let Some(href) = uri.attr(Ns::Xlink, "href") {
                        embedded.push(href.to_string());
                    }
                }
            }
            if !self.fonts.contains_key(name) {
                self.font_order.push(name.to_string());
            }
            self.fonts.insert(
                name.to_string(),
                FontDecl {
                    family,
                    generic: f.attr(Ns::Style, "font-family-generic").map(str::to_string),
                    fixed_pitch: f.attr(Ns::Style, "font-pitch") == Some("fixed"),
                    embedded,
                },
            );
        }
    }

    fn collect_styles(&mut self, xml: &'a Xml, section: &'a Element, auto: Option<Scope>) {
        for s in xml.children(section) {
            let def = Def { el: s, xml };
            match (s.ns, s.local.as_str()) {
                (Ns::Style, "style") => {
                    let (Some(family), Some(name)) =
                        (s.attr(Ns::Style, "family"), s.attr(Ns::Style, "name"))
                    else {
                        continue;
                    };
                    let key = (family.to_string(), name.to_string());
                    match auto {
                        Some(scope) => self.auto[slot(scope)].insert(key, def),
                        None => self.common.insert(key, def),
                    };
                }
                (Ns::Style, "default-style") => {
                    if let Some(family) = s.attr(Ns::Style, "family") {
                        self.defaults.insert(family.to_string(), def);
                    }
                }
                (Ns::Text, "list-style") => {
                    if let Some(name) = s.attr(Ns::Style, "name") {
                        match auto {
                            Some(scope) => {
                                self.lists_auto[slot(scope)].insert(name.to_string(), def)
                            }
                            None => self.lists_common.insert(name.to_string(), def),
                        };
                    }
                }
                (Ns::Text, "outline-style") => self.outline = Some(def),
                (Ns::Draw, local @ ("gradient" | "hatch" | "fill-image" | "opacity")) => {
                    if let Some(name) = s.attr(Ns::Draw, "name") {
                        let kind = match local {
                            "gradient" => "gradient",
                            "hatch" => "hatch",
                            "fill-image" => "fill-image",
                            _ => "opacity",
                        };
                        self.fills.insert((kind, name.to_string()), s);
                    }
                }
                (Ns::Style, "page-layout") => {
                    if let Some(name) = s.attr(Ns::Style, "name") {
                        self.page_layouts.insert(name.to_string(), def);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn find(&self, scope: Scope, family: &str, name: &str) -> Option<Def<'a>> {
        let key = (family.to_string(), name.to_string());
        self.auto[slot(scope)]
            .get(&key)
            .or_else(|| self.common.get(&key))
            .copied()
    }

    fn find_parent(&self, scope: Scope, family: &str, name: &str) -> Option<Def<'a>> {
        let key = (family.to_string(), name.to_string());
        self.common
            .get(&key)
            .or_else(|| self.auto[slot(scope)].get(&key))
            .copied()
    }

    pub fn list_style(&self, scope: Scope, name: &str) -> Option<Def<'a>> {
        self.lists_auto[slot(scope)]
            .get(name)
            .or_else(|| self.lists_common.get(name))
            .copied()
    }

    /// The style and its ancestors, most specific first.
    pub fn chain(&self, scope: Scope, family: &str, name: Option<&str>) -> Vec<Def<'a>> {
        let mut chain = Vec::new();
        let mut current = name.and_then(|n| self.find(scope, family, n));
        while let Some(def) = current {
            if chain.len() >= 32 || chain.iter().any(|d: &Def| std::ptr::eq(d.el, def.el)) {
                break;
            }
            chain.push(def);
            current = def
                .attr(Ns::Style, "parent-style-name")
                .filter(|p| !p.is_empty())
                .and_then(|p| self.find_parent(scope, family, p));
        }
        chain
    }

    pub fn default_of(&self, family: &str) -> Option<Def<'a>> {
        self.defaults.get(family).copied()
    }

    pub fn paragraph(&self, scope: Scope, name: Option<&str>) -> Arc<ParaStyle> {
        let key = (scope, name.unwrap_or_default().to_string());
        if let Some(hit) = self.para_memo.borrow().get(&key) {
            return hit.clone();
        }
        let chain = self.chain(scope, "paragraph", name);
        let mut para = default_para();
        let mut text = default_text();
        let mut layers: Vec<Def> = self.default_of("paragraph").into_iter().collect();
        layers.extend(chain.iter().rev().copied());
        for def in &layers {
            if let Some(p) = def.props("paragraph-properties") {
                apply_para(&mut para, p, def.xml);
            }
            if let Some(t) = def.props("text-properties") {
                apply_text(&mut text, t, &self.fonts);
            }
        }
        let first_attr = |local: &str| {
            chain
                .iter()
                .find_map(|d| d.attr(Ns::Style, local))
                .map(str::to_string)
        };
        let own = chain.first().and_then(|d| d.props("paragraph-properties"));
        let resolved = Arc::new(ParaStyle {
            para: Arc::new(para),
            text: Arc::new(text),
            list_style: first_attr("list-style-name").filter(|s| !s.is_empty()),
            master_page: chain
                .first()
                .and_then(|d| d.attr(Ns::Style, "master-page-name"))
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            outline_level: first_attr("default-outline-level").and_then(|l| l.parse().ok()),
            own_margin: own.is_some_and(|p| {
                p.attr(Ns::Fo, "margin-left").is_some() || p.attr(Ns::Fo, "margin").is_some()
            }),
            own_indent: own.is_some_and(|p| p.attr(Ns::Fo, "text-indent").is_some()),
        });
        self.para_memo.borrow_mut().insert(key, resolved.clone());
        resolved
    }

    /// A character style applied over `base`.
    pub fn text(&self, scope: Scope, base: &Arc<TextProps>, name: Option<&str>) -> Arc<TextProps> {
        let Some(name) = name else {
            return base.clone();
        };
        let key = (Arc::as_ptr(base) as usize, scope, name.to_string());
        if let Some(hit) = self.text_memo.borrow().get(&key) {
            return hit.clone();
        }
        let chain = self.chain(scope, "text", Some(name));
        let resolved = if chain.is_empty() {
            base.clone()
        } else {
            let mut text = (**base).clone();
            for def in chain.iter().rev() {
                if let Some(t) = def.props("text-properties") {
                    apply_text(&mut text, t, &self.fonts);
                }
            }
            Arc::new(text)
        };
        self.text_memo.borrow_mut().insert(key, resolved.clone());
        resolved
    }

    /// The `style:<local>` element of each layer of `family`'s cascade for
    /// `name`, default style first.
    pub fn layers(
        &self,
        scope: Scope,
        family: &str,
        name: Option<&str>,
        local: &str,
    ) -> Vec<(&'a Element, &'a Xml)> {
        let mut out = Vec::new();
        let mut defs: Vec<Def> = self.default_of(family).into_iter().collect();
        defs.extend(self.chain(scope, family, name).into_iter().rev());
        for def in defs {
            if let Some(p) = def.props(local) {
                out.push((p, def.xml));
            }
        }
        out
    }

    pub fn face(&self, name: &str) -> Option<&FontDecl> {
        self.fonts.get(name)
    }
}

pub fn default_face() -> Face {
    Face {
        family: "Liberation Serif".to_string(),
        generic: Some("roman".to_string()),
        fixed_pitch: false,
        size: 12.0,
        weight: 400,
        italic: false,
    }
}

pub fn default_text() -> TextProps {
    TextProps {
        latin: default_face(),
        asian: default_face(),
        complex: default_face(),
        underline: None,
        strike: None,
        overline: None,
        color: None,
        highlight: None,
        transform: Transform::None,
        small_caps: false,
        position: None,
        letter_spacing: 0.0,
        scale: 1.0,
        hidden: false,
        export_hidden: false,
        outline: false,
        shadow: false,
        kerning: false,
        hyphenate: false,
        language: None,
        country: None,
    }
}

pub fn default_para() -> ParaProps {
    ParaProps {
        align: Align::Left,
        align_last: Align::Left,
        rtl: false,
        margin: Sides::default(),
        text_indent: 0.0,
        auto_text_indent: false,
        line_height: LineHeight::Proportional(1.0),
        contextual_spacing: false,
        keep_with_next: false,
        keep_together: false,
        widows: 0,
        orphans: 0,
        break_before: Break::None,
        break_after: Break::None,
        tab_stops: Vec::new(),
        default_tab: 1.25 * 72.0 / 2.54,
        borders: Sides::default(),
        padding: Sides::default(),
        background: None,
        drop_cap: None,
        page_number: None,
    }
}

fn weight(value: &str) -> Option<u16> {
    match value.trim() {
        "normal" => Some(400),
        "bold" => Some(700),
        v => v.parse().ok(),
    }
}

fn line_style(value: &str) -> Option<LineStyle> {
    Some(match value.trim() {
        "none" | "hidden" => return None,
        "solid" => LineStyle::Solid,
        "double" | "double-thin" => LineStyle::Double,
        "dotted" => LineStyle::Dotted,
        "dash" | "dashed" | "fine-dashed" => LineStyle::Dashed,
        "long-dash" => LineStyle::LongDash,
        "dot-dash" | "dash-dot" => LineStyle::DotDash,
        "dot-dot-dash" | "dash-dot-dot" => LineStyle::DotDotDash,
        "wave" => LineStyle::Wave,
        "groove" => LineStyle::Groove,
        "ridge" => LineStyle::Ridge,
        "inset" => LineStyle::Inset,
        "outset" => LineStyle::Outset,
        _ => LineStyle::Solid,
    })
}

/// `0.5pt solid #000000`, in any order.
pub fn border(value: &str) -> Option<Border> {
    let mut width = None;
    let mut style = None;
    let mut color = Color(0);
    for token in value.split_whitespace() {
        if let Some(c) = units::color(token) {
            color = c;
        } else if let Some(l) = units::length(token) {
            width = Some(l);
        } else {
            match token {
                "none" | "hidden" => return None,
                "thin" => width = Some(0.5),
                "medium" => width = Some(1.0),
                "thick" => width = Some(1.5),
                t => style = line_style(t),
            }
        }
    }
    let style = style?;
    Some(Border {
        width: width.unwrap_or(0.5),
        style,
        color,
    })
}

fn text_line(e: &Element, which: &str) -> Option<Option<TextLine>> {
    let style = e.attr(Ns::Style, &format!("text-{which}-style"));
    let kind = e.attr(Ns::Style, &format!("text-{which}-type"));
    if style.is_none() && kind.is_none() {
        return None;
    }
    if kind == Some("none") || style == Some("none") {
        return Some(None);
    }
    let mut line_style = style.and_then(line_style).unwrap_or(LineStyle::Solid);
    if kind == Some("double") {
        line_style = LineStyle::Double;
    }
    let width = e.attr(Ns::Style, &format!("text-{which}-width"));
    let color = e
        .attr(Ns::Style, &format!("text-{which}-color"))
        .and_then(units::color);
    Some(Some(TextLine {
        style: line_style,
        bold: matches!(width, Some("bold") | Some("thick")),
        color,
    }))
}

fn apply_face(face: &mut Face, e: &Element, fonts: &HashMap<String, FontDecl>, suffix: &str) {
    let fo = |local: &str| {
        if suffix.is_empty() {
            e.attr(Ns::Fo, local)
        } else {
            e.attr(Ns::Style, &format!("{local}{suffix}"))
        }
    };
    if let Some(name) = e.attr(Ns::Style, &format!("font-name{suffix}")) {
        match fonts.get(name) {
            Some(decl) => {
                face.family = decl.family.clone();
                face.generic = decl.generic.clone();
                face.fixed_pitch = decl.fixed_pitch;
            }
            None => face.family = name.to_string(),
        }
    }
    if let Some(family) = fo("font-family") {
        face.family = units::family(family);
        face.generic = e
            .attr(Ns::Style, &format!("font-family-generic{suffix}"))
            .map(str::to_string);
        face.fixed_pitch = e.attr(Ns::Style, &format!("font-pitch{suffix}")) == Some("fixed");
    }
    if let Some(size) = fo("font-size") {
        match units::measure(size) {
            Some(Measure::Length(l)) if l > 0.0 => face.size = l,
            Some(Measure::Percent(p)) if p > 0.0 => face.size *= p / 100.0,
            _ => {}
        }
    }
    if let Some(rel) = e.attr(Ns::Style, &format!("font-size-rel{suffix}")) {
        if let Some(l) = units::length(rel) {
            face.size = (face.size + l).max(1.0);
        }
    }
    if let Some(w) = fo("font-weight").and_then(weight) {
        face.weight = w;
    }
    if let Some(s) = fo("font-style") {
        face.italic = matches!(s, "italic" | "oblique");
    }
}

pub fn apply_text(t: &mut TextProps, e: &Element, fonts: &HashMap<String, FontDecl>) {
    apply_face(&mut t.latin, e, fonts, "");
    apply_face(&mut t.asian, e, fonts, "-asian");
    apply_face(&mut t.complex, e, fonts, "-complex");
    if let Some(u) = text_line(e, "underline") {
        t.underline = u;
    }
    if let Some(s) = text_line(e, "line-through") {
        t.strike = s;
    }
    if let Some(o) = text_line(e, "overline") {
        t.overline = o;
    }
    if let Some(c) = e.attr(Ns::Fo, "color") {
        t.color = units::color(c);
    }
    if e.attr(Ns::Style, "use-window-font-color") == Some("true") {
        t.color = None;
    }
    if let Some(bg) = e.attr(Ns::Fo, "background-color") {
        t.highlight = units::color(bg);
    }
    if let Some(tr) = e.attr(Ns::Fo, "text-transform") {
        t.transform = match tr {
            "uppercase" => Transform::Uppercase,
            "lowercase" => Transform::Lowercase,
            "capitalize" => Transform::Capitalize,
            _ => Transform::None,
        };
    }
    if let Some(v) = e.attr(Ns::Fo, "font-variant") {
        t.small_caps = v == "small-caps";
    }
    if let Some(pos) = e.attr(Ns::Style, "text-position") {
        t.position = text_position(pos);
    }
    if let Some(ls) = e.attr(Ns::Fo, "letter-spacing") {
        t.letter_spacing = units::length(ls).unwrap_or(0.0);
    }
    if let Some(sc) = e.attr(Ns::Style, "text-scale").and_then(units::percent) {
        t.scale = sc / 100.0;
    }
    if let Some(d) = e.attr(Ns::Text, "display") {
        t.hidden = d == "none";
    }
    if let Some(o) = e.attr(Ns::Style, "text-outline") {
        t.outline = units::boolean(o);
    }
    if let Some(s) = e.attr(Ns::Fo, "text-shadow") {
        t.shadow = s != "none";
    }
    if let Some(k) = e.attr(Ns::Style, "letter-kerning") {
        t.kerning = units::boolean(k);
    }
    if let Some(h) = e.attr(Ns::Fo, "hyphenate") {
        t.hyphenate = units::boolean(h);
    }
    if let Some(l) = e.attr(Ns::Fo, "language") {
        t.language = Some(l.to_string());
    }
    if let Some(c) = e.attr(Ns::Fo, "country") {
        t.country = Some(c.to_string());
    }
}

fn text_position(value: &str) -> Option<(f32, f32)> {
    let mut parts = value.split_whitespace();
    let shift = match parts.next()? {
        "super" => 33.0,
        "sub" => -33.0,
        other => units::percent(other)?,
    };
    let size = parts
        .next()
        .and_then(units::percent)
        .unwrap_or(if shift == 0.0 { 100.0 } else { 58.0 });
    if shift == 0.0 && (size - 100.0).abs() < f32::EPSILON {
        return None;
    }
    Some((shift / 100.0, size / 100.0))
}

fn align(value: &str, rtl: bool) -> Align {
    match value {
        "start" if rtl => Align::Right,
        "end" if !rtl => Align::Right,
        "end" => Align::Left,
        "right" => Align::Right,
        "center" => Align::Center,
        "justify" => Align::Justify,
        _ => Align::Left,
    }
}

fn apply_measure(target: &mut Pt, value: Option<&str>) {
    match value.and_then(units::measure) {
        Some(Measure::Length(l)) => *target = l,
        Some(Measure::Percent(p)) => *target *= p / 100.0,
        None => {}
    }
}

/// `fo:margin`, then each side, which wins.
pub fn apply_sides(sides: &mut Sides<Pt>, e: &Element, prefix: &str) {
    if let Some(all) = e.attr(Ns::Fo, prefix) {
        for side in [
            &mut sides.top,
            &mut sides.right,
            &mut sides.bottom,
            &mut sides.left,
        ] {
            apply_measure(side, Some(all));
        }
    }
    apply_measure(&mut sides.top, e.attr(Ns::Fo, &format!("{prefix}-top")));
    apply_measure(&mut sides.right, e.attr(Ns::Fo, &format!("{prefix}-right")));
    apply_measure(
        &mut sides.bottom,
        e.attr(Ns::Fo, &format!("{prefix}-bottom")),
    );
    apply_measure(&mut sides.left, e.attr(Ns::Fo, &format!("{prefix}-left")));
}

pub fn apply_borders(borders: &mut Sides<Option<Border>>, e: &Element) {
    if let Some(all) = e.attr(Ns::Fo, "border") {
        let b = border(all);
        *borders = Sides {
            top: b,
            right: b,
            bottom: b,
            left: b,
        };
    }
    for (side, slot) in [
        ("top", &mut borders.top),
        ("right", &mut borders.right),
        ("bottom", &mut borders.bottom),
        ("left", &mut borders.left),
    ] {
        if let Some(v) = e.attr(Ns::Fo, &format!("border-{side}")) {
            *slot = border(v);
        }
    }
}

pub fn background(e: &Element) -> Option<Option<Color>> {
    if let Some(bg) = e.attr(Ns::Fo, "background-color") {
        return Some(units::color(bg));
    }
    match e.attr(Ns::Draw, "fill") {
        Some("solid") => Some(e.attr(Ns::Draw, "fill-color").and_then(units::color)),
        Some("none") => Some(None),
        _ => None,
    }
}

pub fn brk(value: &str) -> Break {
    match value {
        "page" => Break::Page,
        "column" => Break::Column,
        "even-page" => Break::EvenPage,
        "odd-page" => Break::OddPage,
        _ => Break::None,
    }
}

pub fn apply_para(p: &mut ParaProps, e: &Element, xml: &Xml) {
    if let Some(wm) = e.attr(Ns::Style, "writing-mode") {
        p.rtl = wm.starts_with("rl");
    }
    if let Some(a) = e.attr(Ns::Fo, "text-align") {
        p.align = align(a, p.rtl);
    }
    if let Some(a) = e.attr(Ns::Fo, "text-align-last") {
        p.align_last = align(a, p.rtl);
    }
    apply_sides(&mut p.margin, e, "margin");
    apply_measure(&mut p.text_indent, e.attr(Ns::Fo, "text-indent"));
    if let Some(v) = e.attr(Ns::Style, "auto-text-indent") {
        p.auto_text_indent = units::boolean(v);
    }
    if let Some(lh) = e.attr(Ns::Fo, "line-height") {
        p.line_height = match units::measure(lh) {
            Some(Measure::Percent(pct)) => LineHeight::Proportional(pct / 100.0),
            Some(Measure::Length(l)) => LineHeight::Fixed(l),
            None => LineHeight::Proportional(1.0),
        };
    }
    if let Some(l) = e
        .attr(Ns::Style, "line-height-at-least")
        .and_then(units::length)
    {
        p.line_height = LineHeight::AtLeast(l);
    }
    if let Some(l) = e.attr(Ns::Style, "line-spacing").and_then(units::length) {
        p.line_height = LineHeight::Leading(l);
    }
    if let Some(v) = e.attr(Ns::Style, "contextual-spacing") {
        p.contextual_spacing = units::boolean(v);
    }
    if let Some(v) = e.attr(Ns::Fo, "keep-with-next") {
        p.keep_with_next = v == "always";
    }
    if let Some(v) = e.attr(Ns::Fo, "keep-together") {
        p.keep_together = v == "always";
    }
    if let Some(v) = e.attr(Ns::Fo, "widows").and_then(|v| v.trim().parse().ok()) {
        p.widows = v;
    }
    if let Some(v) = e
        .attr(Ns::Fo, "orphans")
        .and_then(|v| v.trim().parse().ok())
    {
        p.orphans = v;
    }
    if let Some(v) = e.attr(Ns::Fo, "break-before") {
        p.break_before = brk(v);
    }
    if let Some(v) = e.attr(Ns::Fo, "break-after") {
        p.break_after = brk(v);
    }
    if let Some(v) = e
        .attr(Ns::Style, "tab-stop-distance")
        .and_then(units::length)
    {
        if v > 0.0 {
            p.default_tab = v;
        }
    }
    if let Some(v) = e.attr(Ns::Style, "page-number") {
        p.page_number = v.trim().parse().ok();
    }
    apply_borders(&mut p.borders, e);
    apply_sides(&mut p.padding, e, "padding");
    if let Some(bg) = background(e) {
        p.background = bg;
    }
    if let Some(tabs) = xml.child(e, Ns::Style, "tab-stops") {
        p.tab_stops = xml
            .children(tabs)
            .filter(|t| t.is(Ns::Style, "tab-stop"))
            .filter_map(|t| {
                let position = t.attr(Ns::Style, "position").and_then(units::length)?;
                let kind = match t.attr(Ns::Style, "type") {
                    Some("center") => TabKind::Center,
                    Some("right") => TabKind::Right,
                    Some("char") => TabKind::Char(
                        t.attr(Ns::Style, "char")
                            .and_then(|c| c.chars().next())
                            .unwrap_or('.'),
                    ),
                    _ => TabKind::Left,
                };
                let leader = match t.attr(Ns::Style, "leader-text") {
                    Some(text) => text.chars().next().filter(|c| *c != ' '),
                    None => match t.attr(Ns::Style, "leader-style") {
                        Some("dotted") => Some('.'),
                        Some("solid") => Some('_'),
                        Some("dash") | Some("long-dash") => Some('-'),
                        _ => None,
                    },
                };
                Some(TabStop {
                    position,
                    kind,
                    leader,
                })
            })
            .collect();
        p.tab_stops.sort_by(|a, b| {
            a.position
                .partial_cmp(&b.position)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    if let Some(dc) = xml.child(e, Ns::Style, "drop-cap") {
        let lines = dc
            .attr(Ns::Style, "lines")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        p.drop_cap = (lines > 1).then(|| DropCap {
            lines,
            length: match dc.attr(Ns::Style, "length") {
                Some("word") => 0,
                Some(n) => n.parse().unwrap_or(1),
                None => 1,
            },
            distance: dc
                .attr(Ns::Style, "distance")
                .and_then(units::length)
                .unwrap_or(0.0),
        });
    }
}

pub fn columns(xml: &Xml, props: &Element) -> Option<Columns> {
    let c = xml.child(props, Ns::Style, "columns")?;
    let count = c
        .attr(Ns::Fo, "column-count")
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(1)
        .max(1);
    let gap = c
        .attr(Ns::Fo, "column-gap")
        .and_then(units::length)
        .unwrap_or(0.0);
    let widths: Vec<f32> = xml
        .children(c)
        .filter(|col| col.is(Ns::Style, "column"))
        .filter_map(|col| {
            col.attr(Ns::Style, "rel-width")
                .and_then(|w| w.trim_end_matches('*').parse().ok())
        })
        .collect();
    let indent = |col: &Element, name| {
        col.attr(Ns::Fo, name)
            .and_then(units::length)
            .unwrap_or(0.0)
    };
    let indents: Vec<(f32, f32)> = xml
        .children(c)
        .filter(|col| col.is(Ns::Style, "column"))
        .map(|col| (indent(col, "start-indent"), indent(col, "end-indent")))
        .collect();
    let separator = xml.child(c, Ns::Style, "column-sep").and_then(|sep| {
        let style = sep.attr(Ns::Style, "style").unwrap_or("solid");
        let width = sep.attr(Ns::Style, "width").unwrap_or("0.5pt");
        let color = sep.attr(Ns::Style, "color").unwrap_or("#000000");
        border(&format!("{width} {style} {color}"))
    });
    Some(Columns {
        count,
        gap,
        widths: if widths.len() == count as usize {
            widths
        } else {
            Vec::new()
        },
        indents: if indents.len() == count as usize {
            indents
        } else {
            Vec::new()
        },
        separator,
    })
}
