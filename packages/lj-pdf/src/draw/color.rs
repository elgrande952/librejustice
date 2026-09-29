//! Colour spaces and their conversion to RGB, without colour management.

use std::rc::Rc;
use std::sync::Arc;

use lj_draw::Rgb;

use super::function::{Function, clip, corrupt, eval_list, number, numbers};
use super::pattern::Pattern;
use crate::objects::{
    ObjectRef, PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfValue, decode_stream,
};

const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug)]
pub(crate) enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    /// The a* and b* ranges.
    Lab([f64; 4]),
    Indexed {
        base: Box<ColorSpace>,
        hival: usize,
        lookup: Vec<u8>,
    },
    /// Separation, and DeviceN with its number of colourants.
    Separation {
        components: usize,
        alternate: Box<ColorSpace>,
        tint: Vec<Function>,
        /// Every colourant is `None`: the space paints nothing.
        none: bool,
    },
    Pattern(Option<Box<ColorSpace>>),
}

fn resolve<'a>(file: &'a PdfFile, value: &'a PdfValue) -> PdfResult<&'a PdfValue> {
    match value {
        PdfValue::Reference(object_ref) => match file.get_object(*object_ref)? {
            PdfObject::Value(value) => Ok(value),
            PdfObject::Stream(_) => Err(corrupt("colour space is a stream")),
        },
        value => Ok(value),
    }
}

fn stream_dict<'a>(file: &'a PdfFile, value: &'a PdfValue) -> Option<&'a PdfDictionary> {
    match value {
        PdfValue::Reference(object_ref) => match file.get_object(*object_ref).ok()? {
            PdfObject::Stream(stream) => Some(&stream.dict),
            PdfObject::Value(PdfValue::Dictionary(dict)) => Some(dict),
            _ => None,
        },
        PdfValue::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

fn bytes(file: &PdfFile, value: &PdfValue) -> PdfResult<Vec<u8>> {
    match value {
        PdfValue::Reference(object_ref) => match file.get_object(*object_ref)? {
            PdfObject::Stream(stream) => decode_stream(stream),
            PdfObject::Value(PdfValue::String(s)) => Ok(s.0.clone()),
            _ => Err(corrupt("Indexed lookup is neither a string nor a stream")),
        },
        PdfValue::String(s) => Ok(s.0.clone()),
        _ => Err(corrupt("Indexed lookup is neither a string nor a stream")),
    }
}

fn device(name: &str) -> Option<ColorSpace> {
    match name {
        "DeviceGray" | "G" | "CalGray" => Some(ColorSpace::Gray),
        "DeviceRGB" | "RGB" | "CalRGB" => Some(ColorSpace::Rgb),
        "DeviceCMYK" | "CMYK" => Some(ColorSpace::Cmyk),
        "Pattern" => Some(ColorSpace::Pattern(None)),
        _ => None,
    }
}

impl ColorSpace {
    /// A name that is no device space is looked up in the `ColorSpace` entry
    /// of `resources`.
    pub fn parse(
        file: &PdfFile,
        value: &PdfValue,
        resources: Option<&PdfDictionary>,
    ) -> PdfResult<ColorSpace> {
        Self::parse_at(file, value, resources, 0)
    }

    fn parse_at(
        file: &PdfFile,
        value: &PdfValue,
        resources: Option<&PdfDictionary>,
        depth: usize,
    ) -> PdfResult<ColorSpace> {
        if depth > MAX_DEPTH {
            return Err(corrupt("colour spaces nest too deep"));
        }
        let next = |value: &PdfValue| Self::parse_at(file, value, resources, depth + 1);
        match resolve(file, value)? {
            PdfValue::Name(name) => match device(name) {
                Some(space) => Ok(space),
                None => {
                    let entry = resources
                        .and_then(|r| r.get("ColorSpace"))
                        .and_then(|v| file.resolve_dict(v).ok())
                        .and_then(|d| d.get(name))
                        .ok_or_else(|| corrupt("unknown colour space"))?;
                    Self::parse_at(file, entry, None, depth + 1)
                }
            },
            PdfValue::Array(items) => {
                let family = items
                    .first()
                    .and_then(|item| resolve(file, item).ok())
                    .and_then(PdfValue::as_name)
                    .ok_or_else(|| corrupt("colour space family"))?;
                let arg = |i: usize| {
                    items
                        .get(i)
                        .ok_or_else(|| corrupt("colour space misses an operand"))
                };
                match family {
                    "ICCBased" => {
                        let dict = stream_dict(file, arg(1)?);
                        let n = dict
                            .and_then(|d| d.get("N"))
                            .and_then(|v| number(file, v))
                            .map(|n| n as usize);
                        let alternate = dict
                            .and_then(|d| d.get("Alternate"))
                            .and_then(|v| next(v).ok());
                        match (n, alternate) {
                            (Some(n), Some(alternate)) if alternate.components() == n => {
                                Ok(alternate)
                            }
                            (Some(1), _) => Ok(ColorSpace::Gray),
                            (Some(4), _) => Ok(ColorSpace::Cmyk),
                            _ => Ok(ColorSpace::Rgb),
                        }
                    }
                    "Lab" => {
                        let range = file
                            .resolve_dict(arg(1)?)
                            .ok()
                            .and_then(|dict| numbers(file, dict.get("Range")))
                            .filter(|r| r.len() == 4)
                            .map_or([-100.0, 100.0, -100.0, 100.0], |r| [r[0], r[1], r[2], r[3]]);
                        Ok(ColorSpace::Lab(range))
                    }
                    "Indexed" | "I" => {
                        let base = next(arg(1)?)?;
                        let hival = arg(2)
                            .ok()
                            .and_then(|v| number(file, v))
                            .unwrap_or(0.0)
                            .clamp(0.0, 255.0) as usize;
                        let mut lookup = bytes(file, arg(3)?)?;
                        lookup.resize((hival + 1) * base.components(), 0);
                        Ok(ColorSpace::Indexed {
                            base: Box::new(base),
                            hival,
                            lookup,
                        })
                    }
                    "Separation" | "DeviceN" => {
                        let names: Vec<&str> = match family {
                            "Separation" => vec![resolve(file, arg(1)?)?.as_name().unwrap_or("")],
                            _ => resolve(file, arg(1)?)?
                                .as_array()
                                .ok_or_else(|| corrupt("DeviceN names"))?
                                .iter()
                                .map(|n| n.as_name().unwrap_or(""))
                                .collect(),
                        };
                        if names.is_empty() {
                            return Err(corrupt("DeviceN without a colorant"));
                        }
                        // A process colorant, or all of them, paints as the
                        // process colour itself rather than through its
                        // alternate, as MuPDF shows it.
                        let process = match names.as_slice() {
                            ["All"] => Some((ColorSpace::Gray, vec![1.0], vec![0.0])),
                            [name @ ("Cyan" | "Magenta" | "Yellow" | "Black")] => {
                                let index = ["Cyan", "Magenta", "Yellow", "Black"]
                                    .iter()
                                    .position(|n| n == name)
                                    .expect("a process colorant");
                                let mut full = vec![0.0; 4];
                                full[index] = 1.0;
                                Some((ColorSpace::Cmyk, vec![0.0; 4], full))
                            }
                            _ => None,
                        };
                        let (alternate, tint) = match process {
                            Some((space, c0, c1)) => (space, vec![Function::linear(c0, c1)]),
                            None => (next(arg(2)?)?, Function::parse_list(file, arg(3)?)?),
                        };
                        Ok(ColorSpace::Separation {
                            components: names.len(),
                            alternate: Box::new(alternate),
                            tint,
                            none: names.iter().all(|n| *n == "None"),
                        })
                    }
                    "Pattern" => Ok(ColorSpace::Pattern(match items.get(1) {
                        Some(under) => Some(Box::new(next(under)?)),
                        None => None,
                    })),
                    family => device(family)
                        .ok_or_else(|| PdfError::Unsupported(format!("colour space {family}"))),
                }
            }
            _ => Err(corrupt("colour space is neither a name nor an array")),
        }
    }

    pub fn components(&self) -> usize {
        match self {
            ColorSpace::Gray | ColorSpace::Indexed { .. } => 1,
            ColorSpace::Rgb | ColorSpace::Lab(_) => 3,
            ColorSpace::Cmyk => 4,
            ColorSpace::Separation { components, .. } => *components,
            ColorSpace::Pattern(under) => under.as_ref().map_or(0, |u| u.components()),
        }
    }

    /// The colour `cs` and `CS` start from.
    pub fn initial(&self) -> Vec<f64> {
        match self {
            ColorSpace::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            ColorSpace::Lab([a0, a1, b0, b1]) => {
                vec![0.0, clip(0.0, [*a0, *a1]), clip(0.0, [*b0, *b1])]
            }
            ColorSpace::Separation { components, .. } => vec![1.0; *components],
            ColorSpace::Pattern(_) => Vec::new(),
            space => vec![0.0; space.components()],
        }
    }

    /// The Decode array of an image whose samples take `bits` bits, a pair
    /// per component.
    pub fn default_decode(&self, bits: u8) -> Vec<f64> {
        match self {
            ColorSpace::Indexed { .. } => vec![0.0, f64::from((1u32 << bits) - 1)],
            ColorSpace::Lab([a0, a1, b0, b1]) => vec![0.0, 100.0, *a0, *a1, *b0, *b1],
            space => [0.0, 1.0].repeat(space.components()),
        }
    }

    pub fn paints_nothing(&self) -> bool {
        matches!(self, ColorSpace::Separation { none: true, .. })
    }

    /// Each channel from 0 to 1.
    pub fn to_rgb(&self, components: &[f64]) -> [f64; 3] {
        let c = |i: usize| components.get(i).copied().unwrap_or(0.0);
        let unit = |v: f64| clip(v, [0.0, 1.0]);
        match self {
            ColorSpace::Gray => [unit(c(0)); 3],
            ColorSpace::Rgb => [unit(c(0)), unit(c(1)), unit(c(2))],
            ColorSpace::Cmyk => {
                let k = unit(c(3));
                [0, 1, 2].map(|i| 1.0 - (unit(c(i)) + k).min(1.0))
            }
            ColorSpace::Lab([a0, a1, b0, b1]) => lab_to_srgb(
                clip(c(0), [0.0, 100.0]),
                clip(c(1), [*a0, *a1]),
                clip(c(2), [*b0, *b1]),
            ),
            ColorSpace::Indexed {
                base,
                hival,
                lookup,
            } => {
                let index = clip(c(0).round(), [0.0, *hival as f64]) as usize;
                let n = base.components();
                let decode = base.default_decode(8);
                let values: Vec<f64> = lookup[index * n..index * n + n]
                    .iter()
                    .zip(decode.as_chunks::<2>().0)
                    .map(|(b, d)| d[0] + f64::from(*b) / 255.0 * (d[1] - d[0]))
                    .collect();
                base.to_rgb(&values)
            }
            ColorSpace::Separation {
                alternate, tint, ..
            } => {
                let input: Vec<f64> = (0..self.components()).map(|i| unit(c(i))).collect();
                alternate.to_rgb(&eval_list(tint, &input))
            }
            ColorSpace::Pattern(under) => under.as_ref().map_or([0.0; 3], |u| u.to_rgb(components)),
        }
    }

    pub fn rgb_bytes(&self, components: &[f64]) -> [u8; 3] {
        self.to_rgb(components).map(byte)
    }

    pub fn rgb(&self, components: &[f64]) -> Rgb {
        let [r, g, b] = self.rgb_bytes(components);
        Rgb(r, g, b)
    }
}

pub(crate) fn byte(v: f64) -> u8 {
    (clip(v, [0.0, 1.0]) * 255.0).round() as u8
}

/// Relative to a D50 white, as viewers convert without a profile.
fn lab_to_srgb(l: f64, a: f64, b: f64) -> [f64; 3] {
    let fy = (l + 16.0) / 116.0;
    let g = |t: f64| match t > 6.0 / 29.0 {
        true => t * t * t,
        false => 3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0),
    };
    let (x, y, z) = (
        0.9642 * g(fy + a / 500.0),
        g(fy),
        0.8249 * g(fy - b / 200.0),
    );
    let encode = |linear: f64| {
        let linear = clip(linear, [0.0, 1.0]);
        match linear <= 0.003_130_8 {
            true => 12.92 * linear,
            false => 1.055 * linear.powf(1.0 / 2.4) - 0.055,
        }
    };
    [
        encode(3.133_856 * x - 1.616_867 * y - 0.490_615 * z),
        encode(-0.978_768 * x + 1.916_142 * y + 0.033_454 * z),
        encode(0.071_945 * x - 0.228_991 * y + 1.405_243 * z),
    ]
}

/// A tiling pattern's cell as drawn in one colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Tile {
    pub source: ObjectRef,
    /// The colour of an uncoloured cell.
    pub under: Option<Rgb>,
}

/// A colour as the graphics state holds it.
#[derive(Clone, Debug)]
pub(crate) struct Color {
    pub space: Arc<ColorSpace>,
    pub components: Vec<f64>,
    /// In a Pattern space, the pattern `scn` names, when it could be read.
    pub pattern: Option<Rc<Pattern>>,
}

impl Color {
    pub fn black() -> Color {
        Color::new(Arc::new(ColorSpace::Gray))
    }

    pub fn new(space: Arc<ColorSpace>) -> Color {
        Color {
            components: space.initial(),
            space,
            pattern: None,
        }
    }

    /// The numbers of `sc`, `scn`, `SC`, `SCN` or a device operator; too few
    /// leave the last components as they were.
    pub fn set(&mut self, values: &[f64]) {
        let n = self.space.components();
        let values = &values[values.len().saturating_sub(n)..];
        self.components.resize(n, 0.0);
        self.components[..values.len()].copy_from_slice(values);
    }

    /// The cell a tiling pattern paints with: the pattern, and for an
    /// uncoloured one the colour it takes.
    pub fn tile(&self) -> Option<Tile> {
        let pattern = self.pattern.as_ref()?;
        let Pattern::Tiling {
            source, coloured, ..
        } = **pattern
        else {
            return None;
        };
        let under = match (&*self.space, coloured) {
            (_, true) => None,
            (ColorSpace::Pattern(Some(under)), false) => Some(under.rgb(&self.components)),
            (_, false) => Some(Rgb::BLACK),
        };
        Some(Tile { source, under })
    }

    /// What a text shown as text is painted with: a pattern gives its
    /// dominant colour. `None` paints nothing.
    pub fn rgb(&self) -> Option<Rgb> {
        match &*self.space {
            space if space.paints_nothing() => None,
            ColorSpace::Pattern(under) => match (&self.pattern, under) {
                (Some(pattern), _) => pattern.representative(under.as_deref(), &self.components),
                (None, _) => None,
            },
            space => Some(space.rgb(&self.components)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmyk_converts_by_the_naive_formula() {
        assert_eq!(
            ColorSpace::Cmyk.rgb(&[1.0, 0.0, 0.0, 0.0]),
            Rgb(0, 255, 255)
        );
        assert_eq!(
            ColorSpace::Cmyk.rgb(&[0.0, 0.5, 0.0, 0.5]),
            Rgb(128, 0, 128)
        );
        assert_eq!(
            ColorSpace::Cmyk.rgb(&ColorSpace::Cmyk.initial()),
            Rgb::BLACK
        );
    }

    #[test]
    fn lab_white_and_black_are_white_and_black() {
        let lab = ColorSpace::Lab([-100.0, 100.0, -100.0, 100.0]);
        assert_eq!(lab.rgb(&[100.0, 0.0, 0.0]), Rgb::WHITE);
        assert_eq!(lab.rgb(&[0.0, 0.0, 0.0]), Rgb::BLACK);
    }

    #[test]
    fn indexed_reads_its_lookup_in_the_base_space() {
        let space = ColorSpace::Indexed {
            base: Box::new(ColorSpace::Rgb),
            hival: 1,
            lookup: vec![255, 0, 0, 0, 0, 255],
        };
        assert_eq!(space.rgb(&[0.0]), Rgb(255, 0, 0));
        assert_eq!(space.rgb(&[1.0]), Rgb(0, 0, 255));
        assert_eq!(space.rgb(&[7.0]), Rgb(0, 0, 255));
    }
}
