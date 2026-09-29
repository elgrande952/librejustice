//! Pictures, shapes and text boxes, set in a line or floating beside the text.

use std::sync::Arc;

use super::{Block, Chart, Color, Margins, RunProps, VAlign};

pub struct Drawing {
    /// The frame, before any rotation.
    pub width: f64,
    pub height: f64,
    /// `None` for an object set in the line like a character.
    pub float: Option<Box<Float>>,
    pub shape: Shape,
    pub transform: Transform,
    pub props: RunProps,
}

impl Drawing {
    /// Paragraphs positioned in a frame, a floating text frame as LibreOffice
    /// converts them: `width` `None` narrows it to its text, and it grows past
    /// `height` unless `exact`.
    pub(crate) fn paragraph_frame(
        float: Float,
        width: Option<f64>,
        height: f64,
        exact: bool,
        blocks: Vec<Block>,
    ) -> Self {
        Drawing {
            width: width.unwrap_or(0.0),
            height,
            float: Some(Box::new(float)),
            shape: Shape::Figure(Box::new(Figure {
                geometry: Geometry::Rect,
                fill: None,
                stroke: None,
                text: Some(TextBox {
                    blocks,
                    inset: Margins::default(),
                    valign: VAlign::Top,
                    grow: !exact,
                    fit: width.is_none(),
                }),
            })),
            transform: Transform::default(),
            props: RunProps::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Float {
    pub horizontal: Position,
    pub vertical: Position,
    pub wrap: Wrap,
    pub side: WrapSide,
    /// Painted under the text.
    pub behind: bool,
    /// Distance the text keeps from the frame.
    pub distance: Margins,
    /// Painting order among floating objects, lowest first.
    pub z: i64,
    /// The outline the text runs along, as shares of the frame's width and
    /// height; empty for the frame itself.
    pub contour: Vec<[f64; 2]>,
    /// Placed against its table cell when anchored in one; otherwise the
    /// cell is no bound, and a place fixed on the page stays there.
    pub in_cell: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub from: Relative,
    pub place: Place,
}

/// What a position is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relative {
    Page,
    Margin,
    Column,
    Paragraph,
    Line,
    Character,
    LeftMargin,
    RightMargin,
    TopMargin,
    BottomMargin,
    InsideMargin,
    OutsideMargin,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    Offset(f64),
    /// Left or top.
    Start,
    Center,
    /// Right or bottom.
    End,
    Inside,
    Outside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    None,
    TopAndBottom,
    /// The text runs beside the frame.
    Around,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapSide {
    Both,
    Left,
    Right,
    Largest,
}

/// A clockwise rotation in degrees about the frame's centre, then mirrors.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Transform {
    pub rotation: f64,
    pub flip_h: bool,
    pub flip_v: bool,
}

pub enum Shape {
    Picture(Picture),
    Figure(Box<Figure>),
    Group(Vec<Child>),
    Chart(Box<Chart>),
    /// A line of text stretched over the frame, as a watermark.
    WordArt(Box<WordArt>),
    /// An embedded object without a picture: only its frame.
    Opaque,
}

pub struct WordArt {
    pub text: String,
    /// The font, and the colour it is filled with.
    pub props: RunProps,
    /// 1 is opaque.
    pub alpha: f64,
}

impl Shape {
    /// The text boxes' blocks, a group's in the order of its children.
    pub fn stories(&self, visit: &mut dyn FnMut(&[Block])) {
        match self {
            Shape::Figure(figure) => {
                if let Some(text) = &figure.text {
                    visit(&text.blocks);
                }
            }
            Shape::Group(children) => {
                for child in children {
                    child.shape.stories(visit);
                }
            }
            Shape::Picture(_) | Shape::Chart(_) | Shape::WordArt(_) | Shape::Opaque => {}
        }
    }
}

impl Shape {
    pub fn first_story(&self) -> Option<&[Block]> {
        match self {
            Shape::Figure(figure) => figure.text.as_ref().map(|t| &t.blocks[..]),
            Shape::Group(children) => children.iter().find_map(|c| c.shape.first_story()),
            Shape::Picture(_) | Shape::Chart(_) | Shape::WordArt(_) | Shape::Opaque => None,
        }
    }
}

pub struct Child {
    /// The child's frame within the group's.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub shape: Shape,
    pub transform: Transform,
}

pub struct Picture {
    pub data: Arc<[u8]>,
    pub format: ImageFormat,
    /// Share of the image cut off on each side.
    pub crop: Margins,
    pub outline: Option<Stroke>,
    pub recolor: Recolor,
}

/// A picture's colours remapped as LibreOffice draws them: its mode first,
/// then a luminance and a contrast in percent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Recolor {
    pub mode: ColorMode,
    pub luminance: f64,
    pub contrast: f64,
    /// Office's order, the luminance before the contrast, which LibreOffice
    /// bakes into the picture when both are set.
    pub office: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    Standard,
    Greys,
    /// Black or white.
    Mono,
    /// Washed out, lighter and flatter.
    Watermark,
}

impl Recolor {
    /// The slope and intercept each channel, in `0..1`, is mapped by.
    pub fn linear(self) -> (f64, f64) {
        let (mut luminance, mut contrast) = (self.luminance, self.contrast);
        if self.mode == ColorMode::Watermark {
            luminance += 50.0;
            contrast -= 70.0;
        }
        let (luminance, contrast) = (
            luminance.clamp(-100.0, 100.0),
            contrast.clamp(-100.0, 100.0),
        );
        let slope = if contrast >= 0.0 {
            128.0 / (128.0 - 1.27 * contrast)
        } else {
            (128.0 + 1.27 * contrast) / 128.0
        };
        let offset = if self.office {
            (luminance * 2.55 - 128.0) * slope + 128.0
        } else {
            luminance * 2.55 + 128.0 - slope * 128.0
        };
        (slope, offset / 255.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
    Svg,
    /// StarView, LibreOffice's metafile.
    Svm,
    Wmf,
    Emf,
    Tiff,
    /// PostScript, drawn by the preview it may carry.
    Eps,
    /// QuickDraw, the Mac metafile.
    Pict,
    Other,
}

impl ImageFormat {
    pub fn sniff(data: &[u8]) -> Self {
        let head = |magic: &[u8]| data.starts_with(magic);
        if head(b"\x89PNG") {
            Self::Png
        } else if head(b"\xFF\xD8") {
            Self::Jpeg
        } else if head(b"GIF8") {
            Self::Gif
        } else if head(b"BM") {
            Self::Bmp
        } else if head(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
            Self::Webp
        } else if head(b"<?xml") || head(b"<svg") {
            Self::Svg
        } else if head(b"VCLMTF") {
            Self::Svm
        } else if head(b"\xD7\xCD\xC6\x9A")
            || head(b"\x01\x00\x09\x00")
            || head(b"\x02\x00\x09\x00")
        {
            Self::Wmf
        } else if head(b"\x01\x00\x00\x00") && data.get(40..44) == Some(b" EMF") {
            Self::Emf
        } else if head(b"II*\0") || head(b"MM\0*") {
            Self::Tiff
        } else if head(b"\xC5\xD0\xD3\xC6") || head(b"%!PS") {
            Self::Eps
        } else if crate::metafile::is_pict(data) {
            Self::Pict
        } else {
            Self::Other
        }
    }

    /// The size in points a bitmap shows at by its own resolution, 96 dpi
    /// when it gives none.
    pub fn natural_size(self, data: &[u8]) -> Option<[f64; 2]> {
        let u16be = |i: usize| Some(u16::from_be_bytes(data.get(i..i + 2)?.try_into().ok()?));
        let u32be = |i: usize| Some(u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?));
        let u16le = |i: usize| Some(u16::from_le_bytes(data.get(i..i + 2)?.try_into().ok()?));
        let i32le = |i: usize| Some(i32::from_le_bytes(data.get(i..i + 4)?.try_into().ok()?));
        let per_metre = |v: f64| (v > 0.0).then_some(v * 0.0254);
        let (width, height, dpi) = match self {
            Self::Png => {
                let (w, h) = (u32be(16)?, u32be(20)?);
                let mut dpi = None;
                let mut at = 8;
                while let (Some(len), Some(kind)) = (u32be(at), data.get(at + 4..at + 8)) {
                    if kind == b"pHYs" && data.get(at + 16) == Some(&1) {
                        dpi = per_metre(f64::from(u32be(at + 8)?));
                    }
                    if kind == b"IDAT" {
                        break;
                    }
                    let Some(next) = (len as usize)
                        .checked_add(12)
                        .and_then(|n| n.checked_add(at))
                    else {
                        break;
                    };
                    at = next;
                }
                (f64::from(w), f64::from(h), dpi)
            }
            Self::Jpeg => {
                let mut dpi = None;
                let mut at = 2;
                loop {
                    if *data.get(at)? != 0xFF {
                        return None;
                    }
                    let marker = *data.get(at + 1)?;
                    let len = usize::from(u16be(at + 2)?);
                    if marker == 0xE0 && data.get(at + 4..at + 9) == Some(b"JFIF\0") {
                        let (unit, x) = (*data.get(at + 11)?, f64::from(u16be(at + 12)?));
                        dpi = match unit {
                            1 if x > 0.0 => Some(x),
                            2 if x > 0.0 => Some(x * 2.54),
                            _ => None,
                        };
                    }
                    if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                        let (h, w) = (u16be(at + 5)?, u16be(at + 7)?);
                        break (f64::from(w), f64::from(h), dpi);
                    }
                    at += 2 + len;
                }
            }
            Self::Gif => (f64::from(u16le(6)?), f64::from(u16le(8)?), None),
            Self::Bmp => (
                f64::from(i32le(18)?.unsigned_abs()),
                f64::from(i32le(22)?.unsigned_abs()),
                per_metre(f64::from(i32le(38)?)),
            ),
            _ => return None,
        };
        let dpi = dpi.unwrap_or(96.0);
        (width > 0.0 && height > 0.0).then(|| [width * 72.0 / dpi, height * 72.0 / dpi])
    }

    pub fn mime(self) -> Option<&'static str> {
        Some(match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Webp => "image/webp",
            Self::Svg => "image/svg+xml",
            Self::Svm
            | Self::Wmf
            | Self::Emf
            | Self::Tiff
            | Self::Eps
            | Self::Pict
            | Self::Other => return None,
        })
    }
}

pub struct Figure {
    pub geometry: Geometry,
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
    pub text: Option<TextBox>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    Rect,
    /// Corners rounded by this share of the shorter side.
    RoundRect(f64),
    Ellipse,
    /// From the frame's top left corner to its bottom right one.
    Line,
    /// Outlines in the frame's unit square, painted in order.
    Path(Arc<[SubPath]>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubPath {
    pub segments: Vec<lj_draw::Segment>,
    pub fill: bool,
    pub stroke: bool,
    /// The fill darkened toward black, below 0, or lightened toward white.
    pub shade: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub color: Color,
    /// 1 is opaque.
    pub alpha: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid(Paint),
    Gradient(Arc<Gradient>),
    Hatch(Arc<Hatch>),
    Image(Arc<ImageFill>),
}

/// Parallel lines `distance` apart, turned `angle` degrees counter-clockwise
/// from horizontal, crossed by a second set at right angles when `lines` is 2
/// and a third at 45 degrees when 3.
#[derive(Clone, Debug, PartialEq)]
pub struct Hatch {
    pub color: Color,
    pub distance: f64,
    pub angle: f64,
    pub lines: u8,
    /// Painted under the lines.
    pub back: Option<Paint>,
    /// 1 is opaque.
    pub alpha: f64,
}

/// A picture stretched over the frame, or repeated from its top left corner
/// at `tile` points wide and high.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageFill {
    pub data: Arc<[u8]>,
    pub format: ImageFormat,
    pub tile: Option<[f64; 2]>,
    /// Interpolated when enlarged, rather than drawn as blocks.
    pub smooth: bool,
    /// 1 is opaque.
    pub alpha: f64,
}

impl Fill {
    /// The one colour that stands for the fill, the mean of a gradient's.
    pub fn mean(&self) -> Paint {
        match self {
            Fill::Solid(paint) => *paint,
            Fill::Hatch(h) => h.back.unwrap_or(Paint {
                color: Color::WHITE,
                alpha: 0.0,
            }),
            // A picture's colours are not read.
            Fill::Image(_) => Paint {
                color: Color::WHITE,
                alpha: 0.0,
            },
            Fill::Gradient(g) => {
                let n = g.stops.len().max(1) as f64;
                let sum = |c: fn(&Paint) -> f64| g.stops.iter().map(|(_, p)| c(p)).sum::<f64>() / n;
                let channel = |v: f64| v.round().clamp(0.0, 255.0) as u8;
                Paint {
                    color: Color(
                        channel(sum(|p| f64::from(p.color.0))),
                        channel(sum(|p| f64::from(p.color.1))),
                        channel(sum(|p| f64::from(p.color.2))),
                    ),
                    alpha: sum(|p| p.alpha),
                }
            }
        }
    }
}

/// A colour changing across the frame's unit square.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub shape: GradientShape,
    /// Each offset along the gradient, from 0 to 1, and its colour.
    pub stops: Vec<(f64, Paint)>,
}

impl Gradient {
    /// An Office shading, VML's or the binary format's, as LibreOffice draws
    /// it: at `angle` 0 from `first` at the bottom, turning clockwise in degrees,
    /// a `focus` near half way making it axial and past half way swapping its
    /// colours; radial about `center` from `first` outside.
    pub fn office(
        first: Paint,
        second: Paint,
        angle: f64,
        focus: f64,
        center: Option<[f64; 2]>,
    ) -> Gradient {
        let focus = focus.clamp(-1.0, 1.0);
        if let Some(center) = center {
            let (inner, outer) = if (-0.5..=0.5).contains(&focus) {
                (second, first)
            } else {
                (first, second)
            };
            return Gradient {
                shape: GradientShape::Radial(center),
                stops: vec![(0.0, inner), (1.0, outer)],
            };
        }
        let mut angle = angle.rem_euclid(360.0);
        let stops = if (0.25..=0.75).contains(&focus.abs()) {
            let (outer, inner) = if (focus > 0.0) == (angle < 180.0) {
                (first, second)
            } else {
                (second, first)
            };
            vec![(0.0, outer), (0.5, inner), (1.0, outer)]
        } else {
            if focus.abs() > 0.5 {
                angle = (angle + 180.0) % 360.0;
            }
            vec![(0.0, first), (1.0, second)]
        };
        Gradient {
            shape: GradientShape::Linear((angle + 270.0) % 360.0),
            stops,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientShape {
    /// Across the frame in this direction, in degrees clockwise from the x
    /// axis, offset 0 at the corner it leaves from.
    Linear(f64),
    /// Out from this point, offset 0, to the farthest corner, offset 1.
    Radial([f64; 2]),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub width: f64,
    pub paint: Paint,
}

pub struct TextBox {
    pub blocks: Vec<Block>,
    /// Between the frame and the text.
    pub inset: Margins,
    pub valign: VAlign,
    /// The frame grows to hold its text.
    pub grow: bool,
    /// The frame narrows to its text's widest line.
    pub fit: bool,
}
