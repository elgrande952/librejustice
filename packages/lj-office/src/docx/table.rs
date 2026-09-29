//! Tables: grid, rows, cells, and the table-level properties a table style and
//! the table itself set.

use anyhow::Result;

use super::body::Story;
use super::props::{on, shading, twips, BordersPatch};
use super::styles::{Condition, Style, TableLayers};
use super::xml::{Element, Ns};
use crate::model::*;

/// The conditional parts of its table style a table shows, from `w:tblLook`.
#[derive(Clone, Copy)]
struct Look {
    first_row: bool,
    last_row: bool,
    first_col: bool,
    last_col: bool,
    h_band: bool,
    v_band: bool,
}

impl Look {
    /// What LibreOffice shows of a table that names none.
    const DEFAULT: Look = Look {
        first_row: true,
        last_row: false,
        first_col: true,
        last_col: false,
        h_band: true,
        v_band: false,
    };

    fn read(e: &Element) -> Look {
        let mask = e.val().and_then(|v| u16::from_str_radix(v, 16).ok());
        let flag = |name: &str, bit: u16| match e.w(name) {
            Some(v) => matches!(v, "1" | "true" | "on"),
            None => mask.is_some_and(|m| m & bit != 0),
        };
        Look {
            first_row: flag("firstRow", 0x20),
            last_row: flag("lastRow", 0x40),
            first_col: flag("firstColumn", 0x80),
            last_col: flag("lastColumn", 0x100),
            h_band: !flag("noHBand", 0x200),
            v_band: !flag("noVBand", 0x400),
        }
    }

    /// The [`Condition::bit`]s a whole row `row` of `rows` meets.
    fn row_conditions(self, row: usize, rows: usize) -> u16 {
        let mut set = 0;
        if self.first_row && row == 0 {
            set |= Condition::FirstRow.bit();
        }
        if self.last_row && row + 1 == rows {
            set |= Condition::LastRow.bit();
        }
        set
    }

    /// The [`Condition::bit`]s of the cell `cell` of `cells` in the row `row`
    /// of `rows`: the first and last rows and columns stay out of the bands.
    fn conditions(self, row: usize, rows: usize, cell: usize, cells: usize) -> u16 {
        let first_row = self.first_row && row == 0;
        let last_row = self.last_row && row + 1 == rows;
        let first_col = self.first_col && cell == 0;
        let last_col = self.last_col && cell + 1 == cells;
        let mut set = 0;
        let mut add = |on: bool, condition: Condition| {
            if on {
                set |= condition.bit();
            }
        };
        let h = row.wrapping_sub(self.first_row as usize);
        let v = cell.wrapping_sub(self.first_col as usize);
        let h_band = self.h_band && !first_row && !last_row;
        let v_band = self.v_band && !first_col && !last_col;
        add(h_band && h.is_multiple_of(2), Condition::Band1Horz);
        add(h_band && !h.is_multiple_of(2), Condition::Band2Horz);
        add(v_band && v.is_multiple_of(2), Condition::Band1Vert);
        add(v_band && !v.is_multiple_of(2), Condition::Band2Vert);
        add(first_row, Condition::FirstRow);
        add(last_row, Condition::LastRow);
        add(first_col, Condition::FirstCol);
        add(last_col, Condition::LastCol);
        add(first_row && first_col, Condition::NwCell);
        add(first_row && last_col, Condition::NeCell);
        add(last_row && first_col, Condition::SwCell);
        add(last_row && last_col, Condition::SeCell);
        set
    }
}

/// The `name` property elements of a table style for a cell meeting
/// `conditions`, in the order they apply: the whole table's, then each
/// condition's.
fn style_parts<'s>(style: &'s Style, conditions: u16, name: &str) -> Vec<&'s Element> {
    let Some(element) = &style.table else {
        return Vec::new();
    };
    let mut parts: Vec<(Option<Condition>, &Element)> = element
        .elements()
        .filter_map(|e| match &*e.name {
            "tblStylePr" => {
                let condition = Condition::parse(e.w("type")?)?;
                if conditions & condition.bit() == 0 {
                    return None;
                }
                Some((Some(condition), e.wc(name)?))
            }
            n if n == name => Some((None, e)),
            _ => None,
        })
        .collect();
    parts.sort_by_key(|p| p.0);
    parts.into_iter().map(|p| p.1).collect()
}

/// A cell margin side set by `w:tblCellMar` or `w:tcMar`, in points.
fn margin(element: &Element) -> Option<f64> {
    match element.w("type") {
        Some("nil") => Some(0.0),
        Some("pct") => None,
        _ => element.w("w").and_then(twips),
    }
}

fn margins(element: &Element, into: &mut Margins) {
    for side in element.elements() {
        let Some(value) = margin(side) else {
            continue;
        };
        match &*side.name {
            "top" => into.top = value,
            "bottom" => into.bottom = value,
            "left" | "start" => into.left = value,
            "right" | "end" => into.right = value,
            _ => {}
        }
    }
}

fn table_props(tblpr: &Element, props: &mut TableProps) {
    for e in tblpr.elements() {
        match &*e.name {
            "tblW" => {
                props.width = match (e.w("type"), e.w("w")) {
                    (Some("dxa") | None, Some(w)) => twips(w)
                        .filter(|w| *w > 0.0)
                        .map_or(TableWidth::Auto, TableWidth::Points),
                    (Some("pct"), Some(w)) => {
                        let fraction = match w.strip_suffix('%') {
                            Some(p) => p.parse::<f64>().ok().map(|p| p / 100.0),
                            None => w.parse::<f64>().ok().map(|p| p / 5000.0),
                        };
                        fraction.map_or(TableWidth::Auto, TableWidth::Fraction)
                    }
                    _ => TableWidth::Auto,
                }
            }
            "jc" => {
                props.alignment = match e.val() {
                    Some("center") => TableAlignment::Center,
                    Some("right" | "end") => TableAlignment::Right,
                    _ => TableAlignment::Left,
                }
            }
            "tblInd" => {
                if let Some(v) = margin(e) {
                    props.indent = v;
                }
            }
            "tblBorders" => BordersPatch::read(e).apply(&mut props.borders),
            "tblCellMar" => margins(e, &mut props.cell_margins),
            "tblCellSpacing" => {
                if let Some(v) = margin(e) {
                    props.cell_spacing = v;
                }
            }
            _ => {}
        }
    }
}

/// The `name` children of a table or row, content controls and custom XML
/// around them opened.
fn wrapped<'e>(parent: &'e Element, name: &str, out: &mut Vec<&'e Element>) {
    for e in parent.elements().filter(|e| e.ns == Ns::W) {
        match &*e.name {
            n if n == name => out.push(e),
            "sdt" => {
                if let Some(content) = e.wc("sdtContent") {
                    wrapped(content, name, out);
                }
            }
            "customXml" | "ins" | "moveTo" => wrapped(e, name, out),
            _ => {}
        }
    }
}

/// `patch` laid over `into`, side by side.
fn layer(into: &mut BordersPatch, patch: BordersPatch) {
    for (side, value) in [
        (&mut into.top, patch.top),
        (&mut into.bottom, patch.bottom),
        (&mut into.left, patch.left),
        (&mut into.right, patch.right),
        (&mut into.between, patch.between),
        (&mut into.vertical, patch.vertical),
    ] {
        if value.is_some() {
            *side = value;
        }
    }
}

/// The rules of the cell `cell` of `cells` in the row `row` of `rows`, as
/// LibreOffice hands the table's rules to its cells: a side the cell sets,
/// even to none, keeps it, and the first row takes the inside rule below it
/// before the table's bottom one.
fn cell_borders(
    patch: &BordersPatch,
    table: &Borders,
    (row, rows): (usize, usize),
    (cell, cells): (usize, usize),
) -> Borders {
    let between = patch.between.unwrap_or(table.between);
    let vertical = patch.vertical.unwrap_or(table.vertical);
    let bottom = if row == 0 && between.is_some() {
        between
    } else if row + 1 == rows {
        table.bottom
    } else {
        between
    };
    Borders {
        top: patch
            .top
            .unwrap_or(if row == 0 { table.top } else { between }),
        bottom: patch.bottom.unwrap_or(bottom),
        left: patch
            .left
            .unwrap_or(if cell == 0 { table.left } else { vertical }),
        right: patch.right.unwrap_or(if cell + 1 == cells {
            table.right
        } else {
            vertical
        }),
        between: None,
        vertical: None,
    }
}

/// Half the left rule of the first cell, and its left margin.
fn first_cell(props: &TableProps, rows: &[Row]) -> (f64, f64) {
    let first = rows.first().and_then(|r| r.cells.first());
    let rule = first
        .and_then(|c| c.borders.left)
        .or(props.borders.left)
        .map_or(0.0, |b| b.weight() / 2.0);
    (
        rule,
        first.map_or(props.cell_margins.left, |c| c.margins.left),
    )
}

/// The frame LibreOffice sets a table positioned by `w:tblpPr` in: its left
/// edge `shift` left of the position, the text running around it.
fn table_float(e: &Element, shift: f64) -> Float {
    let length = |name: &str| e.w(name).and_then(twips).unwrap_or(0.0);
    let place = |spec: Option<&str>, offset: f64| match spec {
        Some("left" | "top") => Place::Start,
        Some("center") => Place::Center,
        Some("right" | "bottom") => Place::End,
        Some("inside") => Place::Inside,
        Some("outside") => Place::Outside,
        _ => Place::Offset(offset),
    };
    Float {
        horizontal: Position {
            from: match e.w("horzAnchor") {
                Some("margin") => Relative::Margin,
                Some("page") => Relative::Page,
                _ => Relative::Column,
            },
            place: place(e.w("tblpXSpec"), length("tblpX") - shift),
        },
        vertical: Position {
            from: match e.w("vertAnchor") {
                Some("text") => Relative::Paragraph,
                Some("page") => Relative::Page,
                _ => Relative::Margin,
            },
            place: place(e.w("tblpYSpec"), length("tblpY")),
        },
        wrap: Wrap::Around,
        side: WrapSide::Both,
        behind: false,
        distance: Margins {
            left: length("leftFromText"),
            right: length("rightFromText"),
            top: length("topFromText"),
            bottom: length("bottomFromText"),
        },
        z: 0,
        contour: Vec::new(),
        in_cell: true,
    }
}

/// The left and right cell margins LibreOffice gives a table that sets none:
/// it reads no default table style.
const CELL_MARGIN: f64 = 5.4;

/// Where LibreOffice puts the table's left edge from the text's: Word 2013
/// and a nested table start at the indent, the middle of the first rule,
/// and an earlier Word pulls an indented table's first text back onto it.
fn edge(props: &TableProps, rows: &[Row], nested: bool, compat: u32, indented: bool) -> f64 {
    let (rule, margin) = first_cell(props, rows);
    if nested {
        props.indent.max(0.0) + rule
    } else if compat >= 15 {
        props.indent + rule
    } else if indented {
        props.indent - margin.max(rule)
    } else {
        props.indent - rule
    }
}

impl Story<'_, '_> {
    pub fn table(&mut self, tbl: &Element) -> Result<Table> {
        let ctx = self.ctx;
        let tblpr = tbl.wc("tblPr");
        let style = tblpr
            .and_then(|e| e.wc("tblStyle"))
            .and_then(|e| e.val())
            .filter(|id| ctx.styles.get(id).is_some());

        let mut props = TableProps {
            cell_margins: Margins {
                left: CELL_MARGIN,
                right: CELL_MARGIN,
                ..Margins::default()
            },
            ..TableProps::default()
        };
        let mut indented = false;
        let chain = style.map(|id| ctx.styles.chain(id)).unwrap_or_default();
        let layers = chain
            .iter()
            .filter_map(|s| s.table.as_ref().and_then(|t| t.wc("tblPr")))
            .chain(tblpr);
        for e in layers {
            indented |= e.wc("tblInd").is_some();
            table_props(e, &mut props);
        }

        let grid = tbl
            .wc("tblGrid")
            .map(|g| {
                g.elements()
                    .filter(|c| &*c.name == "gridCol")
                    .map(|c| c.w("w").and_then(twips).unwrap_or(0.0))
                    .collect()
            })
            .unwrap_or_default();

        let look = tblpr
            .and_then(|e| e.wc("tblLook"))
            .map_or(Look::DEFAULT, Look::read);
        let own = style.and_then(|id| ctx.styles.get(id));

        let mut rows = Vec::new();
        self.tables += 1;
        let mut trs = Vec::new();
        wrapped(tbl, "tr", &mut trs);
        let count = trs.len();
        for (r, tr) in trs.into_iter().enumerate() {
            let trpr = tr.wc("trPr");
            let styled: Vec<&Element> = own.map_or(Vec::new(), |s| {
                style_parts(s, look.row_conditions(r, count), "trPr")
            });
            let property = |name: &str| trpr.and_then(|e| e.wc(name));
            let styled_on = |name: &str| {
                property(name)
                    .or_else(|| styled.iter().rev().find_map(|e| e.wc(name)))
                    .is_some_and(on)
            };
            let height = property("trHeight").and_then(|h| {
                let value = h.val().and_then(twips)?;
                (value > 0.0 && h.w("hRule") != Some("auto")).then_some(RowHeight {
                    value,
                    exact: h.w("hRule") == Some("exact"),
                })
            });
            let mut tcs = Vec::new();
            wrapped(tr, "tc", &mut tcs);
            let mut cells = Vec::new();
            let width = tcs.len();
            for (c, tc) in tcs.into_iter().enumerate() {
                let layers = own.map(|style| TableLayers {
                    style,
                    conditions: look.conditions(r, count, c, width),
                });
                cells.push(self.cell(tc, &props, layers, ((r, count), (c, width)))?);
            }
            rows.push(Row {
                grid_before: property("gridBefore")
                    .and_then(|e| e.val()?.parse().ok())
                    .unwrap_or(0),
                cells,
                height,
                cant_split: property("cantSplit").is_some_and(on),
                header: styled_on("tblHeader"),
            });
        }
        self.tables -= 1;
        props.indent = edge(&props, &rows, self.tables > 0, ctx.compat, indented);
        if let Some(e) = tblpr.and_then(|e| e.wc("tblpPr")) {
            let (rule, margin) = first_cell(&props, &rows);
            let shift = if ctx.compat >= 15 {
                rule
            } else {
                margin.max(rule)
            };
            props.float = Some(Box::new(table_float(e, shift)));
        }
        props.borders = Borders::default();
        props.page_break_before = matches!(
            rows.first().and_then(|r| r.cells.first()).and_then(|c| c.blocks.first()),
            Some(Block::Paragraph(p)) if p.props.page_break_before
        );
        Ok(Table { props, grid, rows })
    }

    fn cell(
        &mut self,
        tc: &Element,
        table: &TableProps,
        layers: Option<TableLayers<'_>>,
        (row, cell): ((usize, usize), (usize, usize)),
    ) -> Result<Cell> {
        let tcpr = tc.wc("tcPr");
        let property = |name: &str| tcpr.and_then(|e| e.wc(name));
        let styled = layers.map_or(Vec::new(), |l| style_parts(l.style, l.conditions, "tcPr"));
        let mut patch = BordersPatch::default();
        let mut fill = None;
        for e in styled.iter().copied().chain(tcpr) {
            if let Some(b) = e.wc("tcBorders") {
                layer(&mut patch, BordersPatch::read(b));
            }
            if let Some(shd) = e.wc("shd") {
                fill = shading(shd);
            }
        }
        let mut cell_margins = table.cell_margins;
        if let Some(e) = property("tcMar") {
            margins(e, &mut cell_margins);
        }
        let mut blocks = Vec::new();
        self.blocks(tc, layers, &mut blocks, &mut Vec::new())?;
        if let Some(Block::Paragraph(p)) = blocks.first_mut() {
            if p.props.spacing.before_auto {
                p.props.spacing.before = 0.0;
            }
        }
        Ok(Cell {
            span: property("gridSpan")
                .and_then(|e| e.val()?.parse().ok())
                .unwrap_or(1)
                .max(1),
            vmerge: match property("vMerge") {
                Some(e) if e.val() == Some("restart") => VMerge::Restart,
                Some(_) => VMerge::Continue,
                None => VMerge::None,
            },
            borders: cell_borders(&patch, &table.borders, row, cell),
            shading: fill,
            margins: cell_margins,
            valign: match property("vAlign").and_then(|e| e.val()) {
                Some("center") => VAlign::Center,
                Some("bottom") => VAlign::Bottom,
                _ => VAlign::Top,
            },
            blocks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Condition::*;

    fn grid(look: Look, rows: usize, cells: usize) -> Vec<Vec<Vec<Condition>>> {
        let all = [
            Band2Horz, Band1Horz, Band2Vert, Band1Vert, LastCol, FirstCol, LastRow, FirstRow,
            SwCell, SeCell, NwCell, NeCell,
        ];
        (0..rows)
            .map(|r| {
                (0..cells)
                    .map(|c| {
                        let set = look.conditions(r, rows, c, cells);
                        all.into_iter().filter(|x| set & x.bit() != 0).collect()
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_first_and_last_rows_and_columns_stay_out_of_the_bands() {
        let every = Look {
            first_row: true,
            last_row: true,
            first_col: true,
            last_col: true,
            h_band: true,
            v_band: true,
        };
        let cells = grid(every, 4, 4);
        assert_eq!(cells[0][0], vec![FirstCol, FirstRow, NwCell]);
        assert_eq!(cells[0][1], vec![Band1Vert, FirstRow]);
        assert_eq!(cells[1][1], vec![Band1Horz, Band1Vert]);
        assert_eq!(cells[2][2], vec![Band2Horz, Band2Vert]);
        assert_eq!(cells[1][3], vec![Band1Horz, LastCol]);
        assert_eq!(cells[3][3], vec![LastCol, LastRow, SeCell]);

        let bands = Look {
            first_row: false,
            first_col: false,
            ..every
        };
        assert_eq!(grid(bands, 2, 2)[0][0], vec![Band1Horz, Band1Vert]);
    }

    #[test]
    fn a_look_reads_its_flags_or_else_its_mask() {
        let parse = |attributes: &str| {
            let xml = format!(
                r#"<w:tblLook xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" {attributes}/>"#
            );
            Look::read(&super::super::xml::parse(xml.as_bytes()).unwrap())
        };
        let mask = parse(r#"w:val="04A0""#);
        assert!(mask.first_row && mask.first_col && mask.h_band);
        assert!(!mask.last_row && !mask.last_col && !mask.v_band);
        let flags = parse(r#"w:val="04A0" w:firstRow="0" w:noVBand="0""#);
        assert!(!flags.first_row && flags.first_col && flags.v_band);
    }
}
