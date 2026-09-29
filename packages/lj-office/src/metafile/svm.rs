//! StarView metafiles, LibreOffice's own, in which it keeps the picture of an
//! embedded chart, spreadsheet or formula.

use std::sync::Arc;

use super::bitmap::png_from_file_dib;
use super::{
    arc, ellipse, flatten, rectangle, ArcKind, Bitmap, HAlign, Metafile, Op, Path, Reader, Text,
    TextAlign,
};
use crate::model::Color;

const UNICODE: u16 = 0xFFFF;
const RELATIVE: u16 = 13;
const MAX_DEPTH: usize = 4;

pub(super) fn read(data: &[u8]) -> Option<Metafile> {
    let mut ops = Vec::new();
    let (size, _) = play(data, &mut ops, 0)?;
    Some(Metafile {
        view: [0.0, 0.0, f64::from(size[0]), f64::from(size[1])],
        ops,
    })
}

#[derive(Clone, Copy)]
struct MapMode {
    unit: u16,
    origin: [f64; 2],
    scale: [f64; 2],
}

impl MapMode {
    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let end = compat(r)?;
        let unit = r.u16()?;
        let origin = [f64::from(r.i32()?), f64::from(r.i32()?)];
        let mut fraction = || {
            let (n, d) = (r.i32()?, r.i32()?);
            Some(if d == 0 {
                1.0
            } else {
                f64::from(n) / f64::from(d)
            })
        };
        let scale = [fraction()?, fraction()?];
        r.at = end;
        Some(Self {
            unit,
            origin,
            scale,
        })
    }

    /// Inches per logical unit along x and y.
    fn inches(&self) -> [f64; 2] {
        let unit = match self.unit {
            0 => 1.0 / 2540.0,
            1 => 1.0 / 254.0,
            2 => 1.0 / 25.4,
            3 => 1.0 / 2.54,
            4 => 1.0 / 1000.0,
            5 => 1.0 / 100.0,
            6 => 1.0 / 10.0,
            7 => 1.0,
            8 => 1.0 / 72.0,
            9 => 1.0 / 1440.0,
            _ => 1.0 / 96.0,
        };
        [unit * self.scale[0], unit * self.scale[1]]
    }
}

/// Where the compatibility block opening at the reader ends.
fn compat(r: &mut Reader<'_>) -> Option<usize> {
    r.u16()?;
    let len = r.u32()? as usize;
    Some(r.at.checked_add(len)?.min(r.data.len()))
}

#[derive(Clone)]
struct Font {
    family: String,
    size: f64,
    bold: bool,
    italic: bool,
    rotated: bool,
    charset: u16,
}

#[derive(Clone)]
struct State {
    line: Option<Color>,
    fill: Option<Color>,
    text: Color,
    align: TextAlign,
    font: Font,
    /// Scale and offset from the current map mode to the picture's.
    to_view: [f64; 4],
}

impl State {
    fn point(&self, p: [i32; 2]) -> [f64; 2] {
        let [sx, sy, dx, dy] = self.to_view;
        [f64::from(p[0]) * sx + dx, f64::from(p[1]) * sy + dy]
    }

    fn length(&self, v: f64) -> f64 {
        v * self.to_view[1].abs()
    }
}

struct Player<'o> {
    ops: &'o mut Vec<Op>,
    pref: MapMode,
    state: State,
    stack: Vec<State>,
    depth: usize,
}

/// Plays the metafile opening `data`, giving its size and where it ends.
fn play(data: &[u8], ops: &mut Vec<Op>, depth: usize) -> Option<([i32; 2], usize)> {
    let mut r = Reader::new(data);
    if r.bytes(6)? != b"VCLMTF" {
        return None;
    }
    let end = compat(&mut r)?;
    r.u32()?;
    let pref = MapMode::read(&mut r)?;
    let size = [r.i32()?, r.i32()?];
    let count = r.u32()?;
    r.at = end;
    let mut player = Player {
        ops,
        pref,
        state: State {
            line: Some(Color::BLACK),
            fill: Some(Color::WHITE),
            text: Color::BLACK,
            align: TextAlign::Baseline,
            font: Font {
                family: String::new(),
                size: 0.0,
                bold: false,
                italic: false,
                rotated: false,
                charset: 0,
            },
            to_view: [1.0, 1.0, 0.0, 0.0],
        },
        stack: Vec::new(),
        depth,
    };
    for _ in 0..count {
        let kind = r.u16()?;
        let version = r.u16()?;
        let len = r.u32()? as usize;
        let body = r.bytes(len.min(r.rest().len()))?;
        let _ = player.action(kind, version, &mut Reader::new(body));
    }
    Some((size, r.at))
}

fn color(r: &mut Reader<'_>) -> Option<(Color, u8)> {
    let [b, g, red, alpha] = r.u32()?.to_le_bytes();
    Some((Color(red, g, b), alpha))
}

fn point(r: &mut Reader<'_>) -> Option<[i32; 2]> {
    Some([r.i32()?, r.i32()?])
}

fn polygon(r: &mut Reader<'_>) -> Option<Vec<[i32; 2]>> {
    let n = usize::from(r.u16()?);
    if n * 8 > r.rest().len() {
        return None;
    }
    (0..n).map(|_| point(r)).collect()
}

/// A polygon with the flags marking its Bézier control points.
fn flagged_polygon(r: &mut Reader<'_>) -> Option<(Vec<[i32; 2]>, Vec<bool>)> {
    let end = compat(r)?;
    let points = polygon(r)?;
    let control = if r.u8()? != 0 {
        r.bytes(points.len())?.iter().map(|&f| f == 2).collect()
    } else {
        Vec::new()
    };
    r.at = end;
    Some((points, control))
}

fn rect(r: &mut Reader<'_>) -> Option<[i32; 4]> {
    Some([r.i32()?, r.i32()?, r.i32()?, r.i32()?])
}

/// A line's width, `None` for no line.
fn line_info(r: &mut Reader<'_>) -> Option<Option<i32>> {
    let end = compat(r)?;
    let style = r.u16()?;
    let width = r.i32()?;
    r.at = end;
    Some((style != 0).then_some(width))
}

fn string(r: &mut Reader<'_>, charset: u16) -> Option<String> {
    if charset == UNICODE {
        let n = r.u32()? as usize;
        let units: Vec<u16> = (0..n).map(|_| r.u16()).collect::<Option<_>>()?;
        return Some(String::from_utf16_lossy(&units));
    }
    let n = usize::from(r.u16()?);
    Some(r.bytes(n)?.iter().map(|&b| char::from(b)).collect())
}

fn utf16(r: &mut Reader<'_>) -> Option<Vec<u16>> {
    let n = usize::from(r.u16()?);
    (0..n).map(|_| r.u16()).collect()
}

impl Player<'_> {
    /// The string an action draws: its UTF-16 copy from version 2.
    fn units(&self, r: &mut Reader<'_>, version: u16, text: &str) -> Option<Vec<u16>> {
        if version >= 2 {
            utf16(r)
        } else {
            Some(text.encode_utf16().collect())
        }
    }

    fn action(&mut self, kind: u16, version: u16, r: &mut Reader<'_>) -> Option<()> {
        match kind {
            102 => {
                let (a, b) = (point(r)?, point(r)?);
                let width = if version >= 2 { line_info(r)? } else { Some(0) };
                self.stroke(vec![vec![self.state.point(a), self.state.point(b)]], width);
            }
            103 => {
                let ring = self.rect_ring(rect(r)?, 0.0, 0.0);
                self.shape(vec![ring]);
            }
            104 => {
                let (bounds, rx, ry) = (rect(r)?, r.u32()?, r.u32()?);
                let ring = self.rect_ring(bounds, f64::from(rx), f64::from(ry));
                self.shape(vec![ring]);
            }
            105 => {
                let ring = ellipse(self.bounds(rect(r)?));
                self.shape(vec![ring]);
            }
            106..=108 => {
                let (bounds, a, b) = (rect(r)?, point(r)?, point(r)?);
                let kind = [ArcKind::Arc, ArcKind::Pie, ArcKind::Chord][usize::from(kind - 106)];
                let ring = arc(
                    self.bounds(bounds),
                    self.state.point(a),
                    self.state.point(b),
                    kind,
                );
                if kind == ArcKind::Arc {
                    self.stroke(vec![ring], Some(0));
                } else {
                    self.shape(vec![ring]);
                }
            }
            109 => {
                let mut points = polygon(r)?;
                let mut control = Vec::new();
                let width = if version >= 2 { line_info(r)? } else { Some(0) };
                if version >= 3 && r.u8()? != 0 {
                    (points, control) = flagged_polygon(r)?;
                }
                let ring = self.ring(&points, &control);
                self.stroke(vec![ring], width);
            }
            110 => {
                let mut points = polygon(r)?;
                let mut control = Vec::new();
                if version >= 2 && r.u8()? != 0 {
                    (points, control) = flagged_polygon(r)?;
                }
                let ring = self.ring(&points, &control);
                self.shape(vec![ring]);
            }
            111 => {
                let rings = self.poly_polygon(version, r)?;
                self.shape(rings);
            }
            112 => {
                let at = point(r)?;
                let text = string(r, self.state.font.charset)?;
                let (index, len) = (r.u16()?, r.u16()?);
                let units = self.units(r, version, &text)?;
                self.text(at, &units, index, len, None);
            }
            113 => {
                let at = point(r)?;
                let text = string(r, self.state.font.charset)?;
                let (index, len) = (r.u16()?, r.u16()?);
                let n = r.i32()?.max(0) as usize;
                if n.checked_mul(4)? > r.rest().len() {
                    return None;
                }
                let dx: Vec<i32> = (0..n).map(|_| r.i32()).collect::<Option<_>>()?;
                let units = self.units(r, version, &text)?;
                self.text(at, &units, index, len, Some(&dx));
            }
            114 => {
                let at = point(r)?;
                let text = string(r, self.state.font.charset)?;
                r.u32()?;
                let (index, len) = (r.u16()?, r.u16()?);
                let units = self.units(r, version, &text)?;
                self.text(at, &units, index, len, None);
            }
            115 => {
                let bounds = rect(r)?;
                let text = string(r, self.state.font.charset)?;
                r.u16()?;
                let units = self.units(r, version, &text)?;
                let align = std::mem::replace(&mut self.state.align, TextAlign::Top);
                self.text([bounds[0], bounds[1]], &units, 0, u16::MAX, None);
                self.state.align = align;
            }
            117 | 118 | 120 | 121 => {
                let png = png_from_file_dib(r.rest());
                let tail = if kind == 117 || kind == 120 { 16 } else { 32 };
                r.at = r.data.len().checked_sub(tail)?;
                let (at, size) = (point(r)?, point(r)?);
                if let Some(png) = png {
                    let [x0, y0] = self.state.point(at);
                    let [x1, y1] = self
                        .state
                        .point([at[0].saturating_add(size[0]), at[1].saturating_add(size[1])]);
                    self.ops.push(Op::Bitmap(Bitmap {
                        rect: [x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs()],
                        png: Arc::from(png),
                    }));
                }
            }
            125 => {
                let bounds = rect(r)?;
                let fill = gradient_mean(r)?;
                let ring = self.rect_ring(bounds, 0.0, 0.0);
                self.ops.push(Op::Path(Path {
                    rings: vec![ring],
                    closed: true,
                    fill: Some((fill, 1.0)),
                    stroke: None,
                    even_odd: true,
                }));
            }
            132 => {
                let (c, alpha) = color(r)?;
                self.state.line = (r.u8()? != 0 && alpha != 255).then_some(c);
            }
            133 => {
                let (c, alpha) = color(r)?;
                self.state.fill = (r.u8()? != 0 && alpha != 255).then_some(c);
            }
            134 => self.state.text = color(r)?.0,
            136 => {
                self.state.align = match r.u16()? {
                    0 => TextAlign::Top,
                    2 => TextAlign::Bottom,
                    _ => TextAlign::Baseline,
                }
            }
            137 => {
                let map = MapMode::read(r)?;
                self.state.to_view = self.to_view(map);
            }
            138 => self.state.font = font(r)?,
            139 => self.stack.push(self.state.clone()),
            140 => {
                if let Some(state) = self.stack.pop() {
                    self.state = state;
                }
            }
            142 => {
                let rings = self.poly_polygon(1, r)?;
                let alpha = 1.0 - f64::from(r.u16()?.min(100)) / 100.0;
                let fill = self.state.fill.map(|c| (c, alpha));
                let stroke = self.state.line.map(|c| (c, 0.0));
                self.ops.push(Op::Path(Path {
                    rings,
                    closed: true,
                    fill,
                    stroke,
                    even_odd: true,
                }));
            }
            147 if self.depth < MAX_DEPTH => {
                let mut inner = Vec::new();
                let (size, len) = play(r.rest(), &mut inner, self.depth + 1)?;
                r.skip(len)?;
                let (at, extent) = (point(r)?, point(r)?);
                self.nest(inner, size, at, extent);
            }
            _ => {}
        }
        Some(())
    }

    fn bounds(&self, rect: [i32; 4]) -> [f64; 4] {
        let [l, t] = self.state.point([rect[0], rect[1]]);
        let [r, b] = self.state.point([rect[2], rect[3]]);
        [l, t, r, b]
    }

    fn rect_ring(&self, rect: [i32; 4], rx: f64, ry: f64) -> Vec<[f64; 2]> {
        rectangle(
            self.bounds(rect),
            self.state.length(rx),
            self.state.length(ry),
        )
    }

    fn ring(&self, points: &[[i32; 2]], control: &[bool]) -> Vec<[f64; 2]> {
        let points: Vec<[f64; 2]> = points.iter().map(|&p| self.state.point(p)).collect();
        if control.iter().any(|&c| c) {
            flatten(&points, control)
        } else {
            points
        }
    }

    fn poly_polygon(&self, version: u16, r: &mut Reader<'_>) -> Option<Vec<Vec<[f64; 2]>>> {
        let n = usize::from(r.u16()?);
        let mut polygons: Vec<(Vec<[i32; 2]>, Vec<bool>)> = (0..n)
            .map(|_| polygon(r).map(|p| (p, Vec::new())))
            .collect::<Option<_>>()?;
        if version >= 2 {
            for _ in 0..r.u16()? {
                let index = usize::from(r.u16()?);
                let complex = flagged_polygon(r)?;
                if let Some(slot) = polygons.get_mut(index) {
                    *slot = complex;
                }
            }
        }
        Some(
            polygons
                .iter()
                .map(|(points, control)| self.ring(points, control))
                .collect(),
        )
    }

    fn shape(&mut self, rings: Vec<Vec<[f64; 2]>>) {
        let fill = self.state.fill.map(|c| (c, 1.0));
        let stroke = self.state.line.map(|c| (c, 0.0));
        if fill.is_some() || stroke.is_some() {
            self.ops.push(Op::Path(Path {
                rings,
                closed: true,
                fill,
                stroke,
                even_odd: true,
            }));
        }
    }

    fn stroke(&mut self, rings: Vec<Vec<[f64; 2]>>, width: Option<i32>) {
        let (Some(c), Some(width)) = (self.state.line, width) else {
            return;
        };
        let width = self.state.length(f64::from(width.max(0)));
        self.ops.push(Op::Path(Path {
            rings,
            closed: false,
            fill: None,
            stroke: Some((c, width)),
            even_odd: true,
        }));
    }

    fn text(&mut self, at: [i32; 2], units: &[u16], index: u16, len: u16, dx: Option<&[i32]>) {
        let font = &self.state.font;
        if font.rotated {
            return;
        }
        let start = usize::from(index).min(units.len());
        let end = start.saturating_add(usize::from(len)).min(units.len());
        let text = String::from_utf16_lossy(&units[start..end]);
        if text.trim().is_empty() {
            return;
        }
        let advances = dx.filter(|dx| dx.len() >= end - start).map(|dx| {
            let (mut previous, mut unit) = (0, 0);
            text.chars()
                .map(|c| {
                    unit += c.len_utf16();
                    let x = dx[unit - 1];
                    let advance = self.state.length(f64::from(x) - f64::from(previous));
                    previous = x;
                    advance
                })
                .collect()
        });
        self.ops.push(Op::Text(Text {
            at: self.state.point(at),
            align: self.state.align,
            h_align: HAlign::Left,
            text,
            family: font.family.clone(),
            size: self.state.length(font.size),
            bold: font.bold,
            italic: font.italic,
            color: self.state.text,
            advances,
        }));
    }

    fn to_view(&self, map: MapMode) -> [f64; 4] {
        if map.unit == RELATIVE {
            let [sx, sy, dx, dy] = self.state.to_view;
            let (sx, sy) = (sx * map.scale[0], sy * map.scale[1]);
            return [sx, sy, dx + map.origin[0] * sx, dy + map.origin[1] * sy];
        }
        let (from, to) = (map.inches(), self.pref.inches());
        let (sx, sy) = (from[0] / to[0], from[1] / to[1]);
        [sx, sy, map.origin[0] * sx, map.origin[1] * sy]
    }

    /// The ops of a nested metafile of `size` drawn into `extent` at `at`.
    fn nest(&mut self, inner: Vec<Op>, size: [i32; 2], at: [i32; 2], extent: [i32; 2]) {
        if size[0] == 0 || size[1] == 0 {
            return;
        }
        let [x0, y0] = self.state.point(at);
        let [x1, y1] = self.state.point([
            at[0].saturating_add(extent[0]),
            at[1].saturating_add(extent[1]),
        ]);
        let (sx, sy) = (
            (x1 - x0) / f64::from(size[0]),
            (y1 - y0) / f64::from(size[1]),
        );
        let map = |p: [f64; 2]| [x0 + p[0] * sx, y0 + p[1] * sy];
        for op in inner {
            self.ops.push(match op {
                Op::Path(mut path) => {
                    for p in path.rings.iter_mut().flatten() {
                        *p = map(*p);
                    }
                    if let Some((_, width)) = &mut path.stroke {
                        *width *= sy.abs();
                    }
                    Op::Path(path)
                }
                Op::Text(mut text) => {
                    text.at = map(text.at);
                    text.size *= sy.abs();
                    for a in text.advances.iter_mut().flatten() {
                        *a *= sx.abs();
                    }
                    Op::Text(text)
                }
                Op::Bitmap(mut bitmap) => {
                    let [x, y] = map([bitmap.rect[0], bitmap.rect[1]]);
                    bitmap.rect = [x, y, bitmap.rect[2] * sx, bitmap.rect[3] * sy];
                    Op::Bitmap(bitmap)
                }
            });
        }
    }
}

fn font(r: &mut Reader<'_>) -> Option<Font> {
    let end = compat(r)?;
    let family = string(r, 0)?;
    string(r, 0)?;
    let (_, height) = (r.i32()?, r.i32()?);
    let charset = r.u16()?;
    r.skip(4)?;
    let weight = r.u16()?;
    r.skip(4)?;
    let italic = r.u16()?;
    r.skip(4)?;
    let orientation = r.i16()?;
    r.at = end;
    Some(Font {
        family: family.split(';').next().unwrap_or_default().to_string(),
        size: f64::from(height.unsigned_abs()),
        bold: weight >= 7,
        italic: italic == 1 || italic == 2,
        rotated: orientation != 0,
        charset,
    })
}

/// The mean of a gradient's two colours, each at its intensity.
fn gradient_mean(r: &mut Reader<'_>) -> Option<Color> {
    let end = compat(r)?;
    r.u16()?;
    let mut old_color = || -> Option<[u16; 3]> {
        if r.u16()? & 0x8000 != 0 {
            Some([r.u16()? >> 8, r.u16()? >> 8, r.u16()? >> 8])
        } else {
            Some([0, 0, 0])
        }
    };
    let (a, b) = (old_color()?, old_color()?);
    r.skip(8)?;
    let (ia, ib) = (u32::from(r.u16()?), u32::from(r.u16()?));
    r.at = end;
    let mix = |k: usize| ((u32::from(a[k]) * ia + u32::from(b[k]) * ib) / 200).min(255) as u8;
    Some(Color(mix(0), mix(1), mix(2)))
}
