//! A page as a display list, and its writing as SVG. Coordinates are points
//! from the top left corner, y growing down; items paint in order.

mod svg;

use std::sync::Arc;

pub use svg::svg;

pub type Point = [f64; 2];

/// `(x, y)` maps to `(a x + c y + e, b x + d y + f)`, as SVG reads a matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f64, y: f64) -> Matrix {
        Matrix {
            e: x,
            f: y,
            ..Matrix::IDENTITY
        }
    }

    pub fn scale(x: f64, y: f64) -> Matrix {
        Matrix {
            a: x,
            d: y,
            ..Matrix::IDENTITY
        }
    }

    /// Turn by `degrees`, clockwise on a page whose y grows down.
    pub fn rotate(degrees: f64) -> Matrix {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Matrix {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            ..Matrix::IDENTITY
        }
    }

    /// `self` first, then `next`.
    pub fn then(self, next: Matrix) -> Matrix {
        Matrix {
            a: self.a * next.a + self.b * next.c,
            b: self.a * next.b + self.b * next.d,
            c: self.c * next.a + self.d * next.c,
            d: self.c * next.b + self.d * next.d,
            e: self.e * next.a + self.f * next.c + next.e,
            f: self.e * next.b + self.f * next.d + next.f,
        }
    }

    pub fn apply(self, [x, y]: Point) -> Point {
        [
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    pub const WHITE: Rgb = Rgb(255, 255, 255);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Color(Rgb),
    /// Index in [`Page::gradients`].
    Gradient(usize),
    /// Index in [`Page::patterns`].
    Pattern(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub paint: Paint,
    /// 1 is opaque.
    pub opacity: f64,
    pub even_odd: bool,
}

impl Fill {
    pub fn color(color: Rgb) -> Fill {
        Fill {
            paint: Paint::Color(color),
            opacity: 1.0,
            even_odd: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub paint: Paint,
    pub opacity: f64,
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    pub dash: Vec<f64>,
    pub dash_offset: f64,
}

impl Stroke {
    pub fn color(color: Rgb, width: f64) -> Stroke {
        Stroke {
            paint: Paint::Color(color),
            opacity: 1.0,
            width,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 4.0,
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Move(Point),
    Line(Point),
    Cubic(Point, Point, Point),
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        /// Corner radius.
        radius: f64,
    },
    Ellipse {
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
    },
    Line {
        from: Point,
        to: Point,
    },
    Path(Vec<Segment>),
}

/// Characters of one family on one baseline, drawn by the browser from the
/// face the page declares under that family.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub x: f64,
    /// The baseline.
    pub y: f64,
    pub family: String,
    pub size: f64,
    pub color: Rgb,
    pub text: String,
    /// Distance from each character to the next.
    pub advances: Vec<f64>,
    /// Every character sits at its advance, rather than the first of each
    /// word alone with the rest flowing in the face's own widths.
    pub each_char: bool,
    pub kerning: bool,
    pub letter_spacing: f64,
    pub synthetic_bold: bool,
    pub synthetic_italic: bool,
}

/// An encoded picture stretched over the unit square that `transform` maps
/// onto the page, its top left corner at the origin.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub transform: Matrix,
    pub mime: &'static str,
    pub data: Arc<[u8]>,
    /// Interpolate between samples when enlarged, rather than show them as
    /// blocks.
    pub smooth: bool,
    pub recolor: Option<Recolor>,
}

/// A picture's colours remapped channel by channel, in sRGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recolor {
    /// First each colour replaced by its grey, these weights of red, green
    /// and blue.
    pub grey: Option<[f64; 3]>,
    /// Then each channel rounded to 0 or 1.
    pub threshold: bool,
    /// Then each channel `v`, in `0..1`, mapped to `slope * v + intercept`.
    pub slope: f64,
    pub intercept: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub shape: Shape,
    pub even_odd: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Applied to the clip and to the items alike.
    pub transform: Matrix,
    pub clip: Option<Clip>,
    pub opacity: f64,
    pub blend: Option<Blend>,
    /// Index in [`Page::masks`].
    pub mask: Option<usize>,
    /// The items composite over a transparent backdrop before the group meets the page.
    pub isolated: bool,
    pub items: Vec<Item>,
}

impl Group {
    pub fn new(items: Vec<Item>) -> Group {
        Group {
            transform: Matrix::IDENTITY,
            clip: None,
            opacity: 1.0,
            blend: None,
            mask: None,
            isolated: false,
            items,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Shape {
        shape: Shape,
        fill: Option<Fill>,
        stroke: Option<Stroke>,
    },
    Text(Text),
    /// A glyph drawn by its outline, index in [`Page::outlines`], placed on the
    /// page by `transform`.
    Glyph {
        outline: usize,
        transform: Matrix,
        fill: Option<Fill>,
        stroke: Option<Stroke>,
    },
    Image(Image),
    Group(Box<Group>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientKind {
    Linear {
        from: Point,
        to: Point,
    },
    Radial {
        from: Point,
        from_radius: f64,
        to: Point,
        to_radius: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub offset: f64,
    pub color: Rgb,
    pub opacity: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// From the gradient's own space to the page.
    pub transform: Matrix,
    pub stops: Vec<Stop>,
}

/// What a group shows through: the luminance of `items`, or their alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub items: Vec<Item>,
    pub luminosity: bool,
}

/// `items` on a tile `width` by `height` from the origin, repeated over the
/// plane that `transform` maps onto the painted item's space.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub transform: Matrix,
    pub width: f64,
    pub height: f64,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub items: Vec<Item>,
    pub outlines: Vec<Vec<Segment>>,
    pub gradients: Vec<Gradient>,
    pub masks: Vec<Mask>,
    pub patterns: Vec<Pattern>,
}

impl Page {
    /// Every family a text of the page is written in, masks and patterns
    /// included.
    pub fn families(&self) -> std::collections::BTreeSet<&str> {
        fn walk<'a>(items: &'a [Item], out: &mut std::collections::BTreeSet<&'a str>) {
            for item in items {
                match item {
                    Item::Text(text) => {
                        out.insert(&text.family);
                    }
                    Item::Group(group) => walk(&group.items, out),
                    _ => {}
                }
            }
        }
        let mut out = std::collections::BTreeSet::new();
        walk(&self.items, &mut out);
        for mask in &self.masks {
            walk(&mask.items, &mut out);
        }
        for pattern in &self.patterns {
            walk(&pattern.items, &mut out);
        }
        out
    }
}
