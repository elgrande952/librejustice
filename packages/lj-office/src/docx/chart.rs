//! A chart part read into the model: the labels and values it caches, its
//! title, axes and legend.

use std::sync::Arc;

use super::body::Story;
use super::theme::ThemeFont;
use super::xml::{parse, Element, Node, Ns};
use crate::model::*;
use crate::Anchor;

const EMU_PER_POINT: f64 = 12700.0;

/// The accents of the default Office theme, for a document without one.
const ACCENTS: [Color; 6] = [
    Color(0x4F, 0x81, 0xBD),
    Color(0xC0, 0x50, 0x4D),
    Color(0x9B, 0xBB, 0x59),
    Color(0x80, 0x64, 0xA2),
    Color(0x4B, 0xAC, 0xC6),
    Color(0xF7, 0x96, 0x46),
];

fn val(e: &Element) -> Option<&str> {
    e.attr(Ns::None, "val")
}

fn value_of<'e>(e: &'e Element, name: &str) -> Option<&'e str> {
    e.child(Ns::C, name).and_then(val)
}

fn number_of(e: &Element, name: &str) -> Option<f64> {
    value_of(e, name)?.trim().parse().ok()
}

/// A boolean element, true when it names no value.
fn flag(e: &Element, name: &str) -> Option<bool> {
    e.child(Ns::C, name)
        .map(|c| !matches!(val(c), Some("0" | "false")))
}

fn text_of(e: &Element) -> String {
    e.children
        .iter()
        .filter_map(|n| match n {
            Node::Text(_, t) => Some(&**t),
            Node::Element(_) => None,
        })
        .collect()
}

fn points(cache: &Element) -> impl Iterator<Item = (usize, &Element)> {
    cache
        .elements()
        .filter(|e| e.is(Ns::C, "pt"))
        .filter_map(|pt| {
            Some((
                pt.attr(Ns::None, "idx")?.parse().ok()?,
                pt.child(Ns::C, "v")?,
            ))
        })
}

fn point_count(cache: &Element) -> usize {
    number_of(cache, "ptCount").map_or(0, |n| n as usize)
}

/// The cache of a reference, or the literal, holding a series' data.
fn cache(data: &Element) -> Option<&Element> {
    data.elements().find_map(|e| match &*e.name {
        "strRef" | "numRef" => e
            .elements()
            .find(|c| matches!(&*c.name, "strCache" | "numCache")),
        "multiLvlStrRef" => e.child(Ns::C, "multiLvlStrCache")?.child(Ns::C, "lvl"),
        "strLit" | "numLit" => Some(e),
        _ => None,
    })
}

fn written(text: &str) -> ChartText {
    ChartText {
        text: text.to_string(),
        anchors: Vec::new(),
    }
}

struct Reader<'s, 'c, 'p> {
    story: &'s Story<'c, 'p>,
    part: Option<u16>,
    date1904: bool,
}

impl Story<'_, '_> {
    /// The paragraphs of a DrawingML text body read from `part`, over the
    /// theme's minor font at `size` in `color`.
    pub(super) fn dml_text(
        &self,
        body: &Element,
        part: Option<u16>,
        size: f64,
        color: Option<Color>,
    ) -> Vec<Block> {
        let reader = Reader {
            story: self,
            part,
            date1904: false,
        };
        let base = RunProps {
            size,
            size_complex: size,
            color,
            ..reader.default_props()
        };
        reader.paragraphs(body, &base, Alignment::Left)
    }

    /// The chart a `c:chart` reference names, `None` when its part is missing
    /// or holds no plot this reader draws.
    pub(super) fn chart(&self, reference: &Element) -> Option<Chart> {
        let name = self.rels?.by_id(reference.attr(Ns::R, "id")?)?;
        let package = self.ctx.package;
        let (part, data) = match package.part(name) {
            Some((part, data)) => (Some(part), data),
            None => (None, package.entry(name)?),
        };
        let root = parse(data).ok()?;
        let reader = Reader {
            story: self,
            part,
            date1904: flag(&root, "date1904").unwrap_or(false),
        };
        reader.read(&root)
    }
}

impl Reader<'_, '_, '_> {
    fn read(&self, root: &Element) -> Option<Chart> {
        let chart = root.child(Ns::C, "chart")?;
        let area = chart.child(Ns::C, "plotArea")?;
        let base = self.text_props(self.default_props(), Some(root));
        let axes: Vec<&Element> = area
            .elements()
            .filter(|e| matches!(&*e.name, "catAx" | "valAx" | "dateAx"))
            .collect();
        let values: Vec<&Element> = axes
            .iter()
            .copied()
            .filter(|a| a.is(Ns::C, "valAx"))
            .collect();
        let primary_id = values.first().and_then(|a| value_of(a, "axId"));

        let mut plots = Vec::new();
        let mut categories = Vec::new();
        let mut index = 0;
        for group in area.elements().filter(|e| e.ns == Ns::C) {
            let grouping = match value_of(group, "grouping") {
                Some("stacked") => Grouping::Stacked,
                Some("percentStacked") => Grouping::Percent,
                _ => Grouping::Standard,
            };
            let kind = match &*group.name {
                "barChart" | "bar3DChart" => {
                    let overlap = if grouping == Grouping::Standard {
                        0.0
                    } else {
                        100.0
                    };
                    PlotKind::Bar {
                        horizontal: value_of(group, "barDir") == Some("bar"),
                        grouping,
                        gap: number_of(group, "gapWidth").unwrap_or(150.0) / 100.0,
                        overlap: number_of(group, "overlap").unwrap_or(overlap) / 100.0,
                    }
                }
                "lineChart" | "line3DChart" | "stockChart" => PlotKind::Line { grouping },
                "areaChart" | "area3DChart" => PlotKind::Area { grouping },
                "pieChart" | "pie3DChart" | "ofPieChart" => PlotKind::Pie {
                    hole: 0.0,
                    first_angle: number_of(group, "firstSliceAng").unwrap_or(0.0),
                },
                "doughnutChart" => PlotKind::Pie {
                    hole: number_of(group, "holeSize").unwrap_or(10.0) / 100.0,
                    first_angle: number_of(group, "firstSliceAng").unwrap_or(0.0),
                },
                _ => continue,
            };
            let vary =
                matches!(kind, PlotKind::Pie { .. }) && flag(group, "varyColors").unwrap_or(true);
            let mut series = Vec::new();
            for ser in group.elements().filter(|e| e.is(Ns::C, "ser")) {
                if categories.is_empty() {
                    categories = self.categories(ser);
                }
                series.push(self.series(ser, kind, vary, index));
                index += 1;
            }
            let secondary = group
                .elements()
                .filter(|e| e.is(Ns::C, "axId"))
                .filter_map(val)
                .any(|id| {
                    Some(id) != primary_id && values.iter().any(|a| value_of(a, "axId") == Some(id))
                });
            plots.push(Plot {
                kind,
                series,
                secondary,
            });
        }
        if plots.is_empty() {
            return None;
        }
        if categories.is_empty() {
            let count = plots
                .iter()
                .flat_map(|p| &p.series)
                .map(|s| s.values.len())
                .max()
                .unwrap_or(0);
            categories = (1..=count).map(|i| written(&i.to_string())).collect();
        }

        let on_ticks = values.first().and_then(|a| value_of(a, "crossBetween")) == Some("midCat");
        let category_axis = axes
            .iter()
            .find(|a| matches!(&*a.name, "catAx" | "dateAx"))
            .and_then(|a| self.axis(a, &base, on_ticks));
        let legend = chart.child(Ns::C, "legend").map(|l| Legend {
            side: match value_of(l, "legendPos") {
                Some("b") => Side::Bottom,
                Some("t") => Side::Top,
                Some("l") => Side::Left,
                _ => Side::Right,
            },
            props: self.text_props(base.clone(), Some(l)),
        });
        let sp = root.child(Ns::C, "spPr");
        let fill = sp
            .and_then(|s| self.story.fill(s, None))
            .unwrap_or(Some(Paint {
                color: Color::WHITE,
                alpha: 1.0,
            }));
        let stroke = match sp.and_then(|s| s.child(Ns::A, "ln")) {
            Some(ln) => self.story.stroke(ln, None),
            None => Some(Stroke {
                width: 0.75,
                paint: Paint {
                    color: Color(0xB3, 0xB3, 0xB3),
                    alpha: 1.0,
                },
            }),
        };
        Some(Chart {
            title: self.title(chart, &base, &plots),
            categories,
            category_axis,
            value_axis: values.first().and_then(|a| self.axis(a, &base, false)),
            secondary_axis: values.get(1).and_then(|a| self.axis(a, &base, false)),
            three_d: area
                .elements()
                .any(|e| e.ns == Ns::C && e.name.ends_with("3DChart")),
            plots,
            legend,
            fill,
            stroke,
        })
    }

    fn default_props(&self) -> RunProps {
        let theme = &self.story.ctx.theme;
        let family: Arc<str> = Arc::from(theme.font(ThemeFont::MinorAscii).unwrap_or("Calibri"));
        let other = |slot| theme.font(slot).map_or_else(|| family.clone(), Arc::from);
        RunProps {
            fonts: FontSet {
                east_asia: other(ThemeFont::MinorEastAsia),
                complex: other(ThemeFont::MinorBidi),
                ascii: family.clone(),
                h_ansi: family.clone(),
            },
            ..RunProps::default()
        }
    }

    /// `props` under the text properties of an element's `c:txPr`.
    fn text_props(&self, props: RunProps, holder: Option<&Element>) -> RunProps {
        let defaults = holder
            .and_then(|h| h.child(Ns::C, "txPr"))
            .and_then(|t| t.child(Ns::A, "p"))
            .and_then(|p| p.child(Ns::A, "pPr"))
            .and_then(|p| p.child(Ns::A, "defRPr"));
        match defaults {
            Some(d) => self.run_props(props, d),
            None => props,
        }
    }

    /// `props` under a DrawingML `a:defRPr` or `a:rPr`.
    fn run_props(&self, mut props: RunProps, r: &Element) -> RunProps {
        if let Some(size) = r.attr(Ns::None, "sz").and_then(|v| v.parse::<f64>().ok()) {
            props.size = size / 100.0;
            props.size_complex = size / 100.0;
        }
        if let Some(b) = r.attr(Ns::None, "b") {
            props.bold = matches!(b, "1" | "true");
        }
        if let Some(i) = r.attr(Ns::None, "i") {
            props.italic = matches!(i, "1" | "true");
        }
        let theme = &self.story.ctx.theme;
        let typeface = |name: &str| -> Option<Arc<str>> {
            let face = r.child(Ns::A, name)?.attr(Ns::None, "typeface")?;
            let resolved = match face {
                "+mn-lt" => theme.font(ThemeFont::MinorAscii)?,
                "+mj-lt" => theme.font(ThemeFont::MajorAscii)?,
                "+mn-ea" => theme.font(ThemeFont::MinorEastAsia)?,
                "+mj-ea" => theme.font(ThemeFont::MajorEastAsia)?,
                "+mn-cs" => theme.font(ThemeFont::MinorBidi)?,
                "+mj-cs" => theme.font(ThemeFont::MajorBidi)?,
                "" => return None,
                face => face,
            };
            Some(Arc::from(resolved))
        };
        if let Some(latin) = typeface("latin") {
            props.fonts.ascii = latin.clone();
            props.fonts.h_ansi = latin;
        }
        if let Some(east) = typeface("ea") {
            props.fonts.east_asia = east;
        }
        if let Some(complex) = typeface("cs") {
            props.fonts.complex = complex;
        }
        if let Some(Some(paint)) = self.story.fill(r, None) {
            props.color = Some(paint.color);
        }
        props
    }

    fn anchored(&self, node: &Element) -> ChartText {
        let mut text = String::new();
        let mut anchors = Vec::new();
        for child in &node.children {
            if let Node::Text(number, t) = child {
                text.push_str(t);
                if let Some(part) = self.part {
                    anchors.extend((0..t.chars().count() as u32).map(|offset| Anchor::XmlText {
                        part,
                        node: *number,
                        offset,
                    }));
                }
            }
        }
        if anchors.len() != text.chars().count() {
            anchors.clear();
        }
        ChartText { text, anchors }
    }

    fn categories(&self, ser: &Element) -> Vec<ChartText> {
        let Some(cache) = ser.child(Ns::C, "cat").and_then(cache) else {
            return Vec::new();
        };
        let format = NumberFormat {
            code: cache
                .child(Ns::C, "formatCode")
                .map(text_of)
                .unwrap_or_default(),
            date1904: self.date1904,
        };
        let numeric = matches!(&*cache.name, "numCache" | "numLit");
        let mut out: Vec<ChartText> = (0..point_count(cache)).map(|_| written("")).collect();
        for (idx, v) in points(cache) {
            if idx >= out.len() {
                out.resize_with(idx + 1, || written(""));
            }
            out[idx] = match text_of(v).trim().parse::<f64>() {
                Ok(number) if numeric => written(&format.format(number)),
                _ => self.anchored(v),
            };
        }
        out
    }

    fn series(&self, ser: &Element, kind: PlotKind, vary: bool, index: usize) -> Series {
        let idx = number_of(ser, "idx").map_or(index, |i| i as usize);
        let tx = ser.child(Ns::C, "tx");
        let name = tx
            .and_then(cache)
            .and_then(|c| points(c).next())
            .map(|(_, v)| self.anchored(v))
            .or_else(|| {
                tx.and_then(|t| t.child(Ns::C, "v"))
                    .map(|v| self.anchored(v))
            })
            .unwrap_or_else(|| written(&format!("Series{}", idx + 1)));
        let mut values = Vec::new();
        if let Some(cache) = ser.child(Ns::C, "val").and_then(cache) {
            values = vec![None; point_count(cache)];
            for (i, v) in points(cache) {
                if i >= values.len() {
                    values.resize(i + 1, None);
                }
                values[i] = text_of(v).trim().parse().ok();
            }
        }
        let sp = ser.child(Ns::C, "spPr");
        let ln = sp.and_then(|s| s.child(Ns::A, "ln"));
        let own = match kind {
            PlotKind::Line { .. } => ln.and_then(|l| self.story.fill(l, None)),
            _ => sp.and_then(|s| self.story.fill(s, None)),
        };
        let paint = own.flatten().unwrap_or_else(|| self.accent(idx));
        let line_width = ln
            .and_then(|l| l.attr(Ns::None, "w"))
            .and_then(|w| w.parse::<f64>().ok())
            .map_or(2.25, |w| w / EMU_PER_POINT);
        let points = if vary {
            (0..values.len())
                .map(|i| {
                    ser.elements()
                        .filter(|d| d.is(Ns::C, "dPt"))
                        .find(|d| number_of(d, "idx") == Some(i as f64))
                        .and_then(|d| d.child(Ns::C, "spPr"))
                        .and_then(|s| self.story.fill(s, None))
                        .flatten()
                        .unwrap_or_else(|| self.accent(i))
                })
                .collect()
        } else {
            Vec::new()
        };
        Series {
            name,
            values,
            paint,
            line_width,
            points,
        }
    }

    /// The automatic colour of series or point `index`: the theme's accents,
    /// darker then lighter on each further round.
    fn accent(&self, index: usize) -> Paint {
        let slot = index % 6;
        let base = self
            .story
            .ctx
            .theme
            .color(&format!("accent{}", slot + 1))
            .unwrap_or(ACCENTS[slot]);
        let round = index / 6;
        let mix = |c: u8| {
            let c = f64::from(c);
            let out = match round {
                0 => c,
                r if r % 2 == 1 => c * 0.6,
                _ => c + (255.0 - c) * 0.4,
            };
            out.round().clamp(0.0, 255.0) as u8
        };
        Paint {
            color: Color(mix(base.0), mix(base.1), mix(base.2)),
            alpha: 1.0,
        }
    }

    fn axis(&self, e: &Element, base: &RunProps, on_ticks: bool) -> Option<Axis> {
        if flag(e, "delete").unwrap_or(false) {
            return None;
        }
        let scaling = e.child(Ns::C, "scaling");
        let bound = |name: &str| scaling.and_then(|s| number_of(s, name));
        let labels = (value_of(e, "tickLblPos") != Some("none"))
            .then(|| self.text_props(base.clone(), Some(e)));
        let format = e
            .child(Ns::C, "numFmt")
            .and_then(|f| f.attr(Ns::None, "formatCode"))
            .unwrap_or("General");
        let line = match e.child(Ns::C, "spPr").and_then(|s| s.child(Ns::A, "ln")) {
            Some(ln) => self.story.stroke(ln, None),
            None => Some(Stroke {
                width: 0.75,
                paint: Paint {
                    color: Color(0x86, 0x86, 0x86),
                    alpha: 1.0,
                },
            }),
        };
        Some(Axis {
            labels,
            format: NumberFormat {
                code: format.to_string(),
                date1904: self.date1904,
            },
            gridlines: e.child(Ns::C, "majorGridlines").is_some(),
            ticks_out: matches!(value_of(e, "majorTickMark"), Some("out" | "cross")),
            min: bound("min"),
            max: bound("max"),
            major: number_of(e, "majorUnit").filter(|u| *u > 0.0),
            line,
            on_ticks,
        })
    }

    /// The `a:p` paragraphs of a DrawingML text body over `base`, aligned
    /// as `alignment` unless a paragraph says otherwise.
    fn paragraphs(&self, body: &Element, base: &RunProps, alignment: Alignment) -> Vec<Block> {
        body.elements()
            .filter(|p| p.is(Ns::A, "p"))
            .map(|p| {
                let ppr = p.child(Ns::A, "pPr");
                let mark = match ppr.and_then(|p| p.child(Ns::A, "defRPr")) {
                    Some(d) => self.run_props(base.clone(), d),
                    None => base.clone(),
                };
                let mut inlines = Vec::new();
                for r in p.elements() {
                    match &*r.name {
                        "r" | "fld" => {
                            let run = match r.child(Ns::A, "rPr") {
                                Some(rpr) => self.run_props(mark.clone(), rpr),
                                None => mark.clone(),
                            };
                            if let Some(t) = r.child(Ns::A, "t") {
                                inlines.extend(self.anchored(t).inlines(&run));
                            }
                        }
                        "br" => inlines.push(Inline::Break(Break::Line)),
                        _ => {}
                    }
                }
                let alignment = match ppr.and_then(|p| p.attr(Ns::None, "algn")) {
                    Some("l") => Alignment::Left,
                    Some("ctr") => Alignment::Center,
                    Some("r") => Alignment::Right,
                    Some("just") => Alignment::Justify,
                    _ => alignment,
                };
                let percent = |name: &str| {
                    ppr?.child(Ns::A, name)?
                        .child(Ns::A, "spcPct")?
                        .attr(Ns::None, "val")?
                        .parse::<f64>()
                        .ok()
                        .map(|v| v / 1000.0)
                };
                let mut props = ParagraphProps {
                    alignment,
                    ..ParagraphProps::default()
                };
                if let Some(line) = percent("lnSpc") {
                    props.spacing.line = LineSpacing::Proportional(line);
                }
                Block::Paragraph(Box::new(Paragraph {
                    props,
                    mark,
                    style: None,
                    label: None,
                    hidden_section: false,
                    inlines,
                }))
            })
            .collect()
    }

    /// The title's paragraphs: its rich text, the cell it names, or what
    /// LibreOffice writes for it, the lone series' name or "Chart Title".
    fn title(&self, chart: &Element, base: &RunProps, plots: &[Plot]) -> Vec<Block> {
        let title = chart.child(Ns::C, "title");
        let mut series = plots.iter().flat_map(|p| &p.series);
        let lone = match (series.next(), series.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        };
        if title.is_none() && (flag(chart, "autoTitleDeleted") != Some(false) || lone.is_none()) {
            return Vec::new();
        }
        let mut props = base.clone();
        props.size = 18.0;
        props.size_complex = 18.0;
        props.bold = true;
        let props = self.text_props(props, title);
        let paragraph = |inlines: Vec<Inline>, mark: RunProps| {
            Block::Paragraph(Box::new(Paragraph {
                props: ParagraphProps {
                    alignment: Alignment::Center,
                    ..ParagraphProps::default()
                },
                mark,
                style: None,
                label: None,
                inlines,
                hidden_section: false,
            }))
        };
        let tx = title.and_then(|t| t.child(Ns::C, "tx"));
        if let Some(rich) = tx.and_then(|t| t.child(Ns::C, "rich")) {
            return self.paragraphs(rich, &props, Alignment::Center);
        }
        let text = tx
            .and_then(cache)
            .and_then(|c| points(c).next())
            .map(|(_, v)| self.anchored(v))
            .or_else(|| lone.map(|s| s.name.clone()))
            .unwrap_or_else(|| written("Chart Title"));
        vec![paragraph(text.inlines(&props), props)]
    }
}
