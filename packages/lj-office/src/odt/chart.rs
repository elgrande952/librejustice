//! An embedded chart object, read from the local table of its `content.xml`.

use std::collections::HashMap;
use std::sync::Arc;

use super::container::OdtPackage;
use super::units;
use super::xml::{Child, Element, Kind, Ns, Xml};
use crate::model as m;
use crate::Anchor;

/// The colours Writer gives series and slices that name none.
const PALETTE: [u32; 12] = [
    0x004586, 0xff420e, 0xffd320, 0x579d1c, 0x7e0021, 0x83caff, 0x314004, 0xaecf00, 0x4b1f6f,
    0xff950e, 0xc5000b, 0x0084d1,
];
const MAX_ROWS: usize = 10_000;
const MAX_COLUMNS: usize = 1_024;
/// Cells of the local table, and points of one range, past which a repeat stops.
const MAX_CELLS: usize = 100_000;

/// A chart, its title still to be set in paragraphs.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub title: Vec<(m::ChartText, m::RunProps)>,
    pub plots: Vec<m::Plot>,
    pub categories: Vec<m::ChartText>,
    pub category_axis: Option<m::Axis>,
    pub value_axis: Option<m::Axis>,
    pub secondary_axis: Option<m::Axis>,
    pub legend: Option<m::Legend>,
    pub fill: Option<m::Paint>,
    pub stroke: Option<m::Stroke>,
    pub three_d: bool,
}

impl Chart {
    pub fn to_model(&self) -> m::Chart {
        let title = self
            .title
            .iter()
            .map(|(text, props)| {
                m::Block::Paragraph(Box::new(m::Paragraph {
                    props: m::ParagraphProps {
                        alignment: m::Alignment::Center,
                        ..m::ParagraphProps::default()
                    },
                    mark: props.clone(),
                    style: None,
                    label: None,
                    inlines: text.inlines(props),
                    hidden_section: false,
                }))
            })
            .collect();
        m::Chart {
            title,
            plots: self.plots.clone(),
            categories: self.categories.clone(),
            category_axis: self.category_axis.clone(),
            value_axis: self.value_axis.clone(),
            secondary_axis: self.secondary_axis.clone(),
            legend: self.legend.clone(),
            fill: self.fill,
            stroke: self.stroke,
            three_d: self.three_d,
        }
    }
}

/// The chart the object in `dir` holds, if it holds one this reader draws.
pub fn read(pkg: &OdtPackage, package: usize, dir: &str) -> Option<Chart> {
    let part = pkg.part(package, &format!("{dir}/content.xml"))?;
    let xml = &pkg.parts[part].xml;
    let root = xml.root()?;
    let chart = xml
        .child(root, Ns::Office, "body")
        .and_then(|b| xml.child(b, Ns::Office, "chart"))
        .and_then(|c| xml.child(c, Ns::Chart, "chart"))?;
    let mut styles = HashMap::new();
    let mut numbers = HashMap::new();
    for group in xml
        .children(root)
        .filter(|e| e.is(Ns::Office, "automatic-styles") || e.is(Ns::Office, "styles"))
    {
        for s in xml.children(group) {
            if let Some(name) = s.attr(Ns::Style, "name") {
                if s.is(Ns::Style, "style") {
                    styles.insert(name, s);
                } else if s.ns == Ns::Number {
                    numbers.insert(name, s);
                }
            }
        }
    }
    let faces = xml
        .children(root)
        .filter(|e| e.is(Ns::Office, "font-face-decls"))
        .flat_map(|d| xml.children(d))
        .filter_map(|f| Some((f.attr(Ns::Style, "name")?, f.attr(Ns::Svg, "font-family")?)))
        .collect();
    let part = part as u16;
    let reader = Reader {
        xml,
        part,
        styles,
        numbers,
        faces,
        table: Table::read(xml, part, chart),
    };
    reader.read(chart)
}

#[derive(Clone)]
struct Cell {
    value: Option<f64>,
    text: m::ChartText,
}

/// The local table the ranges of the chart point into, row by row.
struct Table(Vec<Vec<Cell>>);

impl Table {
    fn read(xml: &Xml, part: u16, chart: &Element) -> Self {
        let mut rows = Vec::new();
        if let Some(table) = xml.child(chart, Ns::Table, "table") {
            collect_rows(xml, part, table, &mut rows, &mut 0);
        }
        Table(rows)
    }

    /// The cells a range address covers, row by row.
    fn range(&self, address: &str) -> Vec<Option<&Cell>> {
        let mut out = Vec::new();
        for part in address.split_whitespace() {
            let mut ends = part.splitn(2, ':').map(cell_address);
            let Some(Some(start)) = ends.next() else {
                continue;
            };
            let end = ends.next().flatten().unwrap_or(start);
            for row in start.1.min(end.1)..=start.1.max(end.1).min(MAX_ROWS) {
                for column in start.0.min(end.0)..=start.0.max(end.0).min(MAX_COLUMNS) {
                    if out.len() == MAX_CELLS {
                        return out;
                    }
                    out.push(self.0.get(row).and_then(|r| r.get(column)));
                }
            }
        }
        out
    }
}

fn collect_rows(xml: &Xml, part: u16, e: &Element, rows: &mut Vec<Vec<Cell>>, cells: &mut usize) {
    for child in xml.children(e) {
        match (child.ns, child.local.as_str()) {
            (Ns::Table, "table-row") => {
                let repeat = child
                    .attr(Ns::Table, "number-rows-repeated")
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1);
                let one = row(xml, part, child);
                for _ in 0..repeat {
                    if rows.len() == MAX_ROWS || *cells >= MAX_CELLS {
                        return;
                    }
                    *cells += one.len().max(1);
                    rows.push(one.clone());
                }
            }
            (Ns::Table, "table-header-rows" | "table-rows" | "table-row-group") => {
                collect_rows(xml, part, child, rows, cells)
            }
            _ => {}
        }
    }
}

fn row(xml: &Xml, part: u16, e: &Element) -> Vec<Cell> {
    let mut cells = Vec::new();
    for c in xml
        .children(e)
        .filter(|c| c.is(Ns::Table, "table-cell") || c.is(Ns::Table, "covered-table-cell"))
    {
        let repeat = c
            .attr(Ns::Table, "number-columns-repeated")
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(1);
        let value = c
            .attr(Ns::Office, "value")
            .and_then(|v| v.trim().parse::<f64>().ok());
        for _ in 0..repeat.min(MAX_COLUMNS.saturating_sub(cells.len())) {
            cells.push(Cell {
                value,
                text: anchored(xml, part, c),
            });
        }
    }
    cells
}

/// A cell address as `$B$2`, table name aside: column and row from zero.
fn cell_address(address: &str) -> Option<(usize, usize)> {
    let local = address.rsplit('.').next()?.replace('$', "");
    let split = local.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = local.split_at(split);
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let column = letters.chars().try_fold(0usize, |n, c| {
        n.checked_mul(26)?
            .checked_add((c.to_ascii_uppercase() as u8 - b'A' + 1) as usize)
    })?;
    let row: usize = digits.parse().ok()?;
    Some((column.checked_sub(1)?, row.checked_sub(1)?))
}

fn anchored(xml: &Xml, part: u16, e: &Element) -> m::ChartText {
    let mut text = String::new();
    let mut anchors = Vec::new();
    collect_text(xml, part, e, &mut text, &mut anchors);
    m::ChartText { text, anchors }
}

fn collect_text(xml: &Xml, part: u16, e: &Element, text: &mut String, anchors: &mut Vec<Anchor>) {
    for child in &e.children {
        match child {
            Child::Node(n) => {
                let node = &xml.nodes[*n as usize];
                let t = match node.kind {
                    Kind::Attr | Kind::Comment => continue,
                    Kind::LineBreak | Kind::Tab => " ",
                    _ => node.text.as_str(),
                };
                text.push_str(t);
                anchors.extend((0..t.chars().count() as u32).map(|offset| Anchor::XmlText {
                    part,
                    node: *n,
                    offset,
                }));
            }
            Child::Element(id) => collect_text(xml, part, xml.element(*id), text, anchors),
        }
    }
}

struct Reader<'a> {
    xml: &'a Xml,
    part: u16,
    styles: HashMap<&'a str, &'a Element>,
    numbers: HashMap<&'a str, &'a Element>,
    faces: HashMap<&'a str, &'a str>,
    table: Table,
}

impl<'a> Reader<'a> {
    fn style(&self, e: &Element) -> Option<&'a Element> {
        self.styles.get(e.attr(Ns::Chart, "style-name")?).copied()
    }

    fn props(&self, e: &Element, kind: &str) -> Option<&'a Element> {
        self.xml.child(self.style(e)?, Ns::Style, kind)
    }

    fn chart_attr(&self, e: &Element, name: &str) -> Option<&'a str> {
        self.props(e, "chart-properties")?.attr(Ns::Chart, name)
    }

    fn chart_flag(&self, e: &Element, name: &str) -> Option<bool> {
        self.chart_attr(e, name).map(units::boolean)
    }

    fn chart_number(&self, e: &Element, name: &str) -> Option<f64> {
        self.chart_attr(e, name)?.trim().parse().ok()
    }

    /// `props` under the text properties of an element's style.
    fn text_props(&self, e: &Element, mut props: m::RunProps) -> m::RunProps {
        let Some(t) = self.props(e, "text-properties") else {
            return props;
        };
        if let Some(size) = t.attr(Ns::Fo, "font-size").and_then(units::length) {
            props.size = f64::from(size);
            props.size_complex = f64::from(size);
        }
        if let Some(weight) = t.attr(Ns::Fo, "font-weight") {
            props.bold = weight == "bold" || weight.parse::<u32>().is_ok_and(|w| w >= 600);
        }
        if let Some(style) = t.attr(Ns::Fo, "font-style") {
            props.italic = style == "italic" || style == "oblique";
        }
        if let Some(color) = t.attr(Ns::Fo, "color").and_then(units::color) {
            props.color = Some(rgb(color.0));
        }
        let family = t
            .attr(Ns::Fo, "font-family")
            .or_else(|| self.faces.get(t.attr(Ns::Style, "font-name")?).copied())
            .map(units::family)
            .filter(|f| !f.is_empty());
        if let Some(family) = family {
            props.fonts = fonts(&family);
        }
        props
    }

    /// `None` without a fill of its own, `Some(None)` for no fill.
    fn fill(&self, e: &Element) -> Option<Option<m::Paint>> {
        let g = self.props(e, "graphic-properties")?;
        if g.attr(Ns::Draw, "fill") == Some("none") {
            return Some(None);
        }
        let color = g.attr(Ns::Draw, "fill-color").and_then(units::color)?;
        Some(Some(paint(color.0)))
    }

    fn stroke(&self, e: &Element) -> Option<m::Stroke> {
        let g = self.props(e, "graphic-properties")?;
        if g.attr(Ns::Draw, "stroke").is_none_or(|s| s == "none") {
            return None;
        }
        Some(line(g))
    }

    fn read(&self, chart: &Element) -> Option<Chart> {
        let area = self.xml.child(chart, Ns::Chart, "plot-area")?;
        let base = m::RunProps {
            fonts: fonts("Liberation Sans"),
            ..m::RunProps::default()
        };
        let class = chart.attr(Ns::Chart, "class").unwrap_or("chart:bar");
        let grouping = if self.chart_flag(area, "percentage") == Some(true) {
            m::Grouping::Percent
        } else if self.chart_flag(area, "stacked") == Some(true) {
            m::Grouping::Stacked
        } else {
            m::Grouping::Standard
        };
        let kind_of = |class: &str| -> Option<m::PlotKind> {
            Some(match class.trim_start_matches("chart:") {
                "bar" => m::PlotKind::Bar {
                    horizontal: self.chart_flag(area, "vertical") == Some(true),
                    grouping,
                    gap: self.chart_number(area, "gap-width").unwrap_or(100.0) / 100.0,
                    overlap: self.chart_number(area, "overlap").unwrap_or(
                        if grouping == m::Grouping::Standard {
                            0.0
                        } else {
                            100.0
                        },
                    ) / 100.0,
                },
                "line" => m::PlotKind::Line { grouping },
                "area" => m::PlotKind::Area { grouping },
                "circle" => m::PlotKind::Pie {
                    hole: 0.0,
                    first_angle: self.first_angle(area),
                },
                "ring" => m::PlotKind::Pie {
                    hole: 0.5,
                    first_angle: self.first_angle(area),
                },
                _ => return None,
            })
        };
        kind_of(class)?;

        let axes: Vec<&Element> = self
            .xml
            .children(area)
            .filter(|e| e.is(Ns::Chart, "axis"))
            .collect();
        let axis_named = |dimension: &str, secondary: bool| {
            axes.iter().copied().find(|a| {
                a.attr(Ns::Chart, "dimension") == Some(dimension)
                    && a.attr(Ns::Chart, "name")
                        .is_some_and(|n| n.starts_with("secondary") == secondary)
            })
        };
        let x = axis_named("x", false);

        let mut plots: Vec<m::Plot> = Vec::new();
        let mut longest = 0;
        for (index, ser) in self
            .xml
            .children(area)
            .filter(|e| e.is(Ns::Chart, "series"))
            .enumerate()
        {
            let Some(kind) = kind_of(ser.attr(Ns::Chart, "class").unwrap_or(class)) else {
                continue;
            };
            let secondary = ser
                .attr(Ns::Chart, "attached-axis")
                .is_some_and(|a| a.starts_with("secondary"));
            let series = self.series(ser, kind, index);
            longest = longest.max(series.values.len());
            match plots
                .iter_mut()
                .find(|p| p.kind == kind && p.secondary == secondary)
            {
                Some(plot) => plot.series.push(series),
                None => plots.push(m::Plot {
                    kind,
                    series: vec![series],
                    secondary,
                }),
            }
        }
        if plots.is_empty() {
            return None;
        }

        let mut categories: Vec<m::ChartText> = x
            .and_then(|x| self.xml.child(x, Ns::Chart, "categories"))
            .and_then(|c| c.attr(Ns::Table, "cell-range-address"))
            .map(|address| {
                self.table
                    .range(address)
                    .into_iter()
                    .map(|c| c.map_or_else(|| written(""), label))
                    .collect()
            })
            .unwrap_or_default();
        if categories.is_empty() {
            categories = (1..=longest).map(|i| written(&i.to_string())).collect();
        }

        let legend = self
            .xml
            .child(chart, Ns::Chart, "legend")
            .map(|l| m::Legend {
                side: match l.attr(Ns::Chart, "legend-position") {
                    Some("start") => m::Side::Left,
                    Some("top") => m::Side::Top,
                    Some("bottom") => m::Side::Bottom,
                    _ => m::Side::Right,
                },
                props: self.text_props(l, base.clone()),
            });

        let mut title = Vec::new();
        for (name, size) in [("title", 13.0), ("subtitle", 11.0)] {
            let Some(t) = self.xml.child(chart, Ns::Chart, name) else {
                continue;
            };
            let props = self.text_props(
                t,
                m::RunProps {
                    size,
                    size_complex: size,
                    ..base.clone()
                },
            );
            for p in self.xml.children(t).filter(|p| p.is(Ns::Text, "p")) {
                title.push((anchored(self.xml, self.part, p), props.clone()));
            }
        }

        Some(Chart {
            title,
            categories,
            category_axis: x.and_then(|a| self.axis(a, &base)),
            value_axis: axis_named("y", false).and_then(|a| self.axis(a, &base)),
            secondary_axis: axis_named("y", true).and_then(|a| self.axis(a, &base)),
            plots,
            legend,
            fill: self.fill(chart).unwrap_or(Some(paint(0xffffff))),
            stroke: self.stroke(chart),
            three_d: self.chart_flag(area, "three-dimensional") == Some(true),
        })
    }

    /// Clockwise from twelve o'clock, where ODF counts counterclockwise from three.
    fn first_angle(&self, area: &Element) -> f64 {
        (90.0 - self.chart_number(area, "angle-offset").unwrap_or(90.0)).rem_euclid(360.0)
    }

    fn series(&self, ser: &Element, kind: m::PlotKind, index: usize) -> m::Series {
        let name = ser
            .attr(Ns::Chart, "label-cell-address")
            .and_then(|a| self.table.range(a).into_iter().flatten().next())
            .map(label)
            .unwrap_or_else(|| written(&format!("Series{}", index + 1)));
        let values: Vec<Option<f64>> = ser
            .attr(Ns::Chart, "values-cell-range-address")
            .map(|a| {
                self.table
                    .range(a)
                    .into_iter()
                    .map(|c| c.and_then(|c| c.value.or_else(|| c.text.text.trim().parse().ok())))
                    .collect()
            })
            .unwrap_or_default();
        let auto = paint(PALETTE[index % PALETTE.len()]);
        let graphic = self.props(ser, "graphic-properties");
        let (series_paint, line_width) = match kind {
            m::PlotKind::Line { .. } => (
                graphic
                    .and_then(|g| g.attr(Ns::Svg, "stroke-color"))
                    .and_then(units::color)
                    .map_or(auto, |c| paint(c.0)),
                graphic.map_or(0.75, |g| line(g).width),
            ),
            _ => (self.fill(ser).flatten().unwrap_or(auto), 0.75),
        };
        let points = if matches!(kind, m::PlotKind::Pie { .. }) {
            let mut styled = Vec::new();
            for point in self
                .xml
                .children(ser)
                .filter(|e| e.is(Ns::Chart, "data-point"))
            {
                let repeat = point
                    .attr(Ns::Chart, "repeated")
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1);
                let own = self.fill(point).flatten();
                for _ in 0..repeat.min(values.len().saturating_sub(styled.len())) {
                    styled.push(own);
                }
            }
            (0..values.len())
                .map(|i| {
                    styled
                        .get(i)
                        .copied()
                        .flatten()
                        .unwrap_or_else(|| paint(PALETTE[i % PALETTE.len()]))
                })
                .collect()
        } else {
            Vec::new()
        };
        m::Series {
            name,
            values,
            paint: series_paint,
            line_width,
            points,
        }
    }

    fn axis(&self, e: &Element, base: &m::RunProps) -> Option<m::Axis> {
        if self.chart_flag(e, "visible") == Some(false) {
            return None;
        }
        let code = match self.chart_flag(e, "link-data-style-to-source") {
            Some(false) => self
                .style(e)
                .and_then(|s| s.attr(Ns::Style, "data-style-name"))
                .and_then(|n| self.numbers.get(n))
                .map_or_else(String::new, |n| self.format_code(n)),
            _ => String::new(),
        };
        Some(m::Axis {
            labels: (self.chart_flag(e, "display-label") != Some(false))
                .then(|| self.text_props(e, base.clone())),
            format: m::NumberFormat {
                code,
                date1904: false,
            },
            gridlines: self.xml.children(e).any(|g| {
                g.is(Ns::Chart, "grid") && g.attr(Ns::Chart, "class").is_none_or(|c| c == "major")
            }),
            ticks_out: self.chart_flag(e, "tick-marks-major-outer").unwrap_or(true),
            min: self.chart_number(e, "minimum"),
            max: self.chart_number(e, "maximum"),
            major: self.chart_number(e, "interval-major").filter(|u| *u > 0.0),
            line: Some(line(self.props(e, "graphic-properties").unwrap_or(e))),
            on_ticks: false,
        })
    }

    /// The spreadsheet code of a number style: decimals, grouping, percent.
    fn format_code(&self, style: &Element) -> String {
        let Some(number) = self.xml.child(style, Ns::Number, "number") else {
            return String::new();
        };
        let decimals = number
            .attr(Ns::Number, "decimal-places")
            .and_then(|d| d.parse::<usize>().ok())
            .unwrap_or(0)
            .min(15);
        let mut code = if number.attr(Ns::Number, "grouping") == Some("true") {
            "#,##0".to_string()
        } else {
            "0".to_string()
        };
        if decimals > 0 {
            code.push('.');
            code.push_str(&"0".repeat(decimals));
        }
        if style.is(Ns::Number, "percentage-style") {
            code.push('%');
        }
        code
    }
}

/// A cell's text; a number without text is written in full.
fn label(cell: &Cell) -> m::ChartText {
    match cell.value {
        Some(v) if cell.text.text.is_empty() => written(&m::NumberFormat::default().format(v)),
        _ => cell.text.clone(),
    }
}

fn line(g: &Element) -> m::Stroke {
    m::Stroke {
        width: g
            .attr(Ns::Svg, "stroke-width")
            .and_then(units::length)
            .map_or(0.75, |w| f64::from(w).max(0.75)),
        paint: paint(
            g.attr(Ns::Svg, "stroke-color")
                .and_then(units::color)
                .map_or(0xb3b3b3, |c| c.0),
        ),
    }
}

fn fonts(family: &str) -> m::FontSet {
    let family: Arc<str> = Arc::from(family);
    m::FontSet {
        ascii: family.clone(),
        h_ansi: family.clone(),
        east_asia: family.clone(),
        complex: family,
    }
}

fn written(text: &str) -> m::ChartText {
    m::ChartText {
        text: text.to_string(),
        anchors: Vec::new(),
    }
}

fn rgb(c: u32) -> m::Color {
    m::Color((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn paint(c: u32) -> m::Paint {
    m::Paint {
        color: rgb(c),
        alpha: 1.0,
    }
}
