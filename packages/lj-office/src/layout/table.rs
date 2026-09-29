//! Tables as LibreOffice lays out a Word table: the grid scaled to its width,
//! rules between rows, merged cells over their rows, rows cut across pages.

use super::float::{self, Area, Exclusion};
use super::flow::{stacked, Flow, EPSILON};
use super::Item;
use crate::model::*;

fn weight(border: Option<Border>) -> f64 {
    border.map_or(0.0, |b| b.weight())
}

/// Grid columns a table may lay out; Word stops at 63, and what lies past
/// this lands in the last one.
const MAX_GRID: usize = 4096;

/// Width of each grid column: the file's grid scaled to the table's width,
/// or that width shared equally when the file gives no grid.
fn widths(table: &Table, available: f64) -> Vec<f64> {
    let columns = table
        .rows
        .iter()
        .map(|r| {
            r.cells
                .iter()
                .fold(r.grid_before, |at, c| at.saturating_add(c.span.max(1)))
        })
        .max()
        .unwrap_or(0)
        .max(table.grid.len())
        .clamp(1, MAX_GRID);
    let mut grid = table.grid.clone();
    grid.resize(columns, 0.0);
    let sum: f64 = grid.iter().sum();
    let total = match table.props.width {
        TableWidth::Points(w) => w,
        TableWidth::Fraction(f) => f * available,
        TableWidth::Auto if sum > 0.0 => sum,
        TableWidth::Auto => available,
    };
    if sum > 0.0 {
        grid.iter().map(|w| w * total / sum).collect()
    } else {
        vec![total / columns as f64; columns]
    }
}

/// Where each grid column starts, and the grid ends.
fn offsets(grid: &[f64]) -> Vec<f64> {
    std::iter::once(0.0)
        .chain(grid.iter().scan(0.0, |sum, w| {
            *sum += w;
            Some(*sum)
        }))
        .collect()
}

/// Grid column each cell of a row starts at.
fn starts(row: &Row) -> Vec<usize> {
    let mut at = row.grid_before;
    row.cells
        .iter()
        .map(|c| {
            let start = at;
            at = at.saturating_add(c.span.max(1));
            start
        })
        .collect()
}

/// The height an item is sorted by when a row is cut: a run's baseline, the
/// top of anything else.
fn key(item: &Item) -> f64 {
    match item {
        Item::Glyphs(g) => g.y,
        Item::Rect { y, .. } => *y,
        Item::Line { from, to, .. } => from[1].min(to[1]),
        Item::Image(image) => image.y,
        Item::Shape(shape) => shape.y,
        Item::Polygon { points, .. } => points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min),
        Item::Path(path) => path
            .rings
            .iter()
            .flatten()
            .map(|p| p[1])
            .fold(f64::INFINITY, f64::min),
        Item::Art(art) => art.frame[1],
    }
}

/// What one cell shows in one row, from the row's top left corner.
#[derive(Clone)]
struct Part {
    x: f64,
    width: f64,
    items: Vec<Item>,
    /// Heights at which the content may be cut, in order.
    cuts: Vec<f64>,
    /// Bottom of the content in this row.
    end: f64,
    shading: Option<Color>,
    top: Option<Border>,
    bottom: Option<Border>,
    left: Option<Border>,
    right: Option<Border>,
}

impl Part {
    fn with(&self, items: Vec<Item>, cuts: Vec<f64>, end: f64) -> Part {
        Part {
            items,
            cuts,
            end,
            ..self.clone_frame()
        }
    }

    fn clone_frame(&self) -> Part {
        Part {
            x: self.x,
            width: self.width,
            items: Vec::new(),
            cuts: Vec::new(),
            end: 0.0,
            shading: self.shading,
            top: self.top,
            bottom: self.bottom,
            left: self.left,
            right: self.right,
        }
    }
}

/// A row laid out, or what of it a page holds.
#[derive(Clone)]
struct Band {
    height: f64,
    /// Above the content: the cells' top rules and margins, or the rule
    /// closing the row above.
    lead: f64,
    /// Below the content: the cells' bottom margins.
    tail: f64,
    /// The widest rule closing the row, which the last row adds below it.
    below: f64,
    splittable: bool,
    /// Kept with the next row, and so never cut.
    keep: bool,
    parts: Vec<Part>,
}

impl Band {
    /// The least of the row a page may take: the first line of each cell, or
    /// all of it when it is not cut.
    fn least(&self) -> f64 {
        if !self.splittable || self.keep {
            return self.height;
        }
        self.parts
            .iter()
            .filter_map(|p| {
                p.cuts
                    .iter()
                    .copied()
                    .filter(|&c| c > self.lead + EPSILON)
                    .reduce(f64::min)
            })
            .reduce(f64::max)
            .map_or(self.height, |c| (c + self.tail).min(self.height))
    }

    /// The row cut to fill `room`, then what goes on, which takes the cells'
    /// top rules and margins again. `None` when no cell can place a line, or,
    /// with `every`, when a cell's first line is taller than the room.
    fn split(&self, room: f64, every: bool) -> Option<(Band, Band)> {
        let limit = room - self.tail;
        let mut placed = false;
        let mut left_out = false;
        let mut rest_end = self.lead;
        let (mut head, mut rest) = (Vec::new(), Vec::new());
        for part in &self.parts {
            let cut = part
                .cuts
                .iter()
                .copied()
                .filter(|&c| c <= limit + EPSILON)
                .reduce(f64::max)
                .unwrap_or(self.lead);
            let shift = cut - self.lead;
            let (mut up, mut down) = (Vec::new(), Vec::new());
            for item in &part.items {
                let mut item = item.clone();
                if key(&item) < cut {
                    up.push(item);
                } else {
                    item.translate(0.0, -shift);
                    down.push(item);
                }
            }
            placed |= !up.is_empty();
            if up.is_empty() {
                let top = part.items.iter().map(key).fold(f64::INFINITY, f64::min);
                let first = part
                    .cuts
                    .iter()
                    .copied()
                    .filter(|&c| c > top)
                    .reduce(f64::min);
                left_out |= first.is_some_and(|c| self.lead + c - top > limit + EPSILON);
            }
            let cuts = part
                .cuts
                .iter()
                .filter(|&&c| c > cut + EPSILON)
                .map(|c| c - shift)
                .collect();
            let end = (part.end - shift).max(self.lead);
            rest_end = rest_end.max(end);
            head.push(part.with(up, Vec::new(), cut));
            rest.push(part.with(down, cuts, end));
        }
        if !placed || (every && left_out) {
            return None;
        }
        Some((
            Band {
                height: room,
                parts: head,
                ..self.clone_frame()
            },
            Band {
                height: rest_end + self.tail,
                keep: false,
                parts: rest,
                ..self.clone_frame()
            },
        ))
    }

    fn clone_frame(&self) -> Band {
        Band {
            height: self.height,
            lead: self.lead,
            tail: self.tail,
            below: self.below,
            splittable: self.splittable,
            keep: self.keep,
            parts: Vec::new(),
        }
    }

    /// Shading, then content, then rules, `height` tall.
    fn render(self, height: f64) -> Vec<Item> {
        let mut back = Vec::new();
        let mut items = Vec::new();
        let mut rules = Vec::new();
        for part in self.parts {
            let (x0, x1) = (part.x, part.x + part.width);
            if let Some(color) = part.shading {
                back.push(Item::Rect {
                    x: x0,
                    y: 0.0,
                    width: part.width,
                    height,
                    color,
                });
            }
            items.extend(part.items);
            let mut rule =
                |border: Option<Border>, horizontal: bool, at: f64, from: f64, to: f64| {
                    let Some(b) = border else {
                        return;
                    };
                    let offsets: &[f64] = if b.double {
                        &[-b.width, b.width]
                    } else {
                        &[0.0]
                    };
                    for d in offsets {
                        let at = at + d;
                        let (from, to) = if horizontal {
                            ([from, at], [to, at])
                        } else {
                            ([at, from], [at, to])
                        };
                        rules.push(Item::Line {
                            from,
                            to,
                            width: b.width,
                            color: b.color.unwrap_or(Color::BLACK),
                        });
                    }
                };
            rule(part.top, true, weight(part.top) / 2.0, x0, x1);
            rule(
                part.bottom,
                true,
                height + weight(part.bottom) / 2.0,
                x0,
                x1,
            );
            rule(part.left, false, x0, 0.0, height);
            rule(part.right, false, x1, 0.0, height);
        }
        back.extend(items);
        back.extend(rules);
        back
    }
}

/// A row set in this column and not drawn yet, which a row moving on may take
/// along.
struct Set {
    row: usize,
    band: Band,
    height: f64,
    advance: f64,
    /// A whole row kept with the next one.
    keep: bool,
    /// A header row repeated at the top of the column.
    repeat: bool,
    /// The row's first part, which sets its footnotes.
    opening: bool,
}

impl<'d> Flow<'_, '_, 'd> {
    pub(super) fn table(&mut self, table: &'d Table) {
        let width = self.column().width;
        let grid = widths(table, width);
        let offset = offsets(&grid);
        let total = offset[grid.len()];
        let bands = self.row_bands(table, &offset, table.rows.len());
        if let (Some(frame), Some(area)) = (table.props.float.as_deref(), self.table_area()) {
            self.floating(table, frame, &area, bands, total);
            return;
        }
        self.open_block();
        let dx = match table.props.alignment {
            TableAlignment::Left => table.props.indent,
            TableAlignment::Center => (width - total) / 2.0,
            TableAlignment::Right => width - total,
        };
        if !self.paged() {
            let last = bands.len().saturating_sub(1);
            for (r, band) in bands.into_iter().enumerate() {
                let height = band.height;
                let below = if r == last { band.below } else { 0.0 };
                self.put(dx, band.render(height), height + below);
            }
            return;
        }
        for cell in table.rows.iter().flat_map(|r| &r.cells) {
            self.escaped(&cell.blocks);
        }
        self.pour(table, bands, dx, None);
    }

    /// A table set apart from the text, which runs around it. Where the page
    /// has no room for it below its paragraph, it runs on over the next pages
    /// and the text goes on beside its last part.
    fn floating(
        &mut self,
        table: &'d Table,
        frame: &Float,
        area: &Area<'_>,
        bands: Vec<Band>,
        width: f64,
    ) {
        let d = frame.distance;
        let last = bands.len().saturating_sub(1);
        let height =
            bands.iter().map(|b| b.height).sum::<f64>() + bands.last().map_or(0.0, |b| b.below);
        let (mut x, mut y) = float::position(
            frame,
            width + d.left + d.right,
            height + d.top + d.bottom,
            area,
        );
        if !matches!(frame.horizontal.place, Place::Offset(_)) {
            x += d.left;
        }
        let paragraph = frame.vertical.from == Relative::Paragraph;
        if !paragraph && !matches!(frame.vertical.place, Place::Offset(_)) {
            y += d.top;
        }
        let around = |top: f64, bottom: f64| Exclusion {
            left: x - d.left,
            top: top - d.top,
            right: x + width + d.right,
            bottom: bottom + d.bottom,
            wrap: Wrap::Around,
            side: WrapSide::Both,
        };
        let mut bottom = self.y + self.room();
        // One set against the foot of the page reaches past its margin.
        match (frame.vertical.from, frame.vertical.place) {
            (Relative::Paragraph, _) => {}
            (Relative::Page, Place::End) => bottom = bottom.max(area.page.height),
            _ => bottom = bottom.max(area.page.height - area.page.margin.bottom),
        }
        if y + height <= bottom + EPSILON {
            let mut items = Vec::new();
            let mut top = y;
            for (r, band) in bands.into_iter().enumerate() {
                let row = band.height;
                let advance = row + if r == last { band.below } else { 0.0 };
                items.extend(band.render(row).into_iter().map(|mut item| {
                    item.translate(x, top);
                    item
                }));
                top += advance;
            }
            self.float_items(items, around(y, y + height));
            return;
        }
        let page = self.fields.page;
        self.open_block();
        self.y = y;
        self.pour(table, bands, x - self.column().x, Some(!paragraph));
        let top = if self.fields.page == page {
            y
        } else {
            self.column_top()
        };
        self.float_items(Vec::new(), around(top, self.y));
        self.y = top;
    }

    /// The rows set down the columns from here, `dx` right of their left edge.
    /// A floating table only cuts a row taller than a column, and stays where
    /// it is set when it is `Some(true)`, fixed on the page.
    fn pour(&mut self, table: &'d Table, bands: Vec<Band>, dx: f64, floating: Option<bool>) {
        let fixed = floating == Some(true);
        let last = bands.len().saturating_sub(1);
        let headers = table
            .rows
            .iter()
            .take_while(|r| r.header)
            .count()
            .min(bands.len());
        let heading: Vec<Band> = bands[..headers].to_vec();
        if !fixed && !self.fits(lead(&bands, headers)) {
            self.next_column();
        }
        let mut first = true;
        let mut column_top = fixed || self.room() + EPSILON >= self.column_height();
        let started_at_top = column_top;
        let mut drawn = false;
        let mut set: Vec<Set> = Vec::new();
        let mut queue: std::collections::VecDeque<(usize, Band)> =
            bands.into_iter().enumerate().collect();
        while let Some((r, mut band)) = queue.pop_front() {
            let below = if r == last { band.below } else { 0.0 };
            let mut opening = true;
            loop {
                let room = self.room() - set.iter().map(|s| s.advance).sum::<f64>();
                let at_top = column_top && !drawn && set.iter().all(|s| s.repeat);
                if band.height + below <= room + EPSILON {
                    let keep = band.keep;
                    set.push(Set {
                        row: r,
                        height: band.height,
                        advance: band.height + below,
                        keep,
                        repeat: false,
                        opening,
                        band,
                    });
                    if !keep {
                        self.draw(table, &mut set, dx);
                        drawn = true;
                    }
                    break;
                }
                let cut = (band.splittable && !band.keep && floating.is_none())
                    || band.height > self.column_height();
                if let Some((head, rest)) = cut.then(|| band.split(room, !at_top)).flatten() {
                    set.push(Set {
                        row: r,
                        band: head,
                        height: room,
                        advance: room,
                        keep: false,
                        repeat: false,
                        opening,
                    });
                    self.draw(table, &mut set, dx);
                    self.next_column();
                    (first, column_top, drawn, opening) = (false, true, false, false);
                    if r >= headers {
                        self.repeat(&mut set, &heading);
                    }
                    band = rest;
                    continue;
                }
                if at_top {
                    set.push(Set {
                        row: r,
                        height: band.height,
                        advance: band.height + below,
                        keep: false,
                        repeat: false,
                        opening,
                        band,
                    });
                    self.draw(table, &mut set, dx);
                    drawn = true;
                    break;
                }
                let chain = set.iter().rev().take_while(|s| s.keep && !s.repeat).count();
                let body = set.iter().filter(|s| !s.repeat).count();
                let carried = if chain > 0 && (chain < body || drawn) {
                    set.split_off(set.len() - chain)
                } else if chain == body && first && !started_at_top {
                    std::mem::take(&mut set)
                } else {
                    Vec::new()
                };
                self.draw(table, &mut set, dx);
                self.next_column();
                (first, column_top, drawn) = (false, true, false);
                if carried.first().map_or(r, |s| s.row) >= headers {
                    self.repeat(&mut set, &heading);
                }
                queue.push_front((r, band));
                for s in carried.into_iter().rev() {
                    queue.push_front((s.row, s.band));
                }
                break;
            }
        }
        self.draw(table, &mut set, dx);
    }

    /// Height of the header rows and of what a page never separates from
    /// them.
    pub(super) fn lead(&self, table: &'d Table) -> f64 {
        if table.props.float.is_some() && self.paged() {
            return 0.0;
        }
        let grid = widths(table, self.column().width);
        let headers = table.rows.iter().take_while(|r| r.header).count();
        let kept = table.rows[headers..]
            .iter()
            .take_while(|r| keeps(r))
            .count();
        let limit = (headers + kept + 1).min(table.rows.len());
        let bands = self.row_bands(table, &offsets(&grid), limit);
        lead(&bands, headers.min(bands.len()))
    }

    fn repeat(&mut self, set: &mut Vec<Set>, heading: &[Band]) {
        for (row, band) in heading.iter().enumerate() {
            set.push(Set {
                row,
                band: band.clone(),
                height: band.height,
                advance: band.height,
                keep: false,
                repeat: true,
                opening: false,
            });
        }
    }

    fn draw(&mut self, table: &Table, set: &mut Vec<Set>, dx: f64) {
        for s in set.drain(..) {
            self.put(dx, s.band.render(s.height), s.advance);
            if s.opening {
                self.row_notes(&table.rows[s.row]);
            }
        }
    }

    fn put(&mut self, dx: f64, items: Vec<Item>, height: f64) {
        let x = self.column().x + dx;
        let y = self.y;
        for mut item in items {
            item.translate(x, y);
            self.push(item);
        }
        self.y += height;
        self.close_block();
    }

    /// The first `limit` rows laid out on the grid whose columns start at
    /// `offset`, each cell's content set once.
    fn row_bands(&self, table: &'d Table, offset: &[f64], limit: usize) -> Vec<Band> {
        let rows = &table.rows[..limit];
        let n = rows.len();
        let columns = offset.len() - 1;
        let outer = &table.props.borders;
        let starts: Vec<Vec<usize>> = rows.iter().map(starts).collect();
        let at = |i: usize, start: usize| starts[i].iter().position(|&s| s == start);

        let mut covered: Vec<Vec<bool>> = rows.iter().map(|r| vec![false; r.cells.len()]).collect();
        let mut span: Vec<Vec<usize>> = rows.iter().map(|r| vec![1; r.cells.len()]).collect();
        for i in 0..n {
            for c in 0..rows[i].cells.len() {
                if covered[i][c] || rows[i].cells[c].vmerge != VMerge::Restart {
                    continue;
                }
                let mut k = 1;
                while let Some(d) = (i + k < n).then(|| at(i + k, starts[i][c])).flatten() {
                    if rows[i + k].cells[d].vmerge != VMerge::Continue {
                        break;
                    }
                    covered[i + k][d] = true;
                    k += 1;
                }
                span[i][c] = k;
            }
        }
        let continued = |i: usize, c: usize| {
            i + 1 < n && at(i + 1, starts[i][c]).is_some_and(|d| covered[i + 1][d])
        };

        struct Geometry {
            x: f64,
            width: f64,
            sides: [Option<Border>; 4],
            pad: (f64, f64),
        }
        let geometry: Vec<Vec<Geometry>> = (0..n)
            .map(|i| {
                let row = &rows[i];
                (0..row.cells.len())
                    .map(|c| {
                        let cell = &row.cells[c];
                        let start = starts[i][c].min(columns - 1);
                        let end = start.saturating_add(cell.span.max(1)).min(columns);
                        let top = (!covered[i][c])
                            .then(|| {
                                cell.borders
                                    .top
                                    .or(if i == 0 { outer.top } else { outer.between })
                            })
                            .flatten();
                        let bottom = (!continued(i, c))
                            .then(|| {
                                cell.borders.bottom.or(if i + 1 == table.rows.len() {
                                    outer.bottom
                                } else {
                                    outer.between
                                })
                            })
                            .flatten();
                        let left =
                            cell.borders
                                .left
                                .or(if c == 0 { outer.left } else { outer.vertical });
                        let right = cell.borders.right.or(if c + 1 == row.cells.len() {
                            outer.right
                        } else {
                            outer.vertical
                        });
                        Geometry {
                            x: offset[start],
                            width: offset[end] - offset[start],
                            sides: [top, bottom, left, right],
                            pad: (
                                cell.margins
                                    .left
                                    .max(weight(left).max(weight(outer.left)) / 2.0),
                                cell.margins
                                    .right
                                    .max(weight(right).max(weight(outer.right)) / 2.0),
                            ),
                        }
                    })
                    .collect()
            })
            .collect();

        let mut lead = vec![0.0; n];
        let mut tail = vec![0.0; n];
        let mut below = vec![0.0; n];
        for i in 0..n {
            let (mut space, mut dist) = (0.0_f64, 0.0_f64);
            for (cell, g) in rows[i].cells.iter().zip(&geometry[i]) {
                space = space.max(weight(g.sides[0]) + cell.margins.top);
                dist = dist.max(cell.margins.top);
                tail[i] = f64::max(tail[i], cell.margins.bottom);
                below[i] = f64::max(below[i], weight(g.sides[1]));
            }
            lead[i] = match i {
                0 => space,
                _ => space.max(below[i - 1] + dist),
            };
        }

        let mut laid: Vec<Vec<Option<_>>> = (0..n)
            .map(|i| {
                rows[i]
                    .cells
                    .iter()
                    .zip(&geometry[i])
                    .zip(&covered[i])
                    .map(|((cell, g), &covered)| {
                        let inner = (g.width - g.pad.0 - g.pad.1).max(1.0);
                        (!covered).then(|| {
                            self.env.over(cell.shading, || {
                                stacked(self.env, &cell.blocks, inner, self.fields)
                            })
                        })
                    })
                    .collect()
            })
            .collect();

        let exact = |i: usize| rows[i].height.is_some_and(|h| h.exact);
        let mut height: Vec<f64> = (0..n)
            .map(|i| {
                let content = laid[i]
                    .iter()
                    .zip(&span[i])
                    .filter(|(_, &k)| k == 1)
                    .filter_map(|(l, _)| l.as_ref().map(|l| l.height))
                    .fold(0.0, f64::max);
                let natural = lead[i] + content + tail[i];
                match rows[i].height {
                    Some(h) if h.exact => h.value,
                    Some(h) => natural.max(h.value),
                    None => natural,
                }
            })
            .collect();
        for i in 0..n {
            for c in 0..rows[i].cells.len() {
                let (k, Some(l)) = (span[i][c], &laid[i][c]) else {
                    continue;
                };
                let e = i + k - 1;
                if k < 2 || exact(e) {
                    continue;
                }
                let room = height[i..=e].iter().sum::<f64>() - lead[i] - tail[e];
                if l.height > room {
                    height[e] += l.height - room;
                }
            }
        }
        let tops: Vec<f64> = height
            .iter()
            .scan(0.0, |sum, h| {
                let top = *sum;
                *sum += h;
                Some(top)
            })
            .collect();

        let mut bands: Vec<Band> = (0..n)
            .map(|i| Band {
                height: height[i],
                lead: lead[i],
                tail: tail[i],
                below: below[i],
                splittable: !rows[i].cant_split && !exact(i),
                keep: keeps(&rows[i]),
                parts: rows[i]
                    .cells
                    .iter()
                    .zip(&geometry[i])
                    .map(|(cell, g)| Part {
                        x: g.x,
                        width: g.width,
                        items: Vec::new(),
                        cuts: Vec::new(),
                        end: lead[i],
                        shading: cell.shading,
                        top: g.sides[0],
                        bottom: g.sides[1],
                        left: g.sides[2],
                        right: g.sides[3],
                    })
                    .collect(),
            })
            .collect();

        for i in 0..n {
            for c in 0..rows[i].cells.len() {
                let Some(content) = laid[i][c].take() else {
                    continue;
                };
                let cell = &rows[i].cells[c];
                let e = i + span[i][c] - 1;
                let targets: Vec<(usize, usize)> = (i..=e)
                    .map(|j| {
                        (
                            j,
                            if j == i {
                                c
                            } else {
                                at(j, starts[i][c]).unwrap_or(c)
                            },
                        )
                    })
                    .collect();
                let top = tops[i] + lead[i];
                let free = (tops[e] + height[e] - tail[e] - top - content.height).max(0.0);
                let dy = top
                    + match cell.valign {
                        VAlign::Top => 0.0,
                        VAlign::Center => free / 2.0,
                        VAlign::Bottom => free,
                    };
                let dx = geometry[i][c].x + geometry[i][c].pad.0;
                let band_of = |y: f64, inclusive: bool| {
                    *targets
                        .iter()
                        .find(|(j, _)| {
                            let bottom = tops[*j] + height[*j];
                            if inclusive {
                                y <= bottom + EPSILON
                            } else {
                                y < bottom
                            }
                        })
                        .unwrap_or(&targets[targets.len() - 1])
                };
                for mut item in content.items {
                    item.translate(dx, dy);
                    let (j, d) = band_of(key(&item), false);
                    item.translate(0.0, -tops[j]);
                    bands[j].parts[d].items.push(item);
                }
                for cut in content.cuts {
                    let y = dy + cut;
                    let (j, d) = band_of(y, true);
                    let part = &mut bands[j].parts[d];
                    part.cuts.push(y - tops[j]);
                    part.end = part.end.max(y - tops[j]);
                }
                for &(j, d) in &targets[1..] {
                    let part = &mut bands[j].parts[d];
                    part.shading = part.shading.or(cell.shading);
                }
            }
        }
        bands
    }
}

/// Height of the header rows, of the rows kept with the next after them, and
/// the least of the row they lead to.
fn lead(bands: &[Band], headers: usize) -> f64 {
    let mut height: f64 = bands[..headers].iter().map(|b| b.height).sum();
    for band in &bands[headers..] {
        if !band.keep {
            return height + band.least();
        }
        height += band.height;
    }
    height
}

/// A row stays on the page of the next one and is never cut, as LibreOffice
/// reads keep with next on the first paragraph of its first cell.
fn keeps(row: &Row) -> bool {
    matches!(
        row.cells.first().and_then(|c| c.blocks.first()),
        Some(Block::Paragraph(p)) if p.props.keep_next
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(lines: &[f64], cuts: &[f64]) -> Part {
        Part {
            x: 0.0,
            width: 100.0,
            items: lines
                .iter()
                .map(|&y| Item::Rect {
                    x: 0.0,
                    y,
                    width: 10.0,
                    height: 10.0,
                    color: Color::BLACK,
                })
                .collect(),
            cuts: cuts.to_vec(),
            end: cuts.iter().copied().fold(0.0, f64::max),
            shading: None,
            top: None,
            bottom: None,
            left: None,
            right: None,
        }
    }

    #[test]
    fn a_row_is_cut_at_each_cells_last_cut_and_goes_on_under_its_lead_again() {
        let band = Band {
            height: 4.0 + 60.0 + 2.0,
            lead: 4.0,
            tail: 2.0,
            below: 0.0,
            splittable: true,
            keep: false,
            parts: vec![
                part(
                    &[4.0, 16.0, 28.0, 40.0, 52.0],
                    &[16.0, 28.0, 40.0, 52.0, 64.0],
                ),
                part(&[4.0], &[16.0]),
            ],
        };
        assert_eq!(band.least(), 18.0);

        let (head, rest) = band.split(45.0, true).unwrap();
        assert_eq!(head.height, 45.0);
        assert_eq!(head.parts[0].items.len(), 3);
        assert_eq!(head.parts[1].items.len(), 1);
        assert_eq!(rest.parts[0].cuts, vec![16.0, 28.0]);
        assert_eq!(key(&rest.parts[0].items[0]), 4.0);
        assert!(rest.parts[1].items.is_empty());
        assert_eq!(rest.height, 4.0 + 24.0 + 2.0);

        assert!(band.split(15.0, false).is_none());
    }

    #[test]
    fn a_row_is_cut_only_where_each_cell_places_its_first_line() {
        let band = Band {
            height: 4.0 + 60.0 + 2.0,
            lead: 4.0,
            tail: 2.0,
            below: 0.0,
            splittable: true,
            keep: false,
            parts: vec![
                part(&[4.0, 16.0, 28.0], &[16.0, 28.0, 40.0]),
                part(&[4.0], &[34.0]),
            ],
        };
        assert_eq!(band.least(), 36.0);
        assert!(band.split(30.0, true).is_none());
        let (head, _) = band.split(30.0, false).unwrap();
        assert_eq!(head.parts[0].items.len(), 2);
        assert!(head.parts[1].items.is_empty());
    }
}
