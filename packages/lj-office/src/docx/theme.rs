//! The theme part: the fonts `minorHAnsi` and its siblings stand for.

use anyhow::Result;

use super::xml::{parse, Ns};
use crate::model::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThemeFont {
    MajorAscii,
    MajorHAnsi,
    MajorEastAsia,
    MajorBidi,
    MinorAscii,
    MinorHAnsi,
    MinorEastAsia,
    MinorBidi,
}

impl ThemeFont {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "majorAscii" => Self::MajorAscii,
            "majorHAnsi" => Self::MajorHAnsi,
            "majorEastAsia" => Self::MajorEastAsia,
            "majorBidi" => Self::MajorBidi,
            "minorAscii" => Self::MinorAscii,
            "minorHAnsi" => Self::MinorHAnsi,
            "minorEastAsia" => Self::MinorEastAsia,
            "minorBidi" => Self::MinorBidi,
            _ => return None,
        })
    }
}

#[derive(Default)]
struct Scheme {
    latin: String,
    east_asia: String,
    complex: String,
}

#[derive(Default)]
pub(crate) struct Theme {
    major: Scheme,
    minor: Scheme,
    /// The colour scheme, by slot name: `dk1`, `accent1`, `hlink`...
    colors: Vec<(String, Color)>,
}

impl Theme {
    pub fn read(part: &[u8]) -> Result<Self> {
        let root = parse(part)?;
        let mut theme = Theme::default();
        let elements = root.child(Ns::A, "themeElements");
        if let Some(scheme) = elements.and_then(|e| e.child(Ns::A, "clrScheme")) {
            theme.colors = scheme
                .elements()
                .filter_map(|slot| {
                    let value = slot.elements().find_map(|c| match &*c.name {
                        "srgbClr" => c.attr(Ns::None, "val"),
                        "sysClr" => c.attr(Ns::None, "lastClr"),
                        _ => None,
                    })?;
                    Some((slot.name.to_string(), Color::hex(value)?))
                })
                .collect();
        }
        let Some(fonts) = elements.and_then(|e| e.child(Ns::A, "fontScheme")) else {
            return Ok(theme);
        };
        for (name, scheme) in [
            ("majorFont", &mut theme.major),
            ("minorFont", &mut theme.minor),
        ] {
            let Some(font) = fonts.child(Ns::A, name) else {
                continue;
            };
            let typeface = |tag: &str| {
                font.child(Ns::A, tag)
                    .and_then(|e| e.attr(Ns::None, "typeface"))
                    .unwrap_or_default()
                    .to_string()
            };
            scheme.latin = typeface("latin");
            scheme.east_asia = typeface("ea");
            scheme.complex = typeface("cs");
        }
        Ok(theme)
    }

    /// A slot of the colour scheme, the text and background aliases mapped
    /// as Word maps them by default.
    pub fn color(&self, name: &str) -> Option<Color> {
        let slot = match name {
            "tx1" => "dk1",
            "bg1" => "lt1",
            "tx2" => "dk2",
            "bg2" => "lt2",
            other => other,
        };
        self.colors.iter().find(|(n, _)| n == slot).map(|(_, c)| *c)
    }

    /// The family a theme font names, `None` when the theme leaves it empty.
    pub fn font(&self, font: ThemeFont) -> Option<&str> {
        use ThemeFont::*;
        let name = match font {
            MajorAscii | MajorHAnsi => &self.major.latin,
            MajorEastAsia => &self.major.east_asia,
            MajorBidi => &self.major.complex,
            MinorAscii | MinorHAnsi => &self.minor.latin,
            MinorEastAsia => &self.minor.east_asia,
            MinorBidi => &self.minor.complex,
        };
        (!name.is_empty()).then_some(name.as_str())
    }
}
