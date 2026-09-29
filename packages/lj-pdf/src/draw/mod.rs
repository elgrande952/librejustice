//! A page drawn as a display list, in points from its top left corner, lined
//! up with the quads of its text.

pub(crate) mod color;
pub(crate) mod function;
pub(crate) mod image;
pub(crate) mod pattern;
pub(crate) mod shading;
pub(crate) mod state;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use lj_draw::{
    Blend, Clip, Fill, Gradient, GradientKind, Group, Image, Item, Mask, Page, Paint, Rgb, Segment,
    Shape, Stroke, Text,
};

use crate::graphics::{Matrix, Point};
use crate::interpret::{Device, PathPaint, ShownGlyph, ShownImage, TransparencyGroup, walk_page};
use crate::objects::{ObjectRef, PageInfo, PdfFile, PdfResult, PdfValue};
use crate::text::Face;
use crate::text::program::{Drawn, Program, UNITS};
use color::{Color, ColorSpace, Tile};
use pattern::Pattern;
use shading::{Kind, Shading};
use state::{ClipPath, GraphicsState};

/// The thinnest stroke, in points: a zero width asks for the thinnest line
/// the device shows.
const HAIRLINE: f64 = 0.5;

pub fn draw_page(file: &PdfFile, page: &PageInfo) -> PdfResult<Page> {
    let size = page.page_box.size();
    let drawing = Drawing {
        page_transform: page.page_box.normalized_transform(),
        flip: Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: -1.0,
            e: 0.0,
            f: size.height,
        },
        page: Page {
            width: size.width,
            height: size.height,
            ..Page::default()
        },
        open: vec![Open {
            clip: None,
            form: Some(0),
            group: Group::new(Vec::new()),
        }],
        run: None,
        outlines: HashMap::new(),
        pictures: HashMap::new(),
        tiles: HashMap::new(),
        drawing_tile: Vec::new(),
        drawing_mask: Vec::new(),
        patterns: HashMap::new(),
    };
    Ok(walk_page(file, page, drawing)?.finish())
}

/// A group still taking items.
struct Open {
    clip: Option<Rc<ClipPath>>,
    /// For the page and each form: how many clips were in force as it began.
    form: Option<usize>,
    group: Group,
}

/// Glyphs on one baseline in one face, becoming one text item.
struct Run {
    /// The text's own space, points with y down, to the page.
    transform: Matrix,
    inverse: Matrix,
    size: f64,
    family: &'static str,
    color: Rgb,
    text: String,
    advances: Vec<f64>,
    last_x: f64,
    /// The characters the last glyph decodes to, which share its advance.
    last_chars: usize,
    /// Where the next glyph stands if the string is set in the font's own
    /// widths.
    next_x: f64,
    clips: Vec<Rc<ClipPath>>,
    blend: Option<Blend>,
    mask: Option<usize>,
    alpha: f64,
}

pub(crate) struct Drawing {
    page_transform: Matrix,
    /// The normalised page, y up, to the drawing, y down.
    flip: Matrix,
    page: Page,
    open: Vec<Open>,
    run: Option<Run>,
    /// Each glyph met so far, by its program and id.
    outlines: HashMap<(*const Program, u32), Outline>,
    /// Each XObject decoded once, whatever the times it is drawn, keyed with
    /// the colour a stencil paints.
    pictures: HashMap<PictureKey, Option<Picture>>,
    /// The items of each tiling cell drawn, in the pattern's space.
    tiles: HashMap<Tile, Rc<Vec<Item>>>,
    /// While a cell is drawn, what it set aside: the page's placement and
    /// the groups open on the page.
    drawing_tile: Vec<(Matrix, Matrix, Vec<Open>)>,
    /// While a soft mask's group is drawn, the groups open on the page.
    drawing_mask: Vec<Vec<Open>>,
    /// Each pattern written, by its cell and its matrix to the item.
    patterns: HashMap<(Tile, [u64; 6]), usize>,
}

/// An XObject, and the colour it paints when it is a stencil.
type PictureKey = (ObjectRef, Option<(u8, u8, u8)>);

#[derive(Clone)]
struct Picture {
    mime: &'static str,
    data: Arc<[u8]>,
    smooth: bool,
    alpha: Option<Arc<[u8]>>,
}

#[derive(Clone, Copy)]
enum Outline {
    /// The program cannot draw it: the glyph is shown as text.
    Broken,
    Blank,
    /// Its index in the page's outlines.
    Drawn(usize),
}

impl Drawing {
    fn to_page(&self, ctm: Matrix) -> Matrix {
        ctm.multiply(self.page_transform).multiply(self.flip)
    }

    fn finish(mut self) -> Page {
        self.flush_run();
        while self.open.len() > 1 {
            self.close();
        }
        let root = self.open.pop().expect("the page group");
        self.page.items = root.group.items;
        // The page blends over a transparent backdrop, not over its paper.
        if blends(&self.page.items) {
            let items = std::mem::take(&mut self.page.items);
            self.page.items = vec![Item::Group(Box::new(Group {
                isolated: true,
                ..Group::new(items)
            }))];
        }
        self.page
    }

    /// Opens and closes clip groups until those open are the ones in force.
    fn sync(&mut self, clips: &[Rc<ClipPath>]) {
        let form = self
            .open
            .iter()
            .rposition(|open| open.form.is_some())
            .expect("the page group");
        let base = self.open[form].form.unwrap_or(0);
        let wanted = clips.get(base..).unwrap_or(&[]);
        let common = self.open[form + 1..]
            .iter()
            .zip(wanted)
            .take_while(|(open, clip)| open.clip.as_ref().is_some_and(|c| Rc::ptr_eq(c, clip)))
            .count();
        while self.open.len() > form + 1 + common {
            self.close();
        }
        for clip in &wanted[common..] {
            let shape = Shape::Path(transformed(
                &clip.segments,
                clip.transform.multiply(self.flip),
            ));
            let clip_shape = Clip {
                shape,
                even_odd: clip.even_odd,
            };
            // A clip an open group already applies adds nothing, and each
            // clip group costs a rasteriser a mask the size of the page.
            let repeated = self
                .open
                .iter()
                .any(|open| open.group.clip.as_ref() == Some(&clip_shape));
            let group = Group {
                clip: (!repeated).then_some(clip_shape),
                ..Group::new(Vec::new())
            };
            self.open.push(Open {
                clip: Some(clip.clone()),
                form: None,
                group,
            });
        }
    }

    fn close(&mut self) {
        let mut group = self.open.pop().expect("an open group").group;
        if group.items.is_empty() {
            return;
        }
        // Isolation only changes what a blend mode inside meets.
        group.isolated &= blends(&group.items);
        let parent = &mut self.open.last_mut().expect("the page group").group.items;
        let plain = group.clip.is_none()
            && group.opacity >= 1.0
            && group.blend.is_none()
            && group.mask.is_none()
            && !group.isolated
            && group.transform == lj_draw::Matrix::IDENTITY;
        match plain {
            true => parent.extend(group.items),
            false => parent.push(Item::Group(Box::new(group))),
        }
    }

    fn push(
        &mut self,
        item: Item,
        clips: &[Rc<ClipPath>],
        blend: Option<Blend>,
        mask: Option<usize>,
    ) {
        self.sync(clips);
        let item = match (blend, mask) {
            (None, None) => item,
            (blend, mask) => Item::Group(Box::new(Group {
                blend,
                mask,
                ..Group::new(vec![item])
            })),
        };
        self.open
            .last_mut()
            .expect("the page group")
            .group
            .items
            .push(item);
    }

    /// What a fill or a stroke paints with, on an item whose own space
    /// `item_to_page` maps onto the page.
    fn paint_of(
        &mut self,
        color: &Color,
        state: &GraphicsState,
        item_to_page: Matrix,
    ) -> Option<Paint> {
        if let (ColorSpace::Pattern(_), Some(pattern)) = (&*color.space, &color.pattern)
            && let Pattern::Shading { shading, matrix } = &**pattern
            && let Some(to_item) = item_to_page.inverse()
        {
            let to_page = self.to_page(matrix.multiply(state.pattern_base));
            if let Some(index) = self.gradient(shading, to_page.multiply(to_item)) {
                return Some(Paint::Gradient(index));
            }
        }
        if let Some(tile) = color.tile()
            && let Some(Pattern::Tiling {
                bbox, step, matrix, ..
            }) = color.pattern.as_deref()
            && let Some(to_item) = item_to_page.inverse()
        {
            let transform = self
                .to_page(matrix.multiply(state.pattern_base))
                .multiply(to_item);
            let key = (
                tile,
                [
                    transform.a,
                    transform.b,
                    transform.c,
                    transform.d,
                    transform.e,
                    transform.f,
                ]
                .map(f64::to_bits),
            );
            if let Some(index) = self.patterns.get(&key) {
                return Some(Paint::Pattern(*index));
            }
            let items = self.tiles.get(&tile)?.clone();
            let pattern_to_page = self.to_page(matrix.multiply(state.pattern_base));
            let window = pattern_to_page.inverse().map(|inverse| {
                let (w, h) = (self.page.width, self.page.height);
                bounds(&polygon(&[[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(
                    |[x, y]| {
                        let p = inverse.transform_point(Point::new(x, y));
                        [p.x, p.y]
                    },
                )))
            })?;
            // A step longer than the page shows one cell at most: the tile
            // shrinks around it, or a rasteriser would fill the whole step.
            let mut offset = [0.0; 2];
            let mut size = *step;
            for axis in 0..2 {
                let (low, high) = (window[axis], window[axis + 2]);
                let (cell_low, cell_high) = (bbox[axis], bbox[axis + 2]);
                let reach = (high - low) + (cell_high - cell_low);
                if step[axis] > reach {
                    let k = ((low - cell_high) / step[axis]).ceil();
                    if k * step[axis] + cell_low > high {
                        return None;
                    }
                    offset[axis] = k * step[axis];
                    size[axis] = reach;
                }
            }
            // The tile starts at the cell's corner, so that a cell no larger
            // than its step lies in one tile.
            let [x0, y0] = [bbox[0], bbox[1]];
            let transform = Matrix::translate(offset[0] + x0, offset[1] + y0).multiply(transform);
            let [width, height] = size;
            // A cell reaching past its step shows on the tiles after it too.
            let copies = |extent: f64, step: f64| (extent / step).ceil().clamp(1.0, 3.0) as i32;
            let (across, up) = (copies(bbox[2] - x0, width), copies(bbox[3] - y0, height));
            let items = (0..across)
                .flat_map(|i| (0..up).map(move |j| (i, j)))
                .map(|(i, j)| {
                    Item::Group(Box::new(Group {
                        transform: lj_draw::Matrix {
                            e: -x0 - f64::from(i) * width,
                            f: -y0 - f64::from(j) * height,
                            ..lj_draw::Matrix::IDENTITY
                        },
                        ..Group::new(items.to_vec())
                    }))
                })
                .collect();
            self.page.patterns.push(lj_draw::Pattern {
                transform: draw_matrix(transform),
                width,
                height,
                items,
            });
            let index = self.page.patterns.len() - 1;
            self.patterns.insert(key, index);
            return Some(Paint::Pattern(index));
        }
        color.rgb().map(Paint::Color)
    }

    /// An axial or radial shading as a gradient, placed on its item by
    /// `to_item`.
    fn gradient(&mut self, shading: &Shading, to_item: Matrix) -> Option<usize> {
        let (kind, stops) = match &shading.kind {
            Kind::Axial {
                from, to, stops, ..
            } => (
                GradientKind::Linear {
                    from: *from,
                    to: *to,
                },
                stops,
            ),
            Kind::Radial {
                from,
                from_radius,
                to,
                to_radius,
                stops,
                ..
            } => (
                GradientKind::Radial {
                    from: *from,
                    from_radius: *from_radius,
                    to: *to,
                    to_radius: *to_radius,
                },
                stops,
            ),
            Kind::Function { .. } | Kind::Mesh { .. } => return None,
        };
        self.page.gradients.push(Gradient {
            kind,
            transform: draw_matrix(to_item),
            stops: stops.clone(),
        });
        Some(self.page.gradients.len() - 1)
    }

    fn text_glyph(&mut self, glyph: &ShownGlyph<'_>, state: &GraphicsState) {
        let mode = state.text.text_render_mode;
        let (paint, alpha) = match mode {
            1 | 5 => (&state.stroke, state.stroke_alpha),
            _ => (&state.fill, state.fill_alpha),
        };
        let Some(color) = paint.rgb() else {
            return;
        };
        let chars: Vec<char> = glyph
            .decoded
            .text
            .chars()
            .filter(|c| !c.is_control() && *c != '\u{FFFD}')
            .collect();
        if chars.is_empty() {
            return;
        }
        let m = glyph.em_to_page.multiply(self.flip);
        let size = m.c.hypot(m.d);
        if !size.is_finite() || size <= 1e-3 {
            return;
        }
        let transform = Matrix::scale(1.0 / size, -1.0 / size).multiply(m);
        let family = family(glyph.font.face());
        let advance = glyph.decoded.width_units / 1000.0 * size;
        if let Some(run) = self.run.as_mut()
            && run.family == family
            && run.color == color
            && (run.size - size).abs() < size * 1e-3
            && same_axes(run.transform, transform)
            && run.blend == state.blend
            && run.mask == state.soft_mask
            && run.alpha == alpha
            && same_clips(&run.clips, &state.clips)
        {
            let origin = run
                .inverse
                .transform_point(Point::new(transform.e, transform.f));
            // The face shown has widths of its own: a glyph set apart from
            // its neighbour, as a word by a TJ offset, starts after a space,
            // where the text is placed anew, and one set back starts an item.
            let gap = origin.x - run.next_x;
            let spaced = gap >= size * 0.1
                && run.text.chars().last().is_some_and(|c| !c.is_whitespace())
                && !chars[0].is_whitespace();
            if origin.y.abs() < size * 0.05 && (gap.abs() < size * 0.1 || spaced) {
                let step = match spaced {
                    true => run.next_x - run.last_x,
                    false => origin.x - run.last_x,
                };
                let len = run.advances.len();
                for last in &mut run.advances[len - run.last_chars..] {
                    *last = step / run.last_chars as f64;
                }
                if spaced {
                    run.text.push(' ');
                    run.advances.push(gap);
                }
                run.last_chars = chars.len();
                for c in chars {
                    run.text.push(c);
                    run.advances.push(0.0);
                }
                run.last_x = origin.x;
                run.next_x = origin.x + advance;
                return;
            }
        }
        self.flush_run();
        let Some(inverse) = transform.inverse() else {
            return;
        };
        self.run = Some(Run {
            transform,
            inverse,
            size,
            family,
            color,
            advances: vec![0.0; chars.len()],
            last_chars: chars.len(),
            text: chars.into_iter().collect(),
            last_x: 0.0,
            next_x: advance,
            clips: state.clips.clone(),
            blend: state.blend,
            mask: state.soft_mask,
            alpha,
        });
    }

    /// A glyph's outline, written once per page.
    fn outline(&mut self, program: &Arc<Program>, gid: u32) -> Outline {
        let key = (Arc::as_ptr(program), gid);
        if let Some(outline) = self.outlines.get(&key) {
            return *outline;
        }
        let outline = match program.outline(gid) {
            None => Outline::Broken,
            Some(segments) if segments.is_empty() => Outline::Blank,
            Some(segments) => {
                // In ems, so that the matrix placing a glyph carries its size
                // in points, which three decimals keep.
                let em = Matrix::scale(1.0 / UNITS, 1.0 / UNITS);
                self.page.outlines.push(transformed(&segments, em));
                Outline::Drawn(self.page.outlines.len() - 1)
            }
        };
        self.outlines.insert(key, outline);
        outline
    }

    fn outline_glyph(&mut self, outline: usize, glyph: &ShownGlyph<'_>, state: &GraphicsState) {
        let mode = state.text.text_render_mode;
        let m = glyph.em_to_page.multiply(self.flip);
        let glyph_scale = (m.a * m.d - m.b * m.c).abs().sqrt();
        if !glyph_scale.is_finite() || glyph_scale < 1e-9 {
            return;
        }
        let fill = match mode {
            0 | 2 | 4 | 6 => self.paint_of(&state.fill, state, m).map(|paint| Fill {
                paint,
                opacity: state.fill_alpha,
                even_odd: false,
            }),
            _ => None,
        };
        let user = self.to_page(state.ctm);
        let factor = (user.a * user.d - user.b * user.c).abs().sqrt() / glyph_scale;
        let stroke = match mode {
            1 | 2 | 5 | 6 => self.paint_of(&state.stroke, state, m).map(|paint| Stroke {
                paint,
                opacity: state.stroke_alpha,
                width: (state.line_width.abs() * factor).max(HAIRLINE / glyph_scale),
                cap: state.cap,
                join: state.join,
                miter_limit: state.miter_limit,
                dash: match state.dash.iter().all(|d| *d >= 0.0) {
                    true => state.dash.iter().map(|d| d * factor).collect(),
                    false => Vec::new(),
                },
                dash_offset: state.dash_phase * factor,
            }),
            _ => None,
        };
        if fill.is_none() && stroke.is_none() {
            return;
        }
        self.flush_run();
        let item = Item::Glyph {
            outline,
            transform: draw_matrix(m),
            fill,
            stroke,
        };
        self.push(item, &state.clips, state.blend, state.soft_mask);
    }

    fn flush_run(&mut self) {
        let Some(mut run) = self.run.take() else {
            return;
        };
        let len = run.advances.len();
        for last in &mut run.advances[len - run.last_chars..] {
            *last = (run.next_x - run.last_x) / run.last_chars as f64;
        }
        let m = run.transform;
        let moved_only = (m.a - 1.0).abs() < 1e-6
            && m.b.abs() < 1e-6
            && m.c.abs() < 1e-6
            && (m.d - 1.0).abs() < 1e-6;
        let (x, y) = if moved_only { (m.e, m.f) } else { (0.0, 0.0) };
        let mut item = Item::Text(Text {
            x,
            y,
            family: run.family.to_string(),
            size: run.size,
            color: run.color,
            text: run.text,
            advances: run.advances,
            each_char: true,
            kerning: false,
            letter_spacing: 0.0,
            synthetic_bold: false,
            synthetic_italic: false,
        });
        if !moved_only {
            item = Item::Group(Box::new(Group {
                transform: draw_matrix(m),
                ..Group::new(vec![item])
            }));
        }
        if run.alpha < 1.0 {
            item = Item::Group(Box::new(Group {
                opacity: run.alpha,
                ..Group::new(vec![item])
            }));
        }
        self.push(item, &run.clips, run.blend, run.mask);
    }
}

impl Device for Drawing {
    const DRAWS: bool = true;

    fn wants_tile(&self, tile: Tile) -> bool {
        !self.tiles.contains_key(&tile)
    }

    fn begin_tile(&mut self) {
        self.flush_run();
        let open = std::mem::replace(
            &mut self.open,
            vec![Open {
                clip: None,
                form: Some(0),
                group: Group::new(Vec::new()),
            }],
        );
        let page_transform = std::mem::replace(&mut self.page_transform, Matrix::identity());
        let flip = std::mem::replace(&mut self.flip, Matrix::identity());
        self.drawing_tile.push((page_transform, flip, open));
    }

    fn end_tile(&mut self, tile: Tile) {
        self.flush_run();
        while self.open.len() > 1 {
            self.close();
        }
        let items = self.open.pop().expect("the cell group").group.items;
        let (page_transform, flip, open) = self.drawing_tile.pop().expect("a cell drawn");
        self.page_transform = page_transform;
        self.flip = flip;
        self.open = open;
        self.tiles.insert(tile, Rc::new(items));
    }

    /// A glyph of an embedded program by its outline, of a font the file
    /// does not embed as text in the free face of its metrics.
    fn glyph(&mut self, glyph: &ShownGlyph<'_>, state: &GraphicsState) {
        if matches!(state.text.text_render_mode, 3 | 7) {
            return;
        }
        match glyph.font.drawn(glyph.decoded) {
            Drawn::Text => self.text_glyph(glyph, state),
            Drawn::Outline(program, gid) => match self.outline(program, gid) {
                Outline::Broken => self.text_glyph(glyph, state),
                Outline::Blank => {}
                Outline::Drawn(outline) => self.outline_glyph(outline, glyph, state),
            },
            Drawn::Procedure(..) | Drawn::Nothing => {}
        }
    }

    /// An image that cannot be decoded draws nothing, as in a viewer.
    fn image(&mut self, file: &PdfFile, shown: &ShownImage<'_>, state: &GraphicsState) {
        self.flush_run();
        let fill = state.fill.rgb();
        let decode = || {
            image::picture(file, shown, fill)
                .ok()
                .flatten()
                .map(|picture| Picture {
                    mime: picture.mime,
                    data: Arc::from(picture.data),
                    smooth: picture.smooth,
                    alpha: picture.alpha.map(Arc::from),
                })
        };
        let picture = match shown.source {
            Some(source) => {
                let paint = fill
                    .filter(|_| image::is_stencil(file, shown.dict))
                    .map(|Rgb(r, g, b)| (r, g, b));
                self.pictures
                    .entry((source, paint))
                    .or_insert_with(decode)
                    .clone()
            }
            None => decode(),
        };
        let Some(picture) = picture else {
            return;
        };
        let upside_down = Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: -1.0,
            e: 0.0,
            f: 1.0,
        };
        let on_page = self.to_page(state.ctm);
        let [width, height] = ["Width", "Height"].map(|key| {
            shown
                .dict
                .get(key)
                .and_then(|value| file.resolve(value).ok())
                .and_then(PdfValue::as_number)
                .unwrap_or(0.0)
        });
        // A viewer filters an image it shrinks, whatever its Interpolate says.
        let shrunk = width * height > on_page.a.hypot(on_page.b) * on_page.c.hypot(on_page.d);
        let transform = draw_matrix(upside_down.multiply(on_page));
        let image = |mime, data| {
            Item::Image(Image {
                transform,
                mime,
                data,
                smooth: picture.smooth || shrunk,
                recolor: None,
            })
        };
        let mut item = image(picture.mime, picture.data.clone());
        if let Some(alpha) = &picture.alpha {
            self.page.masks.push(Mask {
                items: vec![image("image/png", alpha.clone())],
                luminosity: true,
            });
            item = Item::Group(Box::new(Group {
                mask: Some(self.page.masks.len() - 1),
                ..Group::new(vec![item])
            }));
        }
        if state.fill_alpha < 1.0 {
            item = Item::Group(Box::new(Group {
                opacity: state.fill_alpha,
                ..Group::new(vec![item])
            }));
        }
        // An image's own soft mask stands in for the one in force.
        let mask = state
            .soft_mask
            .filter(|_| !shown.dict.contains_key("SMask"));
        self.push(item, &state.clips, state.blend, mask);
    }

    fn begin_form(
        &mut self,
        _form: ObjectRef,
        group: Option<TransparencyGroup>,
        state: &GraphicsState,
    ) {
        self.flush_run();
        self.sync(&state.clips);
        let group = match group {
            Some(TransparencyGroup { isolated, .. }) => Group {
                opacity: state.fill_alpha,
                blend: state.blend,
                mask: state.soft_mask,
                isolated,
                ..Group::new(Vec::new())
            },
            None => Group::new(Vec::new()),
        };
        self.open.push(Open {
            clip: None,
            form: Some(state.clips.len()),
            group,
        });
    }

    fn begin_mask(&mut self) {
        self.flush_run();
        let open = std::mem::replace(
            &mut self.open,
            vec![Open {
                clip: None,
                form: Some(0),
                group: Group::new(Vec::new()),
            }],
        );
        self.drawing_mask.push(open);
    }

    fn end_mask(&mut self, luminosity: bool, backdrop: Option<Rgb>) -> Option<usize> {
        self.flush_run();
        while self.open.len() > 1 {
            self.close();
        }
        let mut items = self.open.pop().expect("the mask group").group.items;
        self.open = self.drawing_mask.pop().expect("a mask drawn");
        if let Some(color) = backdrop.filter(|_| luminosity) {
            let (w, h) = (self.page.width, self.page.height);
            items.insert(
                0,
                Item::Shape {
                    shape: Shape::Path(polygon(&[[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]])),
                    fill: Some(Fill {
                        paint: Paint::Color(color),
                        opacity: 1.0,
                        even_odd: false,
                    }),
                    stroke: None,
                },
            );
        }
        self.page.masks.push(Mask { items, luminosity });
        Some(self.page.masks.len() - 1)
    }

    fn end_form(&mut self) {
        self.flush_run();
        while self.open.len() > 1 {
            let form = self.open.last().is_some_and(|open| open.form.is_some());
            self.close();
            if form {
                break;
            }
        }
    }

    fn paint(&mut self, path: &[Segment], paint: PathPaint, state: &GraphicsState) {
        self.flush_run();
        // A shading pattern fills the path as `sh` fills a clip.
        if let (Some(even_odd), ColorSpace::Pattern(_), Some(pattern)) =
            (paint.fill, &*state.fill.space, &state.fill.pattern)
            && let Pattern::Shading { shading, matrix } = &**pattern
        {
            let mut inside = state.clone();
            inside.clips.push(Rc::new(ClipPath {
                segments: path.to_vec(),
                transform: state.ctm.multiply(self.page_transform),
                even_odd,
            }));
            inside.ctm = matrix.multiply(state.pattern_base);
            self.shading(shading, &inside);
            if paint.stroke {
                self.paint(
                    path,
                    PathPaint {
                        fill: None,
                        stroke: true,
                    },
                    state,
                );
            }
            return;
        }
        let m = self.to_page(state.ctm);
        let scale = (m.a * m.d - m.b * m.c).abs().sqrt();
        let tolerance = 1e-6 * scale.max(1e-9);
        let conformal = (m.a - m.d).abs() + (m.b + m.c).abs() < tolerance
            || (m.a + m.d).abs() + (m.b - m.c).abs() < tolerance;
        // A stroke drawn under a matrix that stretches it keeps that matrix.
        let kept = !conformal && paint.stroke;
        let item_to_page = match kept {
            true => m,
            false => Matrix::identity(),
        };
        let fill = match paint.fill {
            Some(even_odd) => self
                .paint_of(&state.fill, state, item_to_page)
                .map(|paint| Fill {
                    paint,
                    opacity: state.fill_alpha,
                    even_odd,
                }),
            None => None,
        };
        let stroke_paint = match paint.stroke {
            true => self.paint_of(&state.stroke, state, item_to_page),
            false => None,
        };
        let stroke = |factor: f64, thinnest: f64| {
            let paint = stroke_paint?;
            let dash = match state.dash.iter().all(|d| *d >= 0.0) {
                true => state.dash.iter().map(|d| d * factor).collect(),
                false => Vec::new(),
            };
            Some(Stroke {
                paint,
                opacity: state.stroke_alpha,
                width: (state.line_width.abs() * factor).max(thinnest),
                cap: state.cap,
                join: state.join,
                miter_limit: state.miter_limit,
                dash,
                dash_offset: state.dash_phase * factor,
            })
        };
        let (shape, stroke) = match kept {
            false => (transformed(path, m), stroke(scale, HAIRLINE)),
            true => (path.to_vec(), stroke(1.0, HAIRLINE / scale.max(1e-9))),
        };
        if fill.is_none() && stroke.is_none() {
            return;
        }
        let mut item = Item::Shape {
            shape: Shape::Path(shape),
            fill,
            stroke,
        };
        if kept {
            item = Item::Group(Box::new(Group {
                transform: draw_matrix(m),
                ..Group::new(vec![item])
            }));
        }
        self.push(item, &state.clips, state.blend, state.soft_mask);
    }

    fn shading(&mut self, shading: &Shading, state: &GraphicsState) {
        self.flush_run();
        let m = self.to_page(state.ctm);
        let Some(inverse) = m.inverse() else {
            return;
        };
        // The tightest clip bounds what shows; when the shading covers it,
        // the clip itself is filled, which spares a group a rasteriser masks
        // over the whole page.
        let tight = self.tightest(&state.clips);
        let [x0, y0, x1, y1] = match &tight {
            Some(tight) => tight.bounds,
            None => [0.0, 0.0, self.page.width, self.page.height],
        };
        let window: Vec<[f64; 2]> = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
            .map(|[x, y]| {
                let p = inverse.transform_point(Point::new(x, y));
                [p.x, p.y]
            })
            .to_vec();
        let (region, even_odd, covers, paint) = match &shading.kind {
            Kind::Axial {
                from, to, extend, ..
            } => {
                let d = [to[0] - from[0], to[1] - from[1]];
                let length = d[0] * d[0] + d[1] * d[1];
                if length < 1e-12 {
                    return;
                }
                let n = [-d[1], d[0]];
                let span = |v: [f64; 2]| {
                    window
                        .iter()
                        .map(|p| ((p[0] - from[0]) * v[0] + (p[1] - from[1]) * v[1]) / length)
                        .fold([f64::INFINITY, f64::NEG_INFINITY], |[low, high], t| {
                            [low.min(t), high.max(t)]
                        })
                };
                let [t_low, t_high] = span(d);
                let [u_low, u_high] = span(n);
                let t0 = if extend[0] { t_low.min(0.0) } else { 0.0 };
                let t1 = if extend[1] { t_high.max(1.0) } else { 1.0 };
                let at =
                    |t: f64, u: f64| [from[0] + t * d[0] + u * n[0], from[1] + t * d[1] + u * n[1]];
                let band = polygon(&[at(t0, u_low), at(t1, u_low), at(t1, u_high), at(t0, u_high)]);
                let Some(index) = self.gradient(shading, m) else {
                    return;
                };
                let covers = t0 <= t_low && t_high <= t1;
                (band, false, covers, Paint::Gradient(index))
            }
            Kind::Radial {
                from,
                from_radius,
                to,
                to_radius,
                extend,
                ..
            } => {
                let (outer, inner) = match to_radius >= from_radius {
                    true => ((to, *to_radius, extend[1]), (from, *from_radius, extend[0])),
                    false => ((from, *from_radius, extend[0]), (to, *to_radius, extend[1])),
                };
                let hole = !inner.2 && inner.1 > 0.0;
                let inside = window
                    .iter()
                    .all(|p| (p[0] - outer.0[0]).hypot(p[1] - outer.0[1]) <= outer.1);
                let mut region = match outer.2 {
                    true => polygon(&window),
                    false => circle(*outer.0, outer.1),
                };
                if hole {
                    region.extend(circle(*inner.0, inner.1));
                }
                let Some(index) = self.gradient(shading, m) else {
                    return;
                };
                (
                    region,
                    hole,
                    !hole && (outer.2 || inside),
                    Paint::Gradient(index),
                )
            }
            Kind::Mesh {
                outline,
                pieces,
                average,
            } => {
                let flat = |path: &[Segment], color: Rgb| Item::Shape {
                    shape: Shape::Path(transformed(path, m)),
                    fill: Some(Fill {
                        paint: Paint::Color(color),
                        opacity: 1.0,
                        even_odd: false,
                    }),
                    stroke: None,
                };
                // The whole mesh in its average colour first, so that the
                // seams between pieces do not let the page show through.
                let mut items = vec![flat(outline, *average)];
                items.extend(pieces.iter().map(|(path, color)| flat(path, *color)));
                let item = Item::Group(Box::new(Group {
                    opacity: state.fill_alpha,
                    ..Group::new(items)
                }));
                self.push_shading(item, shading, m, state);
                return;
            }
            Kind::Function { to_space, rgb, .. } => {
                let n = shading::FUNCTION_SAMPLES as u32;
                let data = lj_image::png::encode(n, n, 8, lj_image::png::Kind::Rgb, rgb);
                let mut item = Item::Image(Image {
                    transform: draw_matrix(to_space.multiply(m)),
                    mime: "image/png",
                    data: Arc::from(data),
                    smooth: true,
                    recolor: None,
                });
                if state.fill_alpha < 1.0 {
                    item = Item::Group(Box::new(Group {
                        opacity: state.fill_alpha,
                        ..Group::new(vec![item])
                    }));
                }
                self.push_shading(item, shading, m, state);
                return;
            }
        };
        let (shape, even_odd, clips) = match (covers, tight) {
            (true, Some(tight)) => (tight.path, tight.even_odd, tight.others),
            _ => (transformed(&region, m), even_odd, state.clips.clone()),
        };
        let item = Item::Shape {
            shape: Shape::Path(shape),
            fill: Some(Fill {
                paint,
                opacity: state.fill_alpha,
                even_odd,
            }),
            stroke: None,
        };
        self.push_shading_in(item, shading, m, &clips, state);
    }
}

/// The clip whose bounds are the smallest, on the page.
struct Tightest {
    path: Vec<Segment>,
    even_odd: bool,
    bounds: [f64; 4],
    /// The clips still to apply: among those the innermost form sets, all but
    /// the tightest and the upright rectangles around it.
    others: Vec<Rc<ClipPath>>,
}

impl Drawing {
    fn tightest(&self, clips: &[Rc<ClipPath>]) -> Option<Tightest> {
        let paths: Vec<(Vec<Segment>, [f64; 4])> = clips
            .iter()
            .map(|clip| {
                let path = transformed(&clip.segments, clip.transform.multiply(self.flip));
                let bounds = bounds(&path);
                (path, bounds)
            })
            .collect();
        let area = |[x0, y0, x1, y1]: [f64; 4]| (x1 - x0) * (y1 - y0);
        let (index, (path, bounds)) = paths
            .iter()
            .enumerate()
            .min_by(|(_, (_, a)), (_, (_, b))| area(*a).total_cmp(&area(*b)))?;
        let [a, b, c, d] = *bounds;
        // The clips before the innermost form stay: `sync` finds the form's
        // own clips by their position in the list.
        let base = self
            .open
            .iter()
            .rev()
            .find_map(|open| open.form)
            .expect("the page group");
        let others = clips
            .iter()
            .zip(&paths)
            .enumerate()
            .filter(|(other, (_, (path, _)))| {
                *other < base
                    || *other != index
                        && !rectangle(path).is_some_and(|[x0, y0, x1, y1]| {
                            x0 <= a && y0 <= b && c <= x1 && d <= y1
                        })
            })
            .map(|(_, (clip, _))| clip.clone())
            .collect();
        Some(Tightest {
            path: path.clone(),
            even_odd: clips[index].even_odd,
            bounds: *bounds,
            others,
        })
    }

    /// A shading's item, inside its BBox when it has one.
    fn push_shading(&mut self, item: Item, shading: &Shading, m: Matrix, state: &GraphicsState) {
        self.push_shading_in(item, shading, m, &state.clips, state);
    }

    fn push_shading_in(
        &mut self,
        mut item: Item,
        shading: &Shading,
        m: Matrix,
        clips: &[Rc<ClipPath>],
        state: &GraphicsState,
    ) {
        if let Some([x0, y0, x1, y1]) = shading.bbox {
            let bbox = polygon(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
            item = Item::Group(Box::new(Group {
                clip: Some(Clip {
                    shape: Shape::Path(transformed(&bbox, m)),
                    even_odd: false,
                }),
                ..Group::new(vec![item])
            }));
        }
        self.push(item, clips, state.blend, state.soft_mask);
    }
}

/// `[x0, y0, x1, y1]` of a path that is one upright rectangle.
fn rectangle(path: &[Segment]) -> Option<[f64; 4]> {
    let mut points = Vec::with_capacity(5);
    for (index, segment) in path.iter().enumerate() {
        match (index, segment) {
            (0, Segment::Move(p)) | (1..=4, Segment::Line(p)) => points.push(*p),
            (4 | 5, Segment::Close) if index + 1 == path.len() => {}
            _ => return None,
        }
    }
    if points.len() == 5 && points[4] == points[0] {
        points.pop();
    }
    if points.len() != 4 {
        return None;
    }
    let upright = (0..4).all(|i| {
        let (p, q) = (points[i], points[(i + 1) % 4]);
        p[0] == q[0] || p[1] == q[1]
    });
    upright.then(|| bounds(&path[..path.len() - 1]))
}

/// `[x0, y0, x1, y1]` around every point of `path`, control points included.
fn bounds(path: &[Segment]) -> [f64; 4] {
    let mut out = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut add = |[x, y]: lj_draw::Point| {
        out = [out[0].min(x), out[1].min(y), out[2].max(x), out[3].max(y)];
    };
    for segment in path {
        match *segment {
            Segment::Move(p) | Segment::Line(p) => add(p),
            Segment::Cubic(a, b, c) => {
                add(a);
                add(b);
                add(c);
            }
            Segment::Close => {}
        }
    }
    out
}

fn blends(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::Group(group) => group.blend.is_some() || blends(&group.items),
        _ => false,
    })
}

fn polygon(points: &[[f64; 2]]) -> Vec<Segment> {
    let mut segments = vec![Segment::Move(points[0])];
    segments.extend(points[1..].iter().map(|p| Segment::Line(*p)));
    segments.push(Segment::Close);
    segments
}

fn circle([x, y]: [f64; 2], r: f64) -> Vec<Segment> {
    // The control distance of a quarter circle drawn as one cubic.
    let k = 0.552_284_75 * r;
    vec![
        Segment::Move([x + r, y]),
        Segment::Cubic([x + r, y + k], [x + k, y + r], [x, y + r]),
        Segment::Cubic([x - k, y + r], [x - r, y + k], [x - r, y]),
        Segment::Cubic([x - r, y - k], [x - k, y - r], [x, y - r]),
        Segment::Cubic([x + k, y - r], [x + r, y - k], [x + r, y]),
        Segment::Close,
    ]
}

/// The PostScript name of the free face a font's text is shown in, as the
/// faces are declared.
fn family(face: Face) -> &'static str {
    if face.symbol {
        return "DejaVuSans";
    }
    match (face.fixed, face.serif, face.bold, face.italic) {
        (true, _, false, false) => "NimbusMonoPS-Regular",
        (true, _, true, false) => "NimbusMonoPS-Bold",
        (true, _, false, true) => "NimbusMonoPS-Italic",
        (true, _, true, true) => "NimbusMonoPS-BoldItalic",
        (false, true, false, false) => "NimbusRoman-Regular",
        (false, true, true, false) => "NimbusRoman-Bold",
        (false, true, false, true) => "NimbusRoman-Italic",
        (false, true, true, true) => "NimbusRoman-BoldItalic",
        (false, false, false, false) => "NimbusSans-Regular",
        (false, false, true, false) => "NimbusSans-Bold",
        (false, false, false, true) => "NimbusSans-Italic",
        (false, false, true, true) => "NimbusSans-BoldItalic",
    }
}

/// The served font file of a face [`family`] names.
pub fn substitute_file(family: &str) -> Option<&'static str> {
    const FILES: [&str; 13] = [
        "DejaVuSans.ttf",
        "NimbusMonoPS-Regular.otf",
        "NimbusMonoPS-Bold.otf",
        "NimbusMonoPS-Italic.otf",
        "NimbusMonoPS-BoldItalic.otf",
        "NimbusRoman-Regular.otf",
        "NimbusRoman-Bold.otf",
        "NimbusRoman-Italic.otf",
        "NimbusRoman-BoldItalic.otf",
        "NimbusSans-Regular.otf",
        "NimbusSans-Bold.otf",
        "NimbusSans-Italic.otf",
        "NimbusSans-BoldItalic.otf",
    ];
    FILES.into_iter().find(|file| {
        file.rsplit_once('.')
            .is_some_and(|(stem, _)| stem == family)
    })
}

fn same_axes(left: Matrix, right: Matrix) -> bool {
    (left.a - right.a).abs() < 1e-4
        && (left.b - right.b).abs() < 1e-4
        && (left.c - right.c).abs() < 1e-4
        && (left.d - right.d).abs() < 1e-4
}

fn same_clips(left: &[Rc<ClipPath>], right: &[Rc<ClipPath>]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(l, r)| Rc::ptr_eq(l, r))
}

fn draw_matrix(m: Matrix) -> lj_draw::Matrix {
    lj_draw::Matrix {
        a: m.a,
        b: m.b,
        c: m.c,
        d: m.d,
        e: m.e,
        f: m.f,
    }
}

fn transformed(segments: &[Segment], m: Matrix) -> Vec<Segment> {
    let apply = |[x, y]: lj_draw::Point| {
        let p = m.transform_point(Point::new(x, y));
        [p.x, p.y]
    };
    segments
        .iter()
        .map(|segment| match *segment {
            Segment::Move(p) => Segment::Move(apply(p)),
            Segment::Line(p) => Segment::Line(apply(p)),
            Segment::Cubic(a, b, c) => Segment::Cubic(apply(a), apply(b), apply(c)),
            Segment::Close => Segment::Close,
        })
        .collect()
}
