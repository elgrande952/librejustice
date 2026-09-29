//! A metafile's paths, text and bitmaps scaled into its frame.

use std::sync::Arc;

use super::flow::stack;
use super::{Env, Fields, Glyphs, Image, Item, Path};
use crate::metafile::{self, HAlign, Metafile, Op, TextAlign};
use crate::model::*;

/// The width a device's thinnest line takes on a page.
const HAIRLINE: f64 = 0.5;

/// Closer on the page than this, a point adds nothing to its neighbour.
const POINT_GAP: f64 = 0.25;

/// A picture's metafile drawn into a frame of `width` by `height` at the
/// origin, laid out once per frame size.
pub(super) fn items(
    env: &Env<'_>,
    fields: Fields,
    picture: &Picture,
    width: f64,
    height: f64,
) -> Option<Arc<[Item]>> {
    let key = (picture.data.as_ptr(), width.to_bits(), height.to_bits());
    if let Some(items) = env.metafiles.borrow().get(&key) {
        return Some(items.clone());
    }
    let meta = metafile::read(&picture.data, picture.format)?;
    let mut out = Vec::new();
    draw(env, fields, &meta, [0.0, 0.0, width, height], &mut out);
    let items: Arc<[Item]> = Arc::from(out);
    env.metafiles.borrow_mut().insert(key, items.clone());
    Some(items)
}

fn draw(env: &Env<'_>, fields: Fields, meta: &Metafile, frame: [f64; 4], out: &mut Vec<Item>) {
    let [vx, vy, vw, vh] = meta.view;
    let [x, y, width, height] = frame;
    if vw == 0.0 || vh == 0.0 {
        return;
    }
    let (sx, sy) = (width / vw, height / vh);
    let map = |p: [f64; 2]| [x + (p[0] - vx) * sx, y + (p[1] - vy) * sy];
    let scale = (sx * sy).abs().sqrt();
    let bounds = [
        x - EPSILON,
        y - EPSILON,
        x + width + EPSILON,
        y + height + EPSILON,
    ];
    for op in &meta.ops {
        match op {
            Op::Path(path) => clip_path(
                Path {
                    rings: path
                        .rings
                        .iter()
                        .map(|ring| thin(ring.iter().map(|&p| map(p))))
                        .collect(),
                    closed: path.closed,
                    even_odd: path.even_odd,
                    fill: path.fill.map(|(color, alpha)| Paint { color, alpha }),
                    stroke: path.stroke.map(|(color, w)| Stroke {
                        width: if w > 0.0 { w * scale } else { HAIRLINE },
                        paint: Paint { color, alpha: 1.0 },
                    }),
                },
                bounds,
                out,
            ),
            Op::Bitmap(bitmap) => {
                let [l, t, w, h] = bitmap.rect;
                let [x0, y0] = map([l, t]);
                let [x1, y1] = map([l + w, t + h]);
                let (left, top, w, h) = (x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs());
                let shown = [
                    left.max(bounds[0]),
                    top.max(bounds[1]),
                    (left + w).min(bounds[2]),
                    (top + h).min(bounds[3]),
                ];
                if w <= 0.0 || h <= 0.0 || shown[0] >= shown[2] || shown[1] >= shown[3] {
                    continue;
                }
                out.push(Item::Image(Box::new(Image {
                    x: shown[0],
                    y: shown[1],
                    width: shown[2] - shown[0],
                    height: shown[3] - shown[1],
                    data: bitmap.png.clone(),
                    mime: match bitmap.png.starts_with(b"\xFF\xD8") {
                        true => "image/jpeg",
                        false => "image/png",
                    },
                    crop: Margins {
                        left: (shown[0] - left) / w,
                        top: (shown[1] - top) / h,
                        right: (left + w - shown[2]) / w,
                        bottom: (top + h - shown[3]) / h,
                    },
                    transform: Transform::default(),
                    recolor: Recolor::default(),
                })));
            }
            Op::Text(text) => {
                let size = text.size * sy.abs();
                if size >= 0.5 {
                    let mut items = Vec::new();
                    line(env, fields, text, map(text.at), size, sx.abs(), &mut items);
                    out.extend(items.into_iter().filter(|item| match item {
                        Item::Glyphs(g) => {
                            let end = g.x + g.advances.iter().sum::<f64>();
                            g.x < bounds[2]
                                && end > bounds[0]
                                && g.y - 0.8 * g.size < bounds[3]
                                && g.y + 0.2 * g.size > bounds[1]
                        }
                        _ => true,
                    }));
                }
            }
        }
    }
}

/// How far past its frame a drawing may reach before it is cut, for the
/// rounding of its scale.
const EPSILON: f64 = 0.01;

/// `path` cut to `bounds` (left, top, right, bottom): its fill as polygons,
/// its outline as the pieces of line inside.
fn clip_path(path: Path, bounds: [f64; 4], out: &mut Vec<Item>) {
    let points = || path.rings.iter().flatten();
    let inside = |p: &&[f64; 2]| {
        p[0] >= bounds[0] && p[0] <= bounds[2] && p[1] >= bounds[1] && p[1] <= bounds[3]
    };
    if points().all(|p| inside(&p)) {
        out.push(Item::Path(Box::new(path)));
        return;
    }
    if path.closed && path.fill.is_some() {
        let rings: Vec<_> = path
            .rings
            .iter()
            .map(|ring| clip_polygon(ring, bounds))
            .filter(|ring| ring.len() >= 3)
            .collect();
        if !rings.is_empty() {
            out.push(Item::Path(Box::new(Path {
                rings,
                stroke: None,
                ..path.clone()
            })));
        }
    }
    if path.stroke.is_some() {
        let mut lines = Vec::new();
        for ring in &path.rings {
            let mut points = ring.clone();
            if path.closed && ring.len() > 2 {
                points.push(ring[0]);
            }
            clip_polyline(&points, bounds, &mut lines);
        }
        if !lines.is_empty() {
            out.push(Item::Path(Box::new(Path {
                rings: lines,
                closed: false,
                even_odd: false,
                fill: None,
                stroke: path.stroke,
            })));
        }
    }
}

/// Sutherland-Hodgman: the polygon `ring` cut by each side of `bounds`.
fn clip_polygon(ring: &[[f64; 2]], bounds: [f64; 4]) -> Vec<[f64; 2]> {
    let mut polygon = ring.to_vec();
    for (axis, limit, low) in [
        (0, bounds[0], true),
        (1, bounds[1], true),
        (0, bounds[2], false),
        (1, bounds[3], false),
    ] {
        let inside = |p: [f64; 2]| {
            if low {
                p[axis] >= limit
            } else {
                p[axis] <= limit
            }
        };
        let cross = |p: [f64; 2], q: [f64; 2]| {
            let t = (limit - p[axis]) / (q[axis] - p[axis]);
            [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]
        };
        let input = std::mem::take(&mut polygon);
        let Some(&last) = input.last() else {
            break;
        };
        let mut previous = last;
        for &p in &input {
            match (inside(previous), inside(p)) {
                (true, true) => polygon.push(p),
                (true, false) => polygon.push(cross(previous, p)),
                (false, true) => polygon.extend([cross(previous, p), p]),
                (false, false) => {}
            }
            previous = p;
        }
    }
    polygon
}

/// The pieces of the polyline `points` inside `bounds`.
fn clip_polyline(points: &[[f64; 2]], bounds: [f64; 4], out: &mut Vec<Vec<[f64; 2]>>) {
    let mut current = Vec::new();
    let mut flush = |current: &mut Vec<[f64; 2]>| {
        if current.len() >= 2 {
            out.push(std::mem::take(current));
        }
        current.clear();
    };
    for pair in points.windows(2) {
        let Some((p, q)) = clip_segment(pair[0], pair[1], bounds) else {
            flush(&mut current);
            continue;
        };
        if current.is_empty() || p != pair[0] {
            flush(&mut current);
            current.push(p);
        }
        current.push(q);
        if q != pair[1] {
            flush(&mut current);
        }
    }
    flush(&mut current);
}

/// Liang-Barsky: the part of the segment from `p` to `q` inside `bounds`.
fn clip_segment(p: [f64; 2], q: [f64; 2], bounds: [f64; 4]) -> Option<([f64; 2], [f64; 2])> {
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (d, room) in [
        (-dx, p[0] - bounds[0]),
        (dx, bounds[2] - p[0]),
        (-dy, p[1] - bounds[1]),
        (dy, bounds[3] - p[1]),
    ] {
        if d == 0.0 {
            if room < 0.0 {
                return None;
            }
        } else if d < 0.0 {
            t0 = t0.max(room / d);
        } else {
            t1 = t1.min(room / d);
        }
    }
    let at = |t: f64| [p[0] + t * dx, p[1] + t * dy];
    (t0 <= t1).then(|| {
        (
            if t0 > 0.0 { at(t0) } else { p },
            if t1 < 1.0 { at(t1) } else { q },
        )
    })
}

fn line(
    env: &Env<'_>,
    fields: Fields,
    text: &metafile::Text,
    at: [f64; 2],
    size: f64,
    sx: f64,
    out: &mut Vec<Item>,
) {
    let family: Arc<str> = Arc::from(if text.family.is_empty() {
        "Liberation Sans"
    } else {
        text.family.as_str()
    });
    let props = RunProps {
        fonts: FontSet {
            ascii: family.clone(),
            h_ansi: family.clone(),
            east_asia: family.clone(),
            complex: family,
        },
        size,
        size_complex: size,
        bold: text.bold,
        italic: text.italic,
        bold_complex: text.bold,
        italic_complex: text.italic,
        color: Some(text.color),
        ..RunProps::default()
    };
    let paragraph = Paragraph {
        props: ParagraphProps::default(),
        mark: props.clone(),
        style: None,
        label: None,
        hidden_section: false,
        inlines: text
            .text
            .chars()
            .map(|c| Inline::Char(c, props.clone()))
            .collect(),
    };
    let (mut items, height) = stack(
        env,
        &[Block::Paragraph(Box::new(paragraph))],
        f64::from(u16::MAX),
        fields,
    );
    let Some(baseline) = items.iter().find_map(|item| match item {
        Item::Glyphs(g) => Some(g.y),
        _ => None,
    }) else {
        return;
    };
    let top = match text.align {
        TextAlign::Top => at[1],
        TextAlign::Baseline => at[1] - baseline,
        TextAlign::Bottom => at[1] - height,
    };
    let mut glyphs: Vec<&mut Glyphs> = items
        .iter_mut()
        .filter_map(|item| match item {
            Item::Glyphs(g) => Some(g),
            _ => None,
        })
        .collect();
    let count: usize = glyphs.iter().map(|g| g.advances.len()).sum();
    if let Some(advances) = text.advances.as_ref().filter(|a| a.len() == count) {
        let mut x = glyphs.first().map_or(0.0, |g| g.x);
        let mut given = advances.iter();
        for g in &mut glyphs {
            g.x = x;
            for (a, &b) in g.advances.iter_mut().zip(&mut given) {
                *a = b * sx;
                x += *a;
            }
        }
    }
    let start = glyphs.iter().map(|g| g.x).fold(f64::INFINITY, f64::min);
    let end = glyphs
        .iter()
        .map(|g| g.x + g.advances.iter().sum::<f64>())
        .fold(f64::NEG_INFINITY, f64::max);
    let width = (end - start).max(0.0);
    let left = match text.h_align {
        HAlign::Left => at[0],
        HAlign::Center => at[0] - width / 2.0,
        HAlign::Right => at[0] - width,
    };
    out.extend(items.into_iter().map(|mut item| {
        item.translate(left, top);
        item
    }));
}

/// A ring without the points that fall within [`POINT_GAP`] of the one kept
/// before them; its last point stays.
fn thin(points: impl Iterator<Item = [f64; 2]>) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::new();
    let mut dropped = None;
    for p in points {
        match out.last() {
            Some(q) if (p[0] - q[0]).hypot(p[1] - q[1]) < POINT_GAP => dropped = Some(p),
            _ => {
                out.push(p);
                dropped = None;
            }
        }
    }
    out.extend(dropped);
    out
}
