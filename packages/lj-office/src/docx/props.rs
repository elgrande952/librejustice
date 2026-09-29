//! Formatting as a style or a run writes it: every property optional, each
//! layer of the hierarchy laid over the one before.

use std::sync::Arc;

use super::theme::{Theme, ThemeFont};
use super::xml::Element;
use crate::model::*;

/// A length in points from a twentieth of a point, or from a measure with a
/// unit as the strict schema writes it.
pub(crate) fn twips(value: &str) -> Option<f64> {
    measure(value, 20.0)
}

/// A length in points from a count of `per_point` units, or from a measure
/// with its unit.
pub(crate) fn measure(value: &str, per_point: f64) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    Some(match unit {
        "" => number / per_point,
        "pt" => number,
        "in" => number * 72.0,
        "cm" => number * 72.0 / 2.54,
        "mm" => number * 72.0 / 25.4,
        "pc" | "pi" => number * 12.0,
        _ => return None,
    })
}

/// `ST_OnOff`, where an element without a value means on.
pub(crate) fn on(element: &Element) -> bool {
    !matches!(element.val(), Some("0" | "false" | "off"))
}

fn color(value: Option<&str>) -> Option<Color> {
    value.and_then(|v| if v == "auto" { None } else { Color::hex(v) })
}

/// The fill of `w:shd`, its pattern mixed into its background.
pub(crate) fn shading(element: &Element) -> Option<Color> {
    let fill = color(element.w("fill"));
    let ipat = match element.val().unwrap_or("clear") {
        "nil" | "none" => return None,
        "clear" => 0,
        "solid" => 1,
        "pct5" => 2,
        "pct10" => 3,
        "pct20" => 4,
        "pct25" => 5,
        "pct30" => 6,
        "pct40" => 7,
        "pct50" => 8,
        "pct60" => 9,
        "pct70" => 10,
        "pct75" => 11,
        "pct80" => 12,
        "pct90" => 13,
        "horzStripe" => 14,
        "vertStripe" => 15,
        "reverseDiagStripe" => 16,
        "diagStripe" => 17,
        "horzCross" => 18,
        "diagCross" => 19,
        "thinHorzStripe" => 20,
        "thinVertStripe" => 21,
        "thinReverseDiagStripe" => 22,
        "thinDiagStripe" => 23,
        "thinHorzCross" => 24,
        "thinDiagCross" => 25,
        "pct12" => 37,
        "pct15" => 38,
        "pct35" => 43,
        "pct37" => 44,
        "pct45" => 46,
        "pct55" => 49,
        "pct62" => 51,
        "pct65" => 52,
        "pct85" => 57,
        "pct87" => 58,
        "pct95" => 60,
        _ => 0,
    };
    let share = pattern_share(ipat);
    if share == 0 {
        return fill;
    }
    let back = fill.unwrap_or(Color::WHITE);
    let front = color(element.w("color")).unwrap_or(Color::BLACK);
    Some(Color(
        mix(back.0, front.0, share),
        mix(back.1, front.1, share),
        mix(back.2, front.2, share),
    ))
}

pub(crate) fn border(element: &Element) -> Option<Border> {
    let style = element.val()?;
    if matches!(style, "nil" | "none") {
        return None;
    }
    Some(Border {
        width: element
            .w("sz")
            .and_then(|v| measure(v, 8.0))
            .unwrap_or(0.5)
            .max(0.25),
        color: color(element.w("color")),
        space: element
            .w("space")
            .and_then(|v| measure(v, 1.0))
            .unwrap_or(0.0),
        double: style.starts_with("double") || style.starts_with("thinThick"),
    })
}

/// A border side: `None` leaves the inherited one, `Some(None)` removes it.
type Side = Option<Option<Border>>;

#[derive(Clone, Default)]
pub(crate) struct BordersPatch {
    pub top: Side,
    pub bottom: Side,
    pub left: Side,
    pub right: Side,
    pub between: Side,
    pub vertical: Side,
}

impl BordersPatch {
    pub fn read(element: &Element) -> Self {
        let mut patch = Self::default();
        for side in element.elements() {
            let value = Some(border(side));
            match &*side.name {
                "top" => patch.top = value,
                "bottom" => patch.bottom = value,
                "left" | "start" => patch.left = value,
                "right" | "end" => patch.right = value,
                "between" | "insideH" => patch.between = value,
                "insideV" => patch.vertical = value,
                _ => {}
            }
        }
        patch
    }

    pub fn apply(&self, borders: &mut Borders) {
        for (patch, side) in [
            (self.top, &mut borders.top),
            (self.bottom, &mut borders.bottom),
            (self.left, &mut borders.left),
            (self.right, &mut borders.right),
            (self.between, &mut borders.between),
            (self.vertical, &mut borders.vertical),
        ] {
            if let Some(value) = patch {
                *side = value;
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FontName {
    Named(Arc<str>),
    Theme(ThemeFont),
}

#[derive(Clone, Default)]
pub(crate) struct RunPatch {
    pub style: Option<String>,
    pub ascii: Option<FontName>,
    pub h_ansi: Option<FontName>,
    pub east_asia: Option<FontName>,
    pub complex_font: Option<FontName>,
    pub size: Option<f64>,
    pub size_complex: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub bold_complex: Option<bool>,
    pub italic_complex: Option<bool>,
    pub underline: Option<Underline>,
    pub strike: Option<bool>,
    pub double_strike: Option<bool>,
    pub caps: Option<bool>,
    pub small_caps: Option<bool>,
    pub vertical: Option<VerticalAlign>,
    pub position: Option<f64>,
    pub color: Option<Option<Color>>,
    pub highlight: Option<Option<Color>>,
    pub shading: Option<Option<Color>>,
    pub spacing: Option<f64>,
    pub scale: Option<f64>,
    pub kerning: Option<bool>,
    pub hidden: Option<bool>,
    pub complex: Option<bool>,
    pub rtl: Option<bool>,
    pub east_asian_hint: Option<bool>,
}

fn highlight(name: &str) -> Option<Color> {
    Some(match name {
        "black" => Color(0, 0, 0),
        "blue" => Color(0, 0, 255),
        "cyan" => Color(0, 255, 255),
        "green" => Color(0, 255, 0),
        "magenta" => Color(255, 0, 255),
        "red" => Color(255, 0, 0),
        "yellow" => Color(255, 255, 0),
        "white" => Color(255, 255, 255),
        "darkBlue" => Color(0, 0, 128),
        "darkCyan" => Color(0, 128, 128),
        "darkGreen" => Color(0, 128, 0),
        "darkMagenta" => Color(128, 0, 128),
        "darkRed" => Color(128, 0, 0),
        "darkYellow" => Color(128, 128, 0),
        "darkGray" => Color(128, 128, 128),
        "lightGray" => Color(192, 192, 192),
        _ => return None,
    })
}

fn underline(value: &str) -> Underline {
    match value {
        "none" => Underline::None,
        "words" => Underline::Words,
        "double" | "wavyDouble" => Underline::Double,
        "thick" => Underline::Thick,
        "dotted" | "dottedHeavy" => Underline::Dotted,
        v if v.starts_with("dash") || v.starts_with("dot") => Underline::Dashed,
        v if v.starts_with("wav") => Underline::Wave,
        _ => Underline::Single,
    }
}

impl RunPatch {
    pub fn read(rpr: &Element) -> Self {
        let mut p = Self::default();
        for e in rpr.elements() {
            match &*e.name {
                "rStyle" => p.style = e.val().map(str::to_string),
                "rFonts" => {
                    let slot = |name: &str, theme: &str| {
                        e.w(theme)
                            .and_then(ThemeFont::parse)
                            .map(FontName::Theme)
                            .or_else(|| e.w(name).map(|n| FontName::Named(Arc::from(n))))
                    };
                    let set = |target: &mut Option<FontName>, value: Option<FontName>| {
                        if value.is_some() {
                            *target = value;
                        }
                    };
                    set(&mut p.ascii, slot("ascii", "asciiTheme"));
                    set(&mut p.h_ansi, slot("hAnsi", "hAnsiTheme"));
                    set(&mut p.east_asia, slot("eastAsia", "eastAsiaTheme"));
                    set(&mut p.complex_font, slot("cs", "cstheme"));
                    if let Some(hint) = e.w("hint") {
                        p.east_asian_hint = Some(hint == "eastAsia");
                    }
                }
                "sz" => p.size = e.val().and_then(|v| measure(v, 2.0)),
                "szCs" => p.size_complex = e.val().and_then(|v| measure(v, 2.0)),
                "b" => p.bold = Some(on(e)),
                "i" => p.italic = Some(on(e)),
                "bCs" => p.bold_complex = Some(on(e)),
                "iCs" => p.italic_complex = Some(on(e)),
                "u" => p.underline = e.val().map(underline),
                "strike" => p.strike = Some(on(e)),
                "dstrike" => p.double_strike = Some(on(e)),
                "caps" => p.caps = Some(on(e)),
                "smallCaps" => p.small_caps = Some(on(e)),
                "vertAlign" => {
                    p.vertical = Some(match e.val() {
                        Some("superscript") => VerticalAlign::Superscript,
                        Some("subscript") => VerticalAlign::Subscript,
                        _ => VerticalAlign::Baseline,
                    })
                }
                "position" => p.position = e.val().and_then(|v| measure(v, 2.0)),
                "color" => p.color = Some(color(e.val())),
                "highlight" => p.highlight = Some(e.val().and_then(highlight)),
                "shd" => p.shading = Some(shading(e)),
                "spacing" => p.spacing = e.val().and_then(twips),
                "w" => {
                    p.scale = e
                        .val()
                        .and_then(|v| v.trim_end_matches('%').parse::<f64>().ok())
                        .map(|v| v / 100.0)
                }
                "kern" => p.kerning = e.val().and_then(|v| v.parse::<f64>().ok()).map(|v| v > 0.0),
                "vanish" | "specVanish" => p.hidden = Some(on(e)),
                "cs" => p.complex = Some(on(e)),
                "rtl" => p.rtl = Some(on(e)),
                _ => {}
            }
        }
        p
    }

    pub fn apply(&self, props: &mut RunProps, theme: &Theme) {
        let name = |font: &Option<FontName>| -> Option<Arc<str>> {
            match font.as_ref()? {
                FontName::Named(n) => Some(n.clone()),
                FontName::Theme(t) => theme.font(*t).map(Arc::from),
            }
        };
        let fonts = &mut props.fonts;
        for (slot, value) in [
            (&mut fonts.ascii, name(&self.ascii)),
            (&mut fonts.h_ansi, name(&self.h_ansi)),
            (&mut fonts.east_asia, name(&self.east_asia)),
            (&mut fonts.complex, name(&self.complex_font)),
        ] {
            if let Some(value) = value {
                *slot = value;
            }
        }
        macro_rules! set {
            ($($field:ident),* $(,)?) => {
                $(if let Some(v) = self.$field { props.$field = v; })*
            };
        }
        set!(
            size,
            size_complex,
            bold,
            italic,
            bold_complex,
            italic_complex,
            underline,
            caps,
            small_caps,
            vertical,
            position,
            color,
            highlight,
            shading,
            spacing,
            scale,
            kerning,
            hidden,
            complex,
            rtl,
            east_asian_hint,
        );
        match (self.strike, self.double_strike) {
            (_, Some(true)) => props.strike = Strike::Double,
            (Some(true), _) => props.strike = Strike::Single,
            (Some(false), _) | (_, Some(false)) => props.strike = Strike::None,
            _ => {}
        }
    }
}

/// A tab stop as a style lists it; `None` for the kind clears the inherited
/// stop at that position.
#[derive(Clone, Copy)]
pub(crate) struct TabPatch {
    pub position: f64,
    pub kind: Option<TabKind>,
    pub leader: Leader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineRule {
    Auto,
    Exact,
    AtLeast,
}

#[derive(Clone, Default)]
pub(crate) struct ParaPatch {
    pub style: Option<String>,
    pub alignment: Option<Alignment>,
    pub left: Option<f64>,
    pub right: Option<f64>,
    pub first_line: Option<f64>,
    pub before: Option<f64>,
    pub after: Option<f64>,
    pub before_auto: Option<bool>,
    pub after_auto: Option<bool>,
    pub line: Option<f64>,
    pub line_rule: Option<LineRule>,
    pub contextual: Option<bool>,
    pub keep_next: Option<bool>,
    pub keep_lines: Option<bool>,
    pub widow_control: Option<bool>,
    pub page_break_before: Option<bool>,
    pub tabs: Vec<TabPatch>,
    pub borders: BordersPatch,
    pub shading: Option<Option<Color>>,
    pub rtl: Option<bool>,
    pub off_grid: Option<bool>,
    pub suppress_line_numbers: Option<bool>,
    pub num_id: Option<String>,
    pub level: Option<u8>,
}

/// Space an automatic spacing stands for.
const AUTO_SPACING: f64 = 14.0;

impl ParaPatch {
    pub fn read(ppr: &Element) -> Self {
        let mut p = Self::default();
        for e in ppr.elements() {
            match &*e.name {
                "pStyle" => p.style = e.val().map(str::to_string),
                "jc" => {
                    p.alignment = e.val().map(|v| match v {
                        "center" => Alignment::Center,
                        "right" | "end" => Alignment::Right,
                        "both" | "thaiDistribute" | "lowKashida" | "mediumKashida"
                        | "highKashida" => Alignment::Justify,
                        "distribute" => Alignment::Distribute,
                        _ => Alignment::Left,
                    })
                }
                "ind" => {
                    if let Some(v) = e.w("left").or(e.w("start")).and_then(twips) {
                        p.left = Some(v);
                    }
                    if let Some(v) = e.w("right").or(e.w("end")).and_then(twips) {
                        p.right = Some(v);
                    }
                    if let Some(v) = e.w("hanging").and_then(twips) {
                        p.first_line = Some(-v);
                    } else if let Some(v) = e.w("firstLine").and_then(twips) {
                        p.first_line = Some(v);
                    }
                }
                "spacing" => {
                    p.before = e.w("before").and_then(twips).or(p.before);
                    p.after = e.w("after").and_then(twips).or(p.after);
                    if let Some(v) = e.w("beforeAutospacing") {
                        p.before_auto = Some(!matches!(v, "0" | "false" | "off"));
                    }
                    if let Some(v) = e.w("afterAutospacing") {
                        p.after_auto = Some(!matches!(v, "0" | "false" | "off"));
                    }
                    if let Some(v) = e.w("line").and_then(|v| v.parse::<f64>().ok()) {
                        p.line = Some(v.round());
                        p.line_rule = Some(match e.w("lineRule") {
                            Some("exact") => LineRule::Exact,
                            Some("atLeast") => LineRule::AtLeast,
                            _ => LineRule::Auto,
                        });
                    }
                }
                "contextualSpacing" => p.contextual = Some(on(e)),
                "keepNext" => p.keep_next = Some(on(e)),
                "keepLines" => p.keep_lines = Some(on(e)),
                "widowControl" => p.widow_control = Some(on(e)),
                "pageBreakBefore" => p.page_break_before = Some(on(e)),
                "snapToGrid" => p.off_grid = Some(!on(e)),
                "suppressLineNumbers" => p.suppress_line_numbers = Some(on(e)),
                "tabs" => {
                    for tab in e.elements() {
                        let Some(position) = tab.w("pos").and_then(twips) else {
                            continue;
                        };
                        let kind = match tab.val() {
                            Some("clear") => None,
                            Some("center") => Some(TabKind::Center),
                            Some("right" | "end") => Some(TabKind::Right),
                            Some("decimal") => Some(TabKind::Decimal),
                            Some("bar") => Some(TabKind::Bar),
                            _ => Some(TabKind::Left),
                        };
                        let leader = match tab.w("leader") {
                            Some("dot") => Leader::Dot,
                            Some("hyphen") => Leader::Hyphen,
                            Some("underscore" | "heavy") => Leader::Underscore,
                            Some("middleDot") => Leader::MiddleDot,
                            _ => Leader::None,
                        };
                        p.tabs.push(TabPatch {
                            position,
                            kind,
                            leader,
                        });
                    }
                }
                "pBdr" => p.borders = BordersPatch::read(e),
                "shd" => p.shading = Some(shading(e)),
                "bidi" => p.rtl = Some(on(e)),
                "numPr" => {
                    if let Some(id) = e.wc("numId").and_then(|n| n.val()) {
                        p.num_id = Some(id.to_string());
                    }
                    if let Some(level) = e.wc("ilvl").and_then(|n| n.val()?.parse().ok()) {
                        p.level = Some(level);
                    }
                }
                _ => {}
            }
        }
        p
    }

    pub fn apply(&self, props: &mut ParagraphProps) {
        if let Some(v) = self.alignment {
            props.alignment = v;
        }
        let indent = &mut props.indent;
        for (patch, target) in [
            (self.left, &mut indent.left),
            (self.right, &mut indent.right),
            (self.first_line, &mut indent.first_line),
        ] {
            if let Some(v) = patch {
                *target = v;
            }
        }
        let spacing = &mut props.spacing;
        if let Some(v) = self.before {
            spacing.before = v;
            spacing.before_auto = false;
        }
        if let Some(v) = self.after {
            spacing.after = v;
            spacing.after_auto = false;
        }
        if self.before_auto == Some(true) {
            spacing.before = AUTO_SPACING;
            spacing.before_auto = true;
        }
        if self.after_auto == Some(true) {
            spacing.after = AUTO_SPACING;
            spacing.after_auto = true;
        }
        if let (Some(line), Some(rule)) = (self.line, self.line_rule) {
            spacing.line = match rule {
                // LibreOffice keeps the nearest whole percentage: 259 sets
                // 11 pt Carlito lines 14.5 pt apart, as 108 % does.
                LineRule::Auto => LineSpacing::Proportional((line * 100.0 / 240.0).round()),
                LineRule::Exact => LineSpacing::Exact(line.abs() / 20.0),
                LineRule::AtLeast => LineSpacing::AtLeast(line / 20.0),
            };
        }
        for (patch, target) in [
            (self.contextual, &mut props.contextual_spacing),
            (self.keep_next, &mut props.keep_next),
            (self.keep_lines, &mut props.keep_lines),
            (self.widow_control, &mut props.widow_control),
            (self.page_break_before, &mut props.page_break_before),
            (self.rtl, &mut props.rtl),
            (self.off_grid, &mut props.off_grid),
            (self.suppress_line_numbers, &mut props.suppress_line_numbers),
        ] {
            if let Some(v) = patch {
                *target = v;
            }
        }
        for tab in &self.tabs {
            props
                .tabs
                .retain(|t| (t.position - tab.position).abs() > 0.01);
            if let Some(kind) = tab.kind {
                props.tabs.push(TabStop {
                    position: tab.position,
                    kind,
                    leader: tab.leader,
                });
            }
        }
        props.tabs.sort_by(|a, b| a.position.total_cmp(&b.position));
        self.borders.apply(&mut props.borders);
        if let Some(v) = self.shading {
            props.shading = v;
        }
    }
}
