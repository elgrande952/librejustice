//! A drawing shape's outline in its frame's unit square: a custom shape's
//! enhanced path, whose parameters are formulas over the frame and the
//! shape's modifiers, or an SVG path.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use lj_draw::Segment;

use crate::model::SubPath;

/// Hundredths of a millimetre per point, the unit of a view box left empty.
const LOGIC_PER_POINT: f64 = 2540.0 / 72.0;
/// The control point distance of a cubic quarter circle, for a radius of 1.
const KAPPA: f64 = 0.552_284_749_830_793_4;
const MAX_DEPTH: usize = 256;

type Point = [f64; 2];

#[derive(Clone, Debug, PartialEq)]
pub enum ShapePath {
    Enhanced(Enhanced),
    /// An SVG path in its view box, its closed rings filled when `fill`.
    Svg {
        d: String,
        view_box: [f64; 4],
        fill: bool,
    },
}

impl ShapePath {
    pub fn outline(&self, width: f64, height: f64) -> Option<Vec<SubPath>> {
        match self {
            ShapePath::Enhanced(e) => e.outline(width, height),
            ShapePath::Svg { d, view_box, fill } => svg_path(d, *view_box, *fill),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Enhanced {
    pub view_box: [f64; 4],
    pub path: String,
    /// Each `draw:equation`'s name and formula.
    pub equations: Vec<(String, String)>,
    pub modifiers: Vec<f64>,
    /// The outline mirrored across its vertical axis, then its horizontal one.
    pub mirror: (bool, bool),
}

impl Enhanced {
    /// The outlines for a frame `width` by `height` points, `None` when the
    /// path draws nothing.
    pub fn outline(&self, width: f64, height: f64) -> Option<Vec<SubPath>> {
        let [vx, vy, mut vw, mut vh] = self.view_box;
        if vw <= 0.0 || vh <= 0.0 {
            (vw, vh) = (width * LOGIC_PER_POINT, height * LOGIC_PER_POINT);
        }
        if vw <= 0.0 || vh <= 0.0 {
            return None;
        }
        let mut eval = Eval {
            shape: self,
            frame: [vx, vy, vw, vh],
            names: self
                .equations
                .iter()
                .enumerate()
                .map(|(i, (name, _))| (name.as_str(), i))
                .collect(),
            memo: HashMap::new(),
            depth: 0,
        };
        let mut pen = Pen::default();
        let tokens: Vec<&str> = self.path.split_whitespace().collect();
        let mut i = 0;
        let mut command = ' ';
        while i < tokens.len() {
            let token = tokens[i];
            if let [c @ b'A'..=b'Z'] = token.as_bytes() {
                let c = char::from(*c);
                command = c;
                i += 1;
                match c {
                    'Z' => pen.close(),
                    'N' => pen.end(),
                    'F' => pen.fill = false,
                    'S' => pen.stroke = false,
                    'H' => pen.shade = -0.4,
                    'I' => pen.shade = -0.2,
                    'J' => pen.shade = 0.4,
                    'K' => pen.shade = 0.2,
                    _ => {}
                }
                continue;
            }
            let arity = match command {
                'M' | 'L' | 'X' | 'Y' => 2,
                'Q' | 'G' => 4,
                'C' | 'T' | 'U' => 6,
                'A' | 'B' | 'W' | 'V' => 8,
                _ => {
                    i += 1;
                    continue;
                }
            };
            if i + arity > tokens.len() {
                break;
            }
            let v: Vec<f64> = tokens[i..i + arity]
                .iter()
                .map(|t| eval.parameter(t))
                .collect();
            i += arity;
            match command {
                'M' => {
                    pen.move_to([v[0], v[1]]);
                    command = 'L';
                }
                'L' => pen.line_to([v[0], v[1]]),
                'C' => pen.push(Segment::Cubic([v[0], v[1]], [v[2], v[3]], [v[4], v[5]])),
                'Q' => {
                    let p0 = pen.current;
                    let (c, p) = ([v[0], v[1]], [v[2], v[3]]);
                    let third = |a: Point, b: Point| {
                        [
                            a[0] + (b[0] - a[0]) * 2.0 / 3.0,
                            a[1] + (b[1] - a[1]) * 2.0 / 3.0,
                        ]
                    };
                    pen.push(Segment::Cubic(third(p0, c), third(p, c), p));
                }
                'T' | 'U' => {
                    let (center, radii) = ([v[0], v[1]], [v[2], v[3]]);
                    let (start, mut end) = (v[4], v[5]);
                    if radii[0] == 0.0 || radii[1] == 0.0 {
                        continue;
                    }
                    // Counter-clockwise on the page, in degrees.
                    if start.rem_euclid(360.0) == end.rem_euclid(360.0) {
                        end = start + 360.0;
                    } else if start > end {
                        end += 360.0;
                    }
                    let (t0, t1) = (-start.to_radians(), -end.to_radians());
                    pen.arc(center, radii, t0, t1, command == 'U');
                    command = 'T';
                }
                'A' | 'B' | 'W' | 'V' => {
                    let center = [(v[0] + v[2]) / 2.0, (v[1] + v[3]) / 2.0];
                    let radii = [(v[2] - v[0]).abs() / 2.0, (v[3] - v[1]).abs() / 2.0];
                    if radii[0] == 0.0 || radii[1] == 0.0 {
                        continue;
                    }
                    let angle = |x: f64, y: f64| {
                        ((y - center[1]) / radii[1]).atan2((x - center[0]) / radii[0])
                    };
                    let t0 = angle(v[4], v[5]);
                    let mut t1 = angle(v[6], v[7]);
                    let clockwise = matches!(command, 'W' | 'V');
                    if clockwise {
                        while t1 <= t0 {
                            t1 += TAU;
                        }
                    } else {
                        while t1 >= t0 {
                            t1 -= TAU;
                        }
                    }
                    pen.arc(center, radii, t0, t1, matches!(command, 'B' | 'V'));
                    command = match command {
                        'B' => 'A',
                        'V' => 'W',
                        c => c,
                    };
                }
                'X' | 'Y' => {
                    let (p0, p) = (pen.current, [v[0], v[1]]);
                    let (c1, c2) = if command == 'X' {
                        (
                            [p0[0] + KAPPA * (p[0] - p0[0]), p0[1]],
                            [p[0], p[1] + KAPPA * (p0[1] - p[1])],
                        )
                    } else {
                        (
                            [p0[0], p0[1] + KAPPA * (p[1] - p0[1])],
                            [p[0] + KAPPA * (p0[0] - p[0]), p[1]],
                        )
                    };
                    pen.push(Segment::Cubic(c1, c2, p));
                    command = if command == 'X' { 'Y' } else { 'X' };
                }
                'G' => {
                    let (rx, ry) = (v[0], v[1]);
                    let (start, sweep) = (v[2].to_radians(), v[3].to_radians());
                    if rx == 0.0 || ry == 0.0 {
                        continue;
                    }
                    // Office measures its angles as seen on the ellipse.
                    let param = |a: f64| (rx * a.sin()).atan2(ry * a.cos());
                    let t0 = param(start);
                    let mut t1 = param(start + sweep);
                    if sweep.abs() >= TAU {
                        t1 = t0 + TAU * sweep.signum();
                    } else if sweep > 0.0 {
                        while t1 < t0 {
                            t1 += TAU;
                        }
                    } else {
                        while t1 > t0 {
                            t1 -= TAU;
                        }
                    }
                    let p0 = pen.current;
                    let center = [p0[0] - rx * t0.cos(), p0[1] - ry * t0.sin()];
                    pen.arc(center, [rx, ry], t0, t1, false);
                }
                _ => {}
            }
        }
        pen.end();
        let flip = |v: f64, on: bool| if on { 1.0 - v } else { v };
        let unit = |[x, y]: Point| {
            [
                flip((x - vx) / vw, self.mirror.0),
                flip((y - vy) / vh, self.mirror.1),
            ]
        };
        let paths: Vec<SubPath> = pen
            .done
            .into_iter()
            .map(|mut path| {
                for s in &mut path.segments {
                    *s = match *s {
                        Segment::Move(p) => Segment::Move(unit(p)),
                        Segment::Line(p) => Segment::Line(unit(p)),
                        Segment::Cubic(a, b, c) => Segment::Cubic(unit(a), unit(b), unit(c)),
                        Segment::Close => Segment::Close,
                    };
                }
                path
            })
            .collect();
        (!paths.is_empty()).then_some(paths)
    }
}

struct Pen {
    segments: Vec<Segment>,
    current: Point,
    start: Point,
    open: bool,
    fill: bool,
    stroke: bool,
    shade: f64,
    done: Vec<SubPath>,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            segments: Vec::new(),
            current: [0.0, 0.0],
            start: [0.0, 0.0],
            open: false,
            fill: true,
            stroke: true,
            shade: 0.0,
            done: Vec::new(),
        }
    }
}

impl Pen {
    fn move_to(&mut self, p: Point) {
        self.segments.push(Segment::Move(p));
        (self.current, self.start, self.open) = (p, p, true);
    }

    fn line_to(&mut self, p: Point) {
        self.push(Segment::Line(p));
    }

    fn push(&mut self, segment: Segment) {
        if !self.open {
            let at = self.current;
            self.move_to(at);
        }
        if let Segment::Line(p) | Segment::Cubic(_, _, p) = segment {
            self.current = p;
        }
        self.segments.push(segment);
    }

    fn close(&mut self) {
        if self.open {
            self.segments.push(Segment::Close);
            self.current = self.start;
            self.open = false;
        }
    }

    /// The subpath so far, painted with the flags set since the last end.
    fn end(&mut self) {
        let segments = std::mem::take(&mut self.segments);
        if segments.iter().any(|s| !matches!(s, Segment::Move(_))) {
            self.done.push(SubPath {
                segments,
                fill: self.fill,
                stroke: self.stroke,
                shade: self.shade,
            });
        }
        (self.open, self.fill, self.stroke, self.shade) = (false, true, true, 0.0);
    }

    /// The ellipse about `center` from parameter `t0` to `t1`, in radians
    /// turning clockwise on the page, reached by a line or a move.
    fn arc(&mut self, center: Point, [rx, ry]: [f64; 2], t0: f64, t1: f64, move_to: bool) {
        let at = |t: f64| [center[0] + rx * t.cos(), center[1] + ry * t.sin()];
        if move_to || !self.open {
            self.move_to(at(t0));
        } else {
            self.line_to(at(t0));
        }
        let t1 = t0 + (t1 - t0).clamp(-TAU, TAU);
        let pieces = ((t1 - t0).abs() / FRAC_PI_2).ceil().max(1.0) as usize;
        let step = (t1 - t0) / pieces as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        for n in 0..pieces {
            let (a, b) = (t0 + step * n as f64, t0 + step * (n + 1) as f64);
            let c1 = [
                center[0] + rx * (a.cos() - k * a.sin()),
                center[1] + ry * (a.sin() + k * a.cos()),
            ];
            let c2 = [
                center[0] + rx * (b.cos() + k * b.sin()),
                center[1] + ry * (b.sin() - k * b.cos()),
            ];
            self.push(Segment::Cubic(c1, c2, at(b)));
        }
    }
}

struct Eval<'s> {
    shape: &'s Enhanced,
    frame: [f64; 4],
    names: HashMap<&'s str, usize>,
    memo: HashMap<usize, f64>,
    depth: usize,
}

impl Eval<'_> {
    fn parameter(&mut self, token: &str) -> f64 {
        let mut parser = Parser {
            src: token.as_bytes(),
            at: 0,
        };
        parser
            .expression(self)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    }

    fn equation(&mut self, name: &str) -> f64 {
        let Some(&i) = self.names.get(name) else {
            return 0.0;
        };
        if let Some(&v) = self.memo.get(&i) {
            return v;
        }
        // A reference back to a formula being read reads as 0.
        self.memo.insert(i, 0.0);
        let formula = self.shape.equations[i].1.clone();
        let mut parser = Parser {
            src: formula.as_bytes(),
            at: 0,
        };
        let v = parser.expression(self).unwrap_or(0.0);
        let v = if v.is_finite() { v } else { 0.0 };
        self.memo.insert(i, v);
        v
    }

    fn constant(&self, name: &str) -> Option<f64> {
        let [x, y, w, h] = self.frame;
        Some(match name {
            "pi" => PI,
            "left" => x,
            "top" => y,
            "right" => x + w,
            "bottom" => y + h,
            "width" | "logwidth" => w,
            "height" | "logheight" => h,
            "xstretch" | "ystretch" => 0.0,
            "hasstroke" | "hasfill" => 1.0,
            _ => return None,
        })
    }
}

/// A formula of `draw:equation`, or a path parameter, by recursive descent.
struct Parser<'a> {
    src: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    fn skip(&mut self) {
        while self.src.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip();
        let hit = self.src.get(self.at) == Some(&c);
        if hit {
            self.at += 1;
        }
        hit
    }

    fn expression(&mut self, eval: &mut Eval<'_>) -> Option<f64> {
        let mut v = self.term(eval)?;
        loop {
            if self.eat(b'+') {
                v += self.term(eval)?;
            } else if self.eat(b'-') {
                v -= self.term(eval)?;
            } else {
                return Some(v);
            }
        }
    }

    fn term(&mut self, eval: &mut Eval<'_>) -> Option<f64> {
        let mut v = self.factor(eval)?;
        loop {
            if self.eat(b'*') {
                v *= self.factor(eval)?;
            } else if self.eat(b'/') {
                let d = self.factor(eval)?;
                v = if d == 0.0 { 0.0 } else { v / d };
            } else {
                return Some(v);
            }
        }
    }

    /// Nothing past a nesting too deep to follow, through parentheses, signs
    /// and formulas referring to formulas, which would exhaust the stack.
    fn factor(&mut self, eval: &mut Eval<'_>) -> Option<f64> {
        if eval.depth == MAX_DEPTH {
            return None;
        }
        eval.depth += 1;
        let v = self.atom(eval);
        eval.depth -= 1;
        v
    }

    fn atom(&mut self, eval: &mut Eval<'_>) -> Option<f64> {
        self.skip();
        let c = *self.src.get(self.at)?;
        match c {
            b'-' => {
                self.at += 1;
                Some(-self.factor(eval)?)
            }
            b'+' => {
                self.at += 1;
                self.factor(eval)
            }
            b'(' => {
                self.at += 1;
                let v = self.expression(eval)?;
                self.eat(b')');
                Some(v)
            }
            b'?' => {
                self.at += 1;
                let name = self.word();
                Some(eval.equation(name))
            }
            b'$' => {
                self.at += 1;
                let n: usize = self.word().parse().ok()?;
                Some(eval.shape.modifiers.get(n).copied().unwrap_or(0.0))
            }
            b'0'..=b'9' | b'.' => {
                let start = self.at;
                while self
                    .src
                    .get(self.at)
                    .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
                {
                    self.at += 1;
                }
                if matches!(self.src.get(self.at), Some(b'e' | b'E'))
                    && self
                        .src
                        .get(self.at + 1)
                        .is_some_and(|c| c.is_ascii_digit() || *c == b'-' || *c == b'+')
                {
                    self.at += 2;
                    while self.src.get(self.at).is_some_and(u8::is_ascii_digit) {
                        self.at += 1;
                    }
                }
                std::str::from_utf8(&self.src[start..self.at])
                    .ok()?
                    .parse()
                    .ok()
            }
            c if c.is_ascii_alphabetic() => {
                let name = self.word();
                if let Some(v) = eval.constant(name) {
                    return Some(v);
                }
                if !self.eat(b'(') {
                    return None;
                }
                let mut args = vec![self.expression(eval)?];
                while self.eat(b',') {
                    args.push(self.expression(eval)?);
                }
                self.eat(b')');
                let a = |i: usize| args.get(i).copied().unwrap_or(0.0);
                Some(match name {
                    "abs" => a(0).abs(),
                    "sqrt" => a(0).max(0.0).sqrt(),
                    "sin" => a(0).sin(),
                    "cos" => a(0).cos(),
                    "tan" => a(0).tan(),
                    "atan" => a(0).atan(),
                    "atan2" => a(0).atan2(a(1)),
                    "min" => a(0).min(a(1)),
                    "max" => a(0).max(a(1)),
                    "if" => {
                        if a(0) > 0.0 {
                            a(1)
                        } else {
                            a(2)
                        }
                    }
                    _ => return None,
                })
            }
            _ => None,
        }
    }

    fn word(&mut self) -> &'a str {
        let (src, start) = (self.src, self.at);
        while self
            .src
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.at += 1;
        }
        std::str::from_utf8(&src[start..self.at]).unwrap_or_default()
    }
}

fn svg_path(d: &str, view_box: [f64; 4], fill: bool) -> Option<Vec<SubPath>> {
    let [vx, vy, vw, vh] = view_box;
    if vw <= 0.0 || vh <= 0.0 {
        return None;
    }
    let mut numbers = Numbers {
        src: d.as_bytes(),
        at: 0,
    };
    let mut pen = Pen {
        fill,
        ..Pen::default()
    };
    let mut command = b' ';
    let mut last_control: Option<Point> = None;
    loop {
        numbers.skip();
        let Some(&c) = numbers.src.get(numbers.at) else {
            break;
        };
        if c.is_ascii_alphabetic() {
            command = c;
            numbers.at += 1;
            if matches!(command, b'Z' | b'z') {
                pen.close();
                last_control = None;
            }
            continue;
        }
        let relative = command.is_ascii_lowercase();
        let origin = if relative { pen.current } else { [0.0, 0.0] };
        let point = |numbers: &mut Numbers<'_>| -> Option<Point> {
            Some([origin[0] + numbers.next()?, origin[1] + numbers.next()?])
        };
        let control = last_control.take();
        match command.to_ascii_uppercase() {
            b'M' => {
                let p = point(&mut numbers)?;
                pen.move_to(p);
                command = if relative { b'l' } else { b'L' };
            }
            b'L' => pen.line_to(point(&mut numbers)?),
            b'H' => {
                let x = numbers.next()? + if relative { pen.current[0] } else { 0.0 };
                pen.line_to([x, pen.current[1]]);
            }
            b'V' => {
                let y = numbers.next()? + if relative { pen.current[1] } else { 0.0 };
                pen.line_to([pen.current[0], y]);
            }
            b'C' | b'S' => {
                let p0 = pen.current;
                let c1 = if command.eq_ignore_ascii_case(&b'C') {
                    point(&mut numbers)?
                } else {
                    control.map_or(p0, |c| [2.0 * p0[0] - c[0], 2.0 * p0[1] - c[1]])
                };
                let c2 = point(&mut numbers)?;
                let p = point(&mut numbers)?;
                pen.push(Segment::Cubic(c1, c2, p));
                last_control = Some(c2);
            }
            b'Q' => {
                let p0 = pen.current;
                let c = point(&mut numbers)?;
                let p = point(&mut numbers)?;
                let third = |a: Point| {
                    [
                        a[0] + (c[0] - a[0]) * 2.0 / 3.0,
                        a[1] + (c[1] - a[1]) * 2.0 / 3.0,
                    ]
                };
                pen.push(Segment::Cubic(third(p0), third(p), p));
            }
            b'A' => {
                for _ in 0..5 {
                    numbers.next()?;
                }
                pen.line_to(point(&mut numbers)?);
            }
            _ => {
                numbers.next()?;
            }
        }
    }
    pen.end();
    let unit = |[x, y]: Point| [(x - vx) / vw, (y - vy) / vh];
    let paths: Vec<SubPath> = pen
        .done
        .into_iter()
        .map(|mut path| {
            path.fill &= path.segments.contains(&Segment::Close);
            for s in &mut path.segments {
                *s = match *s {
                    Segment::Move(p) => Segment::Move(unit(p)),
                    Segment::Line(p) => Segment::Line(unit(p)),
                    Segment::Cubic(a, b, c) => Segment::Cubic(unit(a), unit(b), unit(c)),
                    Segment::Close => Segment::Close,
                };
            }
            path
        })
        .collect();
    (!paths.is_empty()).then_some(paths)
}

struct Numbers<'a> {
    src: &'a [u8],
    at: usize,
}

impl Numbers<'_> {
    fn skip(&mut self) {
        while self
            .src
            .get(self.at)
            .is_some_and(|c| c.is_ascii_whitespace() || *c == b',')
        {
            self.at += 1;
        }
    }

    fn next(&mut self) -> Option<f64> {
        self.skip();
        let start = self.at;
        if matches!(self.src.get(self.at), Some(b'-' | b'+')) {
            self.at += 1;
        }
        let mut dot = false;
        while let Some(&c) = self.src.get(self.at) {
            if c.is_ascii_digit() || (c == b'.' && !dot) {
                dot |= c == b'.';
                self.at += 1;
            } else if matches!(c, b'e' | b'E') {
                self.at += 1;
                if matches!(self.src.get(self.at), Some(b'-' | b'+')) {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.src[start..self.at])
            .ok()?
            .parse()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(paths: &[SubPath]) -> Vec<Point> {
        paths
            .iter()
            .flat_map(|p| &p.segments)
            .filter_map(|s| match *s {
                Segment::Move(p) | Segment::Line(p) => Some(p),
                _ => None,
            })
            .map(|[x, y]| [(x * 1000.0).round() / 1000.0, (y * 1000.0).round() / 1000.0])
            .collect()
    }

    #[test]
    fn formulas_read_modifiers_equations_and_the_frame() {
        let shape = Enhanced {
            view_box: [0.0, 0.0, 21600.0, 21600.0],
            path: "M 0 0 L 21600 0 ?f0 21600 ?f1 21600 Z N".into(),
            equations: vec![
                ("f0".into(), "21600-$0".into()),
                (
                    "f1".into(),
                    "if(?f0-10800, $0, max(-3, sqrt(4)) * 0)".into(),
                ),
            ],
            modifiers: vec![5400.0],
            mirror: (false, true),
        };
        let paths = shape.outline(100.0, 50.0).unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].fill && paths[0].stroke);
        assert_eq!(
            points(&paths),
            vec![[0.0, 1.0], [1.0, 1.0], [0.75, 0.0], [0.25, 0.0]]
        );
    }

    #[test]
    fn an_empty_view_box_counts_in_hundredths_of_a_millimetre() {
        let shape = Enhanced {
            view_box: [0.0, 0.0, 0.0, 0.0],
            path: "F M 0 0 L logwidth 0 logwidth ?f0 N".into(),
            equations: vec![("f0".into(), "logheight/2".into())],
            modifiers: Vec::new(),
            mirror: (false, false),
        };
        let paths = shape.outline(72.0, 36.0).unwrap();
        assert!(!paths[0].fill);
        assert_eq!(points(&paths), vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.5]]);
    }

    #[test]
    fn a_formula_that_refers_to_itself_reads_itself_as_zero() {
        let shape = Enhanced {
            view_box: [0.0, 0.0, 10.0, 10.0],
            path: "M ?f0 0 L 10 10 N".into(),
            equations: vec![("f0".into(), "?f0+1".into())],
            modifiers: Vec::new(),
            mirror: (false, false),
        };
        assert_eq!(points(&shape.outline(10.0, 10.0).unwrap())[0], [0.1, 0.0]);
    }
}
