use std::rc::Rc;

use lj_draw::{Blend, Cap, Join, Point, Segment};

use super::color::Color;
use crate::graphics::Matrix;

#[derive(Debug, Clone)]
pub(crate) struct TextState {
    pub text_matrix: Matrix,
    pub line_matrix: Matrix,
    pub font_size: f64,
    pub character_spacing: f64,
    pub word_spacing: f64,
    pub text_rise: f64,
    pub horizontal_scaling: f64,
    pub leading: f64,
    pub font: Option<String>,
    /// PDF text rendering mode. Mode 3 = invisible (used in OCR/PDF-A).
    pub text_render_mode: i64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            text_matrix: Matrix::identity(),
            line_matrix: Matrix::identity(),
            font_size: 12.0,
            character_spacing: 0.0,
            word_spacing: 0.0,
            text_rise: 0.0,
            horizontal_scaling: 100.0,
            leading: 0.0,
            font: None,
            text_render_mode: 0,
        }
    }
}

/// A clipping path, kept in the user space it was set in.
#[derive(Debug)]
pub(crate) struct ClipPath {
    pub segments: Vec<Segment>,
    /// User space to the normalised page.
    pub transform: Matrix,
    pub even_odd: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct GraphicsState {
    pub ctm: Matrix,
    /// The CTM the page or the form started with: a pattern's space is
    /// placed on it.
    pub pattern_base: Matrix,
    pub text: TextState,
    pub line_width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    pub dash: Vec<f64>,
    pub dash_phase: f64,
    pub fill: Color,
    pub stroke: Color,
    pub fill_alpha: f64,
    pub stroke_alpha: f64,
    pub blend: Option<Blend>,
    /// The mask the device drew for the soft mask in force.
    pub soft_mask: Option<usize>,
    /// Every clip in force, outermost first.
    pub clips: Vec<Rc<ClipPath>>,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Matrix::identity(),
            pattern_base: Matrix::identity(),
            text: TextState::default(),
            line_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_phase: 0.0,
            fill: Color::black(),
            stroke: Color::black(),
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend: None,
            soft_mask: None,
            clips: Vec::new(),
        }
    }
}

/// The path under construction, in user space.
#[derive(Debug, Default)]
pub(crate) struct PathBuilder {
    segments: Vec<Segment>,
    start: Option<Point>,
    current: Option<Point>,
}

impl PathBuilder {
    pub fn move_to(&mut self, point: Point) {
        if matches!(self.segments.last(), Some(Segment::Move(_))) {
            self.segments.pop();
        }
        self.segments.push(Segment::Move(point));
        self.start = Some(point);
        self.current = Some(point);
    }

    pub fn line_to(&mut self, point: Point) {
        if self.current.is_none() {
            return self.move_to(point);
        }
        self.segments.push(Segment::Line(point));
        self.current = Some(point);
    }

    /// A missing control point stands on the current point (`v`) or on the
    /// end (`y`).
    pub fn curve_to(&mut self, first: Option<Point>, second: Option<Point>, end: Point) {
        let Some(current) = self.current else {
            return self.move_to(end);
        };
        self.segments.push(Segment::Cubic(
            first.unwrap_or(current),
            second.unwrap_or(end),
            end,
        ));
        self.current = Some(end);
    }

    pub fn close(&mut self) {
        if self.current.is_some() && !matches!(self.segments.last(), Some(Segment::Close)) {
            self.segments.push(Segment::Close);
            self.current = self.start;
        }
    }

    pub fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        self.move_to([x, y]);
        self.line_to([x + width, y]);
        self.line_to([x + width, y + height]);
        self.line_to([x, y + height]);
        self.close();
    }

    pub fn take(&mut self) -> Vec<Segment> {
        self.start = None;
        self.current = None;
        std::mem::take(&mut self.segments)
    }
}
