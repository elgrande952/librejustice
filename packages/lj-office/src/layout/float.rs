//! Drawings on the page: the items a picture, a shape or a text box draws,
//! where a floating one lands, and the bands of the column its wrap takes.

use std::sync::Arc;

use super::flow::{stack, Column};
use super::metrics::LineBox;
use super::{metafile, Env, Fields, Item};
use crate::model::*;

/// A picture in its frame, before the frame's transform.
#[derive(Clone, Debug)]
pub struct Image {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub data: Arc<[u8]>,
    pub mime: &'static str,
    /// Share of the picture cut off on each side.
    pub crop: Margins,
    pub transform: Transform,
    pub recolor: Recolor,
}

/// A preset shape filling its frame.
#[derive(Clone, Debug)]
pub struct Outline {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub geometry: Geometry,
    pub fill: Option<Fill>,
    pub stroke: Option<Stroke>,
    pub transform: Transform,
}

/// A picture as the browser reads it, a bitmap it does not read written as
/// PNG; `None` for a metafile.
pub fn displayable(data: &Arc<[u8]>, format: ImageFormat) -> Option<(Arc<[u8]>, &'static str)> {
    let png = match format {
        ImageFormat::Bmp => crate::metafile::png_from_bmp(data),
        ImageFormat::Tiff => lj_image::tiff::to_png(data),
        ImageFormat::Eps => match crate::metafile::eps_preview(data) {
            Some((ImageFormat::Tiff, tiff)) => lj_image::tiff::to_png(tiff),
            _ => None,
        },
        _ => None,
    };
    match png {
        Some(png) => Some((Arc::from(png), "image/png")),
        None => Some((data.clone(), format.mime()?)),
    }
}

/// What a picture nothing here can draw shows instead.
const PLACEHOLDER: Color = Color(242, 242, 242);

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    env: &Env<'_>,
    shape: &Shape,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    transform: Transform,
    fields: Fields,
    out: &mut Vec<Item>,
) {
    let outline = |geometry, fill, stroke| {
        Item::Shape(Box::new(Outline {
            x,
            y,
            width,
            height,
            geometry,
            fill,
            stroke,
            transform,
        }))
    };
    match shape {
        Shape::Picture(picture) => {
            match displayable(&picture.data, picture.format) {
                Some((data, mime)) => out.push(Item::Image(Box::new(Image {
                    x,
                    y,
                    width,
                    height,
                    data,
                    mime,
                    crop: picture.crop,
                    transform,
                    recolor: picture.recolor,
                }))),
                None => match metafile::items(env, fields, picture, width, height) {
                    Some(items) => out.extend(items.iter().cloned().map(|mut item| {
                        item.translate(x, y);
                        item
                    })),
                    None => out.push(Item::Rect {
                        x,
                        y,
                        width,
                        height,
                        color: PLACEHOLDER,
                    }),
                },
            }
            if let Some(stroke) = picture.outline {
                out.push(outline(Geometry::Rect, None, Some(stroke)));
            }
        }
        Shape::Figure(figure) => {
            if figure.fill.is_some() || figure.stroke.is_some() {
                out.push(outline(
                    figure.geometry.clone(),
                    figure.fill.clone(),
                    figure.stroke,
                ));
            }
            if let Some(text) = &figure.text {
                let inset = text.inset;
                let inner = (width - inset.left - inset.right).max(1.0);
                let fill = figure
                    .fill
                    .as_ref()
                    .map(Fill::mean)
                    .filter(|f| f.alpha >= 0.5)
                    .map(|f| f.color);
                let (items, used) = env.over(fill, || stack(env, &text.blocks, inner, fields));
                let room = height - inset.top - inset.bottom;
                let shift = match text.valign {
                    VAlign::Top => 0.0,
                    VAlign::Center => ((room - used) / 2.0).max(0.0),
                    VAlign::Bottom => (room - used).max(0.0),
                };
                out.extend(items.into_iter().map(|mut item| {
                    item.translate(x + inset.left, y + inset.top + shift);
                    item
                }));
            }
        }
        Shape::Group(children) => {
            for child in children {
                draw(
                    env,
                    &child.shape,
                    x + child.x,
                    y + child.y,
                    child.width,
                    child.height,
                    child.transform,
                    fields,
                    out,
                );
            }
        }
        Shape::Chart(chart) => super::chart::draw(env, chart, x, y, width, height, fields, out),
        Shape::WordArt(art) => {
            if let Some(art) = word_art(env, art, [x, y, width, height], transform, fields) {
                out.push(Item::Art(Box::new(art)));
            }
        }
        Shape::Opaque => {}
    }
}

/// Characters drawn as a picture, their capitals stretched over a frame,
/// which the words of the page leave out.
#[derive(Clone, Debug)]
pub struct Art {
    pub glyphs: Vec<super::Glyphs>,
    /// From where the characters are set to the frame.
    pub fit: lj_draw::Matrix,
    /// The frame, `[x, y, width, height]`, and its turn.
    pub frame: [f64; 4],
    pub transform: Transform,
    pub alpha: f64,
}

/// Of a character's size, how far a capital rises above the baseline and a
/// descender drops below it.
const CAP_HEIGHT: f64 = 0.72;
const DESCENDER: f64 = 0.21;

fn word_art(
    env: &Env<'_>,
    art: &WordArt,
    frame: [f64; 4],
    transform: Transform,
    fields: Fields,
) -> Option<Art> {
    let paragraph = Paragraph {
        props: ParagraphProps::default(),
        mark: art.props.clone(),
        style: None,
        label: None,
        inlines: art
            .text
            .chars()
            .map(|c| Inline::Char(c, art.props.clone()))
            .collect(),
        hidden_section: false,
    };
    let (items, _) = stack(env, &[Block::Paragraph(Box::new(paragraph))], 1e6, fields);
    let glyphs: Vec<super::Glyphs> = items
        .into_iter()
        .filter_map(|item| match item {
            Item::Glyphs(g) => Some(g),
            _ => None,
        })
        .collect();
    let left = glyphs.iter().map(|g| g.x).reduce(f64::min)?;
    let right = glyphs
        .iter()
        .map(|g| g.x + g.advances.iter().sum::<f64>() - g.letter_spacing)
        .reduce(f64::max)?;
    let baseline = glyphs.iter().map(|g| g.y).reduce(f64::max)?;
    let size = art.props.size;
    let descends = art.text.chars().any(|c| "gjpqy,;".contains(c));
    let top = baseline - CAP_HEIGHT * size;
    let bottom = baseline + if descends { DESCENDER * size } else { 0.0 };
    let [x, y, width, height] = frame;
    if right <= left || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let fit = lj_draw::Matrix::translate(-left, -top)
        .then(lj_draw::Matrix::scale(
            width / (right - left),
            height / (bottom - top),
        ))
        .then(lj_draw::Matrix::translate(x, y));
    Some(Art {
        glyphs,
        fit,
        frame,
        transform,
        alpha: art.alpha,
    })
}

/// The width of `drawing`'s frame, a text box that fits its text taking its
/// widest line set at most `room` wide.
pub(super) fn width(env: &Env<'_>, drawing: &Drawing, fields: Fields, room: f64) -> f64 {
    let Shape::Figure(figure) = &drawing.shape else {
        return drawing.width;
    };
    let Some(text) = figure.text.as_ref().filter(|t| t.fit) else {
        return drawing.width;
    };
    let key: *const Drawing = drawing;
    if let Some(width) = env.fitted.borrow().get(&key) {
        return *width;
    }
    let insets = text.inset.left + text.inset.right;
    let (items, _) = stack(env, &text.blocks, (room - insets).max(1.0), fields);
    let (left, right) = items
        .iter()
        .filter_map(|item| match item {
            Item::Glyphs(g) => Some((g.x, g.x + g.advances.iter().sum::<f64>())),
            _ => None,
        })
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), (a, b)| {
            (l.min(a), r.max(b))
        });
    // A hair over the widest line, which sets again on one line.
    let width = drawing.width.max(right - left + FIT_SLACK).max(0.0) + insets;
    env.fitted.borrow_mut().insert(key, width);
    width
}

const FIT_SLACK: f64 = 0.01;

/// The height of `drawing`'s frame `width` wide, a text box that grows
/// taking its text's when that is taller.
pub(super) fn height(env: &Env<'_>, drawing: &Drawing, width: f64, fields: Fields) -> f64 {
    let Shape::Figure(figure) = &drawing.shape else {
        return drawing.height;
    };
    let Some(text) = figure.text.as_ref().filter(|t| t.grow) else {
        return drawing.height;
    };
    let key: *const Drawing = drawing;
    if let Some(height) = env.grown.borrow().get(&key) {
        return *height;
    }
    let inner = (width - text.inset.left - text.inset.right).max(1.0);
    let (_, used) = stack(env, &text.blocks, inner, fields);
    let height = drawing
        .height
        .max(used + text.inset.top + text.inset.bottom);
    env.grown.borrow_mut().insert(key, height);
    height
}

/// The box an object `height` high set in a line takes above the baseline,
/// raised or lowered with its run.
pub(super) fn object_box(height: f64, rise: f64) -> LineBox {
    let ascent = (height + rise).max(0.0);
    LineBox {
        ascent,
        height: ascent + (-rise).max(0.0),
    }
}

/// A line holding objects: the line spacing adds to it what it adds to the
/// text alone, `text` being the paragraph mark's box when there is none.
pub(super) fn line_box(
    text: LineBox,
    has_text: bool,
    objects: LineBox,
    rule: LineSpacing,
    spaced: impl Fn(LineBox, LineSpacing) -> LineBox,
) -> LineBox {
    let base = if has_text {
        text.union(objects)
    } else {
        objects
    };
    match rule {
        LineSpacing::Exact(_) => spaced(text, rule),
        LineSpacing::AtLeast(value) if value > base.height => LineBox {
            ascent: base.ascent + value - base.height,
            height: value,
        },
        LineSpacing::AtLeast(_) => base,
        LineSpacing::Proportional(_) => {
            let set = spaced(text, rule);
            LineBox {
                ascent: base.ascent + (set.ascent - text.ascent).min(0.0),
                height: base.height + set.height - text.height,
            }
        }
    }
}

/// What a floating object is placed against.
pub(super) struct Area<'p> {
    pub page: &'p PageSetup,
    pub column: Column,
    /// Where the anchoring paragraph's area and its first line start.
    pub paragraph: f64,
    pub line: f64,
    pub odd: bool,
}

fn span(start: f64, end: f64, size: f64, place: Place, odd: bool) -> f64 {
    match place {
        Place::Offset(offset) => start + offset,
        Place::Start => start,
        Place::End => end - size,
        Place::Center => start + (end - start - size) / 2.0,
        Place::Inside if odd => start,
        Place::Inside => end - size,
        Place::Outside if odd => end - size,
        Place::Outside => start,
    }
}

/// The top left corner of a floating object `width` by `height`.
pub(super) fn position(float: &Float, width: f64, height: f64, area: &Area<'_>) -> (f64, f64) {
    let page = area.page;
    let m = &page.margin;
    let (left, right) = (m.left + page.gutter, page.width - m.right);
    let h = &float.horizontal;
    let (start, end) = match h.from {
        Relative::Page => (0.0, page.width),
        Relative::Margin => (left, right),
        Relative::LeftMargin => (0.0, left),
        Relative::RightMargin => (right, page.width),
        Relative::InsideMargin if area.odd => (0.0, left),
        Relative::InsideMargin => (right, page.width),
        Relative::OutsideMargin if area.odd => (right, page.width),
        Relative::OutsideMargin => (0.0, left),
        _ => (area.column.x, area.column.x + area.column.width),
    };
    let x = span(start, end, width, h.place, area.odd);
    let v = &float.vertical;
    let (top, bottom) = (m.top, page.height - m.bottom);
    let (start, end) = match v.from {
        Relative::Page => (0.0, page.height),
        Relative::Margin => (top, bottom),
        Relative::TopMargin => (0.0, top),
        Relative::BottomMargin => (bottom, page.height),
        Relative::InsideMargin | Relative::OutsideMargin => (0.0, top),
        Relative::Line => (area.line, area.line),
        _ => (area.paragraph, area.paragraph),
    };
    let place = match (v.from, v.place) {
        (Relative::Paragraph | Relative::Line | Relative::Column | Relative::Character, p)
            if !matches!(p, Place::Offset(_)) =>
        {
            Place::Offset(0.0)
        }
        (_, p) => p,
    };
    (x, span(start, end, height, place, true))
}

/// A rectangle of a page the text keeps out of.
#[derive(Clone, Copy, Debug)]
pub(super) struct Exclusion {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub wrap: Wrap,
    pub side: WrapSide,
}

/// An exclusion as one paragraph's lines meet it: `top` and `bottom` from
/// its first line, `left` and `right` from its column.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Band {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
    /// No text beside it.
    pub full: bool,
    pub side: WrapSide,
}

/// Narrower than this, the space beside an object takes no text.
const NARROWEST: f64 = 18.0;

/// Where a line from `top`, `height` high and running from `left` to
/// `right`, may run beside the bands: the widest gap they leave, else `Err`
/// with the first place below where one of them ends.
pub(crate) fn beside(
    bands: &[Band],
    top: f64,
    height: f64,
    left: f64,
    right: f64,
) -> Result<(f64, f64), f64> {
    let hit: Vec<&Band> = bands
        .iter()
        .filter(|b| b.bottom > top && b.top < top + height)
        .filter(|b| b.right > left && b.left < right)
        .collect();
    if hit.is_empty() {
        return Ok((left, right));
    }
    if let Some(bottom) = hit
        .iter()
        .filter(|b| b.full)
        .map(|b| b.bottom)
        .reduce(f64::max)
    {
        return Err(bottom);
    }
    let mut free = vec![(left, right)];
    for band in &hit {
        let (from, to) = match band.side {
            WrapSide::Left => (band.left, f64::INFINITY),
            WrapSide::Right => (f64::NEG_INFINITY, band.right),
            WrapSide::Both => (band.left, band.right),
            WrapSide::Largest if band.left - left >= right - band.right => {
                (band.left, f64::INFINITY)
            }
            WrapSide::Largest => (f64::NEG_INFINITY, band.right),
        };
        free = free
            .into_iter()
            .flat_map(|(a, b)| [(a, b.min(from)), (a.max(to), b)])
            .filter(|(a, b)| b > a)
            .collect();
    }
    free.into_iter()
        .filter(|(a, b)| b - a >= NARROWEST.min(right - left))
        .reduce(|x, y| if y.1 - y.0 > x.1 - x.0 { y } else { x })
        .ok_or_else(|| hit.iter().map(|b| b.bottom).fold(f64::INFINITY, f64::min))
}

/// A floating object placed on a page, its items in page coordinates.
#[derive(Clone)]
pub(super) struct Placed {
    pub z: i64,
    pub behind: bool,
    pub items: Vec<Item>,
}

#[derive(Clone, Default)]
pub(super) struct PageFloats {
    pub placed: Vec<Placed>,
    pub exclusions: Vec<Exclusion>,
}

/// The floating objects of each page, by page index.
#[derive(Default)]
pub(super) struct Floats {
    pub pages: Vec<PageFloats>,
}

impl Floats {
    pub fn page(&mut self, index: usize) -> &mut PageFloats {
        if self.pages.len() <= index {
            self.pages.resize_with(index + 1, PageFloats::default);
        }
        &mut self.pages[index]
    }
}

/// The items of the floats `behind` the text, or over it, in z order.
pub(super) fn layer(placed: &mut [Placed], behind: bool) -> Vec<Item> {
    placed.sort_by_key(|p| p.z);
    placed
        .iter_mut()
        .filter(|p| p.behind == behind)
        .flat_map(|p| std::mem::take(&mut p.items))
        .collect()
}

/// The band `exclusion` makes for a paragraph whose first line starts at
/// `top` in `column`.
pub(super) fn band(exclusion: &Exclusion, top: f64, column: Column) -> Band {
    Band {
        top: exclusion.top - top,
        bottom: exclusion.bottom - top,
        left: exclusion.left - column.x,
        right: exclusion.right - column.x,
        full: exclusion.wrap == Wrap::TopAndBottom,
        side: exclusion.side,
    }
}

#[cfg(test)]
mod tests {
    use super::{beside, Band, WrapSide};

    fn band(left: f64, right: f64, bottom: f64) -> Band {
        Band {
            top: 0.0,
            bottom,
            left,
            right,
            full: false,
            side: WrapSide::Both,
        }
    }

    #[test]
    fn a_line_takes_the_widest_gap_the_objects_leave_else_the_first_bottom() {
        let pictures = [band(0.0, 114.0, 100.0), band(314.0, 425.0, 95.0)];
        assert_eq!(beside(&pictures, 0.0, 14.0, 0.0, 432.0), Ok((114.0, 314.0)));

        let title = band(116.0, 311.0, 30.0);
        let bands = [title, pictures[0], pictures[1]];
        assert_eq!(beside(&bands, 0.0, 14.0, 0.0, 432.0), Err(30.0));
        assert_eq!(beside(&bands, 30.0, 14.0, 0.0, 432.0), Ok((114.0, 314.0)));

        assert_eq!(beside(&pictures[1..], 0.0, 14.0, 0.0, 3.0), Ok((0.0, 3.0)));
    }
}
