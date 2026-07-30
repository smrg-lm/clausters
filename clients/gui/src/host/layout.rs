//! The layout engine: place a typed widget tree into pixel rectangles.
//!
//! Pure geometry, no GPU: given the window's content area and a [`Widget`] tree,
//! it assigns every widget a [`Rect`] in **physical pixels** (top-left origin, the
//! same space `wgpu::RenderPass::set_viewport` wants), so the renderer just sets
//! each widget's viewport and draws into its own clip space. A container splits
//! its area among its children by its [`Layout`]: `col` stacks them vertically,
//! `row` side by side, `grid` into a grid (near-square, or `cols` columns),
//! `free` overlays or positions them. **One pass, no measurement, no constraint
//! solver** — a deliberate boundary: when a layout needs negotiation, the
//! answer is explicit sizes.
//!
//! Along a `row`/`col` main axis the size resolves in **one order**: an
//! explicit ([`Place::w`] / [`Place::h`]) size takes exactly that; else an
//! explicit [`Place::weight`] takes that share of the leftover (the escape
//! hatch that still stretches a button); else the widget's **natural size**
//! ([`WidgetKind::natural_size`] — a pure function of the metrics, never of the
//! widget's data) takes exactly what it wants; else the child shares the
//! leftover at weight 1. So a `col` of controls is a stack of control-high rows
//! with the leftover under them, and a `col` of surfaces still splits evenly.
//! The cross axis fills. In `free`,
//! a child with any of `x`/`y`/`w`/`h` positions absolutely inside the
//! container (missing size = the rest of the area); a child with none keeps
//! the full-area overlay. A container's [`Flow`] tunes the inner `margin`, the
//! `gap` between children, and the `grid` column count.
//!
//! **Two pixel spaces, and this pass knows which is which.** The window's
//! chrome is **logical**: the wire's `w`/`h`/`x`/`y`/`margin`/`gap` are the
//! numbers a script wrote, and they reach physical pixels through the placement
//! [`Space`]'s scale — the window's `ui_scale`, carried by the resolved
//! [`Metrics`] the pass is handed. Inside a `scroll` **workspace** that scale
//! drops to 1: a navigable plane keeps its own units, like the heavy views'
//! `render_width_px`, because it has a zoom of its own and its pan is written
//! in the pixels the pointer moves. So a strip declared `h: 28` is 28 logical
//! pixels of chrome on any display, while a box placed at `x: 400` on a
//! patcher's plane sits at content coordinate 400 and reaches pixels through
//! the plane's zoom — which *defaults* to the window's scale
//! ([`ScrollView::zoom`]), since a plane's content unit is a display unit.
//!
//! [`ScrollView::zoom`]: super::widget::ScrollView::zoom

use super::metrics::Metrics;
use super::scroll;
use super::widget::{Flow, Layout, Place, Widget, WidgetKind};

/// A rectangle in physical pixels, top-left origin.
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

    /// Whether `(px, py)` (physical pixels) falls inside the rectangle.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (px, py) = (px as f32, py as f32);
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// The intersection with `other` (zero-sized when they do not overlap).
    pub fn intersect(&self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Rect {
            x,
            y,
            w: ((self.x + self.w).min(other.x + other.w) - x).max(0.0),
            h: ((self.y + self.h).min(other.y + other.h) - y).max(0.0),
        }
    }
}

/// The space a widget is placed in: how the wire's own lengths reach physical
/// pixels, and the zoom they are seen through.
///
/// `unit` is the logical -> physical multiplier for the declared `w`/`h`/`x`/`y`/
/// `margin`/`gap` — the window's `ui_scale` for the chrome, `1.0` once inside a
/// `scroll` workspace, whose plane is physical. `zoom` is the product of the
/// enclosing workspaces' zooms. [`Placed::scale`] is their product, so text
/// follows both.
#[derive(Debug, Clone, Copy)]
struct Space {
    unit: f32,
    zoom: f32,
}

impl Space {
    /// The window's own space: the wire is logical, nothing is zoomed.
    fn window(metrics: &Metrics) -> Self {
        Self {
            unit: metrics.ui_scale,
            zoom: 1.0,
        }
    }

    /// The space inside a `scroll` workspace at `zoom`: the wire's lengths there
    /// are the plane's own content units, and the zoom — the plane's, defaulting
    /// to the window's scale — is what turns them into pixels. It composes with
    /// any enclosing workspace's.
    fn plane(self, zoom: f32) -> Self {
        Self {
            unit: 1.0,
            zoom: self.zoom * zoom,
        }
    }

    /// One declared length in physical pixels.
    fn px(self, logical: f32) -> f32 {
        super::metrics::snap_px(logical, self.unit)
    }

    /// The scale text draws at here.
    fn scale(self) -> f32 {
        self.unit * self.zoom
    }
}

/// A widget and the rectangle it occupies. Emitted parent-before-child, so
/// drawing in order paints containers under their contents. `clip` is the
/// rectangle the widget must stay visually inside — `None` for the window
/// itself, the enclosing `scroll`'s area for anything scrolled (the renderer
/// clips its geometry to it, and hit-testing ignores the part outside it).
#[derive(Debug, Clone, Copy)]
pub struct Placed<'a> {
    pub rect: Rect,
    pub clip: Option<Rect>,
    /// The scale this widget's **text** is seen through: the window's
    /// `ui_scale` times the accumulated `scroll` zoom (nested workspaces
    /// compose; a workspace's plane is physical, so inside one it is the zoom
    /// alone). Text draws at `text_size * scale`, so a logical `text_size` is
    /// the same apparent size on any display and a zoomed box keeps its
    /// proportions; the rest of a workspace's interior deliberately keeps its
    /// physical-pixel metrics, the patcher posture.
    pub scale: f32,
    pub widget: &'a Widget,
}

/// Lays out `root` into `area` (physical pixels), returning every widget with
/// its rectangle. The spacing a container does not name itself comes from the
/// host's metrics (`margin`/`gap`), so one table sizes every window; what the
/// wire *does* name is logical and reaches physical pixels through that table's
/// `ui_scale` — pass the window's resolved table
/// ([`Host::metrics_for`](super::Host::metrics_for)), not the logical one.
pub fn layout<'a>(area: Rect, root: &'a Widget, metrics: &Metrics) -> Vec<Placed<'a>> {
    let mut out = Vec::new();
    place(area, root, None, Space::window(metrics), metrics, &mut out);
    out
}

fn place<'a>(
    area: Rect,
    widget: &'a Widget,
    clip: Option<Rect>,
    space: Space,
    metrics: &Metrics,
    out: &mut Vec<Placed<'a>>,
) {
    out.push(Placed {
        rect: area,
        clip,
        scale: space.scale(),
        widget,
    });
    let (layout, flow) = match widget.kind {
        WidgetKind::Window { layout, flow, .. } | WidgetKind::Panel { layout, flow } => {
            (layout, flow)
        }
        WidgetKind::Scroll { .. } => {
            return place_scrolled(area, widget, clip, space, metrics, out);
        }
        _ => return, // leaves have no children to place
    };
    let inner = area.inset(margin(flow, space, metrics));
    for (child, rect) in widget.children.iter().zip(child_rects(
        inner,
        widget.children.as_slice(),
        layout,
        flow,
        space,
        metrics,
    )) {
        place(rect, child, clip, space, metrics, out);
    }
}

/// A container's inner margin in physical pixels: its own declared (logical)
/// `margin` when it names one, else the resolved role.
fn margin(flow: Flow, space: Space, metrics: &Metrics) -> f32 {
    flow.margin.map_or(metrics.margin, |m| space.px(m)).max(0.0)
}

/// Places a `scroll` container's children: they lay out into the **virtual
/// content area** (content units, origin at the content's top-left) by the
/// container's ordinary layout, then each rectangle is transformed through the
/// view — offset by the pan, scaled by the zoom — into the window's pixels, and the
/// whole subtree is clipped to the container's area. The transform applies to
/// the direct children's rectangles; their own subtrees lay out normally
/// inside the transformed rects (so a zoom scales the placed boxes), and the
/// subtree's [`Placed::scale`] picks up the zoom so its **text** scales with
/// the boxes — the rest of the plane keeps its physical-pixel metrics.
///
/// The plane itself is **physical** ([`Space::plane`]): its content extent, its
/// pan and its children's declared positions are the units the gesture machine
/// pans in, not the window's logical ones.
fn place_scrolled<'a>(
    area: Rect,
    widget: &'a Widget,
    clip: Option<Rect>,
    space: Space,
    metrics: &Metrics,
    out: &mut Vec<Placed<'a>>,
) {
    let WidgetKind::Scroll { layout, flow, view } = widget.kind else {
        return;
    };
    let (content_w, content_h) = scroll_content(widget, area, metrics);
    let zoom = view.zoom(metrics);
    let slack = view.axis.slack();
    let vx = scroll::clamp_pan(view.view_x, area.w, zoom, content_w, slack);
    let vy = scroll::clamp_pan(view.view_y, area.h, zoom, content_h, slack);
    let space = space.plane(zoom as f32);
    let inner = Rect::new(0.0, 0.0, content_w, content_h).inset(margin(flow, space, metrics));
    let clip = Some(clip.map_or(area, |c| c.intersect(area)));
    for (child, r) in widget.children.iter().zip(child_rects(
        inner,
        widget.children.as_slice(),
        layout,
        flow,
        space,
        metrics,
    )) {
        let rect = Rect::new(
            area.x + ((r.x as f64 - vx) * zoom) as f32,
            area.y + ((r.y as f64 - vy) * zoom) as f32,
            (r.w as f64 * zoom) as f32,
            (r.h as f64 * zoom) as f32,
        );
        place(rect, child, clip, space, metrics, out);
    }
}

/// A `scroll` container's virtual content size in content units: the explicit
/// `content_w`/`content_h` when given; else, for the `free` layout, the
/// children's placement extents (`x + w` / `y + h`, plus the margin all
/// around); else the container's own area (the workspace degenerates to a
/// plain panel at zoom 1). Pure, so the gesture layer clamps against the same
/// size the layout renders.
pub fn scroll_content(widget: &Widget, area: Rect, metrics: &Metrics) -> (f32, f32) {
    let WidgetKind::Scroll { layout, flow, view } = widget.kind else {
        return (area.w, area.h);
    };
    let margin = flow.margin.unwrap_or(metrics.margin).max(0.0);
    let extent = |pos: fn(Place) -> Option<f32>, size: fn(Place) -> Option<f32>| {
        widget
            .children
            .iter()
            .map(|c| c.place)
            .filter(|p| pos(*p).is_some() || size(*p).is_some())
            .map(|p| pos(p).unwrap_or(0.0) + size(p).unwrap_or(0.0))
            .fold(None, |acc: Option<f32>, e| {
                Some(acc.map_or(e, |a| a.max(e)))
            })
            .map(|e| e + 2.0 * margin)
    };
    // A child with an **intrinsic size** (a `patch`, whose graph the host lays
    // out) drives the content: the workspace sizes to the graph but never below
    // the viewport, so a small graph centres in the window and a large one fills
    // the content and pans. An explicit `content_w`/`content_h` still overrides.
    if let Some((nw, nh)) = widget.children.iter().find_map(child_intrinsic_size) {
        return (
            view.content_w.unwrap_or(nw.max(area.w)).max(1.0),
            view.content_h.unwrap_or(nh.max(area.h)).max(1.0),
        );
    }
    let free = layout == Layout::Free;
    (
        view.content_w
            .or_else(|| free.then(|| extent(|p| p.x, |p| p.w)).flatten())
            .unwrap_or(area.w)
            .max(1.0),
        view.content_h
            .or_else(|| free.then(|| extent(|p| p.y, |p| p.h)).flatten())
            .unwrap_or(area.h)
            .max(1.0),
    )
}

/// A child's intrinsic content size, if it has one the host computes rather than
/// the wire declaring — today a `patch`, whose graph the host lays out
/// ([`super::patch::natural_size`]). Drives a scroll workspace's content extent.
fn child_intrinsic_size(widget: &Widget) -> Option<(f32, f32)> {
    match &widget.kind {
        WidgetKind::Patch { patch, .. } => Some(super::patch::natural_size(patch)),
        _ => None,
    }
}

/// The child rectangles for `children` laid out in `inner` by `layout`.
fn child_rects(
    inner: Rect,
    children: &[Widget],
    layout: Layout,
    flow: Flow,
    space: Space,
    metrics: &Metrics,
) -> Vec<Rect> {
    if children.is_empty() {
        return Vec::new();
    }
    let gap = flow.gap.map_or(metrics.gap, |g| space.px(g)).max(0.0);
    match layout {
        Layout::Free => children
            .iter()
            .map(|c| free_rect(inner, c.place, space))
            .collect(),
        Layout::Row => strip(inner, children, gap, true, space, metrics),
        Layout::Col => strip(inner, children, gap, false, space, metrics),
        Layout::Grid => grid(inner, children.len(), gap, flow.cols),
    }
}

/// A `free` child's rectangle: absolute `x`/`y`/`w`/`h` inside `inner` when
/// any is given (missing position = the container's origin, missing size = the
/// rest of the area), the full-area overlay when none is.
fn free_rect(inner: Rect, p: Place, space: Space) -> Rect {
    if p.x.is_none() && p.y.is_none() && p.w.is_none() && p.h.is_none() {
        return inner;
    }
    let (x, y) = (space.px(p.x.unwrap_or(0.0)), space.px(p.y.unwrap_or(0.0)));
    Rect::new(
        inner.x + x,
        inner.y + y,
        p.w.map_or(inner.w - x, |w| space.px(w)).max(0.0),
        p.h.map_or(inner.h - y, |h| space.px(h)).max(0.0),
    )
}

/// Cells along one axis (`horizontal` = a row, else a column), resolved in the
/// one order: an explicit main-axis size (`w` in a row, `h` in a column) is
/// taken as given; an explicit `weight` takes that share of the leftover; a
/// widget with a natural size on this axis takes exactly that; everything else
/// shares the leftover at weight 1. The cross axis fills.
fn strip(
    inner: Rect,
    children: &[Widget],
    gap: f32,
    horizontal: bool,
    space: Space,
    metrics: &Metrics,
) -> Vec<Rect> {
    let gaps = gap * (children.len() as f32 - 1.0);
    let main = if horizontal { inner.w } else { inner.h };
    let fixed_of = |c: &Widget| {
        let p = c.place;
        let explicit = if horizontal { p.w } else { p.h };
        explicit.map(|s| space.px(s)).or_else(|| {
            // An explicit weight overrides the natural size — "stretch this
            // button" stays expressible.
            p.weight.is_none().then(|| {
                let (nw, nh) = c.kind.natural_size(metrics);
                if horizontal { nw } else { nh }
            })?
        })
    };
    let fixed: f32 = children
        .iter()
        .filter_map(fixed_of)
        .map(|s| s.max(0.0))
        .sum();
    let total_weight: f32 = children
        .iter()
        .filter(|c| fixed_of(c).is_none())
        .map(|c| c.place.weight.unwrap_or(1.0).max(0.0))
        .sum();
    let leftover = (main - gaps - fixed).max(0.0);
    let share = |c: &Widget| match fixed_of(c) {
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
        // Two elastic surfaces: nothing knows its own height, so they share.
        let w = tree(
            r#"{"type":"window","layout":"col","children":[
            {"id":1,"type":"panel"},{"id":2,"type":"panel"}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
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
        let placed = layout(area(), &w, &Metrics::default());
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert!(a.x < b.x, "row places side by side");
        assert!((a.y - b.y).abs() < 1e-3, "same y in a row");
    }

    #[test]
    fn single_child_fills_inset_area() {
        let w = tree(r#"{"type":"window","children":[{"id":12,"type":"waveform","data":[]}]}"#);
        let placed = layout(area(), &w, &Metrics::default());
        assert_eq!(placed.len(), 2);
        let r = placed[1].rect;
        // Inset by the margin role on each side.
        let margin = Metrics::default().margin;
        assert!((r.x - margin).abs() < 1e-3 && (r.y - margin).abs() < 1e-3);
        assert!((r.w - (600.0 - 2.0 * margin)).abs() < 1e-3);
    }

    #[test]
    fn grid_of_four_is_two_by_two() {
        let w = tree(
            r#"{"type":"window","layout":"grid","children":[
            {"id":1,"type":"label","text":"a"},{"id":2,"type":"label","text":"b"},
            {"id":3,"type":"label","text":"c"},{"id":4,"type":"label","text":"d"}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
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
        let placed = layout(area(), &w, &Metrics::default());
        let (a, b, c) = (placed[1].rect, placed[2].rect, placed[3].rect);
        assert_eq!(a.w, 100.0, "fixed child takes exactly its w");
        // Leftover 500 split 3:1.
        assert!((b.w - 375.0).abs() < 1e-3, "weight 3 takes 3/4 of the rest");
        assert!((c.w - 125.0).abs() < 1e-3, "weight defaults to 1");
        assert_eq!(c.x + c.w, 600.0, "the row fills the area");
    }

    /// The one resolution order, branch by branch: explicit size, explicit
    /// weight, natural size, then a share of the leftover.
    #[test]
    fn the_main_axis_resolves_in_one_order() {
        let m = Metrics::default();
        let w = tree(
            r#"{"type":"window","layout":"col","margin":0,"gap":0,"children":[
            {"id":1,"type":"button","label":"fixed","h":50},
            {"id":2,"type":"button","label":"stretched","weight":2},
            {"id":3,"type":"button","label":"natural"},
            {"id":4,"type":"panel"}]}"#,
        );
        let placed = layout(area(), &w, &m);
        let (fixed, weighted, natural, elastic) = (
            placed[1].rect,
            placed[2].rect,
            placed[3].rect,
            placed[4].rect,
        );
        assert_eq!(fixed.h, 50.0, "an explicit size is taken as given");
        let control_h = WidgetKind::Button {
            label: None,
            text_size: crate::host::font::DEFAULT_SIZE,
        }
        .natural_size(&m)
        .1
        .unwrap();
        assert_eq!(natural.h, control_h, "the natural size is taken as wanted");
        // The leftover (400 - 50 - the natural row) splits 2:1 between the
        // weighted button — its weight beats its own natural size, the escape
        // hatch that still stretches a control — and the elastic panel.
        let leftover = 400.0 - 50.0 - control_h;
        assert!((weighted.h - leftover * 2.0 / 3.0).abs() < 1e-3);
        assert!((elastic.h - leftover / 3.0).abs() < 1e-3);
        assert!(
            (elastic.y + elastic.h - 400.0).abs() < 1e-3,
            "the column still fills"
        );
        // The cross axis fills regardless.
        assert!(placed[1..].iter().all(|p| p.rect.w == 600.0));
    }

    #[test]
    fn an_all_natural_strip_leaves_the_leftover_empty() {
        // Nothing elastic to absorb it: the controls stack at their own size
        // and the rest of the column stays empty, rather than everyone growing.
        let w = tree(
            r#"{"type":"window","layout":"col","margin":0,"gap":0,"children":[
            {"id":1,"type":"button","label":"a"},{"id":2,"type":"button","label":"b"}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert_eq!(a.h, b.h);
        assert!(b.y + b.h < 400.0 * 0.5, "the column is not filled");
    }

    #[test]
    fn a_natural_size_only_binds_its_own_axis() {
        // A row of controls: a button knows its height, not its width, so the
        // row's main axis (x) still splits evenly and the cross axis fills.
        let w = tree(
            r#"{"type":"window","layout":"row","margin":0,"gap":0,"children":[
            {"id":1,"type":"button","label":"a"},{"id":2,"type":"button","label":"b"}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert_eq!((a.w, b.w), (300.0, 300.0));
        assert_eq!(a.h, 400.0, "the cross axis fills");
    }

    #[test]
    fn the_density_moves_a_natural_row() {
        let json = r#"{"type":"window","layout":"col","margin":0,"gap":0,"children":[
            {"id":1,"type":"button","label":"a"},{"id":2,"type":"panel"}]}"#;
        let (small, big) = (tree(json), tree(json));
        let compact = layout(area(), &small, &Metrics::generated(0.75));
        let comfortable = layout(area(), &big, &Metrics::generated(1.5));
        assert!(
            comfortable[1].rect.h > compact[1].rect.h,
            "one table sizes the strip"
        );
    }

    /// The wire is logical: the same tree on a 2x window is the same shell at
    /// twice the size — the declared strips, the declared gap and the natural
    /// row all double, and the work surface still takes the rest.
    #[test]
    fn a_scaled_window_doubles_the_declared_chrome() {
        let json = r#"{"type":"window","layout":"col","margin":0,"gap":10,"children":[
            {"id":11,"type":"panel","h":28},
            {"id":12,"type":"button","label":"natural"},
            {"id":13,"type":"panel"}]}"#;
        let m = Metrics::default();
        let (one, two) = (tree(json), tree(json));
        let plain = layout(area(), &one, &m);
        let hidpi = layout(Rect::new(0.0, 0.0, 1200.0, 800.0), &two, &m.resolved(2.0));
        assert_eq!(plain[1].rect.h, 28.0);
        assert_eq!(hidpi[1].rect.h, 56.0, "a declared strip is logical");
        assert_eq!(
            hidpi[2].rect.h,
            plain[2].rect.h * 2.0,
            "the natural row follows the resolved table"
        );
        assert_eq!(
            hidpi[2].rect.y,
            plain[2].rect.y * 2.0,
            "the gap doubles too"
        );
        assert_eq!(
            hidpi[3].rect.y + hidpi[3].rect.h,
            800.0,
            "the surface still fills the window"
        );
        // Text is logical as well: the placement scale carries the window's.
        assert_eq!(plain[2].scale, 1.0);
        assert_eq!(hidpi[2].scale, 2.0);
    }

    #[test]
    fn a_scaled_free_placement_scales_position_and_size() {
        let json = r#"{"type":"window","layout":"free","margin":4,"children":[
            {"id":1,"type":"label","text":"a","x":50,"y":30,"w":120,"h":40}]}"#;
        let w = tree(json);
        let placed = layout(area(), &w, &Metrics::default().resolved(2.0));
        let r = placed[1].rect;
        assert_eq!((r.x, r.y), (8.0 + 100.0, 8.0 + 60.0), "margin and position");
        assert_eq!((r.w, r.h), (240.0, 80.0));
    }

    /// A `scroll` workspace's plane keeps its **own units**: the wire's lengths
    /// there are content units, and what turns them into pixels is the plane's
    /// zoom — which *defaults* to the window's scale, because a plane's content
    /// unit is a display unit (a patcher's box is 96 units wide because that is
    /// how wide a box should look).
    #[test]
    fn a_workspace_plane_scales_by_its_zoom_and_defaults_to_the_density() {
        let json = r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","margin":0,"content_w":2000,"content_h":2000,
             "children":[{"id":7,"type":"label","text":"a","x":100,"y":50,"w":80,"h":40}]}]}"#;
        let w = tree(json);
        let placed = layout(area(), &w, &Metrics::default().resolved(2.0));
        let child = placed.iter().find(|p| p.widget.id == Some(7)).unwrap();
        assert_eq!((child.rect.x, child.rect.y), (200.0, 100.0));
        assert_eq!((child.rect.w, child.rect.h), (160.0, 80.0));
        assert_eq!(child.scale, 2.0, "the plane's text rides its zoom");
    }

    #[test]
    fn a_named_plane_zoom_is_literal_at_any_density() {
        // The script said one physical pixel per content unit, so that is what
        // it gets on a doubled display too — the default is the density, a
        // named number is the number.
        let json = r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","margin":0,"view_zoom":1,
             "content_w":2000,"content_h":2000,
             "children":[{"id":7,"type":"label","text":"a","x":100,"y":50,"w":80,"h":40}]}]}"#;
        let w = tree(json);
        let placed = layout(area(), &w, &Metrics::default().resolved(2.0));
        let child = placed.iter().find(|p| p.widget.id == Some(7)).unwrap();
        assert_eq!((child.rect.x, child.rect.y), (100.0, 50.0));
        assert_eq!((child.rect.w, child.rect.h), (80.0, 40.0));
        assert_eq!(child.scale, 1.0);
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
        let placed = layout(area(), &w, &Metrics::default());
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
        let placed = layout(area(), &w, &Metrics::default());
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
        let placed = layout(area(), &w, &Metrics::default());
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
        let placed = layout(area(), &w, &Metrics::default());
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
        let placed = layout(area(), &w, &Metrics::default());
        assert_eq!(
            placed[2].rect.w, 0.0,
            "no leftover: the flexible child collapses"
        );
    }

    /// The `scroll` container's own placed entry, and its children's.
    fn scrolled(w: &Widget) -> (Rect, Vec<(Rect, Option<Rect>)>) {
        let placed = layout(area(), w, &Metrics::default());
        let scroll = placed
            .iter()
            .find(|p| matches!(p.widget.kind, WidgetKind::Scroll { .. }))
            .expect("a scroll in the tree");
        let children = placed
            .iter()
            .filter(|p| p.clip.is_some())
            .map(|p| (p.rect, p.clip))
            .collect();
        (scroll.rect, children)
    }

    #[test]
    fn scroll_content_extents_come_from_the_free_placement() {
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","margin":10,"children":[
              {"id":1,"type":"label","text":"a","x":0,"y":0,"w":100,"h":40},
              {"id":2,"type":"label","text":"b","x":300,"y":500,"w":120,"h":60}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let scroll = &placed[1];
        // The extent is the far child's `x+w` / `y+h`, plus the margin both sides.
        assert_eq!(
            scroll_content(scroll.widget, scroll.rect, &Metrics::default()),
            (300.0 + 120.0 + 20.0, 500.0 + 60.0 + 20.0)
        );
    }

    #[test]
    fn explicit_content_size_wins_and_a_non_free_scroll_falls_back_to_its_area() {
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","content_w":2000,"content_h":1500,"children":[
              {"id":1,"type":"label","text":"a","x":0,"y":0,"w":10,"h":10}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        assert_eq!(
            scroll_content(placed[1].widget, placed[1].rect, &Metrics::default()),
            (2000.0, 1500.0)
        );
        // A `col` scroll has no placement extents: the content is its own area
        // (the workspace degenerates to a plain panel).
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","layout":"col","children":[
              {"id":1,"type":"label","text":"a"}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let s = &placed[1];
        assert_eq!(
            scroll_content(s.widget, s.rect, &Metrics::default()),
            (s.rect.w, s.rect.h)
        );
    }

    #[test]
    fn the_view_transform_offsets_and_scales_the_children() {
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","margin":0,"content_w":2000,"content_h":2000,
             "view_x":100,"view_y":50,"view_zoom":2,"children":[
              {"id":1,"type":"label","text":"a","x":100,"y":50,"w":80,"h":40}]}]}"#,
        );
        let (area, children) = scrolled(&w);
        let (rect, clip) = children[0];
        // The child sits exactly at the view origin, so it lands at the
        // container's top-left, scaled by the zoom.
        assert_eq!((rect.x, rect.y), (area.x, area.y));
        assert_eq!((rect.w, rect.h), (160.0, 80.0));
        assert_eq!(clip, Some(area), "the subtree clips to the container");
    }

    #[test]
    fn the_workspace_zoom_reaches_the_placement_scale() {
        // A scrolled subtree carries the zoom in `Placed::scale` (so its text
        // draws proportionally); nested workspaces compose, and everything
        // outside a workspace stays at 1.0.
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":15,"type":"label","text":"outside"},
            {"id":19,"type":"scroll","margin":0,"view_zoom":2,"content_w":1000,"content_h":1000,
             "children":[
              {"id":11,"type":"label","text":"a","x":0,"y":0,"w":80,"h":40},
              {"id":12,"type":"scroll","margin":0,"view_zoom":0.5,"x":100,"y":0,"w":200,"h":200,
               "content_w":400,"content_h":400,"children":[
                {"id":13,"type":"label","text":"b","x":0,"y":0,"w":80,"h":40}]}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let scale_of = |id: i32| {
            placed
                .iter()
                .find(|p| p.widget.id == Some(id))
                .unwrap()
                .scale
        };
        assert_eq!(scale_of(15), 1.0, "outside any workspace");
        assert_eq!(scale_of(11), 2.0, "the workspace zoom");
        assert_eq!(scale_of(13), 1.0, "nested zooms compose (2 * 0.5)");
    }

    #[test]
    fn a_bounded_axis_pans_no_further_than_its_content() {
        // A constrained scroll view (`axis: "x"`, so no slack) whose content is
        // only as tall as the viewport: x clamps at content - visible =
        // 2000 - 600 = 1400, and y — not a pannable axis here — stays put.
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","margin":0,"axis":"x","zoom":0,
             "content_w":2000,"content_h":100,
             "view_x":9999,"view_y":9999,"children":[
              {"id":1,"type":"label","text":"a","x":0,"y":0,"w":10,"h":10}]}]}"#,
        );
        let (area, children) = scrolled(&w);
        let (rect, _) = children[0];
        assert_eq!(rect.x, area.x - 1400.0);
        assert_eq!(rect.y, area.y);
    }

    #[test]
    fn the_free_plane_pans_half_a_viewport_past_its_content() {
        // The same content on the *free* plane: it is unbounded, so it
        // overscrolls by half the visible size on each axis — 1400 + 300 in x,
        // and in y (where the content is shorter than the pane) it can still
        // be pushed by half a viewport instead of pinning at the corner.
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","margin":0,"content_w":2000,"content_h":100,
             "view_x":9999,"view_y":9999,"children":[
              {"id":1,"type":"label","text":"a","x":0,"y":0,"w":10,"h":10}]}]}"#,
        );
        let (area, children) = scrolled(&w);
        let (rect, _) = children[0];
        assert_eq!(rect.x, area.x - (1400.0 + area.w * 0.5));
        assert_eq!(rect.y, area.y - area.h * 0.5);
    }

    #[test]
    fn a_scrolled_child_keeps_its_own_layout_inside_the_transformed_rect() {
        let w = tree(
            r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"scroll","margin":0,"content_w":1000,"content_h":1000,"children":[
              {"id":5,"type":"panel","layout":"row","margin":0,"gap":0,
               "x":0,"y":0,"w":200,"h":100,"children":[
                 {"id":2,"type":"label","text":"a"},{"id":3,"type":"label","text":"b"}]}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let inner: Vec<Rect> = placed
            .iter()
            .filter(|p| matches!(p.widget.id, Some(2) | Some(3)))
            .map(|p| p.rect)
            .collect();
        assert_eq!(inner[0].w, 100.0, "the panel splits its transformed rect");
        assert!(inner[1].x > inner[0].x);
        assert!(
            placed
                .iter()
                .filter(|p| matches!(p.widget.id, Some(5) | Some(2) | Some(3)))
                .all(|p| p.clip.is_some()),
            "the whole subtree inherits the container's clip"
        );
    }

    #[test]
    fn nested_panel_recurses() {
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"panel","children":[{"id":12,"type":"waveform","data":[]}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        // [window, panel, waveform]
        assert_eq!(placed.len(), 3);
        assert!(placed[2].widget.is_waveform());
        // The waveform sits inside the panel's rect.
        let (panel, wave) = (placed[1].rect, placed[2].rect);
        assert!(wave.x >= panel.x && wave.y >= panel.y);
    }
}
