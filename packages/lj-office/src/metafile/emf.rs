//! Enhanced metafiles, 32-bit records, their frame in hundredths of a
//! millimetre on a reference device.

use super::bitmap::{mean_of_packed_dib, png_from_packed_dib};
use super::gdi::{Brush, Font, Gdi, MapMode, Object, Pen};
use super::{ArcKind, Metafile, Reader};
use crate::model::Color;

const SRCCOPY: u32 = 0x00CC_0020;
const SRCPAINT: u32 = 0x00EE_0086;
const STOCK: u32 = 0x8000_0000;

pub(super) fn read(data: &[u8]) -> Option<Metafile> {
    let mut r = Reader::new(data);
    if r.u32()? != 1 {
        return None;
    }
    let size = r.u32()? as usize;
    r.skip(16)?;
    let frame = [r.i32()?, r.i32()?, r.i32()?, r.i32()?].map(f64::from);
    r.skip(32)?;
    let device = [f64::from(r.i32()?), f64::from(r.i32()?)];
    let millimetres = [f64::from(r.i32()?), f64::from(r.i32()?)];
    if millimetres[0] <= 0.0 || millimetres[1] <= 0.0 {
        return None;
    }
    let per_mm = [device[0] / millimetres[0], device[1] / millimetres[1]];
    let [l, t, rt, b] = [
        frame[0] / 100.0 * per_mm[0],
        frame[1] / 100.0 * per_mm[1],
        frame[2] / 100.0 * per_mm[0],
        frame[3] / 100.0 * per_mm[1],
    ];
    let mut gdi = Gdi::new(per_mm);
    let mut r = Reader::new(data);
    r.skip(size)?;
    while let (Some(kind), Some(len)) = (r.u32(), r.u32()) {
        let Some(body) = r.bytes((len as usize).saturating_sub(8).min(r.rest().len())) else {
            break;
        };
        if kind == 14 || len < 8 {
            break;
        }
        let _ = record(&mut gdi, kind, &mut Reader::new(body), body);
    }
    Some(Metafile {
        view: [l, t, rt - l, b - t],
        ops: gdi.ops,
    })
}

fn colorref(r: &mut Reader<'_>) -> Option<Color> {
    let [red, g, b, _] = r.u32()?.to_le_bytes();
    Some(Color(red, g, b))
}

fn point(r: &mut Reader<'_>) -> Option<[f64; 2]> {
    Some([f64::from(r.i32()?), f64::from(r.i32()?)])
}

fn point16(r: &mut Reader<'_>) -> Option<[f64; 2]> {
    Some([f64::from(r.i16()?), f64::from(r.i16()?)])
}

fn rect(r: &mut Reader<'_>) -> Option<[f64; 4]> {
    Some([r.i32()?, r.i32()?, r.i32()?, r.i32()?].map(f64::from))
}

fn f32(r: &mut Reader<'_>) -> Option<f64> {
    Some(f64::from(f32::from_bits(r.u32()?)))
}

fn xform(r: &mut Reader<'_>) -> Option<[f64; 6]> {
    Some([f32(r)?, f32(r)?, f32(r)?, f32(r)?, f32(r)?, f32(r)?])
}

/// The points of a poly record after its bounds, 16-bit when `short`.
fn points(r: &mut Reader<'_>, short: bool) -> Option<Vec<[f64; 2]>> {
    r.skip(16)?;
    let n = r.u32()? as usize;
    read_points(r, n, short)
}

fn read_points(r: &mut Reader<'_>, n: usize, short: bool) -> Option<Vec<[f64; 2]>> {
    if n.checked_mul(if short { 4 } else { 8 })? > r.rest().len() {
        return None;
    }
    (0..n)
        .map(|_| if short { point16(r) } else { point(r) })
        .collect()
}

fn poly_points(r: &mut Reader<'_>, short: bool) -> Option<Vec<Vec<[f64; 2]>>> {
    r.skip(16)?;
    let n = r.u32()? as usize;
    r.u32()?;
    if n.checked_mul(4)? > r.rest().len() {
        return None;
    }
    let counts: Vec<usize> = (0..n)
        .map(|_| r.u32().map(|c| c as usize))
        .collect::<Option<_>>()?;
    counts.iter().map(|&c| read_points(r, c, short)).collect()
}

/// A packed DIB from the header and bits a bitmap record points at.
fn dib(body: &[u8], header: (u32, u32), bits: (u32, u32)) -> Option<Vec<u8>> {
    let at = |offset: u32| (offset as usize).checked_sub(8);
    let span = |(offset, len): (u32, u32)| {
        let start = at(offset)?;
        body.get(start..start.checked_add(len as usize)?)
    };
    let (head, pixels) = (span(header)?, span(bits)?);
    let mut out = head.to_vec();
    out.extend_from_slice(pixels);
    Some(out)
}

fn offsets(r: &mut Reader<'_>) -> Option<((u32, u32), (u32, u32))> {
    Some(((r.u32()?, r.u32()?), (r.u32()?, r.u32()?)))
}

fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    String::from_utf16_lossy(&units)
}

fn record(gdi: &mut Gdi, kind: u32, r: &mut Reader<'_>, body: &[u8]) -> Option<()> {
    match kind {
        2 | 85 => gdi.polyline(&points(r, kind == 85)?, true),
        3 | 86 => {
            let pts = points(r, kind == 86)?;
            gdi.polygons(&[pts]);
        }
        4 | 87 => gdi.polyline(&points(r, kind == 87)?, false),
        5 | 88 => gdi.poly_to(&points(r, kind == 88)?, true),
        6 | 89 => gdi.poly_to(&points(r, kind == 89)?, false),
        7 | 90 => {
            for figure in poly_points(r, kind == 90)? {
                gdi.polyline(&figure, false);
            }
        }
        8 | 91 => gdi.polygons(&poly_points(r, kind == 91)?),
        9 => gdi.set_window_ext(point(r)?),
        10 => gdi.set_window_org(point(r)?),
        11 => gdi.set_viewport_ext(point(r)?),
        12 => gdi.set_viewport_org(point(r)?),
        17 => gdi.set_map_mode(MapMode::from_code(r.u32()?)),
        19 => gdi.set_winding(r.u32()? == 2),
        22 => gdi.set_align(r.u32()?),
        24 => gdi.set_text_color(colorref(r)?),
        25 => gdi.set_background(colorref(r)?),
        27 => gdi.move_to(point(r)?),
        31 | 32 => {
            let (xn, xd, yn, yd) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
            let ratio = |n: i32, d: i32| {
                if d == 0 {
                    1.0
                } else {
                    f64::from(n) / f64::from(d)
                }
            };
            let (x, y) = (ratio(xn, xd), ratio(yn, yd));
            if kind == 31 {
                let (_, ext) = gdi.viewport();
                gdi.set_viewport_ext([ext[0] * x, ext[1] * y]);
            } else {
                let (_, ext) = gdi.window();
                gdi.set_window_ext([ext[0] * x, ext[1] * y]);
            }
        }
        33 => gdi.save(),
        34 => gdi.restore(r.i32()?),
        35 => gdi.set_world(xform(r)?),
        36 => {
            let m = xform(r)?;
            match r.u32()? {
                1 => gdi.set_world([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
                2 => gdi.modify_world(m, true),
                3 => gdi.modify_world(m, false),
                4 => gdi.set_world(m),
                _ => {}
            }
        }
        37 => {
            let index = r.u32()?;
            if index & STOCK != 0 {
                gdi.select_stock(index & !STOCK);
            } else {
                gdi.select(index as usize);
            }
        }
        38 => {
            let index = r.u32()? as usize;
            let style = r.u32()?;
            let width = r.i32()?;
            r.i32()?;
            let color = colorref(r)?;
            let width = (style & 0xF != 5).then_some(f64::from(width.max(0)));
            gdi.create_at(index, Object::Pen(Pen { color, width }));
        }
        95 => {
            let index = r.u32()? as usize;
            r.skip(16)?;
            let style = r.u32()?;
            let width = r.u32()?;
            let brush = r.u32()?;
            let color = colorref(r)?;
            let geometric = style & 0x000F_0000 != 0;
            let width = (style & 0xF != 5 && brush != 1).then_some(if geometric {
                f64::from(width)
            } else {
                0.0
            });
            gdi.create_at(index, Object::Pen(Pen { color, width }));
        }
        39 => {
            let index = r.u32()? as usize;
            let style = r.u32()?;
            let color = colorref(r)?;
            let color = (style == 0).then_some(color);
            gdi.create_at(index, Object::Brush(Brush { color }));
        }
        93 | 94 => {
            let index = r.u32()? as usize;
            r.u32()?;
            let (header, bits) = offsets(r)?;
            let color = dib(body, header, bits).and_then(|d| mean_of_packed_dib(&d));
            gdi.create_at(index, Object::Brush(Brush { color }));
        }
        82 => {
            let index = r.u32()? as usize;
            let height = r.i32()?;
            r.i32()?;
            let escapement = r.i32()?;
            r.i32()?;
            let weight = r.i32()?;
            let italic = r.u8()? != 0;
            r.skip(7)?;
            let name = utf16(r.bytes(64)?);
            let font = Font {
                family: name.trim_end_matches('\0').to_string(),
                height: f64::from(height),
                bold: weight >= 600,
                italic,
                rotated: escapement != 0,
            };
            gdi.create_at(index, Object::Font(font));
        }
        49 => gdi.create_at(r.u32()? as usize, Object::Other),
        40 => gdi.delete(r.u32()? as usize),
        42 => gdi.ellipse(rect(r)?),
        43 => gdi.rectangle(rect(r)?, [0.0, 0.0]),
        44 => {
            let bounds = rect(r)?;
            gdi.rectangle(bounds, point(r)?);
        }
        45..=47 => {
            let (bounds, start, end) = (rect(r)?, point(r)?, point(r)?);
            let kind = [ArcKind::Arc, ArcKind::Chord, ArcKind::Pie][(kind - 45) as usize];
            gdi.arc(bounds, start, end, kind);
        }
        54 => gdi.line_to(point(r)?),
        59 => gdi.begin_path(),
        61 => gdi.close_figure(),
        62..=64 => {
            if let Some(figures) = gdi.end_path() {
                gdi.paint(figures, kind != 64, kind != 62);
            }
        }
        68 => {
            gdi.end_path();
        }
        76 | 77 | 114 => {
            r.skip(16)?;
            let (x, y) = (f64::from(r.i32()?), f64::from(r.i32()?));
            let (w, h) = (f64::from(r.i32()?), f64::from(r.i32()?));
            let rop = r.u32()?;
            r.skip(8)?;
            xform(r)?;
            r.skip(8)?;
            let (header, bits) = offsets(r)?;
            if header.1 == 0 {
                if kind == 76 {
                    gdi.blit([x, y, x + w, y + h], rop);
                }
            } else if kind == 114 || rop == SRCCOPY {
                if let Some(png) = dib(body, header, bits).and_then(|d| png_from_packed_dib(&d)) {
                    gdi.bitmap([x, y, w, h], png);
                }
            }
        }
        80 => {
            r.skip(16)?;
            let (x, y) = (f64::from(r.i32()?), f64::from(r.i32()?));
            r.skip(8)?;
            let (w, h) = (f64::from(r.i32()?), f64::from(r.i32()?));
            let (header, bits) = offsets(r)?;
            if let Some(png) = dib(body, header, bits).and_then(|d| png_from_packed_dib(&d)) {
                gdi.bitmap([x, y, w, h], png);
            }
        }
        81 => {
            r.skip(16)?;
            let (x, y) = (f64::from(r.i32()?), f64::from(r.i32()?));
            r.skip(16)?;
            let (header, bits) = offsets(r)?;
            r.u32()?;
            let rop = r.u32()?;
            let (w, h) = (f64::from(r.i32()?), f64::from(r.i32()?));
            if rop == SRCCOPY || rop == SRCPAINT {
                if let Some(png) = dib(body, header, bits).and_then(|d| png_from_packed_dib(&d)) {
                    gdi.bitmap([x, y, w, h], png);
                }
            }
        }
        83 | 84 => {
            r.skip(28)?;
            let at = point(r)?;
            let n = r.u32()? as usize;
            let offset = (r.u32()? as usize).checked_sub(8)?;
            let options = r.u32()?;
            let clip = rect(r)?;
            let dx_offset = r.u32()? as usize;
            let text = if kind == 84 {
                utf16(body.get(offset..offset.checked_add(n.checked_mul(2)?)?)?)
            } else {
                let bytes = body.get(offset..offset.checked_add(n)?)?;
                encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned()
            };
            let step = if options & 0x2000 != 0 { 8 } else { 4 };
            let dx: Option<Vec<f64>> = dx_offset.checked_sub(8).and_then(|at| {
                (0..n)
                    .map(|i| {
                        let start = at.checked_add(i.checked_mul(step)?)?;
                        let b = body.get(start..start.checked_add(4)?)?;
                        Some(f64::from(i32::from_le_bytes(b.try_into().ok()?)))
                    })
                    .collect()
            });
            let opaque = (options & 2 != 0).then_some(clip);
            gdi.text(at, &text, dx.as_deref(), opaque);
        }
        _ => {}
    }
    Some(())
}
