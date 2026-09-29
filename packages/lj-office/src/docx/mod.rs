//! The `.docx` reader: the package the splice opens, its main part, styles,
//! theme and settings, read into the document model.

mod body;
mod chart;
mod drawing;
mod embedded;
mod numbering;
mod props;
mod section;
mod styles;
mod table;
mod theme;
mod xml;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use crate::ooxml::OoxmlPackage;
use anyhow::{anyhow, Context as _, Result};

use crate::model::{Block, Document, FontDecl, Generic, RunProps, Settings};
use numbering::Numbering;
pub(crate) use props::RunPatch;
pub(crate) use styles::StyleSheet;
pub(crate) use theme::Theme;
use xml::{parse, Element, Ns};

/// A part's relationships: identifier, type and the target part's name.
pub(crate) struct Rels(Vec<(String, String, String)>);

impl Rels {
    pub fn read(package: &OoxmlPackage, part: &str) -> Result<Self> {
        let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
        let name = if dir.is_empty() {
            format!("_rels/{file}.rels")
        } else {
            format!("{dir}/_rels/{file}.rels")
        };
        let Some(bytes) = package.entry(&name) else {
            return Ok(Self(Vec::new()));
        };
        let root = parse(bytes)?;
        let rels = root
            .elements()
            .filter(|e| e.is(Ns::Rel, "Relationship"))
            .filter(|e| e.attr(Ns::None, "TargetMode") != Some("External"))
            .filter_map(|e| {
                let id = e.attr(Ns::None, "Id")?;
                let kind = e.attr(Ns::None, "Type")?.rsplit('/').next()?;
                let target = resolve(dir, e.attr(Ns::None, "Target")?);
                Some((id.to_string(), kind.to_string(), target))
            })
            .collect();
        Ok(Self(rels))
    }

    pub fn by_id(&self, id: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(i, _, _)| i == id)
            .map(|(_, _, t)| t.as_str())
    }

    fn by_kind(&self, kind: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(_, k, _)| k == kind)
            .map(|(_, _, t)| t.as_str())
    }
}

/// A relationship target as a package entry name.
fn resolve(dir: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// What every story of the document reads through.
pub(crate) struct Context<'p> {
    pub package: &'p OoxmlPackage,
    pub styles: StyleSheet,
    pub theme: Theme,
    pub numbering: Numbering,
    /// Pictures read so far, by part name, shared by every frame showing one.
    pub media: RefCell<HashMap<String, Arc<[u8]>>>,
    pub footnotes: Option<NotePart>,
    pub endnotes: Option<NotePart>,
    /// Footnotes and endnotes numbered so far.
    pub numbered: Cell<[u32; 2]>,
    /// The Word version the file is laid out for.
    pub compat: u32,
}

/// The part holding the footnotes or the endnotes.
pub(crate) struct NotePart {
    pub part: u16,
    pub root: Element,
    pub rels: Rels,
}

impl NotePart {
    fn read(package: &OoxmlPackage, name: Option<&str>) -> Result<Option<Self>> {
        let Some((part, data)) = name.and_then(|name| package.part(name)) else {
            return Ok(None);
        };
        let name = name.expect("a part was found by its name");
        Ok(Some(Self {
            part,
            root: parse(data).with_context(|| format!("reading {name}"))?,
            rels: Rels::read(package, name)?,
        }))
    }
}

pub fn read(bytes: &[u8]) -> Result<Document> {
    let package = OoxmlPackage::open(bytes)?;
    let main = Rels::read(&package, "")?
        .by_kind("officeDocument")
        .map(str::to_string)
        .unwrap_or_else(|| "word/document.xml".to_string());
    let rels = Rels::read(&package, &main)?;
    let entry = |kind: &str| rels.by_kind(kind).and_then(|name| package.entry(name));

    let theme = entry("theme")
        .map(Theme::read)
        .transpose()
        .context("reading the theme")?
        .unwrap_or_default();
    let styles = entry("styles")
        .map(StyleSheet::read)
        .transpose()
        .context("reading the styles")?
        .unwrap_or_default();
    let mut settings = entry("settings")
        .map(read_settings)
        .transpose()
        .context("reading the settings")?
        .unwrap_or(Settings {
            default_tab_stop: 36.0,
            even_and_odd_headers: false,
            justify_shrinks: false,
            break_keeps_spacing: true,
            mark_follows_page_break: false,
            spacing_adds_up: false,
            tab_over_margin: true,
            note_separator: RunProps::default(),
            line_number_font: RunProps::default(),
            external_leading: true,
            endnotes_own_page: false,
            top_spacing: true,
        });
    let numbering = entry("numbering")
        .map(Numbering::read)
        .transpose()
        .context("reading the numbering")?
        .unwrap_or_default();
    let fonts = entry("fontTable")
        .map(read_fonts)
        .transpose()
        .context("reading the font table")?
        .unwrap_or_default();
    let embedded_fonts = match rels.by_kind("fontTable") {
        Some(table) => embedded::read(&package, table).context("reading the embedded fonts")?,
        None => Vec::new(),
    };

    settings.note_separator = styles.run(&theme, None, None, &Default::default());
    settings.line_number_font = styles.character(&theme, "LineNumber");
    let settings_root = entry("settings").and_then(|part| parse(part).ok());
    let compat = settings_root
        .as_ref()
        .map_or(12, |root| compatibility_mode(root.wc("compat")));
    let shows_background = settings_root
        .as_ref()
        .and_then(|root| root.wc("displayBackgroundShape"))
        .is_some_and(props::on);
    let context = Context {
        package: &package,
        styles,
        theme,
        numbering,
        media: RefCell::default(),
        footnotes: NotePart::read(&package, rels.by_kind("footnotes"))?,
        endnotes: NotePart::read(&package, rels.by_kind("endnotes"))?,
        numbered: Cell::new([0, 0]),
        compat,
    };
    let (part, data) = package
        .part(&main)
        .ok_or_else(|| anyhow!("the package has no {main}"))?;
    let root = parse(data).context("reading the main part")?;
    let body = root
        .wc("body")
        .ok_or_else(|| anyhow!("the main part has no body"))?;
    let mut sections = section::sections(&context, &rels, part, body)?;
    if let Some(Block::Paragraph(p)) = sections.first_mut().and_then(|s| s.blocks.first_mut()) {
        if p.props.spacing.before_auto {
            p.props.spacing.before = 0.0;
        }
    }
    let background = root
        .wc("background")
        .filter(|_| shows_background)
        .and_then(drawing::page_background);
    let texture = root
        .wc("background")
        .filter(|_| shows_background)
        .and_then(|b| {
            drawing::page_texture(b, &|id| Some(Arc::from(package.entry(rels.by_id(id)?)?)))
        });
    for section in &mut sections {
        section.page.background = background;
        section.page.texture = texture.clone();
        if let Some(first) = &mut section.first_page {
            first.background = background;
            first.texture = texture.clone();
        }
    }
    Ok(Document {
        sections,
        settings,
        fonts,
        embedded_fonts,
    })
}

fn read_settings(part: &[u8]) -> Result<Settings> {
    let root = parse(part)?;
    let compat = root.wc("compat");
    let mode = compatibility_mode(compat);
    Ok(Settings {
        default_tab_stop: root
            .wc("defaultTabStop")
            .and_then(|e| e.val())
            .and_then(props::twips)
            .filter(|v| *v > 0.0)
            .unwrap_or(36.0),
        even_and_odd_headers: root.wc("evenAndOddHeaders").is_some_and(props::on),
        justify_shrinks: mode >= 15,
        break_keeps_spacing: mode <= 14,
        mark_follows_page_break: compat
            .and_then(|c| c.wc("splitPgBreakAndParaMark"))
            .is_some_and(props::on),
        spacing_adds_up: false,
        tab_over_margin: mode <= 14,
        note_separator: RunProps::default(),
        line_number_font: RunProps::default(),
        external_leading: true,
        endnotes_own_page: false,
        top_spacing: true,
    })
}

/// The Word version the file is laid out for, 12 when it names none.
fn compatibility_mode(compat: Option<&Element>) -> u32 {
    compat
        .into_iter()
        .flat_map(|c| c.elements())
        .filter(|e| e.is(Ns::W, "compatSetting") && e.w("name") == Some("compatibilityMode"))
        .find_map(|e| e.val()?.parse().ok())
        .unwrap_or(12)
}

fn read_fonts(part: &[u8]) -> Result<Vec<FontDecl>> {
    let root = parse(part)?;
    Ok(root
        .elements()
        .filter(|e| e.is(Ns::W, "font"))
        .filter_map(|e| {
            let generic = match e.wc("family").and_then(|f| f.val()) {
                Some("swiss") => Generic::Swiss,
                Some("roman") => Generic::Roman,
                Some("modern") => Generic::Modern,
                Some("script") => Generic::Script,
                Some("decorative") => Generic::Decorative,
                _ => Generic::Unknown,
            };
            Some(FontDecl {
                name: e.w("name")?.to_string(),
                generic,
            })
        })
        .collect())
}
