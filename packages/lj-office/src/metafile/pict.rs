//! QuickDraw pictures, the Mac metafile a `.doc` carries: lines, shapes,
//! text and bitmaps, in version 1 or 2.

use std::sync::Arc;

use lj_image::png::{self, Kind};

use super::{ellipse, rectangle, Bitmap, HAlign, Metafile, Op, Path, Text, TextAlign};
use crate::model::Color;

/// Whether `data` starts as a picture does, after its size and frame.
pub(crate) fn is_pict(data: &[u8]) -> bool {
    matches!(
        data.get(10..14),
        Some([0x00, 0x11, 0x02, 0xFF] | [0x11, 0x01, ..])
    )
}

pub(super) fn read(data: &[u8]) -> Option<Metafile> {
    let mut r = Be { data, at: 2 };
    let frame = r.rect()?;
    let version = match r.data.get(10..12)? {
        [0x11, 0x01] => 1,
        _ => 2,
    };
    r.at = if version == 1 { 12 } else { 14 };
    let mut pict = Pict {
        version,
        view: frame,
        ops: Vec::new(),
        pen: [1.0, 1.0],
        pen_at: [0.0, 0.0],
        text_at: [0.0, 0.0],
        fg: Color::BLACK,
        bg: Color::WHITE,
        pen_pattern: 1.0,
        fill_pattern: 1.0,
        oval: [0.0, 0.0],
        last_rect: [0.0; 4],
        last_poly: Vec::new(),
        font: 0,
        fonts: Vec::new(),
        size: 12.0,
        face: 0,
        rotated: false,
    };
    while let Some(op) = pict.opcode(&mut r) {
        if op == 0xFF || pict.play(op, &mut r).is_none() {
            break;
        }
    }
    let [l, t, rt, b] = pict.view;
    Some(Metafile {
        view: [l, t, rt - l, b - t],
        ops: pict.ops,
    })
}

struct Be<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Be<'a> {
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.bytes(2)?.try_into().ok()?))
    }

    fn i16(&mut self) -> Option<f64> {
        Some(f64::from(self.u16()? as i16))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.bytes(4)?.try_into().ok()?))
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.bytes(n).map(|_| ())
    }

    /// Stored v, h; given x, y.
    fn point(&mut self) -> Option<[f64; 2]> {
        let v = self.i16()?;
        Some([self.i16()?, v])
    }

    /// Stored top, left, bottom, right; given left, top, right, bottom.
    fn rect(&mut self) -> Option<[f64; 4]> {
        let (t, l, b, rt) = (self.i16()?, self.i16()?, self.i16()?, self.i16()?);
        Some([l, t, rt, b])
    }

    fn rgb(&mut self) -> Option<Color> {
        let mut channel = || Some((self.u16()? >> 8) as u8);
        Some(Color(channel()?, channel()?, channel()?))
    }
}

struct Pict {
    version: u8,
    view: [f64; 4],
    ops: Vec<Op>,
    pen: [f64; 2],
    pen_at: [f64; 2],
    text_at: [f64; 2],
    fg: Color,
    bg: Color,
    /// Share of a pattern's pixels in the foreground colour.
    pen_pattern: f64,
    fill_pattern: f64,
    oval: [f64; 2],
    last_rect: [f64; 4],
    last_poly: Vec<[f64; 2]>,
    font: u16,
    fonts: Vec<(u16, String)>,
    size: f64,
    face: u8,
    /// Inside a rotated text comment, whose strings only a device that turns
    /// text draws; the picture carries their image for the others.
    rotated: bool,
}

const TEXT_BEGIN: u16 = 150;
const TEXT_END: u16 = 151;

#[derive(Clone, Copy)]
enum Verb {
    Frame,
    Paint,
    Erase,
    Invert,
    Fill,
}

impl Pict {
    fn opcode(&self, r: &mut Be<'_>) -> Option<u16> {
        if self.version == 1 {
            return r.u8().map(u16::from);
        }
        r.at += r.at % 2;
        r.u16()
    }

    fn play(&mut self, op: u16, r: &mut Be<'_>) -> Option<()> {
        let verb = |op: u16| match op % 8 {
            0 => Some(Verb::Frame),
            1 => Some(Verb::Paint),
            2 => Some(Verb::Erase),
            3 => Some(Verb::Invert),
            4 => Some(Verb::Fill),
            _ => None,
        };
        match op {
            0x00 | 0x1C | 0x1E | 0x17..=0x19 | 0x3D..=0x3F | 0x4D..=0x4F | 0x5D..=0x5F => {}
            0x7D..=0x7F | 0x8D..=0x8F | 0xB0..=0xCF | 0x8000..=0x80FF => {}
            0x01 | 0x80..=0x87 if op == 0x01 || verb(op).is_none() => {
                let size = usize::from(r.u16()?);
                r.skip(size.checked_sub(2)?)?;
            }
            0x02 => r.skip(8)?,
            0x03 => self.font = r.u16()?,
            0x04 => {
                self.face = r.u8()?;
            }
            0x05 | 0x08 | 0x15 | 0x16 => r.skip(2)?,
            0x06 | 0x0C => r.skip(4)?,
            0x07 => {
                let [h, v] = r.point()?;
                self.pen = [h, v];
            }
            0x09 => self.pen_pattern = pattern(r.bytes(8)?),
            0x0A => self.fill_pattern = pattern(r.bytes(8)?),
            0x0B => {
                let [h, v] = r.point()?;
                self.oval = [h, v];
            }
            0x0D => self.size = f64::from(r.u16()?),
            0x0E => self.fg = classic(r.u32()?),
            0x0F => self.bg = classic(r.u32()?),
            0x10 => r.skip(8)?,
            0x11 => r.skip(if self.version == 1 { 1 } else { 2 })?,
            0x12..=0x14 => {
                let density = pixpat(r)?;
                match op {
                    0x13 => self.pen_pattern = density,
                    0x14 => self.fill_pattern = density,
                    _ => {}
                }
            }
            0x1A => self.fg = r.rgb()?,
            0x1B => self.bg = r.rgb()?,
            0x1D | 0x1F => r.skip(6)?,
            0x20 => {
                let from = r.point()?;
                let to = r.point()?;
                self.line(from, to);
            }
            0x21 => {
                let to = r.point()?;
                self.line(self.pen_at, to);
            }
            0x22 => {
                let from = r.point()?;
                let (dh, dv) = (r.u8()? as i8, r.u8()? as i8);
                self.line(from, [from[0] + f64::from(dh), from[1] + f64::from(dv)]);
            }
            0x23 => {
                let (dh, dv) = (r.u8()? as i8, r.u8()? as i8);
                let from = self.pen_at;
                self.line(from, [from[0] + f64::from(dh), from[1] + f64::from(dv)]);
            }
            0x28 => {
                self.text_at = r.point()?;
                self.text(r)?;
            }
            0x29 => {
                self.text_at[0] += f64::from(r.u8()?);
                self.text(r)?;
            }
            0x2A => {
                self.text_at[1] += f64::from(r.u8()?);
                self.text(r)?;
            }
            0x2B => {
                self.text_at[0] += f64::from(r.u8()?);
                self.text_at[1] += f64::from(r.u8()?);
                self.text(r)?;
            }
            0x2C => {
                let len = usize::from(r.u16()?);
                let mut body = Be {
                    data: r.bytes(len)?,
                    at: 0,
                };
                let id = body.u16()?;
                let n = usize::from(body.u8()?);
                let name = encoding_rs::MACINTOSH.decode(body.bytes(n)?).0.into_owned();
                self.fonts.push((id, name));
            }
            0x24..=0x27 | 0x2D..=0x2F | 0x92..=0x97 | 0x9C..=0x9F | 0xA2..=0xAF => {
                let len = usize::from(r.u16()?);
                r.skip(len)?;
            }
            0x30..=0x37 => {
                self.last_rect = r.rect()?;
                self.shape(verb(op), Shape::Rect);
            }
            0x38..=0x3C => self.shape(verb(op), Shape::Rect),
            0x40..=0x47 => {
                self.last_rect = r.rect()?;
                self.shape(verb(op), Shape::RoundRect);
            }
            0x48..=0x4C => self.shape(verb(op), Shape::RoundRect),
            0x50..=0x57 => {
                self.last_rect = r.rect()?;
                self.shape(verb(op), Shape::Oval);
            }
            0x58..=0x5C => self.shape(verb(op), Shape::Oval),
            0x60..=0x67 => {
                self.last_rect = r.rect()?;
                let (start, extent) = (r.i16()?, r.i16()?);
                self.shape(verb(op), Shape::Arc(start, extent));
            }
            0x68..=0x6F => {
                let (start, extent) = (r.i16()?, r.i16()?);
                self.shape(verb(op), Shape::Arc(start, extent));
            }
            0x70..=0x77 => {
                let size = usize::from(r.u16()?);
                r.skip(8)?;
                let n = size.checked_sub(10)? / 4;
                self.last_poly = (0..n).map(|_| r.point()).collect::<Option<_>>()?;
                self.shape(verb(op), Shape::Poly);
            }
            0x78..=0x7C => self.shape(verb(op), Shape::Poly),
            0x80..=0x84 => {
                let size = usize::from(r.u16()?);
                let region = r.bytes(size.checked_sub(2)?)?;
                self.region(verb(op), region);
            }
            0x88..=0x8C => {}
            0x90 | 0x91 | 0x98 | 0x99 | 0x9A | 0x9B => self.bits(op, r)?,
            0xA0 => {
                if r.u16()? == TEXT_END {
                    self.rotated = false;
                }
            }
            0xA1 => {
                let kind = r.u16()?;
                let len = usize::from(r.u16()?);
                let body = r.bytes(len)?;
                match kind {
                    TEXT_BEGIN => {
                        self.rotated = body.get(2..4).is_some_and(|angle| angle != [0, 0])
                    }
                    TEXT_END => self.rotated = false,
                    _ => {}
                }
            }
            0xD0..=0xFE | 0x8100..=0xFFFF => {
                let len = r.u32()? as usize;
                r.skip(len)?;
            }
            0x0C00 => {
                let version = r.u16()? as i16;
                r.skip(2)?;
                if version == -2 {
                    r.skip(8)?;
                    self.view = r.rect()?;
                    r.skip(4)?;
                } else {
                    r.skip(20)?;
                }
            }
            0x0100..=0x7FFF => r.skip(usize::from(op >> 8) * 2)?,
            _ => return None,
        }
        Some(())
    }

    fn mix(&self, share: f64) -> Color {
        let channel =
            |f: u8, b: u8| (f64::from(f) * share + f64::from(b) * (1.0 - share)).round() as u8;
        Color(
            channel(self.fg.0, self.bg.0),
            channel(self.fg.1, self.bg.1),
            channel(self.fg.2, self.bg.2),
        )
    }

    /// The pen's rectangle hangs below and right of the path it follows.
    fn line(&mut self, from: [f64; 2], to: [f64; 2]) {
        self.pen_at = to;
        let [w, h] = self.pen;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let shift = |p: [f64; 2]| [p[0] + w / 2.0, p[1] + h / 2.0];
        let stroke = Some((self.mix(self.pen_pattern), w.max(h)));
        // Writers draw a curve as thousands of short lines: one polyline per
        // run of them, and one path per run of lines in the same pen.
        if let Some(Op::Path(last)) = self.ops.last_mut() {
            if last.stroke == stroke && last.fill.is_none() && !last.closed {
                match last.rings.last_mut() {
                    Some(ring) if ring.last() == Some(&shift(from)) => ring.push(shift(to)),
                    _ => last.rings.push(vec![shift(from), shift(to)]),
                }
                return;
            }
        }
        self.ops.push(Op::Path(Path {
            rings: vec![vec![shift(from), shift(to)]],
            closed: false,
            fill: None,
            stroke,
            even_odd: false,
        }));
    }

    fn shape(&mut self, verb: Option<Verb>, shape: Shape) {
        let Some(verb) = verb else {
            return;
        };
        let [l, t, r, b] = self.last_rect;
        let ring = |inset: f64| {
            let rect = [l + inset, t + inset, r - inset, b - inset];
            match shape {
                Shape::Rect => rectangle(rect, 0.0, 0.0),
                Shape::RoundRect => rectangle(rect, self.oval[0] / 2.0, self.oval[1] / 2.0),
                Shape::Oval => ellipse(rect),
                Shape::Arc(start, extent) => wedge(rect, start, extent),
                Shape::Poly => self.last_poly.clone(),
            }
        };
        let closed =
            !matches!(shape, Shape::Poly) || self.last_poly.first() == self.last_poly.last();
        let path = match verb {
            Verb::Frame => {
                let [w, h] = self.pen;
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                let rings = match shape {
                    Shape::Poly => vec![ring(0.0)
                        .iter()
                        .map(|p| [p[0] + w / 2.0, p[1] + h / 2.0])
                        .collect()],
                    Shape::Arc(..) => vec![arc_edge(&ring(w / 2.0))],
                    _ => vec![ring(w / 2.0)],
                };
                Path {
                    rings,
                    closed: closed && !matches!(shape, Shape::Arc(..)),
                    fill: None,
                    stroke: Some((self.mix(self.pen_pattern), w.max(h))),
                    even_odd: false,
                }
            }
            Verb::Invert => return,
            fill => {
                let color = match fill {
                    Verb::Paint => self.mix(self.pen_pattern),
                    Verb::Fill => self.mix(self.fill_pattern),
                    _ => self.bg,
                };
                Path {
                    rings: vec![ring(0.0)],
                    closed: true,
                    fill: Some((color, 1.0)),
                    stroke: None,
                    even_odd: false,
                }
            }
        };
        self.ops.push(Op::Path(path));
    }

    /// A region as the rectangles its inversion points bound, band by band.
    fn region(&mut self, verb: Option<Verb>, data: &[u8]) {
        let color = match verb {
            Some(Verb::Paint | Verb::Frame) => self.mix(self.pen_pattern),
            Some(Verb::Fill) => self.mix(self.fill_pattern),
            Some(Verb::Erase) => self.bg,
            Some(Verb::Invert) | None => return,
        };
        let mut r = Be { data, at: 0 };
        let Some(bounds) = r.rect() else {
            return;
        };
        let mut rings = Vec::new();
        if data.len() <= 8 {
            rings.push(rectangle(bounds, 0.0, 0.0));
        } else {
            let mut toggles: Vec<f64> = Vec::new();
            let mut band_top = None;
            while let Some(v) = r.u16() {
                if v == 0x7FFF {
                    break;
                }
                let v = f64::from(v as i16);
                if let Some(top) = band_top {
                    for [left, right] in toggles.as_chunks::<2>().0 {
                        rings.push(rectangle([*left, top, *right, v], 0.0, 0.0));
                    }
                }
                while let Some(h) = r.u16() {
                    if h == 0x7FFF {
                        break;
                    }
                    let h = f64::from(h as i16);
                    match toggles.iter().position(|&t| t == h) {
                        Some(at) => {
                            toggles.remove(at);
                        }
                        None => toggles.push(h),
                    }
                }
                toggles.sort_by(f64::total_cmp);
                band_top = Some(v);
            }
        }
        if !rings.is_empty() {
            self.ops.push(Op::Path(Path {
                rings,
                closed: true,
                fill: Some((color, 1.0)),
                stroke: None,
                even_odd: false,
            }));
        }
    }

    fn text(&mut self, r: &mut Be<'_>) -> Option<()> {
        let n = usize::from(r.u8()?);
        let bytes = r.bytes(n)?;
        if self.rotated {
            return Some(());
        }
        let text = encoding_rs::MACINTOSH.decode(bytes).0.into_owned();
        let family = self
            .fonts
            .iter()
            .rev()
            .find(|(id, _)| *id == self.font)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| classic_font(self.font).to_string());
        self.ops.push(Op::Text(Text {
            at: self.text_at,
            align: TextAlign::Baseline,
            h_align: HAlign::Left,
            text,
            family,
            size: self.size,
            bold: self.face & 1 != 0,
            italic: self.face & 2 != 0,
            color: self.fg,
            advances: None,
        }));
        Some(())
    }

    fn bits(&mut self, op: u16, r: &mut Be<'_>) -> Option<()> {
        let direct = matches!(op, 0x9A | 0x9B);
        if direct {
            r.skip(4)?;
        }
        let row_bytes = r.u16()?;
        let bounds = r.rect()?;
        let pixmap = direct || row_bytes & 0x8000 != 0;
        let (mut pack_type, mut pixel_size, mut components) = (0, 1, 1);
        let mut palette = vec![[255, 255, 255], [0, 0, 0]];
        if pixmap {
            r.skip(2)?;
            pack_type = r.u16()?;
            r.skip(12)?;
            r.skip(2)?;
            pixel_size = r.u16()?;
            components = r.u16()?;
            r.skip(14)?;
            if !direct {
                r.skip(6)?;
                let size = usize::from(r.u16()?) + 1;
                palette = vec![[0, 0, 0]; 256];
                for index in 0..size {
                    let value = usize::from(r.u16()?);
                    let color = r.rgb()?;
                    let slot = if value < 256 { value } else { index };
                    if let Some(entry) = palette.get_mut(slot) {
                        *entry = [color.0, color.1, color.2];
                    }
                }
            }
        }
        let source = r.rect()?;
        let target = r.rect()?;
        let mode = r.u16()?;
        if matches!(op, 0x91 | 0x99 | 0x9B) {
            let size = usize::from(r.u16()?);
            r.skip(size.checked_sub(2)?)?;
        }
        let stride = usize::from(row_bytes & 0x3FFF);
        let width = (bounds[2] - bounds[0]).max(0.0) as usize;
        let height = (bounds[3] - bounds[1]).max(0.0) as usize;
        if width == 0 || height == 0 || (width as u64 * height as u64) > lj_image::MAX_PIXELS {
            return None;
        }
        let packed = !(op == 0x90 || op == 0x91 || stride < 8 || pack_type == 1 || pack_type == 2);
        let row_len = match (direct, pack_type) {
            (true, 2) => width * 3,
            (true, 4) => width * usize::from(components),
            _ => stride,
        };
        let needed = match (direct, pixel_size) {
            (true, 32) if pack_type == 4 => 1,
            (true, 32) if pack_type == 2 => width * 3,
            (true, 32) => width * 4,
            (true, 16) => width * 2,
            (false, 1 | 2 | 4 | 8) => (width * usize::from(pixel_size)).div_ceil(8),
            _ => return None,
        };
        if row_len < needed || row_len as u64 * height as u64 > 4 * lj_image::MAX_PIXELS {
            return None;
        }
        let mut rows = Vec::with_capacity(row_len * height);
        for _ in 0..height {
            if packed {
                let count = match stride > 250 {
                    true => usize::from(r.u16()?),
                    false => usize::from(r.u8()?),
                };
                rows.extend(lj_image::packbits::decode(r.bytes(count)?, row_len));
                rows.resize(rows.len().next_multiple_of(row_len), 0);
            } else {
                rows.extend_from_slice(r.bytes(row_len)?);
            }
        }
        let transparent = mode == 36 || (pixel_size == 1 && mode & 3 == 1);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for row in rows.chunks_exact(row_len) {
            for x in 0..width {
                let (rgb, index) = match (direct, pixel_size) {
                    (true, 32) if pack_type == 4 => {
                        let plane = |c: usize| row.get(c * width + x).copied().unwrap_or(0);
                        let skip = usize::from(components.saturating_sub(3));
                        ([plane(skip), plane(skip + 1), plane(skip + 2)], None)
                    }
                    (true, 32) if pack_type == 2 => {
                        let at = x * 3;
                        ([row[at], row[at + 1], row[at + 2]], None)
                    }
                    (true, 32) => {
                        let at = x * 4;
                        ([row[at + 1], row[at + 2], row[at + 3]], None)
                    }
                    (true, 16) => {
                        let v = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]);
                        let five = |s: u16| (((v >> s) & 31) * 255 / 31) as u8;
                        ([five(10), five(5), five(0)], None)
                    }
                    (true, _) => unreachable!("pixel size checked with the row length"),
                    (false, bits) => {
                        let bits = usize::from(bits);
                        let bit = x * bits;
                        let index = (row[bit / 8] >> (8 - bits - bit % 8)) & (0xFF >> (8 - bits));
                        let rgb = match pixmap {
                            true => palette[usize::from(index)],
                            false if index == 1 => [self.fg.0, self.fg.1, self.fg.2],
                            false => [self.bg.0, self.bg.1, self.bg.2],
                        };
                        (rgb, Some(index))
                    }
                };
                let clear = transparent
                    && match (pixmap, index) {
                        (false, Some(index)) => index == 0,
                        _ => rgb == [self.bg.0, self.bg.1, self.bg.2],
                    };
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], if clear { 0 } else { 255 }]);
            }
        }
        let crop = [
            ((source[0] - bounds[0]).max(0.0) as usize).min(width),
            ((source[1] - bounds[1]).max(0.0) as usize).min(height),
            ((source[2] - bounds[0]).max(0.0) as usize).min(width),
            ((source[3] - bounds[1]).max(0.0) as usize).min(height),
        ];
        let (w, h) = (
            crop[2].saturating_sub(crop[0]),
            crop[3].saturating_sub(crop[1]),
        );
        if w == 0 || h == 0 {
            return Some(());
        }
        let mut shown = Vec::with_capacity(w * h * 4);
        for y in crop[1]..crop[3] {
            shown.extend_from_slice(&rgba[(y * width + crop[0]) * 4..(y * width + crop[2]) * 4]);
        }
        let kind = match shown.as_chunks::<4>().0.iter().all(|px| px[3] == 255) {
            true => {
                shown = shown
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|px| [px[0], px[1], px[2]])
                    .collect();
                Kind::Rgb
            }
            false => Kind::Rgba,
        };
        let [l, t, rt, b] = target;
        self.ops.push(Op::Bitmap(Bitmap {
            rect: [l, t, rt - l, b - t],
            png: Arc::from(png::encode(w as u32, h as u32, 8, kind, &shown)),
        }));
        Some(())
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Rect,
    RoundRect,
    Oval,
    /// Start and extent in degrees, clockwise from twelve o'clock.
    Arc(f64, f64),
    Poly,
}

fn wedge(rect: [f64; 4], start: f64, extent: f64) -> Vec<[f64; 2]> {
    let [l, t, r, b] = rect;
    let (cx, cy, rx, ry) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
    let steps = ((extent.abs() / 5.0).ceil() as usize).max(2);
    let mut out = vec![[cx, cy]];
    out.extend((0..=steps).map(|k| {
        let a = (start + extent * k as f64 / steps as f64).to_radians();
        [cx + rx * a.sin(), cy - ry * a.cos()]
    }));
    out
}

/// A wedge's curved edge, without the centre.
fn arc_edge(wedge: &[[f64; 2]]) -> Vec<[f64; 2]> {
    wedge.get(1..).unwrap_or_default().to_vec()
}

fn pattern(bytes: &[u8]) -> f64 {
    f64::from(bytes.iter().map(|b| b.count_ones()).sum::<u32>()) / 64.0
}

/// A pixel pattern's density, from the bit pattern it carries for old
/// devices; its pixels are skipped.
fn pixpat(r: &mut Be<'_>) -> Option<f64> {
    let kind = r.u16()?;
    let density = pattern(r.bytes(8)?);
    match kind {
        1 => {
            let row_bytes = usize::from(r.u16()? & 0x3FFF);
            let bounds = r.rect()?;
            r.skip(2)?;
            let pack_type = r.u16()?;
            r.skip(32)?;
            r.skip(6)?;
            let size = usize::from(r.u16()?) + 1;
            r.skip(size * 8)?;
            let height = (bounds[3] - bounds[1]).max(0.0) as usize;
            for _ in 0..height {
                if row_bytes < 8 || pack_type == 1 {
                    r.skip(row_bytes)?;
                } else {
                    let count = match row_bytes > 250 {
                        true => usize::from(r.u16()?),
                        false => usize::from(r.u8()?),
                    };
                    r.skip(count)?;
                }
            }
        }
        2 => r.skip(6)?,
        _ => {}
    }
    Some(density)
}

/// The eight colours of the original QuickDraw.
fn classic(code: u32) -> Color {
    match code {
        30 => Color::WHITE,
        69 => Color(255, 255, 0),
        137 => Color(255, 0, 255),
        205 => Color(255, 0, 0),
        273 => Color(0, 255, 255),
        341 => Color(0, 255, 0),
        409 => Color(0, 0, 255),
        _ => Color::BLACK,
    }
}

fn classic_font(id: u16) -> &'static str {
    match id {
        2 | 20 => "Times",
        4 | 22 => "Courier",
        23 => "Symbol",
        _ => "Helvetica",
    }
}
