//! **Rows of data**: what a list, a table and a tree are, and where each of
//! their parts is drawn.
//!
//! One model for the three, because they are one structure: a list is the
//! one-column case, and a tree is rows that carry a depth -- a branch row folds
//! the rows under it that are deeper than it. What the host knows is the rows it
//! was sent, which are open, which are selected and which column the owner says
//! they are ordered by; ordering them is the owner's, since the host holds no
//! document.
//!
//! The geometry ([`Geometry`]) is the one function the drawing and the press
//! both read, so a row is hit where it is drawn. Only the rows on screen are
//! laid down: a list of ten thousand rows costs the rows that fit.

use crate::host::font::{self, symbol};
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::size::line_box;

/// One column: its title, and its width in logical pixels when it names one --
/// the columns that do not share what is left.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Column {
    pub title: String,
    pub w: Option<f32>,
}

/// One row: its cells, how deep it is, and whether it is a branch and open.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub cells: Vec<String>,
    /// How many levels under the top this row is -- `0` for a flat list.
    pub depth: u32,
    /// `Some` for a **branch**: whether the rows under it are shown.
    pub open: Option<bool>,
}

/// **How many rows a table carries as a prop**: fifty thousand.
///
/// A table's rows ride its def and every `set` of them, as JSON in one OSC
/// argument, and there is no second way in. Measured
/// (`tests/table_rows_cost.rs`, a release host): a thousand rows of three
/// cells are 35 kB and a third of a millisecond to replace, twenty thousand
/// are 700 kB and ten milliseconds, a hundred thousand are 3.5 MB and
/// thirty-seven -- past the thirty-three a frame is. So the line is where a
/// `set` is still well inside a frame, at about nineteen milliseconds and
/// under two megabytes. What bounds a table sooner is its carrier and not
/// the host: a datagram holds some eighteen hundred rows, a stream frame
/// (16 MiB by default) nine times the line.
///
/// It is a line and not a wall. A table past it is shown whole, and the host
/// says so once a set ([`warn_past_the_wire`]): a list that long is one its
/// owner pages -- it holds the rows, and sends the stretch a reader is in --
/// rather than one that needs a second format for rows.
pub const WIRE_ROWS: usize = 50_000;

/// Says that a table of `rows` rows is past [`WIRE_ROWS`], when it is.
pub fn warn_past_the_wire(rows: usize) {
    if rows > WIRE_ROWS {
        crate::host::diag::warn!(
            "a table of {rows} rows: past {WIRE_ROWS} a set of them costs more than a frame              -- hold the list and send the stretch on screen"
        );
    }
}

/// The rows a reader sees: every row but those under a folded branch, as
/// indices into `rows`.
pub fn visible(rows: &[Row]) -> Vec<usize> {
    let mut out = Vec::with_capacity(rows.len());
    // The depth under which rows are hidden, while inside a folded branch.
    let mut hidden_below: Option<u32> = None;
    for (i, row) in rows.iter().enumerate() {
        if let Some(d) = hidden_below {
            if row.depth > d {
                continue;
            }
            hidden_below = None;
        }
        out.push(i);
        if row.open == Some(false) {
            hidden_below = Some(row.depth);
        }
    }
    out
}

/// The row a `Left` on row `i` goes to: the branch it is under, if any.
pub fn parent(rows: &[Row], i: usize) -> Option<usize> {
    let depth = rows.get(i)?.depth;
    (0..i).rev().find(|&j| rows[j].depth < depth)
}

/// The height of one row.
pub fn row_h(size: f32, m: &Metrics) -> f32 {
    line_box(size, m)
}

/// The width one level of a tree indents by, and the cell a branch's
/// disclosure mark takes.
fn level_w(size: f32, m: &Metrics) -> f32 {
    font::height(size) + m.pad
}

/// Where the parts of a placed list are.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// The header strip -- zero tall when no column has a title.
    pub header: Rect,
    /// Where the rows are drawn, under the header.
    pub list: Rect,
    /// Each column's left edge and width.
    pub columns: Vec<(f32, f32)>,
    pub row_h: f32,
    /// How tall all the visible rows are together.
    pub content: f32,
}

impl Geometry {
    /// The geometry of `columns` and `shown` rows in `body`, drawn at `size`.
    /// `scale` turns a column's logical width into the placement's pixels.
    pub fn new(
        body: Rect,
        columns: &[Column],
        shown: usize,
        size: f32,
        scale: f32,
        m: &Metrics,
    ) -> Self {
        let rh = row_h(size, m);
        let header_h = if columns.iter().any(|c| !c.title.is_empty()) {
            rh.min(body.h)
        } else {
            0.0
        };
        let header = Rect::new(body.x, body.y, body.w, header_h);
        let list = Rect::new(
            body.x,
            body.y + header_h,
            body.w,
            (body.h - header_h).max(0.0),
        );
        // The named widths first; the rest share what is left evenly.
        let n = columns.len().max(1);
        let fixed: f32 = columns.iter().filter_map(|c| c.w).map(|w| w * scale).sum();
        let free = columns.iter().filter(|c| c.w.is_none()).count().max(
            // A list with no columns is one column filling the body.
            usize::from(columns.is_empty()),
        );
        let share = ((body.w - fixed).max(0.0)) / free.max(1) as f32;
        let mut x = body.x;
        let widths: Vec<f32> = if columns.is_empty() {
            vec![body.w]
        } else {
            columns
                .iter()
                .map(|c| c.w.map_or(share, |w| w * scale))
                .collect()
        };
        let columns = widths
            .into_iter()
            .take(n)
            .map(|w| {
                let at = x;
                x += w;
                (at, w)
            })
            .collect();
        Geometry {
            header,
            list,
            columns,
            row_h: rh,
            content: rh * shown as f32,
        }
    }

    /// How far the list can scroll.
    pub fn max_scroll(&self) -> f32 {
        (self.content - self.list.h).max(0.0)
    }

    /// The rectangle of the `k`-th visible row at scroll `scroll` -- outside the
    /// list when it is scrolled out of it.
    pub fn row(&self, k: usize, scroll: f32) -> Rect {
        Rect::new(
            self.list.x,
            self.list.y + k as f32 * self.row_h - scroll,
            self.list.w,
            self.row_h,
        )
    }

    /// The visible rows that are on screen at `scroll`, as positions in the
    /// visible order.
    pub fn on_screen(&self, shown: usize, scroll: f32) -> std::ops::Range<usize> {
        if self.row_h <= 0.0 {
            return 0..0;
        }
        let first = (scroll / self.row_h).floor().max(0.0) as usize;
        let last = ((scroll + self.list.h) / self.row_h).ceil() as usize;
        first.min(shown)..last.min(shown)
    }

    /// The position in the visible order of the row under `y`, if any.
    pub fn row_at(&self, shown: usize, scroll: f32, x: f64, y: f64) -> Option<usize> {
        if !self.list.contains(x, y) || self.row_h <= 0.0 {
            return None;
        }
        let k = ((y - self.list.y as f64 + scroll as f64) / self.row_h as f64).floor();
        (k >= 0.0 && (k as usize) < shown).then_some(k as usize)
    }

    /// The column whose header is under `(x, y)`.
    pub fn header_at(&self, x: f64, y: f64) -> Option<usize> {
        if !self.header.contains(x, y) {
            return None;
        }
        self.columns
            .iter()
            .position(|(at, w)| x >= *at as f64 && x < (*at + *w) as f64)
    }

    /// The disclosure mark of a branch row at `depth`, inside `row`.
    pub fn disclosure(&self, row: Rect, depth: u32, size: f32, m: &Metrics) -> Rect {
        let w = level_w(size, m);
        Rect::new(row.x + m.pad + depth as f32 * w, row.y, w, row.h)
    }

    /// The scroll that keeps visible row `k` on screen, from `scroll`.
    pub fn reveal(&self, k: usize, scroll: f32) -> f32 {
        let top = k as f32 * self.row_h;
        let bottom = top + self.row_h;
        let to = if top < scroll {
            top
        } else if bottom > scroll + self.list.h {
            bottom - self.list.h
        } else {
            scroll
        };
        to.clamp(0.0, self.max_scroll())
    }
}

/// What a list is drawn with this frame.
pub struct Look<'a> {
    pub columns: &'a [Column],
    pub rows: &'a [Row],
    pub shown: &'a [usize],
    pub selected: &'a [usize],
    /// The row under the pointer, as an index into `rows`.
    pub hover: Option<usize>,
    /// The column the rows are ordered by, and whether it is ascending.
    pub sort: Option<(usize, bool)>,
    pub scroll: f32,
    pub size: f32,
    /// A line drawn in the list when it has no rows to show -- an error, or
    /// "empty".
    pub empty: Option<&'a str>,
}

/// Draws a list: its ground, its header, the rows on screen and a scroll mark.
pub fn draw(d: &mut Draw, g: &Geometry, look: &Look) {
    let (mesh, m, theme) = d.parts();
    let size = look.size;
    let glyph = font::height(size);
    let ty = |r: Rect| r.y + (r.h - glyph) * 0.5;
    mesh.rect(g.list, theme.field);
    // The header: each title, the ordering mark on the column the rows are
    // ordered by, and a hairline between columns.
    if g.header.h > 0.0 {
        mesh.rect(g.header, theme.header);
        for (c, (x, w)) in g.columns.iter().enumerate() {
            let cell = Rect::new(*x, g.header.y, *w, g.header.h);
            let title = look.columns.get(c).map_or("", |c| c.title.as_str());
            let mark = match look.sort {
                Some((sc, up)) if sc == c => Some(if up {
                    symbol::POINT_UP
                } else {
                    symbol::POINT_DOWN
                }),
                _ => None,
            };
            let room = if mark.is_some() { glyph + m.pad } else { 0.0 };
            font::text_ellipsis(
                mesh,
                title,
                cell.x + m.pad,
                ty(cell),
                (cell.w - 2.0 * m.pad - room).max(0.0),
                size,
                theme.text_dim,
            );
            if let Some(mark) = mark {
                font::text(
                    mesh,
                    &mark.to_string(),
                    cell.x + cell.w - m.pad - glyph,
                    ty(cell),
                    size,
                    theme.accent,
                );
            }
            if c > 0 {
                mesh.rect(
                    Rect::new(*x, g.header.y, m.divider_w, g.header.h),
                    theme.separator,
                );
            }
        }
        mesh.rect(
            Rect::new(
                g.header.x,
                g.header.y + g.header.h - m.divider_w,
                g.header.w,
                m.divider_w,
            ),
            theme.separator,
        );
    }
    // The rows, cut to the list: a row half scrolled out shows its half.
    let outer = mesh.clip();
    let inner = outer.map_or(g.list, |c| c.intersect(g.list));
    mesh.set_clip(Some(inner));
    if look.shown.is_empty()
        && let Some(line) = look.empty
    {
        font::text_ellipsis(
            mesh,
            line,
            g.list.x + m.pad,
            g.list.y + m.pad,
            (g.list.w - 2.0 * m.pad).max(0.0),
            size,
            theme.text_dim,
        );
    }
    let level = level_w(size, m);
    // In a tree every first cell keeps the room a branch's mark takes, so the
    // leaves line up under their branches.
    let tree = look.rows.iter().any(|r| r.open.is_some());
    for k in g.on_screen(look.shown.len(), look.scroll) {
        let i = look.shown[k];
        let row = &look.rows[i];
        let r = g.row(k, look.scroll);
        let fill = if look.selected.contains(&i) {
            Some(theme.accent_dim)
        } else if look.hover == Some(i) {
            Some(theme.hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            mesh.rect(r, fill);
        }
        let ink = fill.map_or(theme.text, |f| theme.text_on(f));
        for (c, (x, w)) in g.columns.iter().enumerate() {
            let mut left = *x + m.pad;
            // The first cell carries the tree: the depth, and a branch's mark.
            if c == 0 {
                let indent = row.depth as f32 * level;
                if let Some(open) = row.open {
                    let mark = if open {
                        symbol::POINT_DOWN
                    } else {
                        symbol::POINT_RIGHT
                    };
                    font::text(
                        mesh,
                        &mark.to_string(),
                        left + indent,
                        ty(r),
                        size,
                        theme.accent,
                    );
                }
                left += indent + if tree { level } else { 0.0 };
            }
            let text = row.cells.get(c).map_or("", String::as_str);
            font::text_ellipsis(
                mesh,
                text,
                left,
                ty(r),
                (*x + *w - m.pad - left).max(0.0),
                size,
                ink,
            );
        }
    }
    mesh.set_clip(outer);
    // Where the view stands in the rows, when they do not all fit.
    let max = g.max_scroll();
    if max > 0.0 && g.list.h > 0.0 {
        let thick = m.track_thick.min(g.list.w);
        let len = (g.list.h * g.list.h / g.content).max(m.handle_grip.min(g.list.h));
        let at = (look.scroll / max) * (g.list.h - len);
        mesh.rect(
            Rect::new(g.list.x + g.list.w - thick, g.list.y + at, thick, len),
            theme.accent_dim,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(depth: u32, open: Option<bool>) -> Row {
        Row {
            cells: vec![format!("{depth}")],
            depth,
            open,
        }
    }

    #[test]
    fn a_folded_branch_hides_what_is_deeper_than_it_and_nothing_else() {
        let rows = vec![
            row(0, Some(false)),
            row(1, None),
            row(1, Some(true)),
            row(2, None),
            row(0, Some(true)),
            row(1, None),
        ];
        assert_eq!(visible(&rows), vec![0, 4, 5]);
        let mut open = rows.clone();
        open[0].open = Some(true);
        open[2].open = Some(false);
        assert_eq!(visible(&open), vec![0, 1, 2, 4, 5]);
        assert_eq!(parent(&rows, 3), Some(2));
        assert_eq!(parent(&rows, 5), Some(4));
        assert_eq!(parent(&rows, 0), None);
    }

    #[test]
    fn named_widths_are_kept_and_the_rest_share_what_is_left() {
        let m = Metrics::default();
        let cols = vec![
            Column {
                title: "name".into(),
                w: None,
            },
            Column {
                title: "size".into(),
                w: Some(60.0),
            },
        ];
        let g = Geometry::new(Rect::new(0.0, 0.0, 300.0, 200.0), &cols, 10, 2.0, 1.0, &m);
        assert_eq!(g.columns, vec![(0.0, 240.0), (240.0, 60.0)]);
        assert!(g.header.h > 0.0, "a titled column has a header");
        assert_eq!(g.list.y, g.header.h);
        assert_eq!(g.header_at(250.0, 2.0), Some(1));
        let untitled = Geometry::new(Rect::new(0.0, 0.0, 300.0, 200.0), &[], 3, 2.0, 1.0, &m);
        assert_eq!(untitled.header.h, 0.0);
        assert_eq!(untitled.columns, vec![(0.0, 300.0)]);
    }

    #[test]
    fn only_the_rows_on_screen_are_laid_down_and_a_row_is_hit_where_it_is() {
        let m = Metrics::default();
        let g = Geometry::new(Rect::new(0.0, 0.0, 200.0, 100.0), &[], 10_000, 2.0, 1.0, &m);
        let range = g.on_screen(10_000, 0.0);
        assert!(range.len() <= (100.0 / g.row_h).ceil() as usize + 1);
        let scroll = 50.0 * g.row_h;
        let k = g
            .row_at(10_000, scroll, 10.0, (g.row_h * 0.5) as f64)
            .unwrap();
        assert_eq!(k, 50);
        let r = g.row(k, scroll);
        assert!(r.contains(10.0, (g.row_h * 0.5) as f64));
        // Revealing a row below the view brings its foot to the list's foot.
        let to = g.reveal(200, 0.0);
        assert!((g.row(200, to).y + g.row_h - (g.list.y + g.list.h)).abs() < 0.01);
    }
}
