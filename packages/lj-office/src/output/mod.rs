//! What a laid out page is read as: an SVG drawing whose text stays text, the
//! words with their boxes, and the quads of anchored characters.

use std::collections::HashMap;

use lj_draw::{Clip, Fill, Group, Join, Matrix, Rgb, Segment, Shape, Stroke, Text};

use crate::Anchor;

use crate::layout::{Art, Glyphs, Image, Item, Layout, Outline};
use crate::model::{Color, ColorMode, Geometry, GradientShape, Paint, Recolor, Transform};

fn rgb(color: Color) -> Rgb {
    Rgb(color.0, color.1, color.2)
}

fn fill(paint: Paint) -> Fill {
    Fill {
        opacity: paint.alpha,
        ..Fill::color(rgb(paint.color))
    }
}

fn stroke(stroke: crate::model::Stroke) -> Stroke {
    Stroke {
        opacity: stroke.paint.alpha,
        ..Stroke::color(rgb(stroke.paint.color), stroke.width)
    }
}

/// A frame's turn about its centre, after its mirroring.
fn turn(t: Transform, x: f64, y: f64, width: f64, height: f64) -> Matrix {
    let (cx, cy) = (x + width / 2.0, y + height / 2.0);
    let about = |m: Matrix| {
        Matrix::translate(-cx, -cy)
            .then(m)
            .then(Matrix::translate(cx, cy))
    };
    let (sx, sy) = (
        if t.flip_h { -1.0 } else { 1.0 },
        if t.flip_v { -1.0 } else { 1.0 },
    );
    about(Matrix::scale(sx, sy)).then(about(Matrix::rotate(t.rotation)))
}

fn turned(
    t: Transform,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    item: lj_draw::Item,
) -> lj_draw::Item {
    match t == Transform::default() {
        true => item,
        false => lj_draw::Item::Group(Box::new(Group {
            transform: turn(t, x, y, width, height),
            ..Group::new(vec![item])
        })),
    }
}

fn recolor(r: Recolor) -> Option<lj_draw::Recolor> {
    let (slope, intercept) = r.linear();
    let grey = matches!(r.mode, ColorMode::Greys | ColorMode::Mono);
    let threshold = r.mode == ColorMode::Mono;
    (grey || slope != 1.0 || intercept != 0.0).then_some(lj_draw::Recolor {
        grey: grey.then_some([0.2969, 0.5898, 0.1133]),
        threshold,
        slope,
        intercept,
    })
}

/// A picture cropped by the share `crop` cuts off each side of its frame.
fn image(image: &Image) -> Option<lj_draw::Item> {
    let c = image.crop;
    let (visible_w, visible_h) = (1.0 - c.left - c.right, 1.0 - c.top - c.bottom);
    if visible_w <= 0.0 || visible_h <= 0.0 {
        return None;
    }
    let cropped = c.left != 0.0 || c.right != 0.0 || c.top != 0.0 || c.bottom != 0.0;
    let (w, h) = (image.width / visible_w, image.height / visible_h);
    let (x, y) = (image.x - c.left * w, image.y - c.top * h);
    let picture = lj_draw::Item::Image(lj_draw::Image {
        transform: Matrix::scale(w, h).then(Matrix::translate(x, y)),
        mime: image.mime,
        data: image.data.clone(),
        smooth: true,
        recolor: recolor(image.recolor),
    });
    let clip = cropped.then_some(Clip {
        shape: Shape::Rect {
            x: image.x,
            y: image.y,
            width: image.width,
            height: image.height,
            radius: 0.0,
        },
        even_odd: false,
    });
    Some(lj_draw::Item::Group(Box::new(Group {
        transform: turn(image.transform, image.x, image.y, image.width, image.height),
        clip,
        ..Group::new(vec![picture])
    })))
}

/// `c` toward black by the share `-k`, or toward white by `k`.
fn shaded(c: Color, k: f64) -> Color {
    let f = |v: u8| {
        let v = f64::from(v);
        let out = if k < 0.0 {
            v * (1.0 + k)
        } else {
            v + (255.0 - v) * k
        };
        out.round().clamp(0.0, 255.0) as u8
    };
    Color(f(c.0), f(c.1), f(c.2))
}

/// The gradients and patterns a page's fills refer to.
#[derive(Default)]
struct Paints {
    gradients: Vec<lj_draw::Gradient>,
    patterns: Vec<lj_draw::Pattern>,
}

impl Paints {
    fn pattern(&mut self, pattern: lj_draw::Pattern, opacity: f64) -> Fill {
        self.patterns.push(pattern);
        Fill {
            paint: lj_draw::Paint::Pattern(self.patterns.len() - 1),
            opacity,
            even_odd: false,
        }
    }
}

/// Width of a hatch's lines, LibreOffice's hairline at 96 dpi.
const HATCH_LINE: f64 = 0.75;

/// A shape's fill over its frame, `None` for a picture the browser cannot
/// draw.
fn frame_fill(
    paint: &crate::model::Fill,
    [x, y, w, h]: [f64; 4],
    paints: &mut Paints,
) -> Option<Fill> {
    let g = match paint {
        crate::model::Fill::Solid(paint) => return Some(fill(*paint)),
        crate::model::Fill::Gradient(g) => g,
        crate::model::Fill::Hatch(hatch) => {
            let d = hatch.distance.max(0.5);
            let line = |from, to| lj_draw::Item::Shape {
                shape: Shape::Line { from, to },
                fill: None,
                stroke: Some(Stroke::color(rgb(hatch.color), HATCH_LINE)),
            };
            let mut items = Vec::new();
            if let Some(back) = hatch.back {
                items.push(lj_draw::Item::Shape {
                    shape: Shape::Rect {
                        x: 0.0,
                        y: 0.0,
                        width: d,
                        height: d,
                        radius: 0.0,
                    },
                    fill: Some(fill(back)),
                    stroke: None,
                });
            }
            items.push(line([0.0, d / 2.0], [d, d / 2.0]));
            if hatch.lines >= 2 {
                items.push(line([d / 2.0, 0.0], [d / 2.0, d]));
            }
            if hatch.lines >= 3 {
                items.push(line([0.0, 0.0], [d, d]));
            }
            let pattern = lj_draw::Pattern {
                transform: Matrix::rotate(-hatch.angle).then(Matrix::translate(x, y)),
                width: d,
                height: d,
                items,
            };
            return Some(paints.pattern(pattern, hatch.alpha));
        }
        crate::model::Fill::Image(image) => {
            let (data, mime) = crate::layout::displayable(&image.data, image.format)?;
            let [tw, th] = image.tile.unwrap_or([w, h]);
            if tw <= 0.0 || th <= 0.0 {
                return None;
            }
            let pattern = lj_draw::Pattern {
                transform: Matrix::translate(x, y),
                width: tw,
                height: th,
                items: vec![lj_draw::Item::Image(lj_draw::Image {
                    transform: Matrix::scale(tw, th),
                    mime,
                    data,
                    smooth: image.smooth,
                    recolor: None,
                })],
            };
            return Some(paints.pattern(pattern, image.alpha));
        }
    };
    let kind = match g.shape {
        GradientShape::Linear(angle) => {
            let (sin, cos) = angle.to_radians().sin_cos();
            let half = (cos.abs() + sin.abs()) / 2.0;
            lj_draw::GradientKind::Linear {
                from: [0.5 - cos * half, 0.5 - sin * half],
                to: [0.5 + cos * half, 0.5 + sin * half],
            }
        }
        GradientShape::Radial([cx, cy]) => {
            let radius = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
                .iter()
                .map(|[px, py]| (px - cx).hypot(py - cy))
                .fold(0.0, f64::max);
            lj_draw::GradientKind::Radial {
                from: [cx, cy],
                from_radius: 0.0,
                to: [cx, cy],
                to_radius: radius,
            }
        }
    };
    paints.gradients.push(lj_draw::Gradient {
        kind,
        transform: Matrix::scale(w, h).then(Matrix::translate(x, y)),
        stops: g
            .stops
            .iter()
            .map(|(offset, paint)| lj_draw::Stop {
                offset: *offset,
                color: rgb(paint.color),
                opacity: paint.alpha,
            })
            .collect(),
    });
    Some(Fill {
        paint: lj_draw::Paint::Gradient(paints.gradients.len() - 1),
        opacity: 1.0,
        even_odd: false,
    })
}

fn outline(shape: &Outline, paints: &mut Paints) -> lj_draw::Item {
    let (x, y, w, h) = (shape.x, shape.y, shape.width, shape.height);
    let mut paint = |filled: bool, shade: f64| {
        let f = shape.fill.as_ref().filter(|_| filled)?;
        match f {
            crate::model::Fill::Solid(p) if shade != 0.0 => Some(fill(Paint {
                color: shaded(p.color, shade),
                ..*p
            })),
            f => frame_fill(f, [x, y, w, h], paints),
        }
    };
    let (drawn, filled) = match &shape.geometry {
        Geometry::Path(paths) => {
            let place = |[u, v]: [f64; 2]| [x + u * w, y + v * h];
            let items = paths
                .iter()
                .map(|path| lj_draw::Item::Shape {
                    shape: Shape::Path(
                        path.segments
                            .iter()
                            .map(|s| match *s {
                                Segment::Move(p) => Segment::Move(place(p)),
                                Segment::Line(p) => Segment::Line(place(p)),
                                Segment::Cubic(a, b, c) => {
                                    Segment::Cubic(place(a), place(b), place(c))
                                }
                                Segment::Close => Segment::Close,
                            })
                            .collect(),
                    ),
                    fill: paint(path.fill, path.shade),
                    stroke: shape.stroke.filter(|_| path.stroke).map(stroke),
                })
                .collect();
            let group = lj_draw::Item::Group(Box::new(Group::new(items)));
            return turned(shape.transform, x, y, w, h, group);
        }
        Geometry::Rect => (
            Shape::Rect {
                x,
                y,
                width: w,
                height: h,
                radius: 0.0,
            },
            true,
        ),
        Geometry::RoundRect(share) => (
            Shape::Rect {
                x,
                y,
                width: w,
                height: h,
                radius: share * w.min(h),
            },
            true,
        ),
        Geometry::Ellipse => (
            Shape::Ellipse {
                cx: x + w / 2.0,
                cy: y + h / 2.0,
                rx: w / 2.0,
                ry: h / 2.0,
            },
            true,
        ),
        Geometry::Line => (
            Shape::Line {
                from: [x, y],
                to: [x + w, y + h],
            },
            false,
        ),
    };
    let item = lj_draw::Item::Shape {
        shape: drawn,
        fill: paint(filled, 0.0),
        stroke: shape.stroke.map(stroke),
    };
    turned(shape.transform, x, y, w, h, item)
}

impl Layout {
    /// One page as a display list, each run of characters a text in the family
    /// its face's `@font-face` rule declares.
    pub fn drawing(&self, page: usize) -> lj_draw::Page {
        let page = &self.pages[page];
        let mut paints = Paints::default();
        let items = page
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Rect {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => Some(lj_draw::Item::Shape {
                    shape: Shape::Rect {
                        x: *x,
                        y: *y,
                        width: *width,
                        height: *height,
                        radius: 0.0,
                    },
                    fill: Some(Fill::color(rgb(*color))),
                    stroke: None,
                }),
                Item::Line {
                    from,
                    to,
                    width,
                    color,
                } => Some(lj_draw::Item::Shape {
                    shape: Shape::Line {
                        from: *from,
                        to: *to,
                    },
                    fill: None,
                    stroke: Some(Stroke::color(rgb(*color), *width)),
                }),
                Item::Glyphs(glyphs) => Some(lj_draw::Item::Text(self.text(glyphs))),
                Item::Art(art) => Some(self.art(art)),
                Item::Image(picture) => image(picture),
                Item::Shape(shape) => Some(outline(shape, &mut paints)),
                Item::Polygon {
                    points,
                    fill: paint,
                    stroke: line,
                } => {
                    let mut segments: Vec<Segment> = points
                        .iter()
                        .enumerate()
                        .map(|(i, p)| match i {
                            0 => Segment::Move(*p),
                            _ => Segment::Line(*p),
                        })
                        .collect();
                    segments.push(Segment::Close);
                    Some(lj_draw::Item::Shape {
                        shape: Shape::Path(segments),
                        fill: paint.map(fill),
                        stroke: line.map(stroke),
                    })
                }
                Item::Path(path) => {
                    let segments = path
                        .rings
                        .iter()
                        .filter(|ring| !ring.is_empty())
                        .flat_map(|ring| {
                            let points = ring.iter().enumerate().map(|(i, p)| match i {
                                0 => Segment::Move(*p),
                                _ => Segment::Line(*p),
                            });
                            points.chain(path.closed.then_some(Segment::Close))
                        })
                        .collect();
                    Some(lj_draw::Item::Shape {
                        shape: Shape::Path(segments),
                        fill: path.fill.filter(|_| path.closed).map(|paint| Fill {
                            even_odd: path.even_odd,
                            ..fill(paint)
                        }),
                        stroke: path.stroke.map(|line| Stroke {
                            join: Join::Round,
                            ..stroke(line)
                        }),
                    })
                }
            })
            .collect();
        lj_draw::Page {
            width: page.width,
            height: page.height,
            items,
            gradients: paints.gradients,
            patterns: paints.patterns,
            ..Default::default()
        }
    }

    fn text(&self, glyphs: &Glyphs) -> Text {
        Text {
            x: glyphs.x,
            y: glyphs.y,
            family: self.faces[glyphs.face].name.clone(),
            size: glyphs.size,
            color: rgb(glyphs.color),
            text: glyphs.text.clone(),
            advances: glyphs.advances.clone(),
            each_char: false,
            kerning: glyphs.kerning,
            letter_spacing: glyphs.letter_spacing,
            synthetic_bold: glyphs.synthetic_bold,
            synthetic_italic: glyphs.synthetic_italic,
        }
    }

    fn art(&self, art: &Art) -> lj_draw::Item {
        let [x, y, w, h] = art.frame;
        lj_draw::Item::Group(Box::new(Group {
            transform: art.fit.then(turn(art.transform, x, y, w, h)),
            opacity: art.alpha,
            ..Group::new(
                art.glyphs
                    .iter()
                    .map(|g| lj_draw::Item::Text(self.text(g)))
                    .collect(),
            )
        }))
    }

    /// One page as a standalone SVG in points, whose text stays text.
    pub fn svg(&self, page: usize) -> String {
        lj_draw::svg(&self.drawing(page), &format!("p{page}-"))
    }

    fn glyphs(&self, page: usize) -> impl Iterator<Item = &Glyphs> {
        self.pages[page].items.iter().filter_map(|item| match item {
            Item::Glyphs(g) => Some(g),
            _ => None,
        })
    }

    /// Top and bottom of a run's characters.
    fn extent(&self, glyphs: &Glyphs) -> (f64, f64) {
        let face = &self.faces[glyphs.face];
        (
            glyphs.y - face.ascent * glyphs.size,
            glyphs.y + face.descent * glyphs.size,
        )
    }

    /// The words of a page in reading order, each with its box
    /// `[x0, y0, x1, y1]`. A word drawn in two runs is one word, a raised or
    /// lowered part included.
    pub fn words(&self, page: usize) -> Vec<(String, [f64; 4])> {
        let mut words: Vec<(String, [f64; 4])> = Vec::new();
        let mut open = false;
        let mut end: Option<(f64, f64, f64)> = None;
        for glyphs in self.glyphs(page) {
            let (top, bottom) = self.extent(glyphs);
            let joins = end.is_some_and(|(x, above, below)| {
                (x - glyphs.x).abs() < 0.01 && top < below && above < bottom
            });
            if !joins {
                open = false;
            }
            let mut x = glyphs.x;
            for (c, advance) in glyphs.text.chars().zip(&glyphs.advances) {
                if c.is_whitespace() {
                    open = false;
                } else {
                    match words.last_mut() {
                        Some((text, bounds)) if open => {
                            text.push(c);
                            bounds[1] = bounds[1].min(top);
                            bounds[2] = x + advance;
                            bounds[3] = bounds[3].max(bottom);
                        }
                        _ => {
                            words.push((c.to_string(), [x, top, x + advance, bottom]));
                            open = true;
                        }
                    }
                }
                x += advance;
            }
            end = Some((x, top, bottom));
        }
        words
    }

    /// Where the characters of each group of `anchors` are drawn, as a page
    /// and the four corners of each character's box, in the order of the
    /// group. A character of a header shows once per page that repeats it.
    /// One pass over the pages serves every group.
    pub fn quads(&self, groups: &[&[Anchor]]) -> Vec<Vec<(usize, [[f64; 2]; 4])>> {
        let mut wanted: HashMap<Anchor, Vec<(usize, usize)>> = HashMap::new();
        for (group, anchors) in groups.iter().enumerate() {
            for (i, anchor) in anchors.iter().enumerate() {
                wanted.entry(*anchor).or_default().push((group, i));
            }
        }
        type Found = (usize, usize, [[f64; 2]; 4]);
        let mut found: Vec<Vec<Found>> = vec![Vec::new(); groups.len()];
        for page in 0..self.pages.len() {
            for glyphs in self.glyphs(page) {
                let (top, bottom) = self.extent(glyphs);
                let mut x = glyphs.x;
                for (anchor, advance) in glyphs.anchors.iter().zip(&glyphs.advances) {
                    let hits = anchor.as_ref().and_then(|a| wanted.get(a));
                    for &(group, i) in hits.into_iter().flatten() {
                        let x1 = x + advance;
                        found[group].push((
                            i,
                            page,
                            [[x, top], [x1, top], [x1, bottom], [x, bottom]],
                        ));
                    }
                    x += advance;
                }
            }
        }
        found
            .into_iter()
            .map(|mut group| {
                group.sort_by_key(|(i, page, _)| (*i, *page));
                group
                    .into_iter()
                    .map(|(_, page, quad)| (page, quad))
                    .collect()
            })
            .collect()
    }
}
