//! `word/numbering.xml`: the abstract lists, the instances paragraphs name and
//! their overrides, and the counters the body advances as it is read, every
//! instance of one abstract list on one counter as LibreOffice shares them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::props::{on, ParaPatch, RunPatch};
use super::styles::StyleSheet;
use super::xml::{parse, Element, Ns};
use crate::model::{Alignment, LabelSuffix, ParagraphProps};
use crate::numbering::{self, Counters, NumFormat, LEVELS};

#[derive(Clone)]
pub(crate) struct Level {
    pub count: numbering::Level,
    pub suffix: LabelSuffix,
    pub alignment: Alignment,
    /// Indentation the level gives its paragraphs.
    pub left: Option<f64>,
    pub first_line: Option<f64>,
    /// The tab stop a label followed by a tab reaches first.
    pub tab: Option<f64>,
    pub run: RunPatch,
    /// The paragraph style whose indentation the level takes when it sets
    /// none.
    style: Option<String>,
}

fn format(element: &Element) -> NumFormat {
    match element.val().unwrap_or("decimal") {
        "custom" => {
            let sample = element.w("format").unwrap_or("1");
            let width = sample.chars().take_while(char::is_ascii_digit).count();
            NumFormat::Decimal {
                width: width.clamp(1, 9) as u8,
            }
        }
        name => named_format(name),
    }
}

/// The format an `ST_NumberFormat` value names.
pub(super) fn named_format(name: &str) -> NumFormat {
    match name {
        "decimalZero" => NumFormat::Decimal { width: 2 },
        "upperRoman" => NumFormat::UpperRoman,
        "lowerRoman" => NumFormat::LowerRoman,
        "upperLetter" => NumFormat::UpperLetter,
        "lowerLetter" => NumFormat::LowerLetter,
        "decimalEnclosedCircle" | "decimalEnclosedCircleChinese" => NumFormat::Circled,
        "decimalFullWidth" | "decimalFullWidth2" => NumFormat::FullWidth,
        "chicago" => NumFormat::Chicago,
        "bullet" | "none" => NumFormat::None,
        _ => NumFormat::DECIMAL,
    }
}

impl Level {
    fn read(lvl: &Element) -> Self {
        let mut level = Level {
            count: numbering::Level::default(),
            suffix: LabelSuffix::Tab,
            alignment: Alignment::Left,
            left: None,
            first_line: None,
            tab: None,
            run: RunPatch::default(),
            style: None,
        };
        for e in lvl.elements() {
            if e.ns != Ns::W {
                continue;
            }
            match &*e.name {
                "start" => {
                    if let Some(start) = e.val().and_then(|v| v.parse().ok()) {
                        level.count.start = start;
                    }
                }
                "numFmt" => level.count.format = format(e),
                "lvlText" => level.count.text = numbering::parts(e.val().unwrap_or_default()),
                "isLgl" => level.count.legal = on(e),
                "suff" => {
                    level.suffix = match e.val() {
                        Some("space") => LabelSuffix::Space,
                        Some("nothing") => LabelSuffix::Nothing,
                        _ => LabelSuffix::Tab,
                    }
                }
                "lvlJc" => {
                    level.alignment = match e.val() {
                        Some("center") => Alignment::Center,
                        Some("right" | "end") => Alignment::Right,
                        _ => Alignment::Left,
                    }
                }
                "pStyle" => level.style = e.val().map(str::to_string),
                "pPr" => {
                    let patch = ParaPatch::read(e);
                    level.left = patch.left;
                    level.first_line = patch.first_line;
                    level.tab = patch.tabs.last().map(|t| t.position);
                }
                "rPr" => level.run = RunPatch::read(e),
                _ => {}
            }
        }
        level
    }
}

struct Abstract {
    levels: Vec<Option<Level>>,
    /// The numbering style whose list this one stands for.
    num_style_link: Option<String>,
    /// The numbering style this list defines.
    style_link: Option<String>,
}

fn levels(container: &Element) -> Vec<Option<Level>> {
    let mut levels = vec![None; LEVELS];
    for lvl in container.elements().filter(|e| e.is(Ns::W, "lvl")) {
        let index: usize = lvl.w("ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
        if let Some(slot) = levels.get_mut(index) {
            *slot = Some(Level::read(lvl));
        }
    }
    levels
}

struct Instance {
    abstract_id: String,
    start_overrides: HashMap<usize, u32>,
    levels: HashMap<usize, Level>,
}

#[derive(Default)]
struct State {
    counters: Counters<String>,
    /// Instances whose start override has restarted their list.
    restarted: HashSet<String>,
}

#[derive(Default)]
pub(crate) struct Numbering {
    abstracts: HashMap<String, Abstract>,
    nums: HashMap<String, Instance>,
    state: RefCell<State>,
}

/// A numbered paragraph: its level, its label, and the indentation its
/// paragraph style lays over the level's when the numbering comes from it.
pub(crate) struct Numbered {
    pub level: Level,
    pub text: String,
    style_indent: Option<(Option<f64>, Option<f64>)>,
}

impl Numbered {
    /// Lay the level's indentation over the style's, and the style's own
    /// back over it when the style carries the numbering.
    pub fn indent(&self, props: &mut ParagraphProps) {
        let (left, first_line) = self.style_indent.unwrap_or_default();
        let indent = &mut props.indent;
        for (value, target) in [
            (left.or(self.level.left), &mut indent.left),
            (first_line.or(self.level.first_line), &mut indent.first_line),
        ] {
            if let Some(v) = value {
                *target = v;
            }
        }
    }

    /// Whether the label shows anything, which gives it its tab stop.
    pub fn visible(&self) -> bool {
        !self.text.is_empty() || self.level.count.format != NumFormat::None
    }
}

impl Numbering {
    pub fn read(part: &[u8]) -> Result<Self> {
        let root = parse(part)?;
        let mut numbering = Numbering::default();
        for e in root.elements() {
            if e.ns != Ns::W {
                continue;
            }
            match &*e.name {
                "abstractNum" => {
                    let Some(id) = e.w("abstractNumId") else {
                        continue;
                    };
                    let link = |name: &str| e.wc(name).and_then(|l| l.val()).map(str::to_string);
                    numbering.abstracts.insert(
                        id.to_string(),
                        Abstract {
                            levels: levels(e),
                            num_style_link: link("numStyleLink"),
                            style_link: link("styleLink"),
                        },
                    );
                }
                "num" => {
                    let (Some(id), Some(abstract_id)) =
                        (e.w("numId"), e.wc("abstractNumId").and_then(|a| a.val()))
                    else {
                        continue;
                    };
                    let mut instance = Instance {
                        abstract_id: abstract_id.to_string(),
                        start_overrides: HashMap::new(),
                        levels: HashMap::new(),
                    };
                    for o in e.elements().filter(|o| o.is(Ns::W, "lvlOverride")) {
                        let index: usize = o.w("ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                        if let Some(start) =
                            o.wc("startOverride").and_then(|s| s.val()?.parse().ok())
                        {
                            instance.start_overrides.insert(index, start);
                        }
                        if let Some(lvl) = o.wc("lvl") {
                            instance.levels.insert(index, Level::read(lvl));
                        }
                    }
                    numbering.nums.insert(id.to_string(), instance);
                }
                _ => {}
            }
        }
        Ok(numbering)
    }

    /// The abstract list an instance counts on, following a list that stands
    /// for a numbering style to the list that style names.
    fn abstract_of(&self, styles: &StyleSheet, num: &Instance) -> Option<(&str, &Abstract)> {
        let (id, list) = self.abstracts.get_key_value(num.abstract_id.as_str())?;
        let Some(link) = &list.num_style_link else {
            return Some((id, list));
        };
        let named = styles
            .get(link)
            .and_then(|s| s.para.num_id.as_deref())
            .and_then(|n| self.nums.get(n))
            .and_then(|n| self.abstracts.get_key_value(n.abstract_id.as_str()))
            .filter(|(_, a)| a.num_style_link.is_none());
        let defining = || {
            self.abstracts
                .iter()
                .find(|(_, a)| a.style_link.as_deref() == Some(link.as_str()))
        };
        Some(
            named
                .or_else(defining)
                .map_or((id.as_str(), list), |(k, a)| (k.as_str(), a)),
        )
    }

    /// The numbering of a paragraph whose direct properties are `direct` and
    /// whose paragraph style is `style`, counted: called once per paragraph,
    /// in reading order.
    pub fn paragraph(
        &self,
        styles: &StyleSheet,
        direct: &ParaPatch,
        style: Option<&str>,
    ) -> Option<Numbered> {
        let chain = style.map(|s| styles.chain(s)).unwrap_or_default();
        let from_style = chain
            .iter()
            .rev()
            .enumerate()
            .find_map(|(depth, s)| Some((s.para.num_id.as_deref()?, depth > 0)));
        let (num_id, via_style, from_base) = match (direct.num_id.as_deref(), from_style) {
            (Some(id), _) => (id, false, false),
            (None, Some((id, from_base))) => (id, true, from_base),
            (None, None) => return None,
        };
        let num = self.nums.get(num_id)?;
        let level = direct
            .level
            .or_else(|| chain.iter().rev().find_map(|s| s.para.level))
            .map_or(0, usize::from);
        if level >= LEVELS {
            return None;
        }
        let (list_id, list) = self.abstract_of(styles, num)?;
        let level_of = |k: usize| {
            num.levels
                .get(&k)
                .or_else(|| list.levels.get(k).and_then(Option::as_ref))
        };
        let mut chosen = level_of(level)?.clone();
        if let Some(linked) = chosen.style.as_deref().and_then(|s| styles.get(s)) {
            chosen.left = chosen.left.or(linked.para.left);
            chosen.first_line = chosen.first_line.or(linked.para.first_line);
        }
        let starts = std::array::from_fn(|k| level_of(k).map_or(0, |l| l.count.start));
        let mut state = self.state.borrow_mut();
        let restart = num
            .start_overrides
            .get(&level)
            .copied()
            .filter(|_| state.restarted.insert(num_id.to_string()));
        let values = state
            .counters
            .count(list_id.to_string(), level, starts, restart);
        let text = numbering::label(&chosen.count, &values, |k| {
            level_of(k).map_or(NumFormat::DECIMAL, |l| l.count.format)
        });
        let style_indent = via_style.then(|| {
            let own = chain.last().map(|s| &s.para);
            let parent = chain
                .len()
                .checked_sub(2)
                .filter(|_| from_base)
                .map(|i| &chain[i].para);
            let pick = |get: fn(&ParaPatch) -> Option<f64>| {
                own.and_then(get).or_else(|| parent.and_then(get))
            };
            (pick(|p| p.left), pick(|p| p.first_line))
        });
        Some(Numbered {
            level: chosen,
            text,
            style_indent,
        })
    }
}
