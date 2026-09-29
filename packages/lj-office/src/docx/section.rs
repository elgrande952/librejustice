//! The body cut into sections at each `w:sectPr`, with the page setup,
//! columns, headers and footers each one names.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};

use super::body::Story;
use super::numbering::named_format;
use super::props::{border, on, twips};
use super::xml::{parse, Element, Ns};
use super::{Context, Rels};
use crate::model::*;
use crate::numbering::NumFormat;

/// Letter with one-inch margins, what Word lays a section out on when it
/// names no page.
fn default_page() -> PageSetup {
    PageSetup {
        width: 612.0,
        height: 792.0,
        margin: Margins {
            top: 72.0,
            right: 72.0,
            bottom: 72.0,
            left: 72.0,
        },
        header: 36.0,
        footer: 36.0,
        texture: None,
        footer_min_height: 0.0,
        gutter: 0.0,
        background: None,
        borders: None,
    }
}

/// The stories a section that names none of a kind takes from the one
/// before: as LibreOffice reads them, the first page takes what the previous
/// section's first page showed, its default when it had no title page.
fn inherited(set: &HeaderFooters, previous: &Section) -> HeaderFooters {
    HeaderFooters {
        first: if previous.title_page {
            set.first.clone()
        } else {
            set.default.clone()
        },
        ..set.clone()
    }
}

struct Stories<'c, 'p> {
    ctx: &'c Context<'p>,
    rels: &'c Rels,
    read: HashMap<String, Arc<Vec<Block>>>,
}

impl Stories<'_, '_> {
    fn story(&mut self, id: &str) -> Result<Option<Arc<Vec<Block>>>> {
        let Some(name) = self.rels.by_id(id) else {
            return Ok(None);
        };
        if let Some(story) = self.read.get(name) {
            return Ok(Some(story.clone()));
        }
        let Some((part, data)) = self.ctx.package.part(name) else {
            return Ok(None);
        };
        let root = parse(data).with_context(|| format!("reading {name}"))?;
        let rels = Rels::read(self.ctx.package, name)?;
        let mut blocks = Vec::new();
        Story::new(self.ctx, part).with_rels(&rels).blocks(
            &root,
            None,
            &mut blocks,
            &mut Vec::new(),
        )?;
        join_hidden_marks(&mut blocks);
        let story = Arc::new(blocks);
        self.read.insert(name.to_string(), story.clone());
        Ok(Some(story))
    }

    /// The section `sect` sets, its runs appended. LibreOffice sets a table
    /// of contents in columns across the page's width, as its own section.
    fn push(
        &mut self,
        sections: &mut Vec<Section>,
        sect: Option<&Element>,
        runs: Vec<(bool, Vec<Block>)>,
        closed: bool,
    ) -> Result<()> {
        let mut section = self.section(sect, sections.last(), Vec::new())?;
        let columned = section.columns.count > 1 || section.columns.widths.len() > 1;
        let runs = if columned {
            runs
        } else {
            vec![(false, runs.into_iter().flat_map(|(_, b)| b).collect())]
        };
        let count = runs.len();
        let parts: Vec<Section> = runs
            .into_iter()
            .enumerate()
            .map(|(k, (contents, mut blocks))| {
                join_hidden_marks(&mut blocks);
                let mut part = continued(&section, contents);
                if closed && k + 1 == count && blocks.len() > 1 && blocks.last().is_some_and(blank)
                {
                    if let Some(Block::Paragraph(p)) = blocks.pop() {
                        part.dropped_after = Some(p.props.spacing.after);
                    }
                }
                part.blocks = blocks;
                part
            })
            .collect();
        let mut parts = parts.into_iter();
        if let Some(first) = parts.next() {
            section.columns = first.columns;
            section.dropped_after = first.dropped_after;
            section.blocks = first.blocks;
        }
        sections.push(section);
        sections.extend(parts);
        Ok(())
    }

    fn section(
        &mut self,
        sect: Option<&Element>,
        previous: Option<&Section>,
        blocks: Vec<Block>,
    ) -> Result<Section> {
        let mut page = default_page();
        let mut section = Section {
            page: page.clone(),
            first_page: None,
            start: SectionStart::NextPage,
            columns: Columns::default(),
            headers: previous.map_or_else(Default::default, |s| inherited(&s.headers, s)),
            footers: previous.map_or_else(Default::default, |s| inherited(&s.footers, s)),
            title_page: false,
            page_number_start: None,
            page_number_format: NumFormat::DECIMAL,
            line_grid: None,
            line_numbers: None,
            dropped_after: None,
            own_stories: false,
            balanced: false,
            blocks,
        };
        let Some(sect) = sect else {
            return Ok(section);
        };
        let mut page_borders = None;
        for e in sect.elements() {
            match &*e.name {
                "pgSz" => {
                    if let Some(v) = e.w("w").and_then(twips).filter(|v| *v > 0.0) {
                        page.width = v;
                    }
                    if let Some(v) = e.w("h").and_then(twips).filter(|v| *v > 0.0) {
                        page.height = v;
                    }
                }
                "pgMar" => {
                    let side = |name: &str| e.w(name).and_then(twips).map(f64::abs);
                    let m = &mut page.margin;
                    for (value, target) in [
                        (side("top"), &mut m.top),
                        (side("right"), &mut m.right),
                        (side("bottom"), &mut m.bottom),
                        (side("left"), &mut m.left),
                        (side("header"), &mut page.header),
                        (side("footer"), &mut page.footer),
                        (side("gutter"), &mut page.gutter),
                    ] {
                        if let Some(v) = value {
                            *target = v;
                        }
                    }
                }
                "cols" => {
                    let columns = &mut section.columns;
                    let listed: Vec<(f64, f64)> = e
                        .elements()
                        .filter(|c| &*c.name == "col")
                        .map(|c| {
                            (
                                c.w("w").and_then(twips).unwrap_or(0.0),
                                c.w("space").and_then(twips).unwrap_or(0.0),
                            )
                        })
                        .collect();
                    columns.count = e.w("num").and_then(|n| n.parse().ok()).unwrap_or(1).max(1);
                    if let Some(gap) = e.w("space").and_then(twips) {
                        columns.gap = gap;
                    }
                    columns.separator =
                        e.w("sep").is_some_and(|v| matches!(v, "1" | "true" | "on"));
                    let equal = !matches!(e.w("equalWidth"), Some("0" | "false" | "off"));
                    if !equal && !listed.is_empty() {
                        columns.count = listed.len();
                        columns.widths = listed;
                    }
                }
                "type" => {
                    section.start = match e.val() {
                        Some("continuous") => SectionStart::Continuous,
                        Some("evenPage") => SectionStart::EvenPage,
                        Some("oddPage") => SectionStart::OddPage,
                        Some("nextColumn") => SectionStart::NextColumn,
                        _ => SectionStart::NextPage,
                    }
                }
                "titlePg" => section.title_page = on(e),
                "pgBorders" => page_borders = Some(e),
                "docGrid" => {
                    section.line_grid = match e.w("type") {
                        Some("lines" | "linesAndChars" | "snapToChars") => {
                            e.w("linePitch").and_then(twips).filter(|p| *p > 0.0)
                        }
                        _ => None,
                    }
                }
                "lnNumType" => {
                    let count_by = e.w("countBy").and_then(|v| v.parse().ok()).unwrap_or(0);
                    section.line_numbers = (count_by > 0).then(|| LineNumbers {
                        count_by,
                        start: e.w("start").and_then(|v| v.parse().ok()).unwrap_or(0),
                        restart: match e.w("restart") {
                            Some("newSection") => LineRestart::NewSection,
                            Some("continuous") => LineRestart::Continuous,
                            _ => LineRestart::NewPage,
                        },
                        distance: e.w("distance").and_then(twips).filter(|d| *d > 0.0),
                    });
                }
                "pgNumType" => {
                    section.page_number_start = e.w("start").and_then(|v| v.parse().ok());
                    if let Some(format) = e.w("fmt") {
                        section.page_number_format = named_format(format);
                    }
                }
                "headerReference" | "footerReference" => {
                    section.own_stories = true;
                    let Some(story) = e.attr(Ns::R, "id") else {
                        continue;
                    };
                    let story = self.story(story)?;
                    let set = if &*e.name == "headerReference" {
                        &mut section.headers
                    } else {
                        &mut section.footers
                    };
                    let slot = match e.w("type") {
                        Some("first") => &mut set.first,
                        Some("even") => &mut set.even,
                        _ => &mut set.default,
                    };
                    *slot = story;
                }
                _ => {}
            }
        }
        if let Some(e) = page_borders {
            page.borders = page_borders_of(e, &page.margin);
        }
        section.page = page;
        Ok(section)
    }
}

/// `w:pgBorders`, each line `space` from the page's edge or, by default,
/// outside the margin.
fn page_borders_of(e: &Element, margin: &Margins) -> Option<Box<PageBorders>> {
    let side = |name: &str| e.elements().find(|s| &*s.name == name).and_then(border);
    let (top, right, bottom, left) = (side("top"), side("right"), side("bottom"), side("left"));
    if top.is_none() && right.is_none() && bottom.is_none() && left.is_none() {
        return None;
    }
    let from_page = e.w("offsetFrom") == Some("page");
    let inset = |b: &Option<Border>, margin: f64| {
        b.as_ref().map_or(0.0, |b| {
            if from_page {
                b.space + b.weight() / 2.0
            } else {
                margin - b.space - b.weight() / 2.0
            }
        })
    };
    let display = e.w("display");
    Some(Box::new(PageBorders {
        inset: Margins {
            top: inset(&top, margin.top),
            right: inset(&right, margin.right),
            bottom: inset(&bottom, margin.bottom),
            left: inset(&left, margin.left),
        },
        top,
        right,
        bottom,
        left,
        on_first: display != Some("notFirstPage"),
        on_others: display != Some("firstPage"),
    }))
}

/// An empty paragraph, which LibreOffice drops when it only carries the
/// properties of the section it closes.
fn blank(block: &Block) -> bool {
    let Block::Paragraph(p) = block else {
        return false;
    };
    p.label.is_none()
        && p.inlines.iter().all(|inline| match inline {
            Inline::Text(t) => t.text.is_empty(),
            _ => false,
        })
}

pub(super) fn sections(
    ctx: &Context<'_>,
    rels: &Rels,
    part: u16,
    body: &Element,
) -> Result<Vec<Section>> {
    let mut blocks = Vec::new();
    let mut ends = Vec::new();
    let mut story = Story::new(ctx, part).with_rels(rels);
    story.blocks(body, None, &mut blocks, &mut ends)?;
    let contents = story.contents;

    let mut stories = Stories {
        ctx,
        rels,
        read: HashMap::new(),
    };
    let mut sections: Vec<Section> = Vec::new();
    let mut rest = blocks.into_iter();
    let mut taken = 0;
    for (last, sect) in ends {
        let blocks: Vec<Block> = rest.by_ref().take(last + 1 - taken).collect();
        let runs = runs(blocks, taken, &contents);
        stories.push(&mut sections, Some(sect), runs, true)?;
        taken = last + 1;
    }
    let runs = runs(rest.collect(), taken, &contents);
    stories.push(&mut sections, body.wc("sectPr"), runs, false)?;
    Ok(sections)
}

/// Blocks cut where a table of contents starts and ends, each run told
/// whether it is within one; `first` is the index of the first block.
fn runs(blocks: Vec<Block>, first: usize, contents: &[usize]) -> Vec<(bool, Vec<Block>)> {
    let mut out: Vec<(bool, Vec<Block>)> = Vec::new();
    for (k, block) in blocks.into_iter().enumerate() {
        let within = contents.binary_search(&(first + k)).is_ok();
        match out.last_mut() {
            Some((w, run)) if *w == within => run.push(block),
            _ => out.push((within, vec![block])),
        }
    }
    out
}

/// The part of `section` from a table of contents on or back from one, set
/// on below what precedes it; the table runs across the page's width.
fn continued(section: &Section, contents: bool) -> Section {
    Section {
        page: section.page.clone(),
        first_page: None,
        start: SectionStart::Continuous,
        columns: if contents {
            Columns::default()
        } else {
            section.columns.clone()
        },
        headers: section.headers.clone(),
        footers: section.footers.clone(),
        title_page: false,
        page_number_start: None,
        page_number_format: section.page_number_format,
        line_grid: section.line_grid,
        line_numbers: None,
        dropped_after: None,
        own_stories: section.own_stories,
        balanced: false,
        blocks: Vec::new(),
    }
}
