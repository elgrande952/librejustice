//! A frame or a drawing shape of an `.odt` as the layout's drawing: in the
//! line when anchored as a character, floating otherwise.

use super::geometry::ShapePath;
use super::model as odt;
use super::to_model::{color, story};
use crate::model as m;

/// Of a character, above and below its baseline, as a share of its size.
const ASCENT: f64 = 0.9;
const DESCENT: f64 = 0.22;

pub(super) fn drawing(
    doc: &odt::Document,
    f: &odt::Frame,
    props: &m::RunProps,
) -> Option<m::Drawing> {
    let (x, y, width, height, shape) = match &f.content {
        odt::FrameContent::Group(members) => {
            let (x, y, width, height, children) = group(doc, members)?;
            (x, y, width, height, m::Shape::Group(children))
        }
        _ => {
            let width = f64::from(f.width?);
            let height = f64::from(f.height.or(f.min_height)?);
            (f64::from(f.x), f64::from(f.y), width, height, shape(doc, f))
        }
    };
    let transform = m::Transform {
        rotation: f64::from(f.rotation) * mirrored(f),
        flip_h: f.mirror.0,
        flip_v: f.mirror.1,
    };
    let margin = margins(&f.margin);
    if f.anchor != odt::FrameAnchor::AsChar {
        return Some(m::Drawing {
            width,
            height,
            float: Some(Box::new(float(f, x, y, margin))),
            shape,
            transform,
            props: props.clone(),
        });
    }
    // Set as a character, a frame takes its spacing into its box in the line.
    let (outer_w, outer_h) = (
        width + margin.left + margin.right,
        height + margin.top + margin.bottom,
    );
    let (shape, transform) = if margin == m::Margins::default() {
        (shape, transform)
    } else {
        let child = m::Child {
            x: margin.left,
            y: margin.top,
            width,
            height,
            shape,
            transform,
        };
        (m::Shape::Group(vec![child]), m::Transform::default())
    };
    let mut props = props.clone();
    props.position += rise(f, outer_h, props.size);
    Some(m::Drawing {
        width: outer_w,
        height: outer_h,
        float: None,
        shape,
        transform,
        props,
    })
}

/// -1 for a custom shape mirrored on one axis, which LibreOffice mirrors
/// after turning it: turned the other way before, it lands the same.
fn mirrored(f: &odt::Frame) -> f64 {
    match &f.content {
        odt::FrameContent::Shape {
            path: Some(path), ..
        } => match &**path {
            ShapePath::Enhanced(e) if e.mirror.0 != e.mirror.1 => -1.0,
            _ => 1.0,
        },
        _ => 1.0,
    }
}

/// How far the bottom of a frame set as a character stands above the
/// baseline, as LibreOffice's `SwAsCharAnchoredObjectPosition` orients it.
fn rise(f: &odt::Frame, height: f64, size: f64) -> f64 {
    let (ascent, descent) = (ASCENT * size, DESCENT * size);
    let top = match (f.vertical.0.as_str(), f.vertical.1.as_str()) {
        ("from-top", _) => f64::from(f.y),
        ("middle", "char" | "line") => (descent - ascent - height) / 2.0,
        ("top", "char" | "line") => -ascent,
        ("bottom", "char" | "line") => descent - height,
        ("middle", _) => -height / 2.0,
        ("bottom", _) => 0.0,
        _ => -height,
    };
    -(top + height)
}

fn margins(s: &odt::Sides<f32>) -> m::Margins {
    m::Margins {
        top: f64::from(s.top),
        right: f64::from(s.right),
        bottom: f64::from(s.bottom),
        left: f64::from(s.left),
    }
}

fn float(f: &odt::Frame, x: f64, y: f64, distance: m::Margins) -> m::Float {
    let page = matches!(f.anchor, odt::FrameAnchor::Page(_));
    let (pos, rel) = (&f.horizontal.0, &f.horizontal.1);
    let horizontal = m::Position {
        from: match rel.as_str() {
            "page" => m::Relative::Page,
            "page-start-margin" => m::Relative::LeftMargin,
            "page-end-margin" => m::Relative::RightMargin,
            "char" => m::Relative::Character,
            "page-content" => m::Relative::Margin,
            _ if page => m::Relative::Margin,
            _ => m::Relative::Column,
        },
        place: match pos.as_str() {
            "left" => m::Place::Start,
            "center" => m::Place::Center,
            "right" => m::Place::End,
            "inside" => m::Place::Inside,
            "outside" => m::Place::Outside,
            _ => m::Place::Offset(x),
        },
    };
    let (pos, rel) = (&f.vertical.0, &f.vertical.1);
    let vertical = m::Position {
        from: match rel.as_str() {
            "page" => m::Relative::Page,
            "page-content" => m::Relative::Margin,
            _ if page => m::Relative::Margin,
            "char" | "line" | "baseline" | "text" => m::Relative::Line,
            _ => m::Relative::Paragraph,
        },
        place: match pos.as_str() {
            "top" => m::Place::Start,
            "middle" => m::Place::Center,
            "bottom" => m::Place::End,
            _ => m::Place::Offset(y),
        },
    };
    let (wrap, side) = match f.wrap {
        odt::Wrap::None => (m::Wrap::TopAndBottom, m::WrapSide::Both),
        odt::Wrap::RunThrough => (m::Wrap::None, m::WrapSide::Both),
        odt::Wrap::Parallel => (m::Wrap::Around, m::WrapSide::Both),
        odt::Wrap::Dynamic => (m::Wrap::Around, m::WrapSide::Largest),
        odt::Wrap::Left => (m::Wrap::Around, m::WrapSide::Left),
        odt::Wrap::Right => (m::Wrap::Around, m::WrapSide::Right),
    };
    m::Float {
        horizontal,
        vertical,
        wrap,
        side,
        behind: f.behind_text && f.wrap == odt::Wrap::RunThrough,
        distance,
        z: i64::from(f.z_index),
        contour: Vec::new(),
        in_cell: true,
    }
}

/// A group's frame, the union of its members', and the members within it.
fn group(
    doc: &odt::Document,
    members: &[odt::Frame],
) -> Option<(f64, f64, f64, f64, Vec<m::Child>)> {
    let mut laid = Vec::new();
    for member in members {
        let (x, y, width, height, shape) = match &member.content {
            odt::FrameContent::Group(inner) => {
                let (x, y, w, h, children) = group(doc, inner)?;
                (x, y, w, h, m::Shape::Group(children))
            }
            _ => {
                let (Some(w), Some(h)) = (member.width, member.height.or(member.min_height)) else {
                    continue;
                };
                let (w, h) = (f64::from(w), f64::from(h));
                (
                    f64::from(member.x),
                    f64::from(member.y),
                    w,
                    h,
                    shape(doc, member),
                )
            }
        };
        let transform = m::Transform {
            rotation: f64::from(member.rotation) * mirrored(member),
            flip_h: member.mirror.0,
            flip_v: member.mirror.1,
        };
        laid.push((x, y, width, height, shape, transform));
    }
    let left = laid.iter().map(|l| l.0).reduce(f64::min)?;
    let top = laid.iter().map(|l| l.1).reduce(f64::min)?;
    let right = laid.iter().map(|l| l.0 + l.2).reduce(f64::max)?;
    let bottom = laid.iter().map(|l| l.1 + l.3).reduce(f64::max)?;
    let children = laid
        .into_iter()
        .map(|(x, y, width, height, shape, transform)| m::Child {
            x: x - left,
            y: y - top,
            width,
            height,
            shape,
            transform,
        })
        .collect();
    Some((left, top, right - left, bottom - top, children))
}

fn paint(c: odt::Color) -> m::Paint {
    m::Paint {
        color: color(c),
        alpha: 1.0,
    }
}

/// Of a text frame, LibreOffice reads a hatch's distance in hundredths of a
/// millimetre as twips.
const FRAME_HATCH: f64 = 2540.0 / 72.0 / 20.0;

/// A shape's fill or a frame's background, `plain` unless a hatch, a
/// picture or a gradient replaces it.
fn fill(
    doc: &odt::Document,
    f: &odt::Frame,
    plain: Option<odt::Color>,
    text_frame: bool,
) -> Option<m::Fill> {
    let alpha = f64::from(f.opacity);
    if let Some(h) = &f.hatch {
        let scale = if text_frame { FRAME_HATCH } else { 1.0 };
        return Some(m::Fill::Hatch(std::sync::Arc::new(m::Hatch {
            color: color(h.color),
            distance: f64::from(h.distance) * scale,
            angle: f64::from(h.angle),
            lines: h.lines,
            back: h.back.map(paint),
            alpha,
        })));
    }
    if let Some(i) = &f.fill_image {
        let image = doc.images.get(i.image)?;
        let format = m::ImageFormat::sniff(&image.bytes);
        let tile = match (i.stretch, i.tile) {
            (true, _) => None,
            (false, Some((w, h))) => Some([f64::from(w), f64::from(h)]),
            (false, None) => Some(format.natural_size(&image.bytes)?),
        };
        return Some(m::Fill::Image(std::sync::Arc::new(m::ImageFill {
            data: image.bytes.clone(),
            format,
            tile,
            smooth: true,
            alpha,
        })));
    }
    let Some(g) = &f.gradient else {
        let plain = plain?;
        return Some(match &f.transparency {
            Some(t) => {
                let (shape, stops) = gradient(t, |c| {
                    let odt::Color(v) = c;
                    m::Paint {
                        color: color(plain),
                        alpha: f64::from(v & 0xff) / 255.0,
                    }
                });
                m::Fill::Gradient(std::sync::Arc::new(m::Gradient { shape, stops }))
            }
            None => m::Fill::Solid(m::Paint {
                alpha,
                ..paint(plain)
            }),
        });
    };
    let (shape, stops) = gradient(g, |c| m::Paint { alpha, ..paint(c) });
    Some(m::Fill::Gradient(std::sync::Arc::new(m::Gradient {
        shape,
        stops,
    })))
}

/// A `draw:gradient`'s shape and stops, each colour painted by `at`.
fn gradient(
    g: &odt::Gradient,
    at: impl Fn(odt::Color) -> m::Paint,
) -> (m::GradientShape, Vec<(f64, m::Paint)>) {
    let (start, end) = (at(g.start), at(g.end));
    let border = f64::from(g.border);
    let mix = |t: f64| {
        let channel =
            |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
        m::Paint {
            color: m::Color(
                channel(start.color.0, end.color.0),
                channel(start.color.1, end.color.1),
                channel(start.color.2, end.color.2),
            ),
            alpha: start.alpha + (end.alpha - start.alpha) * t,
        }
    };
    // From the start colour, at 0, to the end colour, at 1.
    let ramp: Vec<(f64, m::Paint)> = match g.steps {
        0 | 1 => vec![(0.0, start), (border, start), (1.0, end)],
        n => {
            let mut stops = vec![(0.0, start)];
            for k in 0..n {
                let (from, to) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
                let band = mix(k as f64 / (n - 1) as f64);
                let place = |t: f64| border + (1.0 - border) * t;
                stops.push((place(from), band));
                stops.push((place(to), band));
            }
            stops
        }
    };
    match g.style {
        odt::GradientStyle::Linear => (m::GradientShape::Linear(90.0 - f64::from(g.angle)), ramp),
        odt::GradientStyle::Axial => {
            let half = ramp.iter().map(|&(t, p)| (t / 2.0, p));
            let back = ramp.iter().rev().map(|&(t, p)| (1.0 - t / 2.0, p));
            (
                m::GradientShape::Linear(90.0 - f64::from(g.angle)),
                half.chain(back).collect(),
            )
        }
        odt::GradientStyle::Radial => (
            m::GradientShape::Radial([f64::from(g.center.0), f64::from(g.center.1)]),
            ramp.iter().rev().map(|&(t, p)| (1.0 - t, p)).collect(),
        ),
    }
}

/// A border thinner than LibreOffice's one pixel drawn as wide as a shape's
/// hairline.
fn hairline(s: m::Stroke) -> m::Stroke {
    m::Stroke {
        width: s.width.max(0.5),
        ..s
    }
}

/// The frame's border as one outline, when its four sides are the same.
fn outline(f: &odt::Frame) -> Option<m::Stroke> {
    let b = f.borders.top?;
    let same = [f.borders.right, f.borders.bottom, f.borders.left]
        .iter()
        .all(|s| *s == Some(b));
    same.then(|| m::Stroke {
        width: f64::from(b.width),
        paint: paint(b.color),
    })
}

fn shape(doc: &odt::Document, f: &odt::Frame) -> m::Shape {
    match &f.content {
        odt::FrameContent::Image(i) => picture(doc, *i, f),
        odt::FrameContent::Object { preview, .. } => match preview {
            Some(i) => picture(doc, *i, f),
            None => m::Shape::Opaque,
        },
        odt::FrameContent::Chart(chart) => m::Shape::Chart(Box::new(chart.to_model())),
        odt::FrameContent::TextBox(blocks) => {
            let stroke = outline(f);
            let border = stroke.map_or(0.0, |s| s.width);
            let pad = margins(&f.padding);
            m::Shape::Figure(Box::new(m::Figure {
                geometry: m::Geometry::Rect,
                fill: fill(doc, f, f.background, true),
                stroke: stroke.map(hairline),
                text: Some(m::TextBox {
                    blocks: story(doc, blocks, false),
                    inset: m::Margins {
                        top: pad.top + border,
                        right: pad.right + border,
                        bottom: pad.bottom + border,
                        left: pad.left + border,
                    },
                    valign: valign(f.valign),
                    grow: f.height.is_none(),
                    fit: false,
                }),
            }))
        }
        odt::FrameContent::Shape {
            kind,
            geometry,
            path,
            blocks,
        } => {
            let (width, height) = (
                f64::from(f.width.unwrap_or_default()),
                f64::from(f.height.or(f.min_height).unwrap_or_default()),
            );
            let geometry = match (kind.as_str(), geometry.as_deref()) {
                ("rect", _) => Some(m::Geometry::Rect),
                ("ellipse" | "circle", _) => Some(m::Geometry::Ellipse),
                ("line" | "connector", _) => Some(m::Geometry::Line),
                _ => path
                    .as_ref()
                    .and_then(|p| p.outline(width, height))
                    .map(|paths| m::Geometry::Path(paths.into()))
                    .or_else(|| geometry.as_deref().and_then(custom)),
            };
            let text = (!blocks.is_empty()).then(|| m::TextBox {
                blocks: story(doc, blocks, false),
                inset: m::Margins {
                    top: 3.6,
                    right: 7.2,
                    bottom: 3.6,
                    left: 7.2,
                },
                valign: valign(f.valign),
                grow: false,
                fit: false,
            });
            let Some(geometry) = geometry else {
                return match text {
                    Some(text) => m::Shape::Figure(Box::new(m::Figure {
                        geometry: m::Geometry::Rect,
                        fill: None,
                        stroke: None,
                        text: Some(text),
                    })),
                    None => m::Shape::Opaque,
                };
            };
            m::Shape::Figure(Box::new(m::Figure {
                geometry,
                fill: fill(doc, f, f.fill, false),
                // A zero width is a hairline.
                stroke: f.stroke.map(|(w, c)| m::Stroke {
                    width: f64::from(w).max(0.5),
                    paint: paint(c),
                }),
                text,
            }))
        }
        odt::FrameContent::Group(_) | odt::FrameContent::Empty => m::Shape::Opaque,
    }
}

fn custom(kind: &str) -> Option<m::Geometry> {
    Some(match kind {
        "rectangle"
        | "flowchart-process"
        | "frame"
        | "mso-spt202"
        | "flowchart-predefined-process" => m::Geometry::Rect,
        "round-rectangle" | "flowchart-alternate-process" => m::Geometry::RoundRect(1.0 / 6.0),
        "ellipse" | "circle" | "flowchart-connector" => m::Geometry::Ellipse,
        "line" | "mso-spt20" | "mso-spt32" => m::Geometry::Line,
        _ => return None,
    })
}

fn valign(v: odt::VAlign) -> m::VAlign {
    match v {
        odt::VAlign::Top => m::VAlign::Top,
        odt::VAlign::Middle => m::VAlign::Center,
        odt::VAlign::Bottom => m::VAlign::Bottom,
    }
}

fn picture(doc: &odt::Document, index: usize, f: &odt::Frame) -> m::Shape {
    let Some(image) = doc.images.get(index) else {
        return m::Shape::Opaque;
    };
    m::Shape::Picture(m::Picture {
        format: m::ImageFormat::sniff(&image.bytes),
        data: image.bytes.clone(),
        crop: m::Margins::default(),
        outline: outline(f).map(hairline),
        recolor: m::Recolor::default(),
    })
}
