//! Shadings: axial and radial ones become SVG gradients, a function-based one
//! an image of its samples, a mesh the flat average of its colours.

use lj_draw::{Rgb, Segment, Stop};

use super::color::{ColorSpace, byte};
use super::function::{Bits, Function, corrupt, eval_list, interpolate, number, numbers};
use crate::graphics::Matrix;
use crate::objects::{PdfDictionary, PdfFile, PdfObject, PdfResult, PdfValue, decode_stream};

/// Samples taken along a gradient before the stops a straight line joins are
/// dropped.
const GRADIENT_SAMPLES: usize = 256;
/// One channel may stray this far, out of 255, from the line between stops.
const STOP_TOLERANCE: f64 = 1.0;
/// The side of the image a function-based shading is sampled into.
pub(crate) const FUNCTION_SAMPLES: usize = 128;

#[derive(Debug)]
pub(crate) struct Shading {
    /// `[x0, y0, x1, y1]` in the shading's space.
    pub bbox: Option<[f64; 4]>,
    pub kind: Kind,
}

#[derive(Debug)]
pub(crate) enum Kind {
    Function {
        /// From the unit square, row 0 first, to the shading's space.
        to_space: Matrix,
        /// `FUNCTION_SAMPLES` squared, RGB.
        rgb: Vec<u8>,
        average: Rgb,
    },
    Axial {
        from: [f64; 2],
        to: [f64; 2],
        extend: [bool; 2],
        stops: Vec<Stop>,
    },
    Radial {
        from: [f64; 2],
        from_radius: f64,
        to: [f64; 2],
        to_radius: f64,
        extend: [bool; 2],
        stops: Vec<Stop>,
    },
    Mesh {
        /// Every patch or triangle, in the shading's space.
        outline: Vec<Segment>,
        /// Pieces of the patches and triangles, each in its middle's colour.
        pieces: Vec<(Vec<Segment>, Rgb)>,
        average: Rgb,
    },
}

impl Shading {
    pub fn parse(file: &PdfFile, value: &PdfValue) -> PdfResult<Shading> {
        let (dict, data) = match value {
            PdfValue::Reference(object_ref) => match file.get_object(*object_ref)? {
                PdfObject::Stream(stream) => (&stream.dict, Some(decode_stream(stream)?)),
                PdfObject::Value(PdfValue::Dictionary(dict)) => (dict, None),
                PdfObject::Value(_) => return Err(corrupt("shading is not a dictionary")),
            },
            PdfValue::Dictionary(dict) => (dict, None),
            _ => return Err(corrupt("shading is not a dictionary")),
        };
        let space = ColorSpace::parse(
            file,
            dict.get("ColorSpace")
                .ok_or_else(|| corrupt("shading has no ColorSpace"))?,
            None,
        )?;
        let function = match dict.get("Function") {
            Some(value) => Some(Function::parse_list(file, value)?),
            None => None,
        };
        let bbox = numbers(file, dict.get("BBox"))
            .filter(|b| b.len() == 4)
            .map(|b| {
                [
                    b[0].min(b[2]),
                    b[1].min(b[3]),
                    b[0].max(b[2]),
                    b[1].max(b[3]),
                ]
            });
        let color = |input: &[f64]| -> [f64; 3] {
            match &function {
                Some(function) => space.to_rgb(&eval_list(function, input)),
                None => space.to_rgb(input),
            }
        };
        let kind = match dict
            .get("ShadingType")
            .and_then(|v| number(file, v))
            .map(|n| n as i64)
        {
            Some(1) => {
                if function.is_none() {
                    return Err(corrupt("function shading has no Function"));
                }
                let domain = numbers(file, dict.get("Domain"))
                    .filter(|d| d.len() == 4)
                    .unwrap_or_else(|| vec![0.0, 1.0, 0.0, 1.0]);
                let matrix = numbers(file, dict.get("Matrix"))
                    .filter(|m| m.len() == 6)
                    .map_or_else(Matrix::identity, |m| Matrix {
                        a: m[0],
                        b: m[1],
                        c: m[2],
                        d: m[3],
                        e: m[4],
                        f: m[5],
                    });
                let n = FUNCTION_SAMPLES;
                let mut rgb = Vec::with_capacity(n * n * 3);
                let mut sum = [0.0; 3];
                for row in 0..n {
                    let y = interpolate(
                        (row as f64 + 0.5) / n as f64,
                        [0.0, 1.0],
                        [domain[2], domain[3]],
                    );
                    for column in 0..n {
                        let x = interpolate(
                            (column as f64 + 0.5) / n as f64,
                            [0.0, 1.0],
                            [domain[0], domain[1]],
                        );
                        let pixel = color(&[x, y]);
                        for (total, channel) in sum.iter_mut().zip(pixel) {
                            *total += channel;
                        }
                        rgb.extend(pixel.map(byte));
                    }
                }
                let to_space = Matrix {
                    a: domain[1] - domain[0],
                    b: 0.0,
                    c: 0.0,
                    d: domain[3] - domain[2],
                    e: domain[0],
                    f: domain[2],
                }
                .multiply(matrix);
                Kind::Function {
                    to_space,
                    rgb,
                    average: rgb_of(sum.map(|s| s / (n * n) as f64)),
                }
            }
            Some(kind @ (2 | 3)) => {
                let function = function
                    .as_ref()
                    .ok_or_else(|| corrupt("axial or radial shading has no Function"))?;
                let coords = numbers(file, dict.get("Coords"))
                    .ok_or_else(|| corrupt("shading has no Coords"))?;
                let domain = numbers(file, dict.get("Domain"))
                    .filter(|d| d.len() == 2)
                    .unwrap_or_else(|| vec![0.0, 1.0]);
                let extend = match dict.get("Extend").and_then(|v| file.resolve(v).ok()) {
                    Some(PdfValue::Array(items)) if items.len() == 2 => {
                        [0, 1].map(|i| items[i].as_bool().unwrap_or(false))
                    }
                    _ => [false, false],
                };
                let stops = stops(
                    |t| space.to_rgb(&eval_list(function, &[t])),
                    [domain[0], domain[1]],
                );
                match (kind, coords.as_slice()) {
                    (2, &[x0, y0, x1, y1]) => Kind::Axial {
                        from: [x0, y0],
                        to: [x1, y1],
                        extend,
                        stops,
                    },
                    (3, &[x0, y0, r0, x1, y1, r1]) => Kind::Radial {
                        from: [x0, y0],
                        from_radius: r0.max(0.0),
                        to: [x1, y1],
                        to_radius: r1.max(0.0),
                        extend,
                        stops,
                    },
                    _ => return Err(corrupt("shading Coords")),
                }
            }
            Some(kind @ 4..=7) => {
                let data = data.ok_or_else(|| corrupt("mesh shading is not a stream"))?;
                mesh(file, dict, kind, &data, &space, function.as_deref())?
            }
            _ => return Err(corrupt("shading type")),
        };
        Ok(Shading { bbox, kind })
    }

    /// The colour a text shown as text, or anything that cannot take the
    /// shading itself, is painted with.
    pub fn representative(&self) -> Rgb {
        match &self.kind {
            Kind::Function { average, .. } | Kind::Mesh { average, .. } => *average,
            Kind::Axial { stops, .. } | Kind::Radial { stops, .. } => {
                let [r, g, b] = stops.iter().fold([0.0; 3], |sum, stop| {
                    let Rgb(r, g, b) = stop.color;
                    [
                        sum[0] + f64::from(r),
                        sum[1] + f64::from(g),
                        sum[2] + f64::from(b),
                    ]
                });
                let n = stops.len().max(1) as f64;
                Rgb(
                    (r / n).round() as u8,
                    (g / n).round() as u8,
                    (b / n).round() as u8,
                )
            }
        }
    }
}

fn rgb_of(rgb: [f64; 3]) -> Rgb {
    let [r, g, b] = rgb.map(byte);
    Rgb(r, g, b)
}

/// The colours along `domain`, fewest stops a straight line between them
/// keeps within `STOP_TOLERANCE`.
fn stops(color: impl Fn(f64) -> [f64; 3], domain: [f64; 2]) -> Vec<Stop> {
    let samples: Vec<[f64; 3]> = (0..=GRADIENT_SAMPLES)
        .map(|i| {
            let offset = i as f64 / GRADIENT_SAMPLES as f64;
            color(interpolate(offset, [0.0, 1.0], domain)).map(|c| c.clamp(0.0, 1.0) * 255.0)
        })
        .collect();
    let joins = |from: usize, to: usize| {
        (from + 1..to).all(|k| {
            let t = (k - from) as f64 / (to - from) as f64;
            (0..3).all(|c| {
                let line = samples[from][c] + t * (samples[to][c] - samples[from][c]);
                (samples[k][c] - line).abs() <= STOP_TOLERANCE
            })
        })
    };
    let mut kept = vec![0];
    let mut from = 0;
    while from < GRADIENT_SAMPLES {
        let mut to = from + 1;
        while to < GRADIENT_SAMPLES && joins(from, to + 1) {
            to += 1;
        }
        kept.push(to);
        from = to;
    }
    kept.into_iter()
        .map(|i| Stop {
            offset: i as f64 / GRADIENT_SAMPLES as f64,
            color: rgb_of(samples[i].map(|c| c / 255.0)),
            opacity: 1.0,
        })
        .collect()
}

/// A point of a mesh and its colour components, or its `t` when a function
/// colours the mesh.
type Vertex = ([f64; 2], Vec<f64>);

/// A patch's twelve boundary points, from its first corner round, and the
/// components at its four corners.
type Patch = ([[f64; 2]; 12], [Vec<f64>; 4]);

/// A mesh's outline, its pieces in flat colours, and the average colour of
/// its vertices.
fn mesh(
    file: &PdfFile,
    dict: &PdfDictionary,
    kind: i64,
    data: &[u8],
    space: &ColorSpace,
    function: Option<&[Function]>,
) -> PdfResult<Kind> {
    let bits = |key: &str| {
        dict.get(key)
            .and_then(|v| number(file, v))
            .map(|n| n as u32)
            .filter(|n| (1..=32).contains(n))
    };
    let coordinate = bits("BitsPerCoordinate").ok_or_else(|| corrupt("mesh BitsPerCoordinate"))?;
    let component = bits("BitsPerComponent").ok_or_else(|| corrupt("mesh BitsPerComponent"))?;
    let flag = match kind {
        5 => 0,
        _ => bits("BitsPerFlag").ok_or_else(|| corrupt("mesh BitsPerFlag"))?,
    };
    let components = match function {
        Some(_) => 1,
        None => space.components(),
    };
    let decode = numbers(file, dict.get("Decode"))
        .filter(|d| d.len() >= 4 + 2 * components)
        .ok_or_else(|| corrupt("mesh Decode"))?;
    let scale = |raw: u32, bits: u32, range: &[f64]| {
        let max = ((1u64 << bits) - 1) as f64;
        interpolate(f64::from(raw), [0.0, max], [range[0], range[1]])
    };
    let point = |reader: &mut Bits| -> Option<[f64; 2]> {
        let x = reader.read(coordinate)?;
        let y = reader.read(coordinate)?;
        Some([
            scale(x, coordinate, &decode[0..2]),
            scale(y, coordinate, &decode[2..4]),
        ])
    };
    let color = |reader: &mut Bits| -> Option<Vec<f64>> {
        (0..components)
            .map(|i| {
                let raw = reader.read(component)?;
                Some(scale(raw, component, &decode[4 + 2 * i..6 + 2 * i]))
            })
            .collect()
    };
    let mut reader = Bits { data, position: 0 };
    let mut triangles: Vec<[Vertex; 3]> = Vec::new();
    let mut patches: Vec<Patch> = Vec::new();
    match kind {
        4 => {
            let mut open: Vec<Vertex> = Vec::new();
            while let Some(f) = reader.read(flag) {
                let (Some(p), Some(c)) = (point(&mut reader), color(&mut reader)) else {
                    break;
                };
                reader.align();
                match f {
                    _ if f == 0 || open.len() < 3 => {
                        if open.len() == 3 {
                            open.clear();
                        }
                        open.push((p, c));
                    }
                    1 => open = vec![open[1].clone(), open[2].clone(), (p, c)],
                    _ => open = vec![open[0].clone(), open[2].clone(), (p, c)],
                }
                if let [a, b, c] = open.as_slice() {
                    triangles.push([a.clone(), b.clone(), c.clone()]);
                }
            }
        }
        5 => {
            let per_row = dict
                .get("VerticesPerRow")
                .and_then(|v| number(file, v))
                .map(|n| n as usize)
                .filter(|n| *n >= 2)
                .ok_or_else(|| corrupt("mesh VerticesPerRow"))?;
            let mut rows: Vec<Vec<Vertex>> = vec![Vec::new()];
            while let (Some(p), Some(c)) = (point(&mut reader), color(&mut reader)) {
                if rows.last().is_some_and(|row| row.len() == per_row) {
                    rows.push(Vec::new());
                }
                rows.last_mut().expect("a row").push((p, c));
            }
            for pair in rows.windows(2) {
                let (top, bottom) = (&pair[0], &pair[1]);
                for j in 0..bottom.len().saturating_sub(1) {
                    triangles.push([top[j].clone(), top[j + 1].clone(), bottom[j].clone()]);
                    triangles.push([top[j + 1].clone(), bottom[j + 1].clone(), bottom[j].clone()]);
                }
            }
        }
        _ => {
            let count = if kind == 6 { 12 } else { 16 };
            while let Some(f) = reader.read(flag) {
                let (mut points, mut corners): (Vec<[f64; 2]>, Vec<Vec<f64>>) =
                    match (f, patches.last()) {
                        (0, _) | (_, None) => (Vec::new(), Vec::new()),
                        (f, Some((p, c))) => {
                            let (edge, first) = match f {
                                1 => ([3, 4, 5, 6], 1),
                                2 => ([6, 7, 8, 9], 2),
                                _ => ([9, 10, 11, 0], 3),
                            };
                            (
                                edge.iter().map(|&i| p[i]).collect(),
                                vec![c[first].clone(), c[(first + 1) % 4].clone()],
                            )
                        }
                    };
                let Some(read) = (points.len()..count)
                    .map(|_| point(&mut reader))
                    .collect::<Option<Vec<_>>>()
                else {
                    break;
                };
                points.extend(read);
                let Some(read) = (corners.len()..4)
                    .map(|_| color(&mut reader))
                    .collect::<Option<Vec<_>>>()
                else {
                    break;
                };
                corners.extend(read);
                let boundary: [[f64; 2]; 12] = std::array::from_fn(|i| points[i]);
                let corners: [Vec<f64>; 4] = std::array::from_fn(|i| corners[i].clone());
                patches.push((boundary, corners));
            }
        }
    }
    let rgb = |components: &[f64]| match function {
        Some(function) => space.to_rgb(&eval_list(function, components)),
        None => space.to_rgb(components),
    };
    let mut outline: Vec<Segment> = Vec::new();
    let mut sum = [0.0; 3];
    let mut vertices = 0usize;
    for triangle in &triangles {
        push_polygon(&mut outline, &triangle.each_ref().map(|v| v.0));
        for (_, c) in triangle {
            add(&mut sum, rgb(c));
            vertices += 1;
        }
    }
    for (boundary, corners) in &patches {
        outline.push(Segment::Move(boundary[0]));
        for edge in 0..4 {
            let at = |k: usize| boundary[(3 * edge + k) % 12];
            outline.push(Segment::Cubic(at(1), at(2), at(3)));
        }
        outline.push(Segment::Close);
        for c in corners {
            add(&mut sum, rgb(c));
            vertices += 1;
        }
    }
    if vertices == 0 {
        return Err(corrupt("mesh without a vertex"));
    }
    // Each piece is cut so that the mesh keeps to about this many.
    const PIECES: usize = 4096;
    let mut pieces: Vec<(Vec<Segment>, Rgb)> = Vec::new();
    let mut depth = 0;
    while depth < 2 && triangles.len() * 4usize.pow(depth + 1) <= PIECES {
        depth += 1;
    }
    for triangle in &triangles {
        split(triangle, depth, &mut |[a, b, c]: [Vertex; 3]| {
            let mean: Vec<f64> = (0..a.1.len())
                .map(|i| (a.1[i] + b.1[i] + c.1[i]) / 3.0)
                .collect();
            let mut path = Vec::new();
            push_polygon(&mut path, &[a.0, b.0, c.0]);
            pieces.push((path, rgb_of(rgb(&mean))));
        });
    }
    let cells = ((PIECES / patches.len().max(1)) as f64)
        .sqrt()
        .floor()
        .clamp(1.0, 4.0) as usize;
    for (boundary, corners) in &patches {
        let at = |u: f64, v: f64| coons(boundary, u, v);
        for row in 0..cells {
            for column in 0..cells {
                let (u0, u1) = (
                    column as f64 / cells as f64,
                    (column + 1) as f64 / cells as f64,
                );
                let (v0, v1) = (row as f64 / cells as f64, (row + 1) as f64 / cells as f64);
                let (u, v) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
                let mean: Vec<f64> = (0..corners[0].len())
                    .map(|i| {
                        (1.0 - u) * (1.0 - v) * corners[0][i]
                            + u * (1.0 - v) * corners[1][i]
                            + u * v * corners[2][i]
                            + (1.0 - u) * v * corners[3][i]
                    })
                    .collect();
                let mut path = Vec::new();
                push_polygon(&mut path, &[at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)]);
                pieces.push((path, rgb_of(rgb(&mean))));
            }
        }
    }
    Ok(Kind::Mesh {
        outline,
        pieces,
        average: rgb_of(sum.map(|s| s / vertices as f64)),
    })
}

fn add(sum: &mut [f64; 3], rgb: [f64; 3]) {
    for (total, channel) in sum.iter_mut().zip(rgb) {
        *total += channel;
    }
}

fn push_polygon(outline: &mut Vec<Segment>, points: &[[f64; 2]]) {
    outline.push(Segment::Move(points[0]));
    outline.extend(points[1..].iter().map(|p| Segment::Line(*p)));
    outline.push(Segment::Close);
}

/// A triangle cut `depth` times into four, by the middles of its sides.
fn split(triangle: &[Vertex; 3], depth: u32, out: &mut impl FnMut([Vertex; 3])) {
    if depth == 0 {
        out(triangle.clone());
        return;
    }
    let middle = |a: &Vertex, b: &Vertex| -> Vertex {
        (
            [(a.0[0] + b.0[0]) / 2.0, (a.0[1] + b.0[1]) / 2.0],
            a.1.iter().zip(&b.1).map(|(x, y)| (x + y) / 2.0).collect(),
        )
    };
    let [a, b, c] = triangle;
    let (ab, bc, ca) = (middle(a, b), middle(b, c), middle(c, a));
    for piece in [
        [a.clone(), ab.clone(), ca.clone()],
        [ab.clone(), b.clone(), bc.clone()],
        [ca.clone(), bc.clone(), c.clone()],
        [ab, bc, ca],
    ] {
        split(&piece, depth - 1, out);
    }
}

/// A point of a Coons patch from its twelve boundary points, the four
/// corners at `(0, 0)`, `(1, 0)`, `(1, 1)` and `(0, 1)`.
fn coons(p: &[[f64; 2]; 12], u: f64, v: f64) -> [f64; 2] {
    let cubic = |a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], t: f64| -> [f64; 2] {
        let s = 1.0 - t;
        let w = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
        [0, 1].map(|i| w[0] * a[i] + w[1] * b[i] + w[2] * c[i] + w[3] * d[i])
    };
    let bottom = cubic(p[0], p[1], p[2], p[3], u);
    let right = cubic(p[3], p[4], p[5], p[6], v);
    let top = cubic(p[9], p[8], p[7], p[6], u);
    let left = cubic(p[0], p[11], p[10], p[9], v);
    [0, 1].map(|i| {
        (1.0 - v) * bottom[i] + v * top[i] + (1.0 - u) * left[i] + u * right[i]
            - ((1.0 - u) * (1.0 - v) * p[0][i]
                + u * (1.0 - v) * p[3][i]
                + u * v * p[6][i]
                + (1.0 - u) * v * p[9][i])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linear_ramp_keeps_its_two_ends() {
        let stops = stops(|t| [t, 0.0, 1.0 - t], [0.0, 1.0]);
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].color, Rgb(0, 0, 255));
        assert_eq!(stops[1].color, Rgb(255, 0, 0));
    }

    #[test]
    fn a_step_keeps_a_stop_on_each_side() {
        let stops = stops(|t| if t < 0.5 { [0.0; 3] } else { [1.0; 3] }, [0.0, 1.0]);
        assert!(stops.len() <= 4, "{stops:?}");
        let before = stops.iter().rfind(|s| s.color == Rgb::BLACK).unwrap();
        let after = stops.iter().find(|s| s.color == Rgb::WHITE).unwrap();
        assert!(after.offset - before.offset <= 1.0 / GRADIENT_SAMPLES as f64 + 1e-9);
    }
}
