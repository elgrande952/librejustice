//! Windows metafiles, 16-bit records, a placeable header giving the frame.

use super::bitmap::{mean_of_packed_dib, png_from_packed_dib};
use super::gdi::{Brush, Font, Gdi, MapMode, Object, Pen};
use super::{ArcKind, Metafile, Reader};
use crate::model::Color;

const SRCCOPY: u32 = 0x00CC_0020;

pub(super) fn read(data: &[u8]) -> Option<Metafile> {
    let mut r = Reader::new(data);
    let mut bounds = None;
    if r.u32()? == 0x9AC6_CDD7 {
        r.u16()?;
        let (l, t, rt, b) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?);
        bounds = Some([l, t, rt, b].map(f64::from));
        r.at = 22;
    } else {
        r.at = 0;
    }
    let header = r.u16()?;
    let header_words = r.u16()?;
    if header > 2 || header_words != 9 {
        return None;
    }
    r.skip(14)?;
    let records = r.rest();
    let bounds = bounds.or_else(|| window(records))?;
    let [l, t, rt, b] = bounds;
    let mut gdi = Gdi::new([1.0, 1.0]);
    gdi.set_map_mode(MapMode::Anisotropic);
    gdi.set_window_org([l, t]);
    gdi.set_window_ext([rt - l, b - t]);
    gdi.set_viewport_org([l, t]);
    gdi.set_viewport_ext([rt - l, b - t]);
    let mut r = Reader::new(records);
    while let (Some(size), Some(function)) = (r.u32(), r.u16()) {
        let len = (size as usize).saturating_mul(2).saturating_sub(6);
        let Some(body) = r.bytes(len.min(r.rest().len())) else {
            break;
        };
        if function == 0 {
            break;
        }
        let _ = record(&mut gdi, function, &mut Reader::new(body), bounds);
    }
    Some(Metafile {
        view: [l.min(rt), t.min(b), (rt - l).abs(), (b - t).abs()],
        ops: gdi.ops,
    })
}

/// The window the first records set, the frame of a file without a
/// placeable header.
fn window(records: &[u8]) -> Option<[f64; 4]> {
    let mut r = Reader::new(records);
    let (mut org, mut ext) = ([0.0, 0.0], None);
    while let (Some(size), Some(function)) = (r.u32(), r.u16()) {
        let body = r.bytes((size as usize).saturating_mul(2).saturating_sub(6))?;
        let mut b = Reader::new(body);
        match function {
            0x020B => org = yx(&mut b)?,
            0x020C => ext = Some(yx(&mut b)?),
            0 => break,
            _ if ext.is_some() => break,
            _ => {}
        }
    }
    let [w, h] = ext?;
    Some([org[0], org[1], org[0] + w, org[1] + h])
}

fn colorref(r: &mut Reader<'_>) -> Option<Color> {
    let [red, g, b, _] = r.u32()?.to_le_bytes();
    Some(Color(red, g, b))
}

fn yx(r: &mut Reader<'_>) -> Option<[f64; 2]> {
    let (y, x) = (r.i16()?, r.i16()?);
    Some([f64::from(x), f64::from(y)])
}

fn points(r: &mut Reader<'_>, n: usize) -> Option<Vec<[f64; 2]>> {
    (0..n)
        .map(|_| Some([f64::from(r.i16()?), f64::from(r.i16()?)]))
        .collect()
}

/// Stored bottom, right, top, left; given left, top, right, bottom.
fn rect(r: &mut Reader<'_>) -> Option<[f64; 4]> {
    let (b, rt, t, l) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?);
    Some([l, t, rt, b].map(f64::from))
}

fn string(bytes: &[u8]) -> String {
    encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned()
}

fn record(gdi: &mut Gdi, function: u16, r: &mut Reader<'_>, bounds: [f64; 4]) -> Option<()> {
    match function {
        0x0201 => gdi.set_background(colorref(r)?),
        0x0103 => gdi.set_map_mode(MapMode::from_code(u32::from(r.u16()?))),
        0x0106 => gdi.set_winding(r.u16()? == 2),
        0x0209 => gdi.set_text_color(colorref(r)?),
        0x020B => gdi.set_window_org(yx(r)?),
        0x020C => {
            gdi.set_window_ext(yx(r)?);
            let [l, t, rt, b] = bounds;
            gdi.set_viewport_org([l, t]);
            gdi.set_viewport_ext([rt - l, b - t]);
        }
        0x020F => {
            let [dx, dy] = yx(r)?;
            let (org, _) = gdi.window();
            gdi.set_window_org([org[0] + dx, org[1] + dy]);
        }
        0x0410 => {
            let (yd, yn, xd, xn) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?);
            let (_, ext) = gdi.window();
            let ratio = |n: i16, d: i16| {
                if d == 0 {
                    1.0
                } else {
                    f64::from(n) / f64::from(d)
                }
            };
            gdi.set_window_ext([ext[0] * ratio(xn, xd), ext[1] * ratio(yn, yd)]);
        }
        0x0213 => gdi.line_to(yx(r)?),
        0x0214 => gdi.move_to(yx(r)?),
        0x0817 | 0x081A | 0x0830 => {
            let (end, start) = (yx(r)?, yx(r)?);
            let bounds = rect(r)?;
            let kind = match function {
                0x0817 => ArcKind::Arc,
                0x081A => ArcKind::Pie,
                _ => ArcKind::Chord,
            };
            gdi.arc(bounds, start, end, kind);
        }
        0x0418 => gdi.ellipse(rect(r)?),
        0x041B => gdi.rectangle(rect(r)?, [0.0, 0.0]),
        0x061C => {
            let corner = yx(r)?;
            gdi.rectangle(rect(r)?, corner);
        }
        0x061D => {
            let rop = r.u32()?;
            let (h, w) = (f64::from(r.i16()?), f64::from(r.i16()?));
            let [x, y] = yx(r)?;
            gdi.blit([x, y, x + w, y + h], rop);
        }
        0x001E => gdi.save(),
        0x0127 => gdi.restore(i32::from(r.i16()?)),
        0x0521 => {
            let n = usize::try_from(r.i16()?).ok()?;
            let bytes = r.bytes(n)?;
            r.skip(n % 2)?;
            let at = yx(r)?;
            gdi.text(at, &string(bytes), None, None);
        }
        0x0A32 => {
            let at = yx(r)?;
            let n = usize::try_from(r.i16()?).ok()?;
            let options = r.u16()?;
            let clip = if options & 6 != 0 {
                let (l, t, rt, b) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?);
                Some([l, t, rt, b].map(f64::from))
            } else {
                None
            };
            let bytes = r.bytes(n)?;
            r.skip(n % 2);
            let dx: Option<Vec<f64>> = (0..n).map(|_| r.i16().map(f64::from)).collect();
            let opaque = clip.filter(|_| options & 2 != 0);
            gdi.text(at, &string(bytes), dx.as_deref(), opaque);
        }
        0x0324 | 0x0325 => {
            let n = usize::try_from(r.i16()?).ok()?;
            let pts = points(r, n)?;
            if function == 0x0324 {
                gdi.polygons(&[pts]);
            } else {
                gdi.polyline(&pts, false);
            }
        }
        0x0538 => {
            let n = usize::from(r.u16()?);
            let counts: Vec<usize> = (0..n)
                .map(|_| r.u16().map(usize::from))
                .collect::<Option<_>>()?;
            let rings: Vec<Vec<[f64; 2]>> = counts
                .iter()
                .map(|&c| points(r, c))
                .collect::<Option<_>>()?;
            gdi.polygons(&rings);
        }
        0x012D => gdi.select(usize::from(r.u16()?)),
        0x012E => gdi.set_align(u32::from(r.u16()?)),
        0x01F0 => gdi.delete(usize::from(r.u16()?)),
        0x02FA => {
            let style = r.u16()?;
            let width = r.i16()?;
            r.i16()?;
            let color = colorref(r)?;
            gdi.create(Object::Pen(Pen {
                color,
                width: (style & 0xF != 5).then_some(f64::from(width.max(0))),
            }));
        }
        0x02FB => {
            let height = r.i16()?;
            r.skip(2)?;
            let escapement = r.i16()?;
            r.skip(2)?;
            let weight = r.i16()?;
            let italic = r.u8()? != 0;
            r.skip(7)?;
            let name = r.rest();
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            gdi.create(Object::Font(Font {
                family: string(&name[..end]),
                height: f64::from(height),
                bold: weight >= 600,
                italic,
                rotated: escapement != 0,
            }));
        }
        0x02FC => {
            let style = r.u16()?;
            let color = colorref(r)?;
            gdi.create(Object::Brush(Brush {
                color: (style == 0).then_some(color),
            }));
        }
        0x00F7 | 0x01F9 | 0x06FF => gdi.create(Object::Other),
        0x0142 => {
            r.skip(4)?;
            gdi.create(Object::Brush(Brush {
                color: mean_of_packed_dib(r.rest()),
            }));
        }
        0x0B41 | 0x0F43 => {
            let rop = r.u32()?;
            if function == 0x0F43 {
                r.u16()?;
            }
            r.skip(8)?;
            let (h, w) = (f64::from(r.i16()?), f64::from(r.i16()?));
            let [x, y] = yx(r)?;
            if r.rest().len() < 40 {
                gdi.blit([x, y, x + w, y + h], rop);
            } else if rop == SRCCOPY {
                if let Some(png) = png_from_packed_dib(r.rest()) {
                    gdi.bitmap([x, y, w, h], png);
                }
            }
        }
        0x0940 => {
            let rop = r.u32()?;
            let bare = r.rest().len() < 52;
            r.skip(if bare { 6 } else { 4 })?;
            let (h, w) = (f64::from(r.i16()?), f64::from(r.i16()?));
            let [x, y] = yx(r)?;
            if bare {
                gdi.blit([x, y, x + w, y + h], rop);
            } else if rop == SRCCOPY {
                if let Some(png) = png_from_packed_dib(r.rest()) {
                    gdi.bitmap([x, y, w, h], png);
                }
            }
        }
        _ => {}
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metafile::Op;

    fn record(function: u16, params: &[i16]) -> Vec<u8> {
        let mut out = ((3 + params.len()) as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&function.to_le_bytes());
        for p in params {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out
    }

    #[test]
    fn a_placeable_rectangle_fills_its_frame_with_the_brush() {
        let mut data = vec![0xD7, 0xCD, 0xC6, 0x9A, 0, 0];
        for v in [0i16, 0, 1000, 500] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.extend_from_slice(&[0; 8]);
        data.extend_from_slice(&[1, 0, 9, 0, 0, 3]);
        data.extend_from_slice(&[0; 12]);
        data.extend(record(0x02FC, &[0, 0x00FF, 0, 0]));
        data.extend(record(0x012D, &[0]));
        data.extend(record(0x041B, &[500, 1000, 0, 0]));
        data.extend(record(0, &[]));
        let meta = read(&data).unwrap();
        assert_eq!(meta.view, [0.0, 0.0, 1000.0, 500.0]);
        let [Op::Path(path)] = &meta.ops[..] else {
            panic!("one path");
        };
        assert_eq!(path.fill, Some((Color(255, 0, 0), 1.0)));
        assert_eq!(path.rings[0][2], [1000.0, 500.0]);
    }
}
