//! The little CSS a Word web page or a browser export carries: declarations,
//! rules on a tag or a class, `@page`, lengths and colours.

use crate::model::Color;

pub(crate) type Declarations = Vec<(String, String)>;

/// `name: value; ...`, names lowercased, `!important` dropped.
pub(crate) fn declarations(block: &str) -> Declarations {
    block
        .split(';')
        .filter_map(|d| {
            let (name, value) = d.split_once(':')?;
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            let value = value
                .strip_suffix("!important")
                .map_or(value, str::trim_end)
                .to_string();
            (!name.is_empty() && !value.is_empty()).then_some((name, value))
        })
        .collect()
}

/// A rule on a tag, a class or both.
struct Rule {
    tag: Option<String>,
    class: Option<String>,
    order: usize,
    declarations: Declarations,
}

#[derive(Default)]
pub(crate) struct Sheet {
    rules: Vec<Rule>,
    /// The declarations of the first `@page` rule.
    pub page: Declarations,
}

impl Sheet {
    pub fn parse(css: &str) -> Sheet {
        let mut clean = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(at) = rest.find("/*") {
            clean.push_str(&rest[..at]);
            rest = rest[at + 2..]
                .find("*/")
                .map_or("", |end| &rest[at + 2 + end + 2..]);
        }
        clean.push_str(rest);
        let clean = clean.replace("<!--", " ").replace("-->", " ");
        let mut sheet = Sheet::default();
        let mut rest = clean.as_str();
        while let Some(open) = rest.find('{') {
            let prelude = rest[..open].trim();
            let body_start = open + 1;
            let mut depth = 1;
            let mut end = body_start;
            for (i, c) in rest[body_start..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
                end = rest.len();
            }
            let body = &rest[body_start..end.min(rest.len())];
            if prelude.starts_with("@page") {
                if sheet.page.is_empty() {
                    sheet.page = declarations(body);
                }
            } else if !prelude.starts_with('@') {
                let declarations = declarations(body);
                for selector in prelude.split(',') {
                    if let Some((tag, class)) = simple(selector.trim()) {
                        sheet.rules.push(Rule {
                            tag,
                            class,
                            order: sheet.rules.len(),
                            declarations: declarations.clone(),
                        });
                    }
                }
            }
            rest = rest.get(end + 1..).unwrap_or("");
        }
        sheet
    }

    /// The declarations the rules give an element, least specific first.
    pub fn matching(&self, tag: &str, classes: &[&str]) -> Vec<&(String, String)> {
        let mut rules: Vec<(u8, usize, &Rule)> = self
            .rules
            .iter()
            .filter(|r| r.tag.as_deref().is_none_or(|t| t == tag))
            .filter(|r| {
                r.class
                    .as_deref()
                    .is_none_or(|c| classes.iter().any(|k| k.eq_ignore_ascii_case(c)))
            })
            .map(|r| {
                let specificity = u8::from(r.tag.is_some()) + 2 * u8::from(r.class.is_some());
                (specificity, r.order, r)
            })
            .collect();
        rules.sort_by_key(|(s, o, _)| (*s, *o));
        rules
            .into_iter()
            .flat_map(|(_, _, r)| r.declarations.iter())
            .collect()
    }
}

/// `p`, `.MsoNormal` or `p.MsoNormal`; anything else matches nothing here.
fn simple(selector: &str) -> Option<(Option<String>, Option<String>)> {
    if selector.is_empty()
        || !selector
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*'))
    {
        return None;
    }
    let (tag, class) = match selector.split_once('.') {
        Some((tag, class)) if !class.contains('.') => (tag, Some(class)),
        Some(_) => return None,
        None => (selector, None),
    };
    let tag = match tag {
        "" | "*" => None,
        tag => Some(tag.to_ascii_lowercase()),
    };
    Some((tag, class.map(str::to_string)))
}

/// A length in points; `em` against `font`, `%` against `whole`.
pub(crate) fn length(value: &str, font: f64, whole: f64) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    let split = v
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))
        .unwrap_or(v.len());
    let number: f64 = v[..split].parse().ok()?;
    let points = match v[split..].trim() {
        "pt" => number,
        "px" | "" => number * 0.75,
        "pc" => number * 12.0,
        "in" => number * 72.0,
        "cm" => number * 72.0 / 2.54,
        "mm" => number * 72.0 / 25.4,
        "em" => number * font,
        "ex" => number * font / 2.0,
        "rem" => number * 12.0,
        "%" => number * whole / 100.0,
        _ => return None,
    };
    points.is_finite().then_some(points)
}

/// A font size in points, `parent` being the size it inherits.
pub(crate) fn font_size(value: &str, parent: f64) -> Option<f64> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "xx-small" => 6.75,
        "x-small" => 7.5,
        "small" => 9.75,
        "medium" => 12.0,
        "large" => 13.5,
        "x-large" => 18.0,
        "xx-large" => 24.0,
        "smaller" => parent / 1.2,
        "larger" => parent * 1.2,
        v => length(v, parent, parent).filter(|s| *s > 0.0)?,
    })
}

/// The size `<font size>` names, 3 being the default 12 pt.
pub(crate) fn font_tag_size(value: &str) -> Option<f64> {
    const SIZES: [f64; 7] = [7.5, 10.0, 12.0, 13.5, 18.0, 24.0, 36.0];
    let v = value.trim();
    let n: i32 = v.trim_start_matches('+').parse().ok()?;
    let index = match v.starts_with(['+', '-']) {
        true => 3 + n,
        false => n,
    };
    Some(SIZES[(index.clamp(1, 7) - 1) as usize])
}

pub(crate) enum Paint {
    Color(Color),
    /// No colour of its own: what is behind or the automatic one shows.
    None,
    Inherit,
}

pub(crate) fn color(value: &str) -> Option<Paint> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let d: Vec<u8> = hex
                    .chars()
                    .map(|c| c.to_digit(16).map(|d| d as u8 * 17))
                    .collect::<Option<_>>()?;
                Some(Paint::Color(Color(d[0], d[1], d[2])))
            }
            6 => Color::hex(hex).map(Paint::Color),
            8 => Color::hex(&hex[..6]).map(Paint::Color),
            _ => None,
        };
    }
    if let Some(args) = v
        .strip_prefix("rgba(")
        .or_else(|| v.strip_prefix("rgb("))
        .and_then(|a| a.strip_suffix(')'))
    {
        let channels: Vec<u8> = args
            .split([',', ' ', '/'])
            .filter(|s| !s.is_empty())
            .take(3)
            .map(|s| match s.strip_suffix('%') {
                Some(p) => p.parse::<f64>().ok().map(|p| (p * 2.55).round() as u8),
                None => s.parse::<f64>().ok().map(|c| c.clamp(0.0, 255.0) as u8),
            })
            .collect::<Option<_>>()?;
        return (channels.len() == 3)
            .then(|| Paint::Color(Color(channels[0], channels[1], channels[2])));
    }
    Some(Paint::Color(match v.as_str() {
        "transparent" | "none" | "auto" | "initial" | "windowtext" => return Some(Paint::None),
        "inherit" | "currentcolor" => return Some(Paint::Inherit),
        "black" => Color(0, 0, 0),
        "white" | "window" => Color(255, 255, 255),
        "red" => Color(255, 0, 0),
        "lime" => Color(0, 255, 0),
        "green" => Color(0, 128, 0),
        "blue" => Color(0, 0, 255),
        "yellow" => Color(255, 255, 0),
        "aqua" | "cyan" => Color(0, 255, 255),
        "fuchsia" | "magenta" => Color(255, 0, 255),
        "gray" | "grey" => Color(128, 128, 128),
        "silver" => Color(192, 192, 192),
        "maroon" => Color(128, 0, 0),
        "olive" => Color(128, 128, 0),
        "navy" => Color(0, 0, 128),
        "purple" => Color(128, 0, 128),
        "teal" => Color(0, 128, 128),
        "orange" => Color(255, 165, 0),
        _ => return None,
    }))
}

/// The words of a value, split at white space outside parentheses.
pub(crate) fn words(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, None);
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&value[s..i]);
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(i);
    }
    if let Some(s) = start {
        out.push(&value[s..]);
    }
    out
}

/// The one to four values of a box shorthand, as top, right, bottom, left.
pub(crate) fn sides(value: &str) -> Option<[&str; 4]> {
    let parts = words(value);
    Some(match parts[..] {
        [a] => [a, a, a, a],
        [a, b] => [a, b, a, b],
        [a, b, c] => [a, b, c, b],
        [a, b, c, d] => [a, b, c, d],
        _ => return None,
    })
}
