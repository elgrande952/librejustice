//! ODF attribute values: lengths, percentages, colours.

use super::model::Color;

/// A length in points. ODF writes `cm`, `mm`, `in`, `pt`, `pc`, `px`.
pub fn length(value: &str) -> Option<f32> {
    let v = value.trim();
    let split = v
        .find(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')
        })
        .unwrap_or(v.len());
    let (number, unit) = v.split_at(split);
    let number: f32 = number.parse().ok()?;
    let factor = match unit.trim() {
        "pt" | "" => 1.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "in" | "inch" => 72.0,
        "pc" => 12.0,
        "px" => 0.75,
        "em" => 12.0,
        _ => return None,
    };
    let points = number * factor;
    points.is_finite().then_some(points)
}

pub fn percent(value: &str) -> Option<f32> {
    let v = value.trim().strip_suffix('%')?;
    v.trim().parse::<f32>().ok().filter(|p| p.is_finite())
}

pub enum Measure {
    Length(f32),
    Percent(f32),
}

pub fn measure(value: &str) -> Option<Measure> {
    match percent(value) {
        Some(p) => Some(Measure::Percent(p)),
        None => length(value).map(Measure::Length),
    }
}

/// `#rrggbb`; `transparent` and anything else read as no colour.
pub fn color(value: &str) -> Option<Color> {
    let hex = value.trim().strip_prefix('#')?;
    match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok().map(Color),
        3 => {
            let v = u32::from_str_radix(hex, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(Color((r * 17) << 16 | (g * 17) << 8 | (b * 17)))
        }
        _ => None,
    }
}

/// A font family as `svg:font-family` or `fo:font-family` write it: a list
/// whose first entry wins, quoted when it has spaces.
pub fn family(value: &str) -> String {
    let first = value.split(',').next().unwrap_or(value).trim();
    first
        .trim_matches(|c| c == '\'' || c == '"')
        .trim()
        .to_string()
}

pub fn boolean(value: &str) -> bool {
    value.trim() == "true"
}
