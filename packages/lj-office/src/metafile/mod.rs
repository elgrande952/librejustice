//! Metafiles, the vector pictures of Office and LibreOffice (SVM, WMF, EMF),
//! played into paths, text and bitmaps in the picture's own coordinates.

mod bitmap;
mod emf;
mod gdi;
mod pict;
mod svm;
mod wmf;

use std::sync::Arc;

use crate::model::{Color, ImageFormat};

pub(crate) use bitmap::png_from_bmp;
pub(crate) use pict::is_pict;

pub(crate) struct Metafile {
    /// Left, top, width, height of what the frame shows.
    pub view: [f64; 4],
    pub ops: Vec<Op>,
}

pub(crate) enum Op {
    Path(Path),
    Text(Text),
    Bitmap(Bitmap),
}

pub(crate) struct Path {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub closed: bool,
    /// A colour and its opacity.
    pub fill: Option<(Color, f64)>,
    /// A colour and a width, 0 for a hairline.
    pub stroke: Option<(Color, f64)>,
    pub even_odd: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextAlign {
    Top,
    Baseline,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HAlign {
    Left,
    Center,
    Right,
}

pub(crate) struct Text {
    pub at: [f64; 2],
    pub align: TextAlign,
    pub h_align: HAlign,
    pub text: String,
    pub family: String,
    /// The em.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: Color,
    pub advances: Option<Vec<f64>>,
}

pub(crate) struct Bitmap {
    /// Left, top, width, height.
    pub rect: [f64; 4],
    pub png: Arc<[u8]>,
}

pub(crate) fn read(data: &[u8], format: ImageFormat) -> Option<Metafile> {
    match format {
        ImageFormat::Svm => svm::read(data),
        ImageFormat::Wmf => wmf::read(data),
        ImageFormat::Emf => emf::read(data),
        ImageFormat::Pict => pict::read(data),
        ImageFormat::Eps => match eps_preview(data)? {
            (ImageFormat::Wmf, preview) => wmf::read(preview),
            _ => None,
        },
        _ => None,
    }
}

/// The TIFF or WMF preview a PostScript file carries behind its DOS header.
pub(crate) fn eps_preview(data: &[u8]) -> Option<(ImageFormat, &[u8])> {
    if data.get(..4)? != b"\xC5\xD0\xD3\xC6" {
        return None;
    }
    let mut r = Reader::new(data.get(12..28)?);
    let (wmf_at, wmf_len, tiff_at, tiff_len) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
    let slice = |at: u32, len: u32| data.get(at as usize..(at as usize).checked_add(len as usize)?);
    match (tiff_len, wmf_len) {
        (1.., _) => Some((ImageFormat::Tiff, slice(tiff_at, tiff_len)?)),
        (0, 1..) => Some((ImageFormat::Wmf, slice(wmf_at, wmf_len)?)),
        _ => None,
    }
}

#[derive(Clone)]
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }

    fn i16(&mut self) -> Option<i16> {
        Some(self.u16()? as i16)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }

    fn i32(&mut self) -> Option<i32> {
        Some(self.u32()? as i32)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.bytes(n).map(|_| ())
    }

    fn rest(&self) -> &'a [u8] {
        &self.data[self.at.min(self.data.len())..]
    }
}

/// The outline of `rect` (left, top, right, bottom), its corners rounded by
/// radii `rx`, `ry`.
fn rectangle(rect: [f64; 4], rx: f64, ry: f64) -> Vec<[f64; 2]> {
    let [l, t, r, b] = rect;
    let (rx, ry) = (rx.min((r - l).abs() / 2.0), ry.min((b - t).abs() / 2.0));
    if rx <= 0.0 || ry <= 0.0 {
        return vec![[l, t], [r, t], [r, b], [l, b]];
    }
    let mut out = Vec::new();
    for (cx, cy, from) in [
        (r - rx, t + ry, -90.0),
        (r - rx, b - ry, 0.0),
        (l + rx, b - ry, 90.0),
        (l + rx, t + ry, 180.0),
    ] {
        for k in 0..=6 {
            let a = (from + 15.0 * f64::from(k)).to_radians();
            out.push([cx + rx * a.cos(), cy + ry * a.sin()]);
        }
    }
    out
}

fn ellipse(rect: [f64; 4]) -> Vec<[f64; 2]> {
    let [l, t, r, b] = rect;
    let (cx, cy, rx, ry) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
    (0..48)
        .map(|k| {
            let a = (7.5 * f64::from(k)).to_radians();
            [cx + rx * a.cos(), cy + ry * a.sin()]
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArcKind {
    Arc,
    Pie,
    Chord,
}

/// The arc of the ellipse in `rect` from the ray through `start` counter
/// clockwise to the ray through `end`.
fn arc(rect: [f64; 4], start: [f64; 2], end: [f64; 2], kind: ArcKind) -> Vec<[f64; 2]> {
    let [l, t, r, b] = rect;
    let (cx, cy, rx, ry) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
    let angle = |p: [f64; 2]| (cy - p[1]).atan2(p[0] - cx);
    let a0 = angle(start);
    let mut a1 = angle(end);
    if a1 <= a0 {
        a1 += std::f64::consts::TAU;
    }
    let steps = (((a1 - a0) / 0.1).ceil() as usize).max(2);
    let mut out: Vec<[f64; 2]> = (0..=steps)
        .map(|k| {
            let a = a0 + (a1 - a0) * k as f64 / steps as f64;
            [cx + rx * a.cos(), cy - ry * a.sin()]
        })
        .collect();
    if kind == ArcKind::Pie {
        out.push([cx, cy]);
    }
    out
}

/// Cubic Bézier segments flattened: two control points between two plain ones
/// bend the segment.
fn flatten(points: &[[f64; 2]], control: &[bool]) -> Vec<[f64; 2]> {
    let is_control = |k: usize| control.get(k).copied().unwrap_or(false);
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    let mut i = 0;
    while i < points.len() {
        match out.last() {
            Some(&p0) if i + 2 < points.len() && is_control(i) && is_control(i + 1) => {
                let (c1, c2, p3) = (points[i], points[i + 1], points[i + 2]);
                for k in 1..=16 {
                    let t = f64::from(k) / 16.0;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    out.push([
                        a * p0[0] + b * c1[0] + c * c2[0] + d * p3[0],
                        a * p0[1] + b * c1[1] + c * c2[1] + d * p3[1],
                    ]);
                }
                i += 3;
            }
            _ => {
                out.push(points[i]);
                i += 1;
            }
        }
    }
    out
}
