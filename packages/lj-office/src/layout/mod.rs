//! The document set on pages: a display list per page, in points from the top
//! left corner, y growing down, every glyph carrying its anchor.

mod chart;
mod float;
mod flow;
mod metafile;
mod metrics;
mod table;
mod text;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use crate::Anchor;

use crate::fonts::{self, FaceId, Fonts, Resolved};
use crate::model::*;
use crate::numbering::NumFormat;

pub use float::{displayable, Art, Image, Outline};

pub struct Layout {
    pub pages: Vec<Page>,
    /// What the drawing needs of each face of the [`Fonts`] the layout ran
    /// on, indexed by [`FaceId`].
    pub faces: Vec<FaceStyle>,
}

pub struct Page {
    pub width: f64,
    pub height: f64,
    /// In painting order.
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub struct FaceStyle {
    pub family: String,
    /// What the SVG names the face by, its own PostScript name.
    pub name: String,
    pub weight: u16,
    pub italic: bool,
    pub stretch: u16,
    /// Above and below the baseline, per point of size.
    pub ascent: f64,
    pub descent: f64,
}

#[derive(Clone, Debug)]
pub enum Item {
    Glyphs(Glyphs),
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: Color,
    },
    Line {
        from: [f64; 2],
        to: [f64; 2],
        width: f64,
        color: Color,
    },
    Image(Box<Image>),
    Shape(Box<Outline>),
    /// A closed outline through `points`.
    Polygon {
        points: Vec<[f64; 2]>,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    },
    Path(Box<Path>),
    Art(Box<Art>),
}

/// Outlines through the points of each ring, a fill painting what an odd
/// number of rings encloses when `even_odd`.
#[derive(Clone, Debug)]
pub struct Path {
    pub rings: Vec<Vec<[f64; 2]>>,
    pub closed: bool,
    pub even_odd: bool,
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
}

/// Characters of one face, size and colour on one baseline.
#[derive(Clone, Debug)]
pub struct Glyphs {
    pub x: f64,
    /// The baseline.
    pub y: f64,
    pub face: FaceId,
    pub size: f64,
    pub color: Color,
    pub text: String,
    /// Distance from each character to the next, justification included.
    pub advances: Vec<f64>,
    /// `None` for what the layout wrote rather than read, such as a list
    /// label or a page number.
    pub anchors: Vec<Option<Anchor>>,
    pub synthetic_bold: bool,
    pub synthetic_italic: bool,
    pub kerning: bool,
    /// Added after each character by the run, which the advances include.
    pub letter_spacing: f64,
}

impl Item {
    fn translate(&mut self, dx: f64, dy: f64) {
        match self {
            Item::Glyphs(g) => {
                g.x += dx;
                g.y += dy;
            }
            Item::Rect { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Item::Line { from, to, .. } => {
                from[0] += dx;
                from[1] += dy;
                to[0] += dx;
                to[1] += dy;
            }
            Item::Image(image) => {
                image.x += dx;
                image.y += dy;
            }
            Item::Shape(shape) => {
                shape.x += dx;
                shape.y += dy;
            }
            Item::Polygon { points, .. } => {
                for p in points {
                    p[0] += dx;
                    p[1] += dy;
                }
            }
            Item::Path(path) => {
                for p in path.rings.iter_mut().flatten() {
                    p[0] += dx;
                    p[1] += dy;
                }
            }
            Item::Art(art) => {
                art.fit = art.fit.then(lj_draw::Matrix::translate(dx, dy));
                art.frame[0] += dx;
                art.frame[1] += dy;
            }
        }
    }
}

impl Layout {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page_size(&self, page: usize) -> (f64, f64) {
        (self.pages[page].width, self.pages[page].height)
    }
}

/// Values of the fields a story shows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fields {
    pub page: u32,
    pub pages: u32,
    pub section_pages: u32,
    /// The page's section numbering, which writes the count too.
    pub format: NumFormat,
}

impl Fields {
    fn text(&self, field: Field) -> String {
        let value = match field {
            Field::Page => self.page,
            Field::NumPages => self.pages,
            Field::SectionPages => self.section_pages,
        };
        crate::numbering::number(value, self.format)
    }
}

/// A family asked for, bold, italic.
type Wanted = (Arc<str>, bool, bool);

/// A metafile's data and the bits of its frame's width and height.
type MetafileKey = (*const u8, u64, u64);

/// What every paragraph is set with.
pub(crate) struct Env<'a> {
    pub fonts: &'a Fonts,
    pub settings: &'a Settings,
    generics: HashMap<String, Generic>,
    resolved: RefCell<HashMap<Wanted, Resolved>>,
    covers: RefCell<HashMap<(FaceId, char), bool>>,
    /// The height of each text box that grows to its text, by address.
    grown: RefCell<HashMap<*const Drawing, f64>>,
    /// A metafile's items at the origin, by its data and frame size.
    metafiles: RefCell<HashMap<MetafileKey, Arc<[Item]>>>,
    /// The fill under the paragraphs being set: a cell's, a text box's, the
    /// page's.
    backdrop: Cell<Option<Color>>,
    /// The width of each text box that fits its text, by address.
    fitted: RefCell<HashMap<*const Drawing, f64>>,
}

impl<'a> Env<'a> {
    fn new(document: &'a Document, fonts: &'a Fonts) -> Self {
        Self {
            fonts,
            settings: &document.settings,
            generics: document
                .fonts
                .iter()
                .map(|f| (f.name.to_lowercase(), f.generic))
                .collect(),
            resolved: RefCell::default(),
            covers: RefCell::default(),
            grown: RefCell::default(),
            metafiles: RefCell::default(),
            backdrop: Cell::new(None),
            fitted: RefCell::default(),
        }
    }

    /// Run `f` with `fill`, when there is one, under what it sets.
    fn over<T>(&self, fill: Option<Color>, f: impl FnOnce() -> T) -> T {
        let Some(fill) = fill else {
            return f();
        };
        let previous = self.backdrop.replace(Some(fill));
        let out = f();
        self.backdrop.set(previous);
        out
    }

    fn generic(&self, family: &str) -> Generic {
        self.generics
            .get(&family.to_lowercase())
            .copied()
            .unwrap_or_default()
    }

    fn resolve(&self, family: &Arc<str>, bold: bool, italic: bool) -> Resolved {
        let key = (family.clone(), bold, italic);
        if let Some(found) = self.resolved.borrow().get(&key) {
            return *found;
        }
        let found = self
            .fonts
            .resolve(family, bold, italic, self.generic(family))
            .expect("a layout runs on at least one face");
        self.resolved.borrow_mut().insert(key, found);
        found
    }

    fn covers(&self, face: FaceId, c: char) -> bool {
        *self
            .covers
            .borrow_mut()
            .entry((face, c))
            .or_insert_with(|| self.fonts.face(face).has(c))
    }
}

impl Document {
    /// Lay the document out on `fonts`.
    ///
    /// Panics when `fonts` holds no face: nothing could be measured.
    pub fn layout(&self, fonts: &Fonts) -> Layout {
        assert!(!fonts.is_empty(), "a layout needs at least one face");
        let env = Env::new(self, fonts);
        let mut pages = flow::paginate(self, &env, None);
        if body_counts_pages(self) {
            let total = pages.len() as u32;
            pages = flow::paginate(self, &env, Some(total));
        }
        let faces = (0..fonts.len())
            .map(|id| {
                let face = fonts.face(id);
                let em = f64::from(face.metrics.units_per_em);
                FaceStyle {
                    family: face.family.clone(),
                    name: face.name.clone(),
                    weight: face.weight,
                    italic: face.italic,
                    stretch: face.stretch,
                    ascent: f64::from(face.metrics.ascent) / em,
                    descent: f64::from(face.metrics.descent) / em,
                }
            })
            .collect();
        Layout { pages, faces }
    }

    /// The vendored font files the document is drawn from, known before any
    /// face is loaded.
    pub fn fonts_needed(&self) -> Vec<&'static str> {
        let generics: HashMap<String, Generic> = self
            .fonts
            .iter()
            .map(|f| (f.name.to_lowercase(), f.generic))
            .collect();
        let mut needed = vec![fonts::fallback_file(false, false)];
        let mut add = |props: &RunProps, text: &mut dyn Iterator<Item = char>| {
            let mut slots: Vec<(Arc<str>, bool, bool)> = Vec::new();
            for c in text {
                let (family, _, bold, italic) = text::slot(c, props);
                let slot = if fonts::recode(family, c).is_some() {
                    (Arc::from(fonts::SYMBOL_FACE), bold, italic)
                } else {
                    (family.clone(), bold, italic)
                };
                if !slots.contains(&slot) {
                    slots.push(slot);
                }
            }
            for (family, bold, italic) in slots {
                let generic = generics
                    .get(&family.to_lowercase())
                    .copied()
                    .unwrap_or_default();
                if let Some(file) = fonts::vendored_file(&family, bold, italic, generic) {
                    if !needed.contains(&file) {
                        needed.push(file);
                    }
                }
            }
        };
        fn inlines(
            inlines: &[Inline],
            add: &mut dyn FnMut(&RunProps, &mut dyn Iterator<Item = char>),
        ) {
            for inline in inlines {
                match inline {
                    Inline::Text(t) if !t.props.hidden => add(&t.props, &mut t.text.chars()),
                    Inline::Char(c, props) => add(props, &mut std::iter::once(*c)),
                    Inline::Field(_, props) => add(props, &mut "0123456789".chars()),
                    Inline::Drawing(d) => {
                        if let Shape::Chart(chart) = &d.shape {
                            chart_texts(chart, add);
                        }
                    }
                    _ => {}
                }
            }
        }
        fn chart_texts(
            chart: &Chart,
            add: &mut dyn FnMut(&RunProps, &mut dyn Iterator<Item = char>),
        ) {
            for block in &chart.title {
                if let Block::Paragraph(p) = block {
                    add(&p.mark, &mut " ".chars());
                    inlines(&p.inlines, add);
                }
            }
            let names = chart.plots.iter().flat_map(|p| &p.series).map(|s| &s.name);
            let labels = chart.categories.iter().chain(names);
            let text: String = labels.flat_map(|l| l.text.chars()).collect();
            let axes = [
                &chart.category_axis,
                &chart.value_axis,
                &chart.secondary_axis,
            ];
            let props = axes
                .into_iter()
                .flatten()
                .filter_map(|a| a.labels.as_ref())
                .chain(chart.legend.as_ref().map(|l| &l.props));
            for props in props {
                add(
                    props,
                    &mut text.chars().chain('0'..='9').chain(".,%-".chars()),
                );
            }
        }
        visit_all(self, &mut |paragraph| {
            add(&paragraph.mark, &mut " ".chars());
            if let Some(label) = &paragraph.label {
                add(&label.props, &mut label.text.chars());
            }
            inlines(&paragraph.inlines, &mut add);
        });
        needed.sort_unstable();
        needed
    }
}

/// Every paragraph of the body, the cells and the headers and footers.
fn visit_all(document: &Document, visit: &mut dyn FnMut(&Paragraph)) {
    fn blocks(list: &[Block], visit: &mut dyn FnMut(&Paragraph)) {
        for block in list {
            match block {
                Block::Paragraph(p) => {
                    visit(p);
                    for inline in &p.inlines {
                        if let Inline::Drawing(d) = inline {
                            d.shape.stories(&mut |story| blocks(story, visit));
                        }
                    }
                }
                Block::Table(t) => {
                    for cell in t.rows.iter().flat_map(|r| &r.cells) {
                        blocks(&cell.blocks, visit);
                    }
                }
            }
        }
    }
    for section in &document.sections {
        blocks(&section.blocks, visit);
        for set in [&section.headers, &section.footers] {
            for story in [&set.default, &set.first, &set.even].into_iter().flatten() {
                blocks(story, visit);
            }
        }
    }
}

/// A body that shows the page count must be set twice, the second time
/// knowing it.
fn body_counts_pages(document: &Document) -> bool {
    fn blocks(list: &[Block]) -> bool {
        list.iter().any(|block| match block {
            Block::Paragraph(p) => p
                .inlines
                .iter()
                .any(|i| matches!(i, Inline::Field(Field::NumPages | Field::SectionPages, _))),
            Block::Table(t) => t
                .rows
                .iter()
                .flat_map(|r| &r.cells)
                .any(|c| blocks(&c.blocks)),
        })
    }
    document.sections.iter().any(|s| blocks(&s.blocks))
}
