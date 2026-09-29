//! The Windows device context WMF and EMF records drive: objects, mapping,
//! paths, and what they draw.

use std::sync::Arc;

use super::{arc, ellipse, flatten, rectangle, ArcKind, Bitmap, HAlign, Op, Path, Text, TextAlign};
use crate::model::Color;

#[derive(Clone)]
pub(super) struct Pen {
    pub color: Color,
    /// Logical units, 0 for a hairline, `None` for no line.
    pub width: Option<f64>,
}

#[derive(Clone)]
pub(super) struct Brush {
    pub color: Option<Color>,
}

#[derive(Clone)]
pub(super) struct Font {
    pub family: String,
    pub height: f64,
    pub bold: bool,
    pub italic: bool,
    pub rotated: bool,
}

#[derive(Clone)]
pub(super) enum Object {
    Pen(Pen),
    Brush(Brush),
    Font(Font),
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MapMode {
    Text,
    LoMetric,
    HiMetric,
    LoEnglish,
    HiEnglish,
    Twips,
    Isotropic,
    Anisotropic,
}

impl MapMode {
    pub fn from_code(code: u32) -> Self {
        match code {
            2 => Self::LoMetric,
            3 => Self::HiMetric,
            4 => Self::LoEnglish,
            5 => Self::HiEnglish,
            6 => Self::Twips,
            7 => Self::Isotropic,
            8 => Self::Anisotropic,
            _ => Self::Text,
        }
    }

    fn millimetres(self) -> f64 {
        match self {
            Self::LoMetric => 0.1,
            Self::HiMetric => 0.01,
            Self::LoEnglish => 0.254,
            Self::HiEnglish => 0.0254,
            Self::Twips => 25.4 / 1440.0,
            _ => 1.0,
        }
    }
}

#[derive(Clone)]
struct Dc {
    pen: Pen,
    brush: Brush,
    font: Font,
    text: Color,
    background: Color,
    align: u32,
    winding: bool,
    map: MapMode,
    window_org: [f64; 2],
    window_ext: [f64; 2],
    viewport_org: [f64; 2],
    viewport_ext: [f64; 2],
    /// m11, m12, m21, m22, dx, dy.
    world: [f64; 6],
    at: [f64; 2],
}

pub(super) struct Gdi {
    pub ops: Vec<Op>,
    dc: Dc,
    saved: Vec<Dc>,
    objects: Vec<Option<Object>>,
    pixels_per_mm: [f64; 2],
    /// The figures of an open path bracket, on the device.
    path: Option<Vec<Vec<[f64; 2]>>>,
}

const TA_UPDATECP: u32 = 1;

fn bezier_controls(n: usize) -> Vec<bool> {
    (0..n).map(|i| i % 3 != 0).collect()
}

impl Gdi {
    pub fn new(pixels_per_mm: [f64; 2]) -> Self {
        Self {
            ops: Vec::new(),
            dc: Dc {
                pen: Pen {
                    color: Color::BLACK,
                    width: Some(0.0),
                },
                brush: Brush {
                    color: Some(Color::WHITE),
                },
                font: Font {
                    family: String::new(),
                    height: 12.0,
                    bold: false,
                    italic: false,
                    rotated: false,
                },
                text: Color::BLACK,
                background: Color::WHITE,
                align: 0,
                winding: false,
                map: MapMode::Text,
                window_org: [0.0, 0.0],
                window_ext: [1.0, 1.0],
                viewport_org: [0.0, 0.0],
                viewport_ext: [1.0, 1.0],
                world: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                at: [0.0, 0.0],
            },
            saved: Vec::new(),
            objects: Vec::new(),
            pixels_per_mm,
            path: None,
        }
    }

    pub fn device(&self, p: [f64; 2]) -> [f64; 2] {
        let dc = &self.dc;
        let [a, b, c, d, e, f] = dc.world;
        let (x, y) = (a * p[0] + c * p[1] + e, b * p[0] + d * p[1] + f);
        let (sx, sy) = self.scale();
        [
            (x - dc.window_org[0]) * sx + dc.viewport_org[0],
            (y - dc.window_org[1]) * sy + dc.viewport_org[1],
        ]
    }

    fn scale(&self) -> (f64, f64) {
        let dc = &self.dc;
        match dc.map {
            MapMode::Text => (1.0, 1.0),
            MapMode::Isotropic | MapMode::Anisotropic => {
                let ratio = |v: f64, w: f64| if w == 0.0 { 1.0 } else { v / w };
                let (sx, sy) = (
                    ratio(dc.viewport_ext[0], dc.window_ext[0]),
                    ratio(dc.viewport_ext[1], dc.window_ext[1]),
                );
                if dc.map == MapMode::Isotropic {
                    let s = sx.abs().min(sy.abs());
                    (s * sx.signum(), s * sy.signum())
                } else {
                    (sx, sy)
                }
            }
            fixed => {
                let mm = fixed.millimetres();
                (mm * self.pixels_per_mm[0], -mm * self.pixels_per_mm[1])
            }
        }
    }

    /// A logical length along y, on the device.
    fn length(&self, v: f64) -> f64 {
        let [_, _, c, d, _, _] = self.dc.world;
        v * (c * c + d * d).sqrt() * self.scale().1.abs()
    }

    fn x_scale(&self) -> f64 {
        let [a, b, _, _, _, _] = self.dc.world;
        (a * a + b * b).sqrt() * self.scale().0.abs()
    }

    pub fn set_map_mode(&mut self, map: MapMode) {
        self.dc.map = map;
    }

    pub fn set_window_org(&mut self, p: [f64; 2]) {
        self.dc.window_org = p;
    }

    pub fn set_window_ext(&mut self, e: [f64; 2]) {
        self.dc.window_ext = e;
    }

    pub fn set_viewport_org(&mut self, p: [f64; 2]) {
        self.dc.viewport_org = p;
    }

    pub fn set_viewport_ext(&mut self, e: [f64; 2]) {
        self.dc.viewport_ext = e;
    }

    pub fn window(&self) -> ([f64; 2], [f64; 2]) {
        (self.dc.window_org, self.dc.window_ext)
    }

    pub fn viewport(&self) -> ([f64; 2], [f64; 2]) {
        (self.dc.viewport_org, self.dc.viewport_ext)
    }

    pub fn set_world(&mut self, m: [f64; 6]) {
        self.dc.world = m;
    }

    /// Composes `m` with the world transform, applied before it when `left`.
    pub fn modify_world(&mut self, m: [f64; 6], left: bool) {
        let mul = |p: [f64; 6], q: [f64; 6]| {
            [
                p[0] * q[0] + p[1] * q[2],
                p[0] * q[1] + p[1] * q[3],
                p[2] * q[0] + p[3] * q[2],
                p[2] * q[1] + p[3] * q[3],
                p[4] * q[0] + p[5] * q[2] + q[4],
                p[4] * q[1] + p[5] * q[3] + q[5],
            ]
        };
        let w = self.dc.world;
        self.dc.world = if left { mul(m, w) } else { mul(w, m) };
    }

    pub fn set_text_color(&mut self, c: Color) {
        self.dc.text = c;
    }

    pub fn set_background(&mut self, c: Color) {
        self.dc.background = c;
    }

    pub fn set_align(&mut self, align: u32) {
        self.dc.align = align;
    }

    pub fn set_winding(&mut self, winding: bool) {
        self.dc.winding = winding;
    }

    pub fn save(&mut self) {
        self.saved.push(self.dc.clone());
    }

    /// Back to the `n`th saved state, counted from the last when negative.
    pub fn restore(&mut self, n: i32) {
        let depth = self.saved.len() as i64;
        let index = if n < 0 {
            depth + i64::from(n)
        } else {
            i64::from(n) - 1
        };
        if index < 0 || index >= depth {
            return;
        }
        self.saved.truncate(index as usize + 1);
        if let Some(dc) = self.saved.pop() {
            self.dc = dc;
        }
    }

    /// Into the lowest free slot, as a WMF numbers its objects.
    pub fn create(&mut self, object: Object) {
        match self.objects.iter().position(Option::is_none) {
            Some(i) => self.objects[i] = Some(object),
            None => self.objects.push(Some(object)),
        }
    }

    pub fn create_at(&mut self, index: usize, object: Object) {
        if index > 0xFFFF {
            return;
        }
        if self.objects.len() <= index {
            self.objects.resize(index + 1, None);
        }
        self.objects[index] = Some(object);
    }

    pub fn delete(&mut self, index: usize) {
        if let Some(slot) = self.objects.get_mut(index) {
            *slot = None;
        }
    }

    pub fn select(&mut self, index: usize) {
        match self.objects.get(index).cloned().flatten() {
            Some(Object::Pen(p)) => self.dc.pen = p,
            Some(Object::Brush(b)) => self.dc.brush = b,
            Some(Object::Font(f)) => self.dc.font = f,
            _ => {}
        }
    }

    pub fn select_stock(&mut self, index: u32) {
        let grey = |v: u8| Brush {
            color: Some(Color(v, v, v)),
        };
        let pen = |color, width| Pen { color, width };
        match index {
            0 => self.dc.brush = grey(255),
            1 => self.dc.brush = grey(192),
            2 => self.dc.brush = grey(128),
            3 => self.dc.brush = grey(64),
            4 => self.dc.brush = grey(0),
            5 => self.dc.brush = Brush { color: None },
            6 => self.dc.pen = pen(Color::WHITE, Some(0.0)),
            7 => self.dc.pen = pen(Color::BLACK, Some(0.0)),
            8 => self.dc.pen = pen(Color::BLACK, None),
            _ => {}
        }
    }

    pub fn move_to(&mut self, p: [f64; 2]) {
        self.dc.at = p;
        let d = self.device(p);
        if let Some(path) = &mut self.path {
            path.push(vec![d]);
        }
    }

    pub fn line_to(&mut self, p: [f64; 2]) {
        self.poly_to(&[p], false);
    }

    /// On from the current position through `points`, as Bézier segments
    /// when `bezier`.
    pub fn poly_to(&mut self, points: &[[f64; 2]], bezier: bool) {
        let Some(&last) = points.last() else {
            return;
        };
        let mut device = vec![self.device(self.dc.at)];
        device.extend(points.iter().map(|&p| self.device(p)));
        if bezier {
            device = flatten(&device, &bezier_controls(device.len()));
        }
        self.dc.at = last;
        match &mut self.path {
            Some(path) => match path.last_mut() {
                Some(figure) => figure.extend_from_slice(&device[1..]),
                None => path.push(device),
            },
            None => self.stroke(vec![device]),
        }
    }

    pub fn polyline(&mut self, points: &[[f64; 2]], bezier: bool) {
        if points.is_empty() {
            return;
        }
        let figure = self.figure(points, bezier);
        match &mut self.path {
            Some(path) => path.push(figure),
            None => self.stroke(vec![figure]),
        }
    }

    fn figure(&self, points: &[[f64; 2]], bezier: bool) -> Vec<[f64; 2]> {
        let device: Vec<[f64; 2]> = points.iter().map(|&p| self.device(p)).collect();
        if bezier {
            flatten(&device, &bezier_controls(device.len()))
        } else {
            device
        }
    }

    pub fn polygons(&mut self, rings: &[Vec<[f64; 2]>]) {
        let rings: Vec<Vec<[f64; 2]>> = rings.iter().map(|r| self.figure(r, false)).collect();
        match &mut self.path {
            Some(path) => path.extend(rings),
            None => self.shape(rings),
        }
    }

    pub fn rectangle(&mut self, rect: [f64; 4], corner: [f64; 2]) {
        let ring = self.device_ring(rectangle(rect, corner[0] / 2.0, corner[1] / 2.0));
        self.close_shape(ring);
    }

    pub fn ellipse(&mut self, rect: [f64; 4]) {
        let ring = self.device_ring(ellipse(rect));
        self.close_shape(ring);
    }

    pub fn arc(&mut self, rect: [f64; 4], start: [f64; 2], end: [f64; 2], kind: ArcKind) {
        let ([l, t], [r, b]) = (
            self.device([rect[0], rect[1]]),
            self.device([rect[2], rect[3]]),
        );
        let bounds = [l.min(r), t.min(b), l.max(r), t.max(b)];
        let (mut s, mut e) = (self.device(start), self.device(end));
        if (r < l) != (b < t) {
            std::mem::swap(&mut s, &mut e);
        }
        let ring = arc(bounds, s, e, kind);
        if kind != ArcKind::Arc {
            self.close_shape(ring);
            return;
        }
        match &mut self.path {
            Some(path) => path.push(ring),
            None => self.stroke(vec![ring]),
        }
    }

    fn device_ring(&self, ring: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
        ring.into_iter().map(|p| self.device(p)).collect()
    }

    fn close_shape(&mut self, ring: Vec<[f64; 2]>) {
        match &mut self.path {
            Some(path) => path.push(ring),
            None => self.shape(vec![ring]),
        }
    }

    fn shape(&mut self, rings: Vec<Vec<[f64; 2]>>) {
        self.paint(rings, true, true);
    }

    fn pen(&self) -> Option<(Color, f64)> {
        let pen = &self.dc.pen;
        pen.width.map(|w| (pen.color, self.length(w)))
    }

    fn stroke(&mut self, rings: Vec<Vec<[f64; 2]>>) {
        self.paint(rings, false, true);
    }

    /// What a raster operation without a source bitmap paints over `rect`:
    /// the brush, black, white, or nothing for one that keeps or inverts.
    pub fn blit(&mut self, rect: [f64; 4], rop: u32) {
        match rop {
            0x00F0_0021 => self.fill_rect(rect, None),
            0x0000_0042 => self.fill_rect(rect, Some(Color::BLACK)),
            0x00FF_0062 => self.fill_rect(rect, Some(Color::WHITE)),
            _ => {}
        }
    }

    /// A rectangle filled with `color`, or with the brush.
    pub fn fill_rect(&mut self, rect: [f64; 4], color: Option<Color>) {
        let Some(color) = color.or(self.dc.brush.color) else {
            return;
        };
        let ring = self.device_ring(rectangle(rect, 0.0, 0.0));
        self.ops.push(Op::Path(Path {
            rings: vec![ring],
            closed: true,
            fill: Some((color, 1.0)),
            stroke: None,
            even_odd: true,
        }));
    }

    pub fn begin_path(&mut self) {
        self.path = Some(Vec::new());
    }

    pub fn close_figure(&mut self) {
        if let Some(figure) = self.path.as_mut().and_then(|p| p.last_mut()) {
            if let Some(&first) = figure.first() {
                figure.push(first);
            }
        }
    }

    pub fn end_path(&mut self) -> Option<Vec<Vec<[f64; 2]>>> {
        self.path.take()
    }

    pub fn paint(&mut self, rings: Vec<Vec<[f64; 2]>>, fill: bool, stroke: bool) {
        let fill = fill.then_some(self.dc.brush.color).flatten();
        let stroke = stroke.then(|| self.pen()).flatten();
        if fill.is_none() && stroke.is_none() {
            return;
        }
        self.ops.push(Op::Path(Path {
            rings,
            closed: fill.is_some(),
            fill: fill.map(|c| (c, 1.0)),
            stroke,
            even_odd: !self.dc.winding,
        }));
    }

    /// A bitmap into the logical rectangle left, top, width, height.
    pub fn bitmap(&mut self, rect: [f64; 4], png: Vec<u8>) {
        let [x, y, w, h] = rect;
        let (a, b) = (self.device([x, y]), self.device([x + w, y + h]));
        self.ops.push(Op::Bitmap(Bitmap {
            rect: [
                a[0].min(b[0]),
                a[1].min(b[1]),
                (b[0] - a[0]).abs(),
                (b[1] - a[1]).abs(),
            ],
            png: Arc::from(png),
        }));
    }

    /// `dx` gives each character's logical advance, `opaque` the rectangle
    /// the background colour fills first.
    pub fn text(&mut self, at: [f64; 2], text: &str, dx: Option<&[f64]>, opaque: Option<[f64; 4]>) {
        if let Some(rect) = opaque {
            self.fill_rect(rect, Some(self.dc.background));
        }
        let text = text.trim_end_matches('\0');
        if self.dc.font.rotated || text.trim().is_empty() {
            return;
        }
        let update = self.dc.align & TA_UPDATECP != 0;
        let origin = if update { self.dc.at } else { at };
        let n = text.chars().count();
        let advances: Option<Vec<f64>> = dx
            .filter(|dx| dx.len() >= n)
            .map(|dx| dx[..n].iter().map(|d| d * self.x_scale()).collect());
        if update {
            if let Some(dx) = dx.filter(|dx| dx.len() >= n) {
                self.dc.at[0] += dx[..n].iter().sum::<f64>();
            }
        }
        let font = &self.dc.font;
        self.ops.push(Op::Text(Text {
            at: self.device(origin),
            align: match self.dc.align & 24 {
                8 => TextAlign::Bottom,
                24 => TextAlign::Baseline,
                _ => TextAlign::Top,
            },
            h_align: match self.dc.align & 6 {
                2 => HAlign::Right,
                6 => HAlign::Center,
                _ => HAlign::Left,
            },
            text: text.to_string(),
            family: font.family.clone(),
            size: self.length(font.height.abs()),
            bold: font.bold,
            italic: font.italic,
            color: self.dc.text,
            advances,
        }));
    }
}
