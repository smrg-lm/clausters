//! The layout engine: place a typed widget tree into pixel rectangles.
//!
//! Pure geometry, no GPU: given the window's content area and a [`Widget`] tree,
//! it assigns every widget a [`Rect`] in **device pixels** (top-left origin, the
//! same space `wgpu::RenderPass::set_viewport` wants), so the renderer just sets
//! each widget's viewport and draws into its own clip space. A container splits
//! its area among its children by its [`Layout`]: `col` stacks them vertically,
//! `row` side by side, `grid` into a grid (near-square, or `cols` columns),
//! `free` overlays or positions them. **One pass, no measurement, no constraint
//! solver** — a deliberate boundary: when a layout needs negotiation, the
//! answer is explicit sizes.
//!
//! Along a `row`/`col` main axis a child with a fixed size ([`Place::w`] /
//! [`Place::h`]) takes exactly that; the remaining children share the leftover
//! by [`Place::weight`] (absent = 1), so a tree with no place props lays out
//! exactly as it always did — an even split. The cross axis fills. In `free`,
//! a child with any of `x`/`y`/`w`/`h` positions absolutely inside the
//! container (missing size = the rest of the area); a child with none keeps
//! the full-area overlay. A container's [`Flow`] tunes the inner `margin`, the
//! `gap` between children, and the `grid` column count.

use super::widget::{Flow, Layout, Place, Widget, WidgetKind};

/// A rectangle in device pixels, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Shrinks the rectangle by `m` on every side (never below zero size).
    fn inset(self, m: f32) -> Rect {
        Rect {
            x: self.x + m,
            y: self.y + m,
            w: (self.w - 2.0 * m).max(0.0),
            h: (self.h - 2.0 * m).max(0.0),
        }
    }

    /// Whether `(px, py)` (device pixels) falls inside the rectangle.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (px, py) = (px as f32, py as f32);
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// A widget and the rectangle it occupies. Emitted parent-before-child, so
/// drawing in order paints containers under their contents.
#[derive(Debug, Clone, Copy)]
pub struct Placed<'a> {
    pub rect: Rect,
    pub widget: &'a Widget,
}

/// Default margin inside a container before its children, and default gap
/// between children (a container's `margin`/`gap` props override them).
const MARGIN: f32 = 6.0;
const GAP: f32 = 6.0;

/// Lays out `root` into `area`, returning every widget with its rectangle.
pub fn layout(area: Rect, root: &Widget) -> Vec<Placed<'_>> {
    let mut out = Vec::new();
    place(area, root, &mut out);
    out
}

fn place<'a>(area: Rect, widget: &'a Widget, out: &mut Vec<Placed<'a>>) {
    out.push(Placed { rect: area, widget });
    let (layout, flow) = match widget.kind {
        WidgetKind::Window { layout, flow, .. } | WidgetKind::Panel { layout, flow } => {
            (layout, flow)
        }
        _ => return, // leaves have no children to place
    };
    let inner = area.inset(flow.margin.unwrap_or(MARGIN).max(0.0));
    for (child, rect) in
        widget
            .children
            .iter()
            .zip(child_rects(inner, widget.children.as_slice(), layout, flow))
    {
        place(rect, child, out);
    }
}

/// The child rectangles for `children` laid out in `inner` by `layout`.
fn child_rects(inner: Rect, children: &[Widget], layout: Layout, flow: Flow) -> Vec<Rect> {
    if children.is_empty() {
        return Vec::new();
    }
    let gap = flow.gap.unwrap_or(GAP).max(0.0);
    match layout {
        Layout::Free => children.iter().map(|c| free_rect(inner, c.place)).collect(),
        Layout::Row => strip(inner, children, gap, true),
        Layout::Col => strip(inner, children, gap, false),
        Layout::Grid => grid(inner, children.len(), gap, flow.cols),
    }
}

/// A `free` child's rectangle: absolute `x`/`y`/`w`/`h` inside `inner` when
/// any is given (missing position = the container's origin, missing size = the
/// rest of the area), the full-area overlay when none is.
fn free_rect(inner: Rect, p: Place) -> Rect {
    if p.x.is_none() && p.y.is_none() && p.w.is_none() && p.h.is_none() {
        return inner;
    }
    let (x, y) = (p.x.unwrap_or(0.0), p.y.unwrap_or(0.0));
    Rect::new(
        inner.x + x,
        inner.y + y,
        p.w.unwrap_or(inner.w - x).max(0.0),
        p.h.unwrap_or(inner.h - y).max(0.0),
    )
}

/// Cells along one axis (`horizontal` = a row, else a column): a child with a
/// fixed main-axis size (`w` in a row, `h` in a column) takes exactly that;
/// the rest share the leftover by `weight` (absent = 1). The cross axis fills.
fn strip(inner: Rect, children: &[Widget], gap: f32, horizontal: bool) -> Vec<Rect> {
    let gaps = gap * (children.len() as f32 - 1.0);
    let main = if horizontal { inner.w } else { inner.h };
    let fixed_of = |p: Place| if horizontal { p.w } else { p.h };
    let fixed: f32 = children.iter().filter_map(|c| fixed_of(c.place)).sum();
    let total_weight: f32 = children
        .iter()
        .filter(|c| fixed_of(c.place).is_none())
        .map(|c| c.place.weight.unwrap_or(1.0).max(0.0))
        .sum();
    let leftover = (main - gaps - fixed).max(0.0);
    let share = |c: &Widget| match fixed_of(c.place) {
        Some(px) => px.max(0.0),
        None if total_weight > 0.0 => {
            leftover * c.place.weight.unwrap_or(1.0).max(0.0) / total_weight
        }
        None => 0.0,
    };
    let mut at = if horizontal { inner.x } else { inner.y };
    children
        .iter()
        .map(|c| {
            let size = share(c);
            let rect = if horizontal {
                Rect::new(at, inner.y, size, inner.h)
            } else {
                Rect::new(inner.x, at, inner.w, size)
            };
            at += size + gap;
            rect
        })
        .collect()
}

/// `n` grid cells (row-major, left-to-right, top-down): `cols` columns when
/// given, else near-square. Cells are equal; place props do not apply here.
fn grid(inner: Rect, n: usize, gap: f32, cols: Option<u32>) -> Vec<Rect> {
    let cols = match cols {
        Some(c) => (c as usize).clamp(1, n.max(1)),
        None => (n as f64).sqrt().ceil() as usize,
    };
    let rows = n.div_ceil(cols);
    let cw = ((inner.w - gap * (cols as f32 - 1.0)) / cols as f32).max(0.0);
    let ch = ((inner.h - gap * (rows as f32 - 1.0)) / rows as f32).max(0.0);
    (0..n)
        .map(|i| {
            let (c, r) = (i % cols, i / cols);
            Rect::new(
                inner.x + c as f32 * (cw + gap),
                inner.y + r as f32 * (ch + gap),
                cw,
                ch,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::guidef::GuiNode;
    use crate::host::widget::Widget;

    fn tree(json: &str) -> Widget {
        Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap()
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 600.0, 400.0)
    }

    #[test]
    fn col_splits_height_evenly() {
        let w = tree(
            r#"{"type":"window","layout":"col","children":[
            {"id":1,"type":"label","text":"a"},{"id":2,"type":"label","text":"b"}]}"#,
        );
        let placed = layout(area(), &w);
        // [window, child a, child b]
        assert_eq!(placed.len(), 3);
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert!((a.w - b.w).abs() < 1e-3 && (a.h - b.h).abs() < 1e-3);
        assert!(a.y < b.y, "col stacks vertically");
        assert!((a.x - b.x).abs() < 1e-3, "same x in a column");
    }

    #[test]
    fn row_splits_width_evenly() {
        let w = tree(
            r#"{"type":"window","layout":"row","children":[
            {"id":1,"type":"label","text":"a"},{"id":2,"type":"label","text":"b"}]}"#,
        );
        let placed = layout(area(), &w);
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert!(a.x < b.x, "row places side by side");
        assert!((a.y - b.y).abs() < 1e-3, "same y in a row");
    }

    #[test]
    fn single_child_fills_inset_area() {
        let w = tree(r#"{"type":"window","children":[{"id":12,"type":"waveform","data":[]}]}"#);
        let placed = layout(area(), &w);
        assert_eq!(placed.len(), 2);
        let r = placed[1].rect;
        // Inset by MARGIN on each side.
        assert!((r.x - MARGIN).abs() < 1e-3 && (r.y - MARGIN).abs() < 1e-3);
        assert!((r.w - (600.0 - 2.0 * MARGIN)).abs() < 1e-3);
    }

    #[test]
    fn grid_of_four_is_two_by_two() {
        let w = tree(
            r#"{"type":"window","layout":"grid","children":[
            {"id":1,"type":"label","text":"a"},{"id":2,"type":"label","text":"b"},
            {"id":3,"type":"label","text":"c"},{"id":4,"type":"label","text":"d"}]}"#,
        );
        let placed = layout(area(), &w);
        let cells: Vec<Rect> = placed[1..].iter().map(|p| p.rect).collect();
        // 2x2: cell 0 and 1 share a row (same y), 0 and 2 share a column (same x).
        assert!((cells[0].y - cells[1].y).abs() < 1e-3);
        assert!((cells[0].x - cells[2].x).abs() < 1e-3);
        assert!(cells[1].x > cells[0].x && cells[2].y > cells[0].y);
    }

    #[test]
    fn fixed_size_and_weight_split_a_row() {
        let w = tree(
            r#"{"type":"window","layout":"row","margin":0,"gap":0,"children":[
            {"id":1,"type":"label","text":"a","w":100},
            {"id":2,"type":"label","text":"b","weight":3},
            {"id":3,"type":"label","text":"c"}]}"#,
        );
        let placed = layout(area(), &w);
        let (a, b, c) = (placed[1].rect, placed[2].rect, placed[3].rect);
        assert_eq!(a.w, 100.0, "fixed child takes exactly its w");
        // Leftover 500 split 3:1.
        assert!((b.w - 375.0).abs() < 1e-3, "weight 3 takes 3/4 of the rest");
        assert!((c.w - 125.0).abs() < 1e-3, "weight defaults to 1");
        assert_eq!(c.x + c.w, 600.0, "the row fills the area");
    }

    #[test]
    fn the_application_shell_lays_out() {
        // The acceptance shape: a col window with a fixed menu bar, a weighted
        // content area and a fixed status bar.
        let w = tree(
            r#"{"type":"window","layout":"col","margin":0,"gap":0,"children":[
            {"id":11,"type":"panel","layout":"row","h":28},
            {"id":12,"type":"panel"},
            {"id":13,"type":"label","text":"ready","h":20}]}"#,
        );
        let placed = layout(area(), &w);
        let bars: Vec<Rect> = placed
            .iter()
            .filter(|p| matches!(p.widget.id, Some(11) | Some(12) | Some(13)))
            .map(|p| p.rect)
            .collect();
        assert_eq!(bars[0].h, 28.0, "menu bar is fixed");
        assert_eq!(bars[2].h, 20.0, "status bar is fixed");
        assert!(
            (bars[1].h - (400.0 - 48.0)).abs() < 1e-3,
            "content takes the rest"
        );
        assert_eq!(bars[2].y + bars[2].h, 400.0, "the shell fills the window");
    }

    #[test]
    fn margin_and_gap_props_override_the_defaults() {
        let w = tree(
            r#"{"type":"window","layout":"col","margin":10,"gap":2,"children":[
            {"id":1,"type":"label","text":"a"},{"id":2,"type":"label","text":"b"}]}"#,
        );
        let placed = layout(area(), &w);
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert_eq!((a.x, a.y), (10.0, 10.0), "margin insets the content");
        assert!(
            (b.y - (a.y + a.h + 2.0)).abs() < 1e-3,
            "gap separates the children"
        );
    }

    #[test]
    fn grid_cols_prop_fixes_the_column_count() {
        let w = tree(
            r#"{"type":"window","layout":"grid","cols":4,"children":[
            {"id":1,"type":"label","text":"a"},{"id":2,"type":"label","text":"b"},
            {"id":3,"type":"label","text":"c"},{"id":4,"type":"label","text":"d"},
            {"id":5,"type":"label","text":"e"}]}"#,
        );
        let placed = layout(area(), &w);
        let cells: Vec<Rect> = placed[1..].iter().map(|p| p.rect).collect();
        // 4 columns: the first four share a row, the fifth starts the next.
        assert!((cells[0].y - cells[3].y).abs() < 1e-3);
        assert!(cells[4].y > cells[0].y);
        assert!((cells[4].x - cells[0].x).abs() < 1e-3, "row-major wrap");
    }

    #[test]
    fn free_children_position_absolutely_or_overlay() {
        let w = tree(
            r#"{"type":"window","layout":"free","margin":0,"children":[
            {"id":1,"type":"label","text":"a","x":50,"y":30,"w":120,"h":40},
            {"id":2,"type":"label","text":"b","w":200},
            {"id":3,"type":"label","text":"c"}]}"#,
        );
        let placed = layout(area(), &w);
        let (a, b, c) = (placed[1].rect, placed[2].rect, placed[3].rect);
        assert_eq!((a.x, a.y, a.w, a.h), (50.0, 30.0, 120.0, 40.0));
        assert_eq!((b.x, b.w), (0.0, 200.0), "position defaults to the origin");
        assert_eq!(b.h, 400.0, "missing size takes the rest of the area");
        assert_eq!(
            (c.x, c.y, c.w, c.h),
            (0.0, 0.0, 600.0, 400.0),
            "no props = overlay"
        );
    }

    #[test]
    fn oversized_fixed_children_never_go_negative() {
        let w = tree(
            r#"{"type":"window","layout":"row","margin":0,"gap":0,"children":[
            {"id":1,"type":"label","text":"a","w":900},
            {"id":2,"type":"label","text":"b"}]}"#,
        );
        let placed = layout(area(), &w);
        assert_eq!(
            placed[2].rect.w, 0.0,
            "no leftover: the flexible child collapses"
        );
    }

    #[test]
    fn nested_panel_recurses() {
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"panel","children":[{"id":12,"type":"waveform","data":[]}]}]}"#,
        );
        let placed = layout(area(), &w);
        // [window, panel, waveform]
        assert_eq!(placed.len(), 3);
        assert!(placed[2].widget.is_waveform());
        // The waveform sits inside the panel's rect.
        let (panel, wave) = (placed[1].rect, placed[2].rect);
        assert!(wave.x >= panel.x && wave.y >= panel.y);
    }
}
