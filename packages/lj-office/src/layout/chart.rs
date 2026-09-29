//! A chart in its frame, laid out as LibreOffice lays it out: the plot sized
//! around the axis labels, the labels drawn after it, title and legend last.

use super::float::Outline;
use super::flow::stack;
use super::{Env, Fields, Item};
use crate::model::*;

const TITLE_OFFSET: f64 = 6.0;
/// The share of the frame's inner width a title wraps in.
const TITLE_WIDTH: f64 = 0.8;
const TITLE_GAP: f64 = 9.0;
const LABEL_GAP: f64 = 2.75;
const CATEGORY_GAP: f64 = 1.4;
const TICK: f64 = 4.25;
const LEGEND_BELOW_GAP: f64 = 11.1;
const LEGEND_SIDE_GAP: f64 = 12.0;
const MAX_INTERVALS: f64 = 10.0;
const GRID: Color = Color(0xD9, 0xD9, 0xD9);

struct Label {
    items: Vec<Item>,
    width: f64,
    height: f64,
    /// The first line's baseline.
    baseline: f64,
}

impl Label {
    fn place(self, x: f64, y: f64, out: &mut Vec<Item>) {
        out.extend(self.items.into_iter().map(|mut item| {
            item.translate(x, y);
            item
        }));
    }
}

fn set(env: &Env<'_>, fields: Fields, blocks: &[Block], width: f64) -> Label {
    let (items, height) = stack(env, blocks, width, fields);
    let (mut left, mut right, mut baseline) = (f64::INFINITY, f64::NEG_INFINITY, None);
    for item in &items {
        if let Item::Glyphs(g) = item {
            left = left.min(g.x);
            right = right.max(g.x + g.advances.iter().sum::<f64>());
            baseline.get_or_insert(g.y);
        }
    }
    Label {
        items,
        width: (right - left).max(0.0),
        height,
        baseline: baseline.unwrap_or(height * 0.8),
    }
}

fn line(
    env: &Env<'_>,
    fields: Fields,
    text: &ChartText,
    props: &RunProps,
    alignment: Alignment,
    width: f64,
) -> Label {
    let paragraph = Paragraph {
        props: ParagraphProps {
            alignment,
            ..ParagraphProps::default()
        },
        mark: props.clone(),
        style: None,
        label: None,
        inlines: text.inlines(props),
        hidden_section: false,
    };
    set(env, fields, &[Block::Paragraph(Box::new(paragraph))], width)
}

fn written(text: String) -> ChartText {
    ChartText {
        text,
        anchors: Vec::new(),
    }
}

#[derive(Clone, Copy)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Rect {
    fn width(&self) -> f64 {
        self.right - self.left
    }

    fn height(&self) -> f64 {
        self.bottom - self.top
    }
}

/// The ticks of a value axis, from `min` to `max` every `step`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scale {
    min: f64,
    max: f64,
    step: f64,
}

impl Scale {
    fn ticks(&self) -> Vec<f64> {
        let count = ((self.max - self.min) / self.step).round().max(0.0) as usize;
        (0..=count)
            .map(|i| self.min + i as f64 * self.step)
            .collect()
    }

    fn share(&self, value: f64) -> f64 {
        ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }
}

/// The scale LibreOffice computes for values from `lo` to `hi`: zero
/// included, the smallest round step taking `intervals` at most, and with
/// `margin` one step more past a value nearer the end than a twentieth.
fn scale(lo: f64, hi: f64, axis: Option<&Axis>, intervals: f64, margin: bool) -> Scale {
    let (fixed_min, fixed_max, major) =
        axis.map_or((None, None, None), |a| (a.min, a.max, a.major));
    let lo = fixed_min.unwrap_or(lo.min(0.0));
    let mut hi = fixed_max.unwrap_or(hi.max(0.0));
    if hi <= lo {
        hi = lo + 1.0;
    }
    let mut magnitude = 10f64.powf(((hi - lo) / intervals).log10().floor());
    let mut norms = [1.0, 2.0, 2.5, 5.0].iter().cycle();
    loop {
        let norm = *norms.next().expect("the steps cycle");
        let step = major.unwrap_or(norm * magnitude);
        if norm == 5.0 {
            magnitude *= 10.0;
        }
        let mut min = fixed_min.unwrap_or(((lo / step) + 1e-9).floor() * step);
        let mut max = fixed_max.unwrap_or(((hi / step) - 1e-9).ceil() * step);
        if margin && fixed_max.is_none() && hi > 0.0 && (max - hi) / (max - min) < 0.05 {
            max += step;
        }
        if margin && fixed_min.is_none() && lo < 0.0 && (lo - min) / (max - min) < 0.05 {
            min -= step;
        }
        if major.is_some() || (max - min) / step <= intervals + 1e-9 {
            return Scale { min, max, step };
        }
    }
}

/// The lowest and highest value `plots` draw, stacks summed, and whether
/// they draw shares of the whole.
fn extent<'p>(plots: impl Iterator<Item = &'p Plot>) -> (f64, f64, bool) {
    let (mut lo, mut hi, mut percent) = (f64::INFINITY, f64::NEG_INFINITY, false);
    for plot in plots {
        let grouping = match plot.kind {
            PlotKind::Bar { grouping, .. }
            | PlotKind::Line { grouping }
            | PlotKind::Area { grouping } => grouping,
            PlotKind::Pie { .. } => continue,
        };
        match grouping {
            Grouping::Percent => percent = true,
            Grouping::Stacked => {
                let count = plot
                    .series
                    .iter()
                    .map(|s| s.values.len())
                    .max()
                    .unwrap_or(0);
                for i in 0..count {
                    let (mut up, mut down) = (0.0, 0.0);
                    for v in plot
                        .series
                        .iter()
                        .filter_map(|s| s.values.get(i).copied().flatten())
                    {
                        if v >= 0.0 {
                            up += v;
                        } else {
                            down += v;
                        }
                    }
                    lo = lo.min(down);
                    hi = hi.max(up);
                }
            }
            Grouping::Standard => {
                for v in plot.series.iter().flat_map(|s| s.values.iter().flatten()) {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
        }
    }
    if lo > hi {
        (0.0, 1.0, percent)
    } else {
        (lo, hi, percent)
    }
}

fn axis_scale(chart: &Chart, secondary: bool, intervals: f64) -> Scale {
    let axis = if secondary {
        chart.secondary_axis.as_ref()
    } else {
        chart.value_axis.as_ref()
    };
    let (lo, hi, percent) = extent(chart.plots.iter().filter(|p| p.secondary == secondary));
    if percent {
        return Scale {
            min: axis
                .and_then(|a| a.min)
                .unwrap_or(if lo < 0.0 { -1.0 } else { 0.0 }),
            max: axis.and_then(|a| a.max).unwrap_or(1.0),
            step: axis.and_then(|a| a.major).unwrap_or(0.1),
        };
    }
    scale(lo, hi, axis, intervals, !chart.three_d)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    env: &Env<'_>,
    chart: &Chart,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    fields: Fields,
    out: &mut Vec<Item>,
) {
    if chart.fill.is_some() || chart.stroke.is_some() {
        out.push(Item::Shape(Box::new(Outline {
            x,
            y,
            width,
            height,
            geometry: Geometry::Rect,
            fill: chart.fill.map(Fill::Solid),
            stroke: chart.stroke,
            transform: Transform::default(),
        })));
    }
    let (pad_x, pad_y) = (0.02 * width, 0.02 * height);
    let frame = Rect {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    };
    let mut area = Rect {
        left: x + pad_x,
        top: y + pad_y,
        right: x + width - pad_x,
        bottom: y + height - pad_y,
    };
    let mut last = Vec::new();
    if !chart.title.is_empty() {
        let wrap = TITLE_WIDTH * area.width();
        let title = set(env, fields, &chart.title, wrap);
        let top = area.top + TITLE_OFFSET;
        area.top = top + title.height + TITLE_GAP;
        title.place(x + (width - wrap) / 2.0, top, &mut last);
    }
    if let Some(legend) = &chart.legend {
        legend_items(env, fields, chart, legend, frame, &mut area, &mut last);
    }
    if chart
        .plots
        .iter()
        .all(|p| matches!(p.kind, PlotKind::Pie { .. }))
    {
        pie(chart, area, out);
    } else {
        axes(env, fields, chart, area, out);
    }
    out.append(&mut last);
}

/// What the legend names: the categories of a pie, the series otherwise,
/// each with its colour and whether it is a line.
fn entries(chart: &Chart, side: Side) -> Vec<(&ChartText, Paint, bool)> {
    let first = &chart.plots[0];
    if let (PlotKind::Pie { .. }, Some(series)) = (first.kind, first.series.first()) {
        return chart
            .categories
            .iter()
            .zip(&series.points)
            .map(|(text, paint)| (text, *paint, false))
            .collect();
    }
    let mut all: Vec<_> = chart
        .plots
        .iter()
        .flat_map(|p| {
            let line = matches!(p.kind, PlotKind::Line { .. });
            p.series.iter().map(move |s| (&s.name, s.paint, line))
        })
        .collect();
    let bottom_up = match first.kind {
        PlotKind::Bar {
            grouping,
            horizontal,
            ..
        } => horizontal || grouping != Grouping::Standard,
        PlotKind::Line { grouping } | PlotKind::Area { grouping } => grouping != Grouping::Standard,
        PlotKind::Pie { .. } => false,
    };
    if bottom_up && matches!(side, Side::Left | Side::Right) {
        all.reverse();
    }
    all
}

fn legend_items(
    env: &Env<'_>,
    fields: Fields,
    chart: &Chart,
    legend: &Legend,
    frame: Rect,
    area: &mut Rect,
    out: &mut Vec<Item>,
) {
    let size = legend.props.size;
    let (mark, gap, spacing) = (0.6 * size, 0.25 * size, 0.73 * size);
    let labels: Vec<(Label, Paint, bool)> = entries(chart, legend.side)
        .into_iter()
        .map(|(text, paint, is_line)| {
            let label = line(env, fields, text, &legend.props, Alignment::Left, 1e4);
            (label, paint, is_line)
        })
        .collect();
    if labels.is_empty() {
        return;
    }
    let height = labels.iter().map(|l| l.0.height).fold(0.0, f64::max);
    let widest = labels.iter().map(|l| l.0.width).fold(0.0, f64::max);
    let place =
        |label: Label, paint: Paint, is_line: bool, x: f64, top: f64, out: &mut Vec<Item>| {
            let middle = top + label.baseline - 0.35 * size;
            if is_line {
                out.push(Item::Line {
                    from: [x, middle],
                    to: [x + mark, middle],
                    width: 1.5,
                    color: paint.color,
                });
            } else {
                out.push(Item::Rect {
                    x,
                    y: middle - mark / 2.0,
                    width: mark,
                    height: mark,
                    color: paint.color,
                });
            }
            label.place(x + mark + gap, top, out);
        };
    match legend.side {
        Side::Right | Side::Left => {
            let pitch = height + 0.034 * size;
            let mut top = (frame.top + frame.bottom) / 2.0 - pitch * labels.len() as f64 / 2.0;
            let x = if legend.side == Side::Right {
                let x = area.right - 1.0 - widest - gap - mark;
                area.right = x - LEGEND_SIDE_GAP;
                x
            } else {
                let x = area.left + 1.0;
                area.left = x + mark + gap + widest + LEGEND_SIDE_GAP;
                x
            };
            for (label, paint, is_line) in labels {
                place(label, paint, is_line, x, top, out);
                top += pitch;
            }
        }
        Side::Bottom | Side::Top => {
            let total = labels.iter().map(|l| mark + gap + l.0.width).sum::<f64>()
                + spacing * (labels.len() - 1) as f64;
            let rows = if total > area.width() {
                labels.len()
            } else {
                1
            };
            let pitch = if rows > 1 { 1.55 * size } else { height };
            let block = pitch * (rows - 1) as f64 + height;
            let top = if legend.side == Side::Bottom {
                let top = frame.bottom - 3.0 - block;
                area.bottom = top - LEGEND_BELOW_GAP;
                top
            } else {
                let top = area.top;
                area.top = top + block + TITLE_GAP;
                top
            };
            if rows > 1 {
                let x = (frame.left + frame.right - mark - gap - widest) / 2.0;
                for (i, (label, paint, is_line)) in labels.into_iter().enumerate() {
                    place(label, paint, is_line, x, top + i as f64 * pitch, out);
                }
                return;
            }
            let mut x = (frame.left + frame.right - total) / 2.0;
            for (label, paint, is_line) in labels {
                let advance = mark + gap + label.width + spacing;
                place(label, paint, is_line, x, top, out);
                x += advance;
            }
        }
    }
}

struct ValueLabels {
    labels: Vec<(f64, Label)>,
    widest: f64,
    height: f64,
}

fn value_labels(
    env: &Env<'_>,
    fields: Fields,
    axis: Option<&Axis>,
    scale: Scale,
) -> Option<ValueLabels> {
    let axis = axis?;
    let props = axis.labels.as_ref()?;
    let labels: Vec<(f64, Label)> = scale
        .ticks()
        .into_iter()
        .map(|v| {
            let text = written(axis.format.format(v));
            (v, line(env, fields, &text, props, Alignment::Left, 1e4))
        })
        .collect();
    Some(ValueLabels {
        widest: labels.iter().map(|l| l.1.width).fold(0.0, f64::max),
        height: labels.iter().map(|l| l.1.height).fold(0.0, f64::max),
        labels,
    })
}

fn tick(axis: Option<&Axis>) -> f64 {
    if axis.is_some_and(|a| a.ticks_out) {
        TICK
    } else {
        0.0
    }
}

fn axes(env: &Env<'_>, fields: Fields, chart: &Chart, area: Rect, out: &mut Vec<Item>) {
    let count = chart
        .plots
        .iter()
        .flat_map(|p| &p.series)
        .map(|s| s.values.len())
        .max()
        .unwrap_or(0)
        .max(chart.categories.len())
        .max(1);
    let horizontal = chart.plots.iter().any(|p| {
        matches!(
            p.kind,
            PlotKind::Bar {
                horizontal: true,
                ..
            }
        )
    });
    let category = chart.category_axis.as_ref();
    let value_axis = chart.value_axis.as_ref();
    let category_props = category.and_then(|a| a.labels.as_ref());
    let value_props = value_axis.and_then(|a| a.labels.as_ref());
    let value_size = value_props.map_or(0.0, |p| p.size);
    let intervals = match value_props {
        Some(props) => {
            let sample = |text: &str| {
                line(
                    env,
                    fields,
                    &written(text.into()),
                    props,
                    Alignment::Left,
                    1e4,
                )
            };
            let (room, needed) = if horizontal {
                let widest = value_labels(
                    env,
                    fields,
                    value_axis,
                    axis_scale(chart, false, MAX_INTERVALS),
                )
                .map_or(1.0, |v| v.widest);
                (area.width(), widest)
            } else {
                let below = category_props.map_or(0.0, |p| {
                    line(env, fields, &written("0".into()), p, Alignment::Left, 1e4).height
                        + CATEGORY_GAP
                        + tick(category)
                });
                (area.height() - 0.5 * value_size - below, sample("0").height)
            };
            (room / needed.max(1.0)).floor().clamp(1.0, MAX_INTERVALS)
        }
        None => MAX_INTERVALS,
    };
    let primary = axis_scale(chart, false, intervals);
    let secondary = chart
        .plots
        .iter()
        .any(|p| p.secondary)
        .then(|| axis_scale(chart, true, intervals));
    let values = value_labels(env, fields, value_axis, primary);
    let second =
        secondary.and_then(|s| value_labels(env, fields, chart.secondary_axis.as_ref(), s));
    let on_ticks = category.is_some_and(|a| a.on_ticks) && count > 1;
    let slots = if on_ticks {
        (count - 1) as f64
    } else {
        count as f64
    };

    let mut plot = area;
    let mut category_labels = Vec::new();
    if horizontal {
        if let Some(props) = category_props {
            category_labels = chart
                .categories
                .iter()
                .map(|c| line(env, fields, c, props, Alignment::Left, 1e4))
                .collect();
            let widest = category_labels.iter().map(|l| l.width).fold(0.0, f64::max);
            plot.left += widest + LABEL_GAP + tick(category);
            plot.top += 0.5 * props.size;
        }
        if let Some(v) = &values {
            plot.bottom -= v.height + CATEGORY_GAP + tick(value_axis);
            plot.right -= v.labels.last().map_or(0.0, |l| l.1.width / 2.0);
        }
    } else {
        if let Some(v) = &values {
            plot.left += v.widest + LABEL_GAP + tick(value_axis);
            plot.top += 0.5 * value_size;
        }
        if let Some(v) = &second {
            plot.right -= v.widest + LABEL_GAP + tick(chart.secondary_axis.as_ref());
        }
        if let Some(props) = category_props {
            let slot = (plot.width() / slots).max(1.0);
            category_labels = chart
                .categories
                .iter()
                .map(|c| line(env, fields, c, props, Alignment::Center, slot))
                .collect();
            let tallest = category_labels.iter().map(|l| l.height).fold(0.0, f64::max);
            plot.bottom -= tallest + CATEGORY_GAP + tick(category);
        }
    }
    if plot.width() <= 1.0 || plot.height() <= 1.0 {
        return;
    }

    let slot = if horizontal {
        plot.height()
    } else {
        plot.width()
    } / slots;
    let centre = |i: usize| {
        let offset = if on_ticks { i as f64 } else { i as f64 + 0.5 } * slot;
        if horizontal {
            plot.bottom - offset
        } else {
            plot.left + offset
        }
    };
    let position = |scale: Scale, v: f64| {
        if horizontal {
            plot.left + scale.share(v) * plot.width()
        } else {
            plot.bottom - scale.share(v) * plot.height()
        }
    };
    let across = |p: f64| {
        if horizontal {
            ([p, plot.top], [p, plot.bottom])
        } else {
            ([plot.left, p], [plot.right, p])
        }
    };

    if value_axis.is_some_and(|a| a.gridlines) {
        for v in primary.ticks() {
            let (from, to) = across(position(primary, v));
            out.push(Item::Line {
                from,
                to,
                width: 0.75,
                color: GRID,
            });
        }
    }

    for p in &chart.plots {
        let scale = if p.secondary {
            secondary.unwrap_or(primary)
        } else {
            primary
        };
        let totals: Vec<f64> = (0..count)
            .map(|i| {
                p.series
                    .iter()
                    .filter_map(|s| s.values.get(i).copied().flatten())
                    .map(f64::abs)
                    .sum()
            })
            .collect();
        match p.kind {
            PlotKind::Bar {
                grouping,
                gap,
                overlap,
                ..
            } => {
                let stacked = grouping != Grouping::Standard;
                let n = if stacked {
                    1.0
                } else {
                    p.series.len().max(1) as f64
                };
                let overlap = if stacked { 1.0 } else { overlap };
                let bar = slot / (n - (n - 1.0) * overlap + gap);
                for (i, total) in totals.iter().enumerate() {
                    let (mut up, mut down) = (0.0, 0.0);
                    let start = gap * bar / 2.0 - slot / 2.0;
                    for (k, s) in p.series.iter().enumerate() {
                        let Some(mut v) = s.values.get(i).copied().flatten() else {
                            continue;
                        };
                        if grouping == Grouping::Percent && *total > 0.0 {
                            v /= total;
                        }
                        let (from, to) = if !stacked {
                            (0.0, v)
                        } else if v >= 0.0 {
                            up += v;
                            (up - v, up)
                        } else {
                            down += v;
                            (down - v, down)
                        };
                        let (a, b) = (position(scale, from), position(scale, to));
                        let offset = start
                            + if stacked {
                                0.0
                            } else {
                                k as f64 * bar * (1.0 - overlap)
                            };
                        let (rx, ry, rw, rh) = if horizontal {
                            (a.min(b), centre(i) - offset - bar, (b - a).abs(), bar)
                        } else {
                            (centre(i) + offset, a.min(b), bar, (b - a).abs())
                        };
                        out.push(Item::Rect {
                            x: rx,
                            y: ry,
                            width: rw,
                            height: rh,
                            color: s.paint.color,
                        });
                    }
                }
            }
            PlotKind::Line { grouping } | PlotKind::Area { grouping } => {
                let filled = matches!(p.kind, PlotKind::Area { .. });
                let mut base = vec![0.0; count];
                for s in &p.series {
                    let mut points = Vec::new();
                    let mut floor = Vec::new();
                    for i in 0..count {
                        let Some(mut v) = s.values.get(i).copied().flatten() else {
                            continue;
                        };
                        if grouping == Grouping::Percent && totals[i] > 0.0 {
                            v /= totals[i];
                        }
                        let below = if grouping == Grouping::Standard {
                            0.0
                        } else {
                            base[i]
                        };
                        base[i] = below + v;
                        points.push([centre(i), position(scale, below + v)]);
                        floor.push([centre(i), position(scale, below)]);
                    }
                    if filled {
                        if points.len() > 1 {
                            floor.reverse();
                            points.extend(floor);
                            out.push(Item::Polygon {
                                points,
                                fill: Some(s.paint),
                                stroke: None,
                            });
                        }
                    } else {
                        for pair in points.windows(2) {
                            out.push(Item::Line {
                                from: pair[0],
                                to: pair[1],
                                width: s.line_width,
                                color: s.paint.color,
                            });
                        }
                    }
                }
            }
            PlotKind::Pie { .. } => {}
        }
    }

    if let Some(stroke) = category.and_then(|a| a.line) {
        let (from, to) = across(position(primary, 0.0f64.clamp(primary.min, primary.max)));
        out.push(Item::Line {
            from,
            to,
            width: stroke.width,
            color: stroke.paint.color,
        });
    }
    if let Some(stroke) = value_axis.and_then(|a| a.line) {
        let (from, to) = if horizontal {
            ([plot.left, plot.bottom], [plot.right, plot.bottom])
        } else {
            ([plot.left, plot.top], [plot.left, plot.bottom])
        };
        out.push(Item::Line {
            from,
            to,
            width: stroke.width,
            color: stroke.paint.color,
        });
    }

    if horizontal {
        for (i, label) in category_labels.into_iter().enumerate() {
            let x = plot.left - tick(category) - LABEL_GAP - label.width;
            let top = centre(i) - label.height / 2.0;
            label.place(x, top, out);
        }
        if let Some(v) = values {
            let top = plot.bottom + tick(value_axis) + CATEGORY_GAP;
            for (value, label) in v.labels {
                let x = position(primary, value) - label.width / 2.0;
                label.place(x, top, out);
            }
        }
        return;
    }
    let top = plot.bottom + tick(category) + CATEGORY_GAP;
    for (i, label) in category_labels.into_iter().enumerate() {
        label.place(centre(i) - slot / 2.0, top, out);
    }
    if let Some(v) = values {
        let right = plot.left - tick(value_axis) - LABEL_GAP;
        for (value, label) in v.labels {
            let (top, x) = (
                position(primary, value) - label.height / 2.0,
                right - label.width,
            );
            label.place(x, top, out);
        }
    }
    if let (Some(v), Some(scale)) = (second, secondary) {
        let left = plot.right + tick(chart.secondary_axis.as_ref()) + LABEL_GAP;
        for (value, label) in v.labels {
            let top = position(scale, value) - label.height / 2.0;
            label.place(left, top, out);
        }
    }
}

fn pie(chart: &Chart, area: Rect, out: &mut Vec<Item>) {
    let Some(plot) = chart.plots.first() else {
        return;
    };
    let (PlotKind::Pie { hole, first_angle }, Some(series)) = (plot.kind, plot.series.first())
    else {
        return;
    };
    let total: f64 = series.values.iter().flatten().map(|v| v.abs()).sum();
    if total <= 0.0 {
        return;
    }
    let radius = 0.45 * area.width().min(area.height());
    let (cx, cy) = (
        (area.left + area.right) / 2.0,
        (area.top + area.bottom) / 2.0,
    );
    let point = |angle: f64, r: f64| {
        let a = angle.to_radians();
        [cx + r * a.sin(), cy - r * a.cos()]
    };
    let mut angle = first_angle;
    for (i, v) in series.values.iter().enumerate() {
        let Some(v) = v else {
            continue;
        };
        let sweep = v.abs() / total * 360.0;
        let steps = (sweep / 3.0).ceil().max(1.0) as usize;
        let at = |k: usize| angle + sweep * k as f64 / steps as f64;
        let mut points: Vec<[f64; 2]> = (0..=steps).map(|k| point(at(k), radius)).collect();
        if hole > 0.0 {
            points.extend((0..=steps).rev().map(|k| point(at(k), radius * hole)));
        } else {
            points.push([cx, cy]);
        }
        out.push(Item::Polygon {
            points,
            fill: Some(series.points.get(i).copied().unwrap_or(series.paint)),
            stroke: Some(Stroke {
                width: 0.75,
                paint: Paint {
                    color: Color::WHITE,
                    alpha: 1.0,
                },
            }),
        });
        angle += sweep;
    }
}

#[cfg(test)]
mod tests {
    use super::{scale, Scale};

    #[test]
    fn the_value_axis_takes_the_round_step_libreoffice_takes() {
        let s = |lo, hi| scale(lo, hi, None, 10.0, true);
        assert_eq!(
            s(1.8, 4.4),
            Scale {
                min: 0.0,
                max: 5.0,
                step: 0.5
            }
        );
        assert_eq!(
            s(1.8, 5.0),
            Scale {
                min: 0.0,
                max: 6.0,
                step: 1.0
            }
        );
        assert_eq!(
            s(0.0, 9.65),
            Scale {
                min: 0.0,
                max: 12.0,
                step: 2.0
            }
        );
        assert_eq!(
            s(0.0, 12.3),
            Scale {
                min: 0.0,
                max: 14.0,
                step: 2.0
            }
        );
        assert_eq!(
            s(70.0, 150.0),
            Scale {
                min: 0.0,
                max: 160.0,
                step: 20.0
            }
        );
        assert_eq!(
            s(0.0, 30321.0),
            Scale {
                min: 0.0,
                max: 35000.0,
                step: 5000.0
            }
        );
        assert_eq!(
            scale(0.0, 5.0, None, 10.0, false),
            Scale {
                min: 0.0,
                max: 5.0,
                step: 0.5
            }
        );
        assert_eq!(
            scale(0.0, 5.0, None, 3.0, true),
            Scale {
                min: 0.0,
                max: 6.0,
                step: 2.0
            }
        );
    }
}
