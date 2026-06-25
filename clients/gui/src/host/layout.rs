//! The layout engine: place a typed widget tree into pixel rectangles.
//!
//! Pure geometry, no GPU: given the window's content area and a [`Widget`] tree,
//! it assigns every widget a [`Rect`] in **device pixels** (top-left origin, the
//! same space `wgpu::RenderPass::set_viewport` wants), so the renderer just sets
//! each widget's viewport and draws into its own clip space. A container splits
//! its area among its children by its [`Layout`]: `col` stacks them vertically,
//! `row` side by side, `grid` into a near-square arrangement, `free` overlays
//! them on the whole area. Children are evenly sized at this milestone (the
//! editor-grade per-widget sizing is future work); a small margin and gap keep
//! them visually separated.

use super::widget::{Layout, Widget, WidgetKind};

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

/// Margin inside a container before its children, and the gap between children.
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
    let layout = match widget.kind {
        WidgetKind::Window { layout, .. } | WidgetKind::Panel { layout, .. } => layout,
        _ => return, // leaves have no children to place
    };
    let inner = area.inset(MARGIN);
    for (child, rect) in
        widget
            .children
            .iter()
            .zip(child_rects(inner, widget.children.len(), layout))
    {
        place(rect, child, out);
    }
}

/// The child rectangles for `n` children laid out in `inner` by `layout`.
fn child_rects(inner: Rect, n: usize, layout: Layout) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    match layout {
        Layout::Free => vec![inner; n],
        Layout::Row => strip(inner, n, true),
        Layout::Col => strip(inner, n, false),
        Layout::Grid => grid(inner, n),
    }
}

/// `n` equal cells along one axis (`horizontal` = a row, else a column).
fn strip(inner: Rect, n: usize, horizontal: bool) -> Vec<Rect> {
    let n_f = n as f32;
    let gaps = GAP * (n_f - 1.0);
    if horizontal {
        let w = ((inner.w - gaps) / n_f).max(0.0);
        (0..n)
            .map(|i| Rect::new(inner.x + i as f32 * (w + GAP), inner.y, w, inner.h))
            .collect()
    } else {
        let h = ((inner.h - gaps) / n_f).max(0.0);
        (0..n)
            .map(|i| Rect::new(inner.x, inner.y + i as f32 * (h + GAP), inner.w, h))
            .collect()
    }
}

/// `n` cells in a near-square grid (row-major), filling left-to-right, top-down.
fn grid(inner: Rect, n: usize) -> Vec<Rect> {
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    let cw = ((inner.w - GAP * (cols as f32 - 1.0)) / cols as f32).max(0.0);
    let ch = ((inner.h - GAP * (rows as f32 - 1.0)) / rows as f32).max(0.0);
    (0..n)
        .map(|i| {
            let (c, r) = (i % cols, i / cols);
            Rect::new(
                inner.x + c as f32 * (cw + GAP),
                inner.y + r as f32 * (ch + GAP),
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
