//! Pictures, shapes, groups and text boxes, from DrawingML and from VML.

use std::sync::Arc;

use super::body::Story;
use super::xml::{parse, Element, Node, Ns};
use super::Rels;
use crate::model::*;

const EMU_PER_POINT: f64 = 12700.0;

fn emu(value: Option<&str>) -> Option<f64> {
    value?.trim().parse::<f64>().ok().map(|v| v / EMU_PER_POINT)
}

fn yes(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "t" | "on"))
}

fn text(element: &Element) -> String {
    element
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Text(_, t) => Some(&**t),
            Node::Element(_) => None,
        })
        .collect()
}

fn transform(xfrm: &Element) -> Transform {
    Transform {
        rotation: xfrm
            .attr(Ns::None, "rot")
            .and_then(|r| r.parse::<f64>().ok())
            .map_or(0.0, |r| r / 60000.0),
        flip_h: yes(xfrm.attr(Ns::None, "flipH")),
        flip_v: yes(xfrm.attr(Ns::None, "flipV")),
    }
}

/// Offset and extent of an `a:xfrm` in EMU, `off`/`ext` or `chOff`/`chExt`.
fn frame(xfrm: Option<&Element>, off: &str, ext: &str) -> Option<[f64; 4]> {
    let xfrm = xfrm?;
    let at = |name: &str, a: &str| {
        xfrm.child(Ns::A, name)
            .and_then(|e| e.attr(Ns::None, a))
            .and_then(|v| v.parse::<f64>().ok())
    };
    Some([
        at(off, "x").unwrap_or(0.0),
        at(off, "y").unwrap_or(0.0),
        at(ext, "cx")?,
        at(ext, "cy")?,
    ])
}

/// A `a:blip`'s colour effects, Word's washout being LibreOffice's watermark.
fn blip_recolor(blip: &Element) -> Recolor {
    let mut r = Recolor::default();
    for e in blip.elements().filter(|e| e.ns == Ns::A) {
        let percent = |a: &str| {
            e.attr(Ns::None, a)
                .and_then(|v| v.parse::<f64>().ok())
                .map_or(0.0, |v| (v / 1000.0).trunc().clamp(-100.0, 100.0))
        };
        match &*e.name {
            "lum" => (r.luminance, r.contrast) = (percent("bright"), percent("contrast")),
            "grayscl" => r.mode = ColorMode::Greys,
            "biLevel" => r.mode = ColorMode::Mono,
            _ => {}
        }
    }
    if r.mode == ColorMode::Standard && r.luminance == 70.0 && r.contrast == -70.0 {
        (r.mode, r.luminance, r.contrast) = (ColorMode::Watermark, 0.0, 0.0);
    }
    r.office = r.luminance != 0.0 && r.contrast != 0.0;
    r
}

fn geometry(preset: &str) -> Option<Geometry> {
    Some(match preset {
        "rect"
        | "flowChartProcess"
        | "frame"
        | "snip1Rect"
        | "snip2SameRect"
        | "round1Rect"
        | "round2SameRect"
        | "flowChartPredefinedProcess"
        | "flowChartInternalStorage" => Geometry::Rect,
        "roundRect" | "flowChartAlternateProcess" => Geometry::RoundRect(1.0 / 6.0),
        "ellipse" | "flowChartConnector" => Geometry::Ellipse,
        "line" | "straightConnector1" | "bentConnector2" | "bentConnector3" => Geometry::Line,
        _ => return None,
    })
}

/// Hue, saturation and lightness, each in `0..1`.
fn hsl(c: Color) -> (f64, f64, f64) {
    let (r, g, b) = (
        f64::from(c.0) / 255.0,
        f64::from(c.1) / 255.0,
        f64::from(c.2) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Color {
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |t: f64| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Color(channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

fn named(name: &str) -> Option<Color> {
    Some(match name.to_ascii_lowercase().as_str() {
        "black" | "windowtext" | "buttontext" => Color(0, 0, 0),
        "white" | "window" => Color(255, 255, 255),
        "red" => Color(255, 0, 0),
        "green" => Color(0, 128, 0),
        "lime" => Color(0, 255, 0),
        "blue" => Color(0, 0, 255),
        "yellow" => Color(255, 255, 0),
        "navy" => Color(0, 0, 128),
        "gray" | "grey" => Color(128, 128, 128),
        "silver" => Color(192, 192, 192),
        "maroon" => Color(128, 0, 0),
        "purple" => Color(128, 0, 128),
        "teal" => Color(0, 128, 128),
        "olive" => Color(128, 128, 0),
        "aqua" | "cyan" => Color(0, 255, 255),
        "fuchsia" | "magenta" => Color(255, 0, 255),
        "orange" => Color(255, 165, 0),
        "buttonface" | "threedface" => Color(240, 240, 240),
        _ => return None,
    })
}

/// `#rrggbb`, `#rgb` or a name, an `[index]` after it ignored.
fn vml_color(value: &str) -> Option<Color> {
    let value = value.split_whitespace().next()?.split('[').next()?;
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            6 => Color::hex(hex),
            3 => Color::hex(&hex.chars().flat_map(|c| [c, c]).collect::<String>()),
            _ => None,
        };
    }
    named(value)
}

/// `c` scaled toward black by `k`, or toward white for a tint, in linear
/// light as LibreOffice does.
fn shade(c: Color, k: f64, tint: bool) -> Color {
    let channel = |v: u8| {
        let v = f64::from(v) / 255.0;
        let linear = if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        };
        let out = if tint {
            1.0 - (1.0 - linear) * k
        } else {
            linear * k
        };
        let srgb = if out <= 0.003_130_8 {
            out * 12.92
        } else {
            1.055 * out.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Color(channel(c.0), channel(c.1), channel(c.2))
}

/// A VML colour, `fill darken(n)` and `fill lighten(n)` read against `base`.
fn vml_color_of(value: &str, base: Color) -> Option<Color> {
    let Some(rest) = value.trim().strip_prefix("fill") else {
        return vml_color(value);
    };
    let rest = rest.trim();
    let share = |s: &str| {
        s.trim_end_matches(')')
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| (n / 255.0).clamp(0.0, 1.0))
    };
    if let Some(k) = rest.strip_prefix("darken(").and_then(share) {
        return Some(shade(base, k, false));
    }
    if let Some(k) = rest.strip_prefix("lighten(").and_then(share) {
        return Some(shade(base, k, true));
    }
    Some(base)
}

/// A `v:fill` gradient from `first`, the shape's fill colour.
fn vml_gradient(fill: &Element, first: Paint) -> Option<Gradient> {
    let kind = fill.attr(Ns::None, "type")?;
    if !kind.starts_with("gradient") {
        return None;
    }
    let second = Paint {
        color: fill
            .attr(Ns::None, "color2")
            .and_then(|v| vml_color_of(v, first.color))
            .unwrap_or(Color::WHITE),
        alpha: fill
            .attr(Ns::O, "opacity2")
            .and_then(fraction)
            .unwrap_or(first.alpha),
    };
    let focus = fill
        .attr(Ns::None, "focus")
        .and_then(|f| match f.trim().strip_suffix('%') {
            Some(p) => p.trim().parse::<f64>().ok().map(|p| p / 100.0),
            None => fraction(f),
        })
        .unwrap_or(0.0);
    let center = (kind == "gradientRadial").then(|| {
        let pair = |name: &str| {
            let v: Vec<f64> = fill
                .attr(Ns::None, name)
                .unwrap_or_default()
                .split(',')
                .map(|v| fraction(v).unwrap_or(0.0).clamp(0.0, 1.0))
                .collect();
            [
                v.first().copied().unwrap_or(0.0),
                v.get(1).copied().unwrap_or(0.0),
            ]
        };
        let (at, size) = (pair("focusposition"), pair("focussize"));
        [
            (at[0] + (at[0] + size[0]).min(1.0)) / 2.0,
            (at[1] + (at[1] + size[1]).min(1.0)) / 2.0,
        ]
    });
    let angle = fill
        .attr(Ns::None, "angle")
        .and_then(|a| a.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    Some(Gradient::office(first, second, angle, focus, center))
}

/// The colour of `w:background`, or of the pattern or gradient its VML shape
/// paints, a pattern taken for its background colour.
pub(crate) fn page_background(e: &Element) -> Option<Color> {
    let color = e.w("color").filter(|v| *v != "auto").and_then(Color::hex);
    let Some(shape) = e.child(Ns::V, "background") else {
        return color;
    };
    let fill = shape
        .attr(Ns::None, "fillcolor")
        .and_then(vml_color)
        .or(color);
    let Some(v_fill) = shape.child(Ns::V, "fill") else {
        return fill;
    };
    let second = v_fill.attr(Ns::None, "color2").and_then(vml_color);
    match (v_fill.attr(Ns::None, "type"), fill, second) {
        (Some("pattern"), _, Some(back)) => Some(back),
        (Some(t), Some(a), Some(b)) if t.starts_with("gradient") => Some(Color(
            ((u16::from(a.0) + u16::from(b.0)) / 2) as u8,
            ((u16::from(a.1) + u16::from(b.1)) / 2) as u8,
            ((u16::from(a.2) + u16::from(b.2)) / 2) as u8,
        )),
        _ => fill,
    }
}

/// The picture `w:background`'s VML shape paints, over its colour.
pub(crate) fn page_texture(e: &Element, media: &dyn Fn(&str) -> Option<Arc<[u8]>>) -> Option<Fill> {
    let shape = e.child(Ns::V, "background")?;
    let fore = Paint {
        color: shape
            .attr(Ns::None, "fillcolor")
            .and_then(vml_color)
            .unwrap_or(Color::WHITE),
        alpha: 1.0,
    };
    vml_texture(shape.child(Ns::V, "fill")?, fore, media)
}

/// A `v:fill` picture: a texture repeated at its own size, a picture
/// stretched over the shape, or a two-colour pattern in the fill colour
/// over `color2`.
fn vml_texture(
    fill: &Element,
    fore: Paint,
    media: &dyn Fn(&str) -> Option<Arc<[u8]>>,
) -> Option<Fill> {
    let kind = fill.attr(Ns::None, "type")?;
    if !matches!(kind, "tile" | "frame" | "pattern") {
        return None;
    }
    let mut data = fill
        .attr(Ns::R, "id")
        .or_else(|| fill.attr(Ns::O, "relid"))
        .and_then(media)?;
    if kind == "pattern" {
        let back = fill
            .attr(Ns::None, "color2")
            .and_then(vml_color)
            .unwrap_or(Color::WHITE);
        data = Arc::from(recolored_gif(&data, fore.color, back)?);
    }
    let format = ImageFormat::sniff(&data);
    let tile = match kind {
        "frame" => None,
        _ => Some(format.natural_size(&data)?),
    };
    Some(Fill::Image(Arc::new(ImageFill {
        data,
        format,
        tile,
        smooth: kind != "pattern",
        alpha: fore.alpha,
    })))
}

/// A GIF whose light palette entries turn to `fore` and dark ones to `back`,
/// in its global table and its first image's own.
fn recolored_gif(data: &[u8], fore: Color, back: Color) -> Option<Vec<u8>> {
    if !data.starts_with(b"GIF8") {
        return None;
    }
    let mut out = data.to_vec();
    let mut recolor = |at: usize, flags: u8| -> Option<usize> {
        if flags & 0x80 == 0 {
            return Some(at);
        }
        let len = 3 * (2usize << (flags & 7));
        for entry in out.get_mut(at..at + len)?.as_chunks_mut::<3>().0 {
            let luma =
                299 * u32::from(entry[0]) + 587 * u32::from(entry[1]) + 114 * u32::from(entry[2]);
            let Color(r, g, b) = if luma < 128_000 { back } else { fore };
            *entry = [r, g, b];
        }
        Some(at + len)
    };
    let mut at = recolor(13, *data.get(10)?)?;
    loop {
        match *data.get(at)? {
            0x21 => {
                at += 2;
                while *data.get(at)? != 0 {
                    at += 1 + usize::from(data[at]);
                }
                at += 1;
            }
            0x2C => {
                recolor(at + 10, *data.get(at + 9)?)?;
                return Some(out);
            }
            _ => return None,
        }
    }
}

/// A CSS length, pixels when it has no unit.
fn vml_length(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    Some(match unit {
        "pt" => number,
        "in" => number * 72.0,
        "cm" => number * 72.0 / 2.54,
        "mm" => number * 72.0 / 25.4,
        "pc" | "pi" => number * 12.0,
        "emu" => number / EMU_PER_POINT,
        "" | "px" => number * 0.75,
        _ => return None,
    })
}

/// A number, or `65536`ths when it ends in `f`.
fn fraction(value: &str) -> Option<f64> {
    match value.trim().strip_suffix('f') {
        Some(n) => n.parse::<f64>().ok().map(|v| v / 65536.0),
        None => value.trim().parse().ok(),
    }
}

fn style(value: &str) -> Vec<(String, String)> {
    value
        .split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect()
}

fn get<'s>(style: &'s [(String, String)], key: &str) -> Option<&'s str> {
    style
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn relative(value: &str) -> Option<Relative> {
    Some(match value {
        "page" => Relative::Page,
        "margin" => Relative::Margin,
        "column" | "text" => Relative::Column,
        "paragraph" => Relative::Paragraph,
        "line" => Relative::Line,
        "character" | "char" => Relative::Character,
        "leftMargin" | "left-margin-area" => Relative::LeftMargin,
        "rightMargin" | "right-margin-area" => Relative::RightMargin,
        "topMargin" | "top-margin-area" => Relative::TopMargin,
        "bottomMargin" | "bottom-margin-area" => Relative::BottomMargin,
        "insideMargin" | "inner-margin-area" => Relative::InsideMargin,
        "outsideMargin" | "outer-margin-area" => Relative::OutsideMargin,
        _ => return None,
    })
}

fn align(value: &str) -> Option<Place> {
    Some(match value.trim() {
        "left" | "top" => Place::Start,
        "center" => Place::Center,
        "right" | "bottom" => Place::End,
        "inside" => Place::Inside,
        "outside" => Place::Outside,
        _ => return None,
    })
}

fn wrap_side(value: Option<&str>) -> WrapSide {
    match value {
        Some("left") => WrapSide::Left,
        Some("right") => WrapSide::Right,
        Some("largest") => WrapSide::Largest,
        _ => WrapSide::Both,
    }
}

fn anchor(e: &Element) -> Float {
    let dist = |name: &str| emu(e.attr(Ns::None, name)).unwrap_or(0.0);
    let position = |name: &str, default: Relative| {
        let Some(p) = e.child(Ns::Wp, name) else {
            return Position {
                from: default,
                place: Place::Offset(0.0),
            };
        };
        let from = p
            .attr(Ns::None, "relativeFrom")
            .and_then(relative)
            .unwrap_or(default);
        let place = p
            .child(Ns::Wp, "align")
            .and_then(|a| align(&text(a)))
            .or_else(|| {
                p.child(Ns::Wp, "posOffset")
                    .and_then(|o| emu(Some(&text(o))))
                    .map(Place::Offset)
            })
            .unwrap_or(Place::Offset(0.0));
        Position { from, place }
    };
    let (mut horizontal, mut vertical) = (
        position("positionH", Relative::Column),
        position("positionV", Relative::Paragraph),
    );
    if yes(e.attr(Ns::None, "simplePos")) {
        if let Some(simple) = e.child(Ns::Wp, "simplePos") {
            let at = |a: &str| Position {
                from: Relative::Page,
                place: Place::Offset(emu(simple.attr(Ns::None, a)).unwrap_or(0.0)),
            };
            (horizontal, vertical) = (at("x"), at("y"));
        }
    }
    let (wrap, side, contour) = e
        .elements()
        .filter(|w| w.ns == Ns::Wp)
        .find_map(|w| {
            let side = wrap_side(w.attr(Ns::None, "wrapText"));
            Some(match &*w.name {
                "wrapNone" => (Wrap::None, side, Vec::new()),
                "wrapTopAndBottom" => (Wrap::TopAndBottom, side, Vec::new()),
                "wrapSquare" => (Wrap::Around, side, Vec::new()),
                "wrapTight" | "wrapThrough" => (Wrap::Around, side, wrap_polygon(w)),
                _ => return None,
            })
        })
        .unwrap_or((Wrap::None, WrapSide::Both, Vec::new()));
    Float {
        horizontal,
        vertical,
        wrap,
        side,
        behind: yes(e.attr(Ns::None, "behindDoc")),
        distance: Margins {
            top: dist("distT"),
            right: dist("distR"),
            bottom: dist("distB"),
            left: dist("distL"),
        },
        z: e.attr(Ns::None, "relativeHeight")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        contour,
        in_cell: e.attr(Ns::None, "layoutInCell") != Some("0"),
    }
}

/// The first element named `name` under `e`, depth first.
fn descendant<'e>(e: &'e Element, name: &str) -> Option<&'e Element> {
    e.elements().find_map(|c| {
        if &*c.name == name {
            Some(c)
        } else {
            descendant(c, name)
        }
    })
}

/// Where a polygon's coordinates reach the frame's far edge.
const POLYGON_UNIT: f64 = 21600.0;

/// The points of a `wp:wrapPolygon`, as shares of the frame.
fn wrap_polygon(wrap: &Element) -> Vec<[f64; 2]> {
    let Some(polygon) = wrap.child(Ns::Wp, "wrapPolygon") else {
        return Vec::new();
    };
    polygon
        .elements()
        .filter(|p| p.ns == Ns::Wp && matches!(&*p.name, "start" | "lineTo"))
        .filter_map(|p| {
            let at = |a: &str| p.attr(Ns::None, a)?.parse::<f64>().ok();
            Some([at("x")? / POLYGON_UNIT, at("y")? / POLYGON_UNIT])
        })
        .collect()
}

/// The points of a VML shape's `wrapcoords`, as shares of the frame.
fn wrap_coords(coords: &str) -> Vec<[f64; 2]> {
    let values: Vec<f64> = coords
        .split([' ', ','])
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[x, y]| [x / POLYGON_UNIT, y / POLYGON_UNIT])
        .collect()
}

impl Story<'_, '_> {
    pub(super) fn drawing(&mut self, element: &Element, props: &RunProps, out: &mut Vec<Inline>) {
        let mut found = Vec::new();
        for e in element.elements() {
            match (e.ns, &*e.name) {
                (Ns::Wp, "inline") => found.extend(self.dml(e, false, props)),
                (Ns::Wp, "anchor") => found.extend(self.dml(e, true, props)),
                (Ns::W, "drawing") => self.drawing(e, props, out),
                (Ns::V, "shape" | "rect" | "roundrect" | "oval" | "line" | "image" | "group") => {
                    found.extend(self.vml(e, props))
                }
                _ => {}
            }
        }
        out.extend(found.into_iter().map(|d| Inline::Drawing(Box::new(d))));
    }

    fn media(&self, id: &str) -> Option<Arc<[u8]>> {
        let rels = match &self.diagram {
            Some((_, rels)) => rels,
            None => self.rels?,
        };
        let name = rels.by_id(id)?;
        let mut cache = self.ctx.media.borrow_mut();
        if let Some(data) = cache.get(name) {
            return Some(data.clone());
        }
        let data: Arc<[u8]> = Arc::from(self.ctx.package.entry(name)?);
        cache.insert(name.to_string(), data.clone());
        Some(data)
    }

    fn dml(&mut self, e: &Element, anchored: bool, props: &RunProps) -> Option<Drawing> {
        if e.child(Ns::Wp, "docPr")
            .is_some_and(|d| yes(d.attr(Ns::None, "hidden")))
        {
            return None;
        }
        let extent = e.child(Ns::Wp, "extent")?;
        let width = emu(extent.attr(Ns::None, "cx"))?;
        let height = emu(extent.attr(Ns::None, "cy"))?;
        let data = e.child(Ns::A, "graphic")?.child(Ns::A, "graphicData")?;
        let (shape, transform) = self.graphic(data, width, height);
        Some(Drawing {
            width,
            height,
            float: anchored.then(|| Box::new(anchor(e))),
            shape,
            transform,
            props: props.clone(),
        })
    }

    fn graphic(&mut self, data: &Element, width: f64, height: f64) -> (Shape, Transform) {
        for child in data.elements() {
            match (child.ns, &*child.name) {
                (Ns::Pic, "pic") => return self.picture(child),
                (Ns::C, "chart") => {
                    if let Some(chart) = self.chart(child) {
                        return (Shape::Chart(Box::new(chart)), Transform::default());
                    }
                }
                (Ns::Wps, "wsp") => return self.wsp(child, width, height),
                (Ns::Wpg, "wgp") => {
                    let xfrm = child
                        .child(Ns::Wpg, "grpSpPr")
                        .and_then(|p| p.child(Ns::A, "xfrm"));
                    return (
                        self.group(child, width, height),
                        xfrm.map(transform).unwrap_or_default(),
                    );
                }
                (Ns::Wpc, "wpc") => {
                    return (self.canvas(child, width, height), Transform::default())
                }
                (Ns::Dgm, "relIds") => {
                    if let Some(shape) = self.diagram(child, width, height) {
                        return (shape, Transform::default());
                    }
                }
                _ => {}
            }
        }
        (Shape::Opaque, Transform::default())
    }

    fn picture(&mut self, pic: &Element) -> (Shape, Transform) {
        let sp = pic.child(Ns::Pic, "spPr");
        let transform = sp
            .and_then(|s| s.child(Ns::A, "xfrm"))
            .map(transform)
            .unwrap_or_default();
        let outline = sp
            .and_then(|s| s.child(Ns::A, "ln"))
            .and_then(|ln| self.stroke(ln, None));
        (
            self.blip(pic.child(Ns::Pic, "blipFill"), outline),
            transform,
        )
    }

    fn blip(&self, fill: Option<&Element>, outline: Option<Stroke>) -> Shape {
        let data = fill
            .and_then(|f| f.child(Ns::A, "blip"))
            .and_then(|b| b.attr(Ns::R, "embed"))
            .and_then(|id| self.media(id));
        let Some(data) = data else {
            return Shape::Opaque;
        };
        let crop = fill
            .and_then(|f| f.child(Ns::A, "srcRect"))
            .map(|r| {
                let side = |name: &str| {
                    r.attr(Ns::None, name)
                        .and_then(|v| v.parse::<f64>().ok())
                        .map_or(0.0, |v| v / 100_000.0)
                };
                Margins {
                    top: side("t"),
                    right: side("r"),
                    bottom: side("b"),
                    left: side("l"),
                }
            })
            .unwrap_or_default();
        Shape::Picture(Picture {
            format: ImageFormat::sniff(&data),
            data,
            crop,
            outline,
            recolor: fill
                .and_then(|f| f.child(Ns::A, "blip"))
                .map(blip_recolor)
                .unwrap_or_default(),
        })
    }

    /// A colour element and its modifiers, `phClr` standing for `placeholder`.
    pub(super) fn paint(&self, e: &Element, placeholder: Option<Paint>) -> Option<Paint> {
        let value = |a: &str| e.attr(Ns::None, a);
        let base = match &*e.name {
            "srgbClr" => Color::hex(value("val")?)?,
            "sysClr" => value("lastClr")
                .and_then(Color::hex)
                .or_else(|| value("val").and_then(named))?,
            "prstClr" => named(value("val")?)?,
            "schemeClr" if value("val") == Some("phClr") => placeholder?.color,
            "schemeClr" => self.ctx.theme.color(value("val")?)?,
            "scrgbClr" => {
                let c = |a: &str| {
                    value(a).and_then(|v| v.parse::<f64>().ok()).map_or(0, |v| {
                        (v / 100_000.0 * 255.0).round().clamp(0.0, 255.0) as u8
                    })
                };
                Color(c("r"), c("g"), c("b"))
            }
            _ => return None,
        };
        let (mut h, mut s, mut l) = hsl(base);
        let mut color = base;
        let mut alpha = 1.0;
        let mut pending = false;
        for m in e.elements() {
            let Some(v) = m
                .attr(Ns::None, "val")
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| v / 100_000.0)
            else {
                continue;
            };
            match &*m.name {
                "lumMod" => (l, pending) = (l * v, true),
                "lumOff" => (l, pending) = (l + v, true),
                "satMod" => (s, pending) = (s * v, true),
                "alpha" => alpha = v,
                "tint" | "shade" => {
                    if pending {
                        color = from_hsl(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
                        pending = false;
                    }
                    let tint = &*m.name == "tint";
                    let mix = |c: u8| {
                        let c = f64::from(c);
                        let out = if tint { 255.0 - (255.0 - c) * v } else { c * v };
                        out.round().clamp(0.0, 255.0) as u8
                    };
                    color = Color(mix(color.0), mix(color.1), mix(color.2));
                    (h, s, l) = hsl(color);
                }
                _ => {}
            }
        }
        if pending {
            color = from_hsl(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
        }
        Some(Paint { color, alpha })
    }

    /// The paint of a fill: `Some(None)` is no fill, `None` names none.
    pub(super) fn fill(
        &self,
        container: &Element,
        placeholder: Option<Paint>,
    ) -> Option<Option<Paint>> {
        let colour = |holder: Option<&Element>| {
            holder
                .and_then(|h| h.elements().next())
                .and_then(|c| self.paint(c, placeholder))
        };
        for e in container.elements().filter(|e| e.ns == Ns::A) {
            match &*e.name {
                "noFill" => return Some(None),
                "solidFill" => return Some(colour(Some(e))),
                "gradFill" => {
                    let stop = e.child(Ns::A, "gsLst").and_then(|l| l.child(Ns::A, "gs"));
                    return Some(colour(stop));
                }
                "pattFill" => return Some(colour(e.child(Ns::A, "fgClr"))),
                _ => {}
            }
        }
        None
    }

    /// A shape's fill: `Some(None)` is no fill, `None` names none.
    fn shape_fill(&self, container: &Element, placeholder: Option<Paint>) -> Option<Option<Fill>> {
        if let Some(g) = container
            .child(Ns::A, "gradFill")
            .and_then(|g| self.gradient(g, placeholder))
        {
            return Some(Some(Fill::Gradient(Arc::new(g))));
        }
        self.fill(container, placeholder)
            .map(|paint| paint.map(Fill::Solid))
    }

    fn gradient(&self, g: &Element, placeholder: Option<Paint>) -> Option<Gradient> {
        let mut stops: Vec<(f64, Paint)> = g
            .child(Ns::A, "gsLst")?
            .elements()
            .filter(|gs| &*gs.name == "gs")
            .filter_map(|gs| {
                let pos = gs.attr(Ns::None, "pos")?.parse::<f64>().ok()? / 100_000.0;
                Some((
                    pos.clamp(0.0, 1.0),
                    self.paint(gs.elements().next()?, placeholder)?,
                ))
            })
            .collect();
        if stops.is_empty() {
            return None;
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let number = |e: Option<&Element>, name: &str| {
            e.and_then(|e| e.attr(Ns::None, name))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let shape = match g.child(Ns::A, "path") {
            Some(path) => {
                let rect = path.child(Ns::A, "fillToRect");
                let side = |name: &str| number(rect, name) / 100_000.0;
                GradientShape::Radial([
                    (side("l") + 1.0 - side("r")) / 2.0,
                    (side("t") + 1.0 - side("b")) / 2.0,
                ])
            }
            None => GradientShape::Linear(number(g.child(Ns::A, "lin"), "ang") / 60_000.0),
        };
        Some(Gradient { shape, stops })
    }

    pub(super) fn stroke(&self, ln: &Element, style: Option<(f64, Paint)>) -> Option<Stroke> {
        let paint = match self.fill(ln, style.map(|s| s.1)) {
            Some(paint) => paint?,
            None => style?.1,
        };
        let width = emu(ln.attr(Ns::None, "w"))
            .or(style.map(|s| s.0))
            .unwrap_or(0.75);
        Some(Stroke { width, paint })
    }

    /// A `wps:style` reference's index and colour, `None` for index 0.
    fn style_ref(&self, wsp: &Element, name: &str) -> Option<(usize, Paint)> {
        let r = wsp.child(wsp.ns, "style")?.child(Ns::A, name)?;
        let idx: usize = r.attr(Ns::None, "idx")?.parse().ok()?;
        let paint = r.elements().find_map(|c| self.paint(c, None))?;
        (idx > 0).then_some((idx, paint))
    }

    /// A shape `width` by `height`, of a Word drawing or a SmartArt one.
    fn wsp(&mut self, wsp: &Element, width: f64, height: f64) -> (Shape, Transform) {
        let sp = wsp.child(wsp.ns, "spPr");
        let transform = sp
            .and_then(|s| s.child(Ns::A, "xfrm"))
            .map(transform)
            .unwrap_or_default();
        let text = self.shape_text(wsp);
        if let Some(fill) = sp.and_then(|s| s.child(Ns::A, "blipFill")) {
            let picture = self.blip(Some(fill), None);
            let Some(text) = text else {
                return (picture, transform);
            };
            let child = |shape| Child {
                x: 0.0,
                y: 0.0,
                width,
                height,
                shape,
                transform: Transform::default(),
            };
            let over = Shape::Figure(Box::new(Figure {
                geometry: Geometry::Rect,
                fill: None,
                stroke: None,
                text: Some(text),
            }));
            return (Shape::Group(vec![child(picture), child(over)]), transform);
        }
        let preset = sp
            .and_then(|s| s.child(Ns::A, "prstGeom"))
            .and_then(|g| g.attr(Ns::None, "prst"))
            .and_then(geometry);
        let fill_style = self.style_ref(wsp, "fillRef").map(|(_, p)| p);
        let line_style = self
            .style_ref(wsp, "lnRef")
            .map(|(idx, p)| (idx.min(3) as f64 * 0.5, p));
        let fill = sp
            .and_then(|s| self.shape_fill(s, fill_style))
            .unwrap_or(fill_style.map(Fill::Solid));
        let stroke = match sp.and_then(|s| s.child(Ns::A, "ln")) {
            Some(ln) => self.stroke(ln, line_style),
            None => line_style.map(|(width, paint)| Stroke { width, paint }),
        };
        let figure = match preset {
            Some(geometry) => Figure {
                geometry,
                fill,
                stroke,
                text,
            },
            None => Figure {
                geometry: Geometry::Rect,
                fill: None,
                stroke: None,
                text,
            },
        };
        (Shape::Figure(Box::new(figure)), transform)
    }

    /// The text of a shape: a Word text box, or the DrawingML text of a
    /// SmartArt shape, in the colour its style names.
    fn shape_text(&mut self, wsp: &Element) -> Option<TextBox> {
        let (body, blocks) = if wsp.ns == Ns::Dsp {
            let tx = wsp.child(Ns::Dsp, "txBody")?;
            let color = wsp
                .child(Ns::Dsp, "style")
                .and_then(|s| s.child(Ns::A, "fontRef"))
                .and_then(|r| r.elements().find_map(|c| self.paint(c, None)))
                .map(|p| p.color);
            let part = self.diagram.as_ref().and_then(|d| d.0);
            (
                tx.child(Ns::A, "bodyPr"),
                self.dml_text(tx, part, 18.0, color),
            )
        } else {
            let content = wsp.child(Ns::Wps, "txbx")?.wc("txbxContent")?;
            (wsp.child(Ns::Wps, "bodyPr"), self.text_box(content).ok()?)
        };
        let inset = |name: &str, default: f64| {
            emu(body.and_then(|b| b.attr(Ns::None, name))).unwrap_or(default)
        };
        Some(TextBox {
            blocks,
            inset: Margins {
                top: inset("tIns", 3.6),
                right: inset("rIns", 7.2),
                bottom: inset("bIns", 3.6),
                left: inset("lIns", 7.2),
            },
            valign: match body.and_then(|b| b.attr(Ns::None, "anchor")) {
                Some("ctr") => VAlign::Center,
                Some("b") => VAlign::Bottom,
                _ => VAlign::Top,
            },
            grow: body.is_some_and(|b| b.child(Ns::A, "spAutoFit").is_some()),
            fit: false,
        })
    }

    /// A group's children, placed in the `width` by `height` frame it fills.
    fn group(&mut self, g: &Element, width: f64, height: f64) -> Shape {
        let xfrm = g
            .child(g.ns, "grpSpPr")
            .and_then(|p| p.child(Ns::A, "xfrm"));
        let space = frame(xfrm, "chOff", "chExt").or_else(|| frame(xfrm, "off", "ext"));
        let [ox, oy, cw, ch] =
            space.unwrap_or([0.0, 0.0, width * EMU_PER_POINT, height * EMU_PER_POINT]);
        let mut children = Vec::new();
        self.children(g, [ox, oy, cw, ch], width, height, &mut children);
        Shape::Group(children)
    }

    /// The shapes Word saved for a SmartArt diagram, which its data part
    /// names.
    fn diagram(&mut self, ids: &Element, width: f64, height: f64) -> Option<Shape> {
        let rels = self.rels?;
        let package = self.ctx.package;
        let data = parse(package.entry(rels.by_id(ids.attr(Ns::R, "dm")?)?)?).ok()?;
        let name = rels.by_id(descendant(&data, "dataModelExt")?.attr(Ns::None, "relId")?)?;
        let (part, bytes) = match package.part(name) {
            Some((part, bytes)) => (Some(part), bytes),
            None => (None, package.entry(name)?),
        };
        let root = parse(bytes).ok()?;
        let tree = root.child(Ns::Dsp, "spTree")?;
        let own = Rels::read(package, name).ok()?;
        let outer = self.diagram.replace((part, own));
        let mut children = Vec::new();
        let space = [0.0, 0.0, width * EMU_PER_POINT, height * EMU_PER_POINT];
        self.children(tree, space, width, height, &mut children);
        self.diagram = outer;
        Some(Shape::Group(children))
    }

    /// A drawing canvas, its background and outline under shapes placed in
    /// its own extent.
    fn canvas(&mut self, c: &Element, width: f64, height: f64) -> Shape {
        let fill = c
            .child(Ns::Wpc, "bg")
            .and_then(|bg| self.shape_fill(bg, None))
            .flatten();
        let stroke = c
            .child(Ns::Wpc, "whole")
            .and_then(|w| w.child(Ns::A, "ln"))
            .and_then(|ln| self.stroke(ln, None));
        let mut children = Vec::new();
        if fill.is_some() || stroke.is_some() {
            children.push(Child {
                x: 0.0,
                y: 0.0,
                width,
                height,
                shape: Shape::Figure(Box::new(Figure {
                    geometry: Geometry::Rect,
                    fill,
                    stroke,
                    text: None,
                })),
                transform: Transform::default(),
            });
        }
        let space = [0.0, 0.0, width * EMU_PER_POINT, height * EMU_PER_POINT];
        self.children(c, space, width, height, &mut children);
        Shape::Group(children)
    }

    /// The shapes, pictures and groups of `g`, whose child `space` fills a
    /// `width` by `height` frame.
    fn children(
        &mut self,
        g: &Element,
        space: [f64; 4],
        width: f64,
        height: f64,
        children: &mut Vec<Child>,
    ) {
        let [ox, oy, cw, ch] = space;
        let sx = if cw > 0.0 { width / cw } else { 0.0 };
        let sy = if ch > 0.0 { height / ch } else { 0.0 };
        for child in g.elements() {
            let props = match (child.ns, &*child.name) {
                (Ns::Wps, "wsp") | (Ns::Dsp, "sp") => child.child(child.ns, "spPr"),
                (Ns::Pic, "pic") => child.child(Ns::Pic, "spPr"),
                (Ns::Wpg, "grpSp" | "wgp") | (Ns::Dsp, "grpSp") => child.child(child.ns, "grpSpPr"),
                _ => continue,
            };
            let xfrm = props.and_then(|p| p.child(Ns::A, "xfrm"));
            let Some([x, y, w, h]) = frame(xfrm, "off", "ext") else {
                continue;
            };
            let (x, y, w, h) = ((x - ox) * sx, (y - oy) * sy, w * sx, h * sy);
            let (shape, transform) = match &*child.name {
                "wsp" | "sp" => self.wsp(child, w, h),
                "pic" => self.picture(child),
                _ => (
                    self.group(child, w, h),
                    xfrm.map(transform).unwrap_or_default(),
                ),
            };
            children.push(Child {
                x,
                y,
                width: w,
                height: h,
                shape,
                transform,
            });
        }
    }

    /// A VML shape, floating when its style places it absolutely.
    fn vml(&mut self, e: &Element, props: &RunProps) -> Option<Drawing> {
        let css = style(e.attr(Ns::None, "style").unwrap_or_default());
        if get(&css, "visibility") == Some("hidden") {
            return None;
        }
        let length = |key: &str| get(&css, key).and_then(vml_length);
        let mut transform = vml_transform(&css);
        let mut origin = (None, None);
        let (width, height) = if &*e.name == "line" {
            let (from, to) = (line_end(e, "from"), line_end(e, "to"));
            transform.flip_h ^= (to.0 < from.0) != (to.1 < from.1);
            origin = (Some(from.0.min(to.0)), Some(from.1.min(to.1)));
            ((to.0 - from.0).abs(), (to.1 - from.1).abs())
        } else {
            (length("width")?, length("height")?)
        };
        let shape = self.vml_shape(e, &css, width, height);
        let float = (get(&css, "position") == Some("absolute")).then(|| {
            let wrap = e.child(Ns::W10, "wrap");
            let wrap_type = wrap.and_then(|w| w.attr(Ns::None, "type"));
            let offset = |a: &str, b: &str, line: Option<f64>| {
                length(a).or_else(|| length(b)).or(line).unwrap_or(0.0)
            };
            let place = |key: &str, offset: f64| {
                get(&css, key)
                    .and_then(align)
                    .unwrap_or(Place::Offset(offset))
            };
            let z: i64 = get(&css, "z-index")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let distance = |key: &str, default: f64| length(key).unwrap_or(default);
            Box::new(Float {
                horizontal: Position {
                    from: get(&css, "mso-position-horizontal-relative")
                        .and_then(relative)
                        .unwrap_or(Relative::Column),
                    place: place(
                        "mso-position-horizontal",
                        offset("margin-left", "left", origin.0),
                    ),
                },
                vertical: Position {
                    from: get(&css, "mso-position-vertical-relative")
                        .and_then(|v| match v {
                            "text" => Some(Relative::Paragraph),
                            v => relative(v),
                        })
                        .unwrap_or(Relative::Paragraph),
                    place: place(
                        "mso-position-vertical",
                        offset("margin-top", "top", origin.1),
                    ),
                },
                wrap: match wrap_type {
                    Some("square" | "tight" | "through") => Wrap::Around,
                    Some("topAndBottom") => Wrap::TopAndBottom,
                    _ => Wrap::None,
                },
                side: wrap_side(wrap.and_then(|w| w.attr(Ns::None, "side"))),
                behind: z < 0,
                distance: Margins {
                    top: distance("mso-wrap-distance-top", 0.0),
                    right: distance("mso-wrap-distance-right", 9.0),
                    bottom: distance("mso-wrap-distance-bottom", 0.0),
                    left: distance("mso-wrap-distance-left", 9.0),
                },
                z,
                contour: match (wrap_type, e.attr(Ns::None, "wrapcoords")) {
                    (Some("tight" | "through"), Some(coords)) => wrap_coords(coords),
                    _ => Vec::new(),
                },
                in_cell: !matches!(e.attr(Ns::O, "allowincell"), Some("f" | "false")),
            })
        });
        Some(Drawing {
            width,
            height,
            float,
            shape,
            transform,
            props: props.clone(),
        })
    }

    fn vml_shape(
        &mut self,
        e: &Element,
        css: &[(String, String)],
        width: f64,
        height: f64,
    ) -> Shape {
        if &*e.name == "group" {
            return self.vml_group(e, width, height);
        }
        let kind = e
            .attr(Ns::None, "type")
            .and_then(|t| t.rsplit_once("_t"))
            .and_then(|(_, n)| n.parse::<u32>().ok())
            .or_else(|| {
                e.attr(Ns::O, "spt")
                    .and_then(|n| n.parse::<f32>().ok())
                    .map(|n| n as u32)
            });
        if let Some(image) = e.child(Ns::V, "imagedata") {
            let data = image
                .attr(Ns::R, "id")
                .or_else(|| image.attr(Ns::O, "relid"))
                .and_then(|id| self.media(id));
            let Some(data) = data else {
                return Shape::Opaque;
            };
            let side = |name: &str| image.attr(Ns::None, name).and_then(fraction).unwrap_or(0.0);
            let gain = image.attr(Ns::None, "gain").and_then(fraction);
            let black = image.attr(Ns::None, "blacklevel").and_then(fraction);
            let mode = if yes(image.attr(Ns::None, "grayscale")) {
                ColorMode::Greys
            } else if gain.is_some_and(|g| (g - 0.3).abs() < 0.01)
                && black.is_some_and(|b| (b - 0.35).abs() < 0.01)
            {
                ColorMode::Watermark
            } else {
                ColorMode::Standard
            };
            return Shape::Picture(Picture {
                format: ImageFormat::sniff(&data),
                data,
                crop: Margins {
                    top: side("croptop"),
                    right: side("cropright"),
                    bottom: side("cropbottom"),
                    left: side("cropleft"),
                },
                outline: None,
                recolor: Recolor {
                    mode,
                    ..Recolor::default()
                },
            });
        }
        if let Some(path) = e.child(Ns::V, "textpath") {
            return self.word_art(e, path);
        }
        if kind == Some(136) {
            return Shape::Opaque;
        }
        let geometry = match (&*e.name, kind) {
            ("rect", _) | (_, Some(1 | 202)) => Some(Geometry::Rect),
            ("roundrect", _) | (_, Some(2)) => Some(Geometry::RoundRect(
                e.attr(Ns::None, "arcsize")
                    .and_then(fraction)
                    .unwrap_or(0.2)
                    / 2.0,
            )),
            ("oval", _) | (_, Some(3)) => Some(Geometry::Ellipse),
            ("line", _) | (_, Some(20 | 32)) => Some(Geometry::Line),
            _ => None,
        };
        let text = e.child(Ns::V, "textbox").and_then(|t| {
            let blocks = self.text_box(t.wc("txbxContent")?).ok()?;
            let insets: Vec<Option<f64>> = t
                .attr(Ns::None, "inset")
                .unwrap_or_default()
                .split(',')
                .map(vml_length)
                .collect();
            let inset =
                |i: usize, default: f64| insets.get(i).copied().flatten().unwrap_or(default);
            let box_style = style(t.attr(Ns::None, "style").unwrap_or_default());
            Some(TextBox {
                blocks,
                inset: Margins {
                    left: inset(0, 7.2),
                    top: inset(1, 3.6),
                    right: inset(2, 7.2),
                    bottom: inset(3, 3.6),
                },
                valign: match get(css, "v-text-anchor") {
                    Some(a) if a.starts_with("middle") => VAlign::Center,
                    Some(a) if a.starts_with("bottom") => VAlign::Bottom,
                    _ => VAlign::Top,
                },
                grow: get(&box_style, "mso-fit-shape-to-text") == Some("t"),
                fit: false,
            })
        });
        let Some(geometry) = geometry.or(text.is_some().then_some(Geometry::Rect)) else {
            return Shape::Opaque;
        };
        let fill_child = e.child(Ns::V, "fill");
        let stroke_child = e.child(Ns::V, "stroke");
        let off = |v: Option<&str>| matches!(v, Some("f" | "false" | "0"));
        let filled = !off(e.attr(Ns::None, "filled"))
            && !off(fill_child.and_then(|f| f.attr(Ns::None, "on")))
            && geometry != Geometry::Line;
        let fill = filled.then(|| {
            let paint = Paint {
                color: e
                    .attr(Ns::None, "fillcolor")
                    .and_then(vml_color)
                    .unwrap_or(Color::WHITE),
                alpha: fill_child
                    .and_then(|f| f.attr(Ns::None, "opacity"))
                    .and_then(fraction)
                    .unwrap_or(1.0),
            };
            if let Some(texture) =
                fill_child.and_then(|f| vml_texture(f, paint, &|id| self.media(id)))
            {
                return texture;
            }
            match fill_child.and_then(|f| vml_gradient(f, paint)) {
                Some(g) => Fill::Gradient(Arc::new(g)),
                None => Fill::Solid(paint),
            }
        });
        let stroked = !off(e.attr(Ns::None, "stroked"))
            && !off(stroke_child.and_then(|s| s.attr(Ns::None, "on")));
        let stroke = stroked.then(|| Stroke {
            width: e
                .attr(Ns::None, "strokeweight")
                .or_else(|| stroke_child.and_then(|s| s.attr(Ns::None, "weight")))
                .and_then(vml_length)
                .unwrap_or(0.75),
            paint: Paint {
                color: e
                    .attr(Ns::None, "strokecolor")
                    .or_else(|| stroke_child.and_then(|s| s.attr(Ns::None, "color")))
                    .and_then(vml_color)
                    .unwrap_or(Color::BLACK),
                alpha: 1.0,
            },
        });
        Shape::Figure(Box::new(Figure {
            geometry,
            fill,
            stroke,
            text,
        }))
    }

    /// A `v:textpath`'s string stretched over the shape, in its fill colour.
    fn word_art(&self, e: &Element, path: &Element) -> Shape {
        let Some(text) = path
            .attr(Ns::None, "string")
            .filter(|t| !t.trim().is_empty())
        else {
            return Shape::Opaque;
        };
        let css = style(path.attr(Ns::None, "style").unwrap_or_default());
        let family: Arc<str> = Arc::from(
            get(&css, "font-family")
                .unwrap_or("Arial")
                .trim_matches(|c| c == '"' || c == '\'' || c == ' '),
        );
        let fill = e.child(Ns::V, "fill");
        let props = RunProps {
            fonts: FontSet {
                ascii: family.clone(),
                h_ansi: family.clone(),
                east_asia: family.clone(),
                complex: family,
            },
            size: 36.0,
            size_complex: 36.0,
            bold: get(&css, "font-weight") == Some("bold"),
            italic: get(&css, "font-style") == Some("italic"),
            color: Some(
                e.attr(Ns::None, "fillcolor")
                    .and_then(vml_color)
                    .unwrap_or(Color::WHITE),
            ),
            ..RunProps::default()
        };
        Shape::WordArt(Box::new(WordArt {
            text: text.to_string(),
            props,
            alpha: fill
                .and_then(|f| f.attr(Ns::None, "opacity"))
                .and_then(fraction)
                .unwrap_or(1.0),
        }))
    }

    /// A `v:group`, its children placed by `coordorigin` and `coordsize`.
    fn vml_group(&mut self, g: &Element, width: f64, height: f64) -> Shape {
        let pair = |name: &str, default: (f64, f64)| {
            g.attr(Ns::None, name)
                .and_then(|v| {
                    let (a, b) = v.split_once(',')?;
                    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                })
                .unwrap_or(default)
        };
        let (ox, oy) = pair("coordorigin", (0.0, 0.0));
        let (cw, ch) = pair("coordsize", (1000.0, 1000.0));
        let sx = if cw != 0.0 { width / cw } else { 0.0 };
        let sy = if ch != 0.0 { height / ch } else { 0.0 };
        let mut children = Vec::new();
        for child in g.elements().filter(|c| c.ns == Ns::V) {
            if !matches!(
                &*child.name,
                "shape" | "rect" | "roundrect" | "oval" | "line" | "image" | "group"
            ) {
                continue;
            }
            let css = style(child.attr(Ns::None, "style").unwrap_or_default());
            let number = |key: &str| get(&css, key).and_then(|v| v.trim().parse::<f64>().ok());
            let (x, y, w, h) = if &*child.name == "line" {
                let (from, to) = (line_end(child, "from"), line_end(child, "to"));
                (
                    from.0.min(to.0),
                    from.1.min(to.1),
                    (to.0 - from.0).abs(),
                    (to.1 - from.1).abs(),
                )
            } else {
                (
                    number("left").unwrap_or(0.0),
                    number("top").unwrap_or(0.0),
                    number("width").unwrap_or(0.0),
                    number("height").unwrap_or(0.0),
                )
            };
            let (x, y, w, h) = ((x - ox) * sx, (y - oy) * sy, w * sx, h * sy);
            let shape = self.vml_shape(child, &css, w, h);
            children.push(Child {
                x,
                y,
                width: w,
                height: h,
                shape,
                transform: vml_transform(&css),
            });
        }
        Shape::Group(children)
    }
}

fn line_end(e: &Element, name: &str) -> (f64, f64) {
    e.attr(Ns::None, name)
        .and_then(|v| {
            let (a, b) = v.split_once(',')?;
            Some((vml_length(a)?, vml_length(b)?))
        })
        .unwrap_or((0.0, 0.0))
}

fn vml_transform(css: &[(String, String)]) -> Transform {
    let flip = get(css, "flip").unwrap_or_default();
    Transform {
        rotation: get(css, "rotation")
            .and_then(|r| match r.strip_suffix("fd") {
                Some(fixed) => fixed.parse::<f64>().ok().map(|v| v / 65536.0),
                None => r.parse().ok(),
            })
            .unwrap_or(0.0),
        flip_h: flip.contains('x'),
        flip_v: flip.contains('y'),
    }
}
