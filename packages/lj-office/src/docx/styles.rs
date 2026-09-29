//! The style sheet and the order its layers apply in: document defaults, table
//! style, paragraph style, character style, direct formatting, each style
//! after the chain it is based on. A toggle such as bold set by two styles is
//! on, not cancelled, as LibreOffice reads it.

use std::collections::HashMap;

use anyhow::Result;

use super::props::{ParaPatch, RunPatch};
use super::theme::Theme;
use super::xml::{parse, Element, Node, Ns};
use crate::model::{ParagraphProps, RunProps, Strike};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Paragraph,
    Character,
    Table,
    Numbering,
}

pub(crate) struct Style {
    pub kind: Kind,
    pub based_on: Option<String>,
    /// The character style a paragraph style is linked to, and back.
    pub link: Option<String>,
    pub para: ParaPatch,
    pub run: RunPatch,
    /// The style element itself for a table style, whose table, row and cell
    /// properties the table reader takes.
    pub table: Option<Element>,
    /// A table style's conditional paragraph and run properties, in the order
    /// they apply.
    pub conditional: Vec<(Condition, ParaPatch, RunPatch)>,
    /// The frame a paragraph style sets its paragraphs in.
    pub frame: Option<Element>,
}

/// A part of a table style that applies to some cells only, in the order
/// LibreOffice lays them, each over the ones before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Condition {
    Band2Horz,
    Band1Horz,
    Band2Vert,
    Band1Vert,
    LastCol,
    FirstCol,
    LastRow,
    FirstRow,
    SwCell,
    SeCell,
    NwCell,
    NeCell,
}

impl Condition {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "band2Horz" => Self::Band2Horz,
            "band1Horz" => Self::Band1Horz,
            "band2Vert" => Self::Band2Vert,
            "band1Vert" => Self::Band1Vert,
            "lastCol" => Self::LastCol,
            "firstCol" => Self::FirstCol,
            "lastRow" => Self::LastRow,
            "firstRow" => Self::FirstRow,
            "swCell" => Self::SwCell,
            "seCell" => Self::SeCell,
            "nwCell" => Self::NwCell,
            "neCell" => Self::NeCell,
            _ => return None,
        })
    }

    pub fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// What a table style lays under the paragraphs of one cell: its own
/// paragraph and run properties, not those of the style it is based on, as
/// LibreOffice reads it, then those of the conditions the cell meets.
#[derive(Clone, Copy)]
pub(crate) struct TableLayers<'s> {
    pub style: &'s Style,
    /// [`Condition::bit`]s.
    pub conditions: u16,
}

impl<'s> TableLayers<'s> {
    fn conditional(self) -> impl Iterator<Item = &'s (Condition, ParaPatch, RunPatch)> {
        self.style
            .conditional
            .iter()
            .filter(move |(c, _, _)| self.conditions & c.bit() != 0)
    }
}

#[derive(Default)]
pub(crate) struct StyleSheet {
    pub para_defaults: ParaPatch,
    pub run_defaults: RunPatch,
    styles: HashMap<String, Style>,
    default_paragraph: Option<String>,
    default_character: Option<String>,
    pub default_table: Option<String>,
}

/// Longest `basedOn` chain followed, which also stops a cycle.
const DEPTH: usize = 32;

impl StyleSheet {
    pub fn read(part: &[u8]) -> Result<Self> {
        let root = parse(part)?;
        let mut sheet = StyleSheet::default();
        if let Some(defaults) = root.wc("docDefaults") {
            if let Some(rpr) = defaults.wc("rPrDefault").and_then(|e| e.wc("rPr")) {
                sheet.run_defaults = RunPatch::read(rpr);
            }
            if let Some(ppr) = defaults.wc("pPrDefault").and_then(|e| e.wc("pPr")) {
                sheet.para_defaults = ParaPatch::read(ppr);
            }
        }
        for node in root.children {
            let Node::Element(mut e) = node else {
                continue;
            };
            if &*e.name != "style" {
                continue;
            }
            let Some(id) = e.w("styleId").map(str::to_string) else {
                continue;
            };
            let kind = match e.w("type") {
                Some("character") => Kind::Character,
                Some("table") => Kind::Table,
                Some("numbering") => Kind::Numbering,
                _ => Kind::Paragraph,
            };
            if e.w("default")
                .is_some_and(|v| matches!(v, "1" | "true" | "on"))
            {
                let slot = match kind {
                    Kind::Paragraph => &mut sheet.default_paragraph,
                    Kind::Character => &mut sheet.default_character,
                    Kind::Table => &mut sheet.default_table,
                    Kind::Numbering => &mut None,
                };
                slot.get_or_insert_with(|| id.clone());
            }
            let value = |name: &str| e.wc(name).and_then(|c| c.val()).map(str::to_string);
            let (based_on, link) = (value("basedOn"), value("link"));
            let para = e.wc("pPr").map(ParaPatch::read).unwrap_or_default();
            let run = e.wc("rPr").map(RunPatch::read).unwrap_or_default();
            let mut conditional: Vec<_> = e
                .elements()
                .filter(|c| &*c.name == "tblStylePr")
                .filter_map(|c| {
                    let condition = Condition::parse(c.w("type")?)?;
                    let para = c.wc("pPr").map(ParaPatch::read).unwrap_or_default();
                    let run = c.wc("rPr").map(RunPatch::read).unwrap_or_default();
                    Some((condition, para, run))
                })
                .collect();
            conditional.sort_by_key(|c| c.0);
            let frame = e.children.iter_mut().find_map(|node| match node {
                Node::Element(ppr) if ppr.is(Ns::W, "pPr") => {
                    let at = ppr
                        .children
                        .iter()
                        .position(|c| matches!(c, Node::Element(f) if f.is(Ns::W, "framePr")))?;
                    match ppr.children.remove(at) {
                        Node::Element(frame) => Some(frame),
                        _ => None,
                    }
                }
                _ => None,
            });
            let style = Style {
                kind,
                based_on,
                link,
                para,
                run,
                table: (kind == Kind::Table).then_some(e),
                conditional,
                frame,
            };
            sheet.styles.entry(id).or_insert(style);
        }
        Ok(sheet)
    }

    pub fn get(&self, id: &str) -> Option<&Style> {
        self.styles.get(id)
    }

    /// A style and the ones it is based on, root first.
    pub fn chain(&self, id: &str) -> Vec<&Style> {
        let mut chain = Vec::new();
        let mut next = Some(id);
        while let Some(id) = next {
            let Some(style) = self.styles.get(id) else {
                break;
            };
            if chain.len() == DEPTH {
                break;
            }
            chain.push(style);
            next = style.based_on.as_deref();
        }
        chain.reverse();
        chain
    }

    /// The paragraph style a paragraph takes: the one it names when that
    /// exists as a paragraph style, else the default one.
    pub fn paragraph_style<'a>(&'a self, named: Option<&'a str>) -> Option<&'a str> {
        named
            .filter(|id| {
                self.styles
                    .get(*id)
                    .is_some_and(|s| s.kind == Kind::Paragraph)
            })
            .or(self.default_paragraph.as_deref())
    }

    /// The character style a run takes. A run naming a paragraph style gets
    /// the character style linked to it, or its run properties.
    fn character_chain(&self, named: Option<&str>) -> Vec<&Style> {
        let Some(id) = named.or(self.default_character.as_deref()) else {
            return Vec::new();
        };
        match self.styles.get(id) {
            Some(style) if style.kind == Kind::Paragraph => match &style.link {
                Some(link) if self.styles.contains_key(link) => self.chain(link),
                _ => self.chain(id),
            },
            Some(_) => self.chain(id),
            None => Vec::new(),
        }
    }

    /// Paragraph properties from the defaults, the table style, the
    /// paragraph style and `direct`, in that order.
    pub fn paragraph(
        &self,
        table_style: Option<TableLayers<'_>>,
        style: Option<&str>,
        direct: &ParaPatch,
    ) -> ParagraphProps {
        self.paragraph_layered(table_style, style, &|_| {}, direct)
    }

    /// Paragraph properties with `layer`, a list level's, laid between the
    /// paragraph style and `direct`.
    pub fn paragraph_layered(
        &self,
        table_style: Option<TableLayers<'_>>,
        style: Option<&str>,
        layer: &dyn Fn(&mut ParagraphProps),
        direct: &ParaPatch,
    ) -> ParagraphProps {
        let mut props = ParagraphProps {
            widow_control: true,
            ..ParagraphProps::default()
        };
        self.para_defaults.apply(&mut props);
        if let Some(table) = table_style {
            table.style.para.apply(&mut props);
            for (_, para, _) in table.conditional() {
                para.apply(&mut props);
            }
        }
        if let Some(id) = self.paragraph_style(style) {
            for s in self.chain(id) {
                s.para.apply(&mut props);
            }
        }
        layer(&mut props);
        direct.apply(&mut props);
        props
    }

    /// Run properties from the defaults and the character style `id`, as
    /// what LibreOffice writes outside any paragraph takes them.
    pub fn character(&self, theme: &Theme, id: &str) -> RunProps {
        let mut props = RunProps::default();
        self.run_defaults.apply(&mut props, theme);
        for s in self.character_chain(Some(id)) {
            s.run.apply(&mut props, theme);
        }
        props
    }

    /// Run properties from the defaults, the table style, the paragraph
    /// style, the character style and `direct`, in that order.
    pub fn run(
        &self,
        theme: &Theme,
        table_style: Option<TableLayers<'_>>,
        paragraph_style: Option<&str>,
        direct: &RunPatch,
    ) -> RunProps {
        let mut props = RunProps::default();
        self.run_defaults.apply(&mut props, theme);
        if let Some(table) = table_style {
            table.style.run.apply(&mut props, theme);
            for (_, _, run) in table.conditional() {
                run.apply(&mut props, theme);
            }
        }
        if let Some(id) = self.paragraph_style(paragraph_style) {
            for s in self.chain(id) {
                s.run.apply(&mut props, theme);
            }
        }
        let below = props.clone();
        let chain = self.character_chain(direct.style.as_deref());
        for s in &chain {
            s.run.apply(&mut props, theme);
        }
        toggle(&chain, &below, &mut props);
        direct.apply(&mut props, theme);
        props
    }
}

/// Word's toggle properties: a character style turning one on where the
/// layers below already have it turns it off, as LibreOffice reads them.
fn toggle(chain: &[&Style], below: &RunProps, props: &mut RunProps) {
    let set = |get: fn(&RunPatch) -> Option<bool>| {
        chain.iter().rev().find_map(|s| get(&s.run)) == Some(true)
    };
    for (get, was, now) in [
        (
            (|p| p.bold) as fn(&RunPatch) -> Option<bool>,
            below.bold,
            &mut props.bold,
        ),
        (|p| p.italic, below.italic, &mut props.italic),
        (
            |p| p.bold_complex,
            below.bold_complex,
            &mut props.bold_complex,
        ),
        (
            |p| p.italic_complex,
            below.italic_complex,
            &mut props.italic_complex,
        ),
        (|p| p.caps, below.caps, &mut props.caps),
        (|p| p.small_caps, below.small_caps, &mut props.small_caps),
        (|p| p.hidden, below.hidden, &mut props.hidden),
    ] {
        if was && set(get) {
            *now = false;
        }
    }
    for (get, kind) in [
        (
            (|p| p.strike) as fn(&RunPatch) -> Option<bool>,
            Strike::Single,
        ),
        (|p| p.double_strike, Strike::Double),
    ] {
        if below.strike == kind && set(get) {
            props.strike = Strike::None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Alignment, Color, TabKind};

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn sheet(body: &str) -> StyleSheet {
        let xml = format!(
            r#"<w:styles {W}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>{body}</w:styles>"#
        );
        StyleSheet::read(xml.as_bytes()).unwrap()
    }

    fn element(xml: &str) -> Element {
        let tag = &xml[1..xml.find(['>', ' ']).unwrap()];
        let xml = xml.replacen(tag, &format!("{tag} {W}"), 1);
        parse(xml.as_bytes()).unwrap()
    }

    fn run(sheet: &StyleSheet, para: Option<&str>, rpr: &str) -> RunProps {
        let direct = RunPatch::read(&element(&format!("<w:rPr>{rpr}</w:rPr>")));
        sheet.run(&Theme::default(), None, para, &direct)
    }

    #[test]
    fn each_layer_overrides_the_one_below_it_and_nothing_else() {
        let sheet = sheet(concat!(
            r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:rPr><w:sz w:val="24"/></w:rPr></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="Base"><w:basedOn w:val="Normal"/><w:rPr><w:b/><w:color w:val="FF0000"/></w:rPr></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="Heading"><w:basedOn w:val="Base"/><w:rPr><w:sz w:val="32"/></w:rPr></w:style>"#,
            r#"<w:style w:type="character" w:styleId="Quiet"><w:rPr><w:b w:val="0"/><w:i/></w:rPr></w:style>"#,
        ));
        let normal = run(&sheet, None, "");
        assert_eq!((normal.size, normal.bold), (12.0, false));
        assert_eq!(&*normal.fonts.ascii, "Calibri");

        let heading = run(&sheet, Some("Heading"), "");
        assert_eq!(heading.size, 16.0);
        assert!(heading.bold);
        assert_eq!(heading.color, Some(Color(255, 0, 0)));

        let quiet = run(&sheet, Some("Heading"), r#"<w:rStyle w:val="Quiet"/>"#);
        assert!(!quiet.bold && quiet.italic);

        let direct = run(
            &sheet,
            Some("Heading"),
            r#"<w:rStyle w:val="Quiet"/><w:b/><w:sz w:val="18"/>"#,
        );
        assert!(direct.bold && direct.italic);
        assert_eq!(direct.size, 9.0);
    }

    /// LibreOffice 26.2 on the same styles: a character style toggles what
    /// the defaults or the paragraph style turned on, direct formatting sets.
    #[test]
    fn a_character_style_toggles_what_the_paragraph_has_on() {
        let sheet = sheet(concat!(
            r#"<w:style w:type="paragraph" w:styleId="Strong"><w:rPr><w:b/><w:caps/></w:rPr></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="Child"><w:basedOn w:val="Strong"/></w:style>"#,
            r#"<w:style w:type="character" w:styleId="Emph"><w:rPr><w:b/><w:caps/></w:rPr></w:style>"#,
            r#"<w:style w:type="character" w:styleId="EmphChild"><w:basedOn w:val="Emph"/></w:style>"#,
        ));
        let emph = r#"<w:rStyle w:val="Emph"/>"#;
        let toggled = run(&sheet, Some("Strong"), emph);
        assert!(!toggled.bold && !toggled.caps);
        assert!(!run(&sheet, Some("Child"), emph).bold);
        assert!(!run(&sheet, Some("Strong"), r#"<w:rStyle w:val="EmphChild"/>"#).bold);
        assert!(run(&sheet, None, emph).bold);
        assert!(run(&sheet, Some("Strong"), r#"<w:rStyle w:val="Emph"/><w:b/>"#).bold);
        assert!(run(&sheet, Some("Strong"), "").bold);
    }

    #[test]
    fn an_unknown_paragraph_style_falls_back_to_the_default_one() {
        let sheet = sheet(
            r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:pPr><w:jc w:val="both"/></w:pPr></w:style>"#,
        );
        let props = sheet.paragraph(None, Some("Missing"), &ParaPatch::default());
        assert_eq!(props.alignment, Alignment::Justify);
        assert_eq!(props.spacing.after, 8.0);
    }

    #[test]
    fn a_tab_cleared_by_a_later_layer_is_gone_and_the_others_stay() {
        let sheet = sheet(
            r#"<w:style w:type="paragraph" w:styleId="Toc"><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/><w:tab w:val="right" w:leader="dot" w:pos="9000"/></w:tabs></w:pPr></w:style>"#,
        );
        let ppr = element(
            r#"<w:pPr><w:tabs><w:tab w:val="clear" w:pos="720"/><w:tab w:val="center" w:pos="4500"/></w:tabs></w:pPr>"#,
        );
        let props = sheet.paragraph(None, Some("Toc"), &ParaPatch::read(&ppr));
        let stops: Vec<(f64, TabKind)> = props.tabs.iter().map(|t| (t.position, t.kind)).collect();
        assert_eq!(
            stops,
            vec![(225.0, TabKind::Center), (450.0, TabKind::Right)]
        );
    }
}
