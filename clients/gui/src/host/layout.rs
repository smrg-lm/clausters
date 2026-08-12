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
//! ([`Widget::natural_size`] — a pure function of the metrics, and of the
//! widget's content only where a container asked to be fitted to it) takes
//! exactly what it wants; else the child shares the
//! leftover at weight 1. So a `col` of controls is a stack of control-high rows
//! with the leftover under them, and a `col` of surfaces still splits evenly.
//! A container is one of those surfaces unless it carries **`hug`**, in which
//! case it wants its content ([`Widget::hug_size`]).
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

use std::collections::HashMap;

use crate::viewport::View;

use super::metrics::Metrics;
use super::scroll;
use super::timeline::{self, GroupKey, group_key};
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

/// The space a widget is placed in: how the wire's own lengths become this
/// space's coordinates, the scale everything in it is drawn at, and the size
/// table resolved there.
///
/// Three spaces exist, and every widget is in exactly one:
///
/// - **The window.** `unit` is the window's `ui_scale`, so a declared `h: 28` is
///   28 logical pixels; `scale` is the same, so the metrics and the text are the
///   window's own.
/// - **A workspace's content plane**, used to place a `scroll`'s direct children:
///   `unit` and `scale` are 1, because those placements are *content units* and
///   the whole rectangle is scaled by the plane's zoom afterwards. The table
///   there is the logical one, so a metric default (a `margin`, a `gap`) and a
///   natural size come out in content units too.
/// - **Inside a scrolled child**, whose rectangle is already scaled: `unit` and
///   `scale` are the product of the zooms over it. So its own children's declared
///   lengths, its metrics and its text all carry that zoom together — a zoomed
///   box is an enlargement of itself, not a box with oversized text in it.
#[derive(Debug, Clone, Copy)]
struct Space {
    /// A declared length, into this space's coordinates — and the scale the
    /// space is drawn at, since the two are the same number: text draws at
    /// `text_size * unit` and [`Space::metrics`] is resolved there.
    unit: f32,
    /// The accumulated zoom of the workspaces over this space (1.0 in the
    /// window). Carried so a nested plane composes with the one around it.
    zooms: f32,
    /// The size table at [`Space::unit`].
    metrics: Metrics,
    /// The **time window** the container placed this widget on, when it is a
    /// time container's contents: a clip gets the slice of its own `[0, dur]`
    /// its (clamped) rectangle shows. `None` everywhere else — most of a window
    /// is not on a time axis at all.
    time: Option<View>,
}

impl Space {
    /// The window's own space.
    fn window(metrics: &Metrics) -> Self {
        Self {
            unit: metrics.ui_scale,
            zooms: 1.0,
            metrics: *metrics,
            time: None,
        }
    }

    /// The content plane of a `scroll`, for placing its direct children:
    /// **content units**, measured with the logical table, so what the plane's
    /// zoom multiplies afterwards is one coherent set of numbers.
    fn plane(self) -> Self {
        Self {
            unit: 1.0,
            zooms: self.zooms,
            metrics: self.metrics.at(1.0),
            time: self.time,
        }
    }

    /// Inside a scrolled child, whose rectangle the plane's zoom has already
    /// scaled: an ordinary space again, at the accumulated zoom instead of the
    /// window's scale.
    fn scrolled(self, zoom: f32) -> Self {
        let zooms = self.zooms * zoom;
        Self {
            unit: zooms,
            zooms,
            metrics: self.metrics.at(zooms),
            time: self.time,
        }
    }

    /// The same space on a time axis: the window a time container placed this
    /// widget on. What makes a clip's contents a coordinate system — they read
    /// this and their rectangle, never the lane's gutter or the group's window.
    fn on_time(self, view: View) -> Self {
        Self {
            time: Some(view),
            ..self
        }
    }

    /// One declared length in this space's coordinates.
    fn px(self, declared: f32) -> f32 {
        super::metrics::snap_px(declared, self.unit)
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
    /// The scale this widget is seen at: the window's `ui_scale` outside any
    /// workspace, the accumulated `scroll` zoom inside one (nested workspaces
    /// compose). Text draws at `text_size * scale` and
    /// [`metrics`](Self::metrics) is resolved there, so a logical `text_size` is
    /// the same apparent size on any display and a zoomed box is an
    /// **enlargement of itself** — padding, parts and text together — rather
    /// than a box with oversized text in it.
    pub scale: f32,
    /// The size table this widget is drawn and hit-tested with: the host's,
    /// resolved at [`Placed::scale`] (see [`Metrics::at`]). The same table the
    /// layout measured it with, so a zoomed widget's parts keep their
    /// proportions instead of growing only where text is involved.
    pub metrics: Metrics,
    /// Where this widget's navigation group starts its bodies inside a
    /// member's rect — the shared gutter of the axis it is on, `0` for anything
    /// that is not on one. Resolved once per window here, because the renderer
    /// and the hit-test must agree on it and both read this vector.
    ///
    /// It is `0` inside a time container too, whatever group the child is on:
    /// the gutter is the *container's*, and a lane's body already starts past
    /// it — a member drawn in there (a clip's take, a heavy view used as a
    /// lane's body) would otherwise indent by it a second time.
    pub indent: f32,
    /// The visible window of the **time axis this placement's rectangle spans**,
    /// when its container placed it on one: a clip carries the slice of its own
    /// `[0, dur]` that its (clamped) rectangle shows, so everything drawn or hit
    /// inside it maps through `(rect, time)` alone. `None` for anything not on a
    /// time axis. Resolved here because the renderer and the hit-test must agree
    /// on it and both read this vector.
    pub time: Option<View>,
    /// The index of this widget's container in the returned vector, `None` for
    /// the root. The pass emits parent-before-child, so an ancestry is walked
    /// back from any placement without searching the tree for it: it is the
    /// containment the layout already knows, kept instead of thrown away.
    pub parent: Option<usize>,
    pub widget: &'a Widget,
}

/// Where a **time container** gets the window it places its contents through:
/// a `track` places its clips on its navigation group's visible window, so the
/// layout of a multitrack is a function of where the axis currently stands.
///
/// It is a seam rather than a lookup because the groups live on the `Host` and
/// this pass is pure geometry; a caller with no groups (a test, a measurement)
/// passes [`NoAxis`] and every time container falls back to its own content
/// span, which is what an un-navigated lane shows anyway.
pub trait AxisSource {
    /// The visible window of the group member `id` belongs to.
    fn nav(&self, id: i32, link: Option<i32>) -> Option<View>;
}

/// An axis source that knows nothing: every time container falls back to its
/// own full content span.
pub struct NoAxis;

impl AxisSource for NoAxis {
    fn nav(&self, _id: i32, _link: Option<i32>) -> Option<View> {
        None
    }
}

impl<F: Fn(i32, Option<i32>) -> Option<View>> AxisSource for F {
    fn nav(&self, id: i32, link: Option<i32>) -> Option<View> {
        self(id, link)
    }
}

/// Lays out `root` into `area` (physical pixels), returning every widget with
/// its rectangle. The spacing a container does not name itself comes from the
/// host's metrics (`margin`/`gap`), so one table sizes every window; what the
/// wire *does* name is logical and reaches physical pixels through that table's
/// `ui_scale` — pass the window's resolved table
/// ([`Host::metrics_for`](super::Host::metrics_for)), not the logical one.
pub fn layout<'a>(area: Rect, root: &'a Widget, metrics: &Metrics) -> Vec<Placed<'a>> {
    layout_on(area, root, metrics, &NoAxis)
}

/// [`layout`] with the navigation windows its time containers place on — the
/// form the renderer and the hit-test call, so a clip lands on the same pixels
/// for drawing and for dragging.
pub fn layout_on<'a>(
    area: Rect,
    root: &'a Widget,
    metrics: &Metrics,
    axis: &dyn AxisSource,
) -> Vec<Placed<'a>> {
    let floor = timeline::group_indents(root, metrics);
    let out = place_all(area, root, metrics, axis, floor.clone());
    // A value ruler's labels are a property of the data, so the width one needs
    // is only known once its member has a height. That is one pass too late, so
    // the members are measured and the pass is taken again — but only when the
    // measure asks for more than the roles reserved, which an ordinary window
    // never does.
    match timeline::measured_indents(&out, &floor) {
        Some(indents) => place_all(area, root, metrics, axis, indents),
        None => out,
    }
}

/// One layout pass with a settled gutter table.
fn place_all<'a>(
    area: Rect,
    root: &'a Widget,
    metrics: &Metrics,
    axis: &dyn AxisSource,
    indents: HashMap<GroupKey, f32>,
) -> Vec<Placed<'a>> {
    let mut out = Vec::new();
    let ctx = Ctx {
        metrics,
        axis,
        indents,
    };
    place(
        area,
        root,
        None,
        Space::window(metrics),
        &ctx,
        None,
        &mut out,
        None,
    );
    out
}

/// What one layout pass carries besides its recursion state: the window's size
/// table, where each navigation group starts its bodies, and the axis those
/// groups currently stand at.
struct Ctx<'x> {
    metrics: &'x Metrics,
    axis: &'x dyn AxisSource,
    indents: HashMap<GroupKey, f32>,
}

impl Ctx<'_> {
    /// The shared gutter of the group `widget` is on (0 when it is on none).
    fn indent(&self, widget: &Widget) -> f32 {
        let Some((id, editor)) = widget.id.zip(widget.kind.editor()) else {
            return 0.0;
        };
        self.indents
            .get(&group_key(id, editor.link))
            .copied()
            .unwrap_or(0.0)
    }
}

/// Places `widget` at `area` and recurses into its children.
///
/// `indent` overrides the gutter stamped on the placement: `None` asks the
/// group table (the ordinary case), `Some(0.0)` says the container already took
/// the gutter out of the rectangle it is handing down, so its contents draw
/// from the rectangle's own left edge.
#[allow(clippy::too_many_arguments)] // one recursion's state, all by value
fn place<'a>(
    area: Rect,
    widget: &'a Widget,
    clip: Option<Rect>,
    space: Space,
    ctx: &Ctx,
    parent: Option<usize>,
    out: &mut Vec<Placed<'a>>,
    indent: Option<f32>,
) {
    let me = out.len();
    out.push(Placed {
        rect: area,
        clip,
        scale: space.unit,
        metrics: space.metrics,
        indent: indent.unwrap_or_else(|| ctx.indent(widget)),
        time: space.time,
        parent,
        widget,
    });
    let (layout, flow) = match widget.kind {
        WidgetKind::Window { layout, flow, .. } | WidgetKind::Panel { layout, flow, .. } => {
            (layout, flow)
        }
        WidgetKind::Scroll { .. } => {
            return place_scrolled(area, widget, clip, space, ctx, me, out);
        }
        // One child at a time: the shown page fills the container, and the
        // hidden ones are not placed at all — no rectangle, so nothing draws
        // them and nothing hits them. They keep their place in the *tree*
        // (their GPU slots and bus watches are collected from there), which is
        // what makes flipping back free.
        WidgetKind::Stack { index, margin, .. } => {
            let inner = area.inset(
                margin
                    .map_or(space.metrics.margin, |m| space.px(m))
                    .max(0.0),
            );
            if let Some(child) = usize::try_from(index)
                .ok()
                .and_then(|i| widget.children.get(i))
            {
                place(inner, child, clip, space, ctx, Some(me), out, None);
            }
            return;
        }
        // The time containers: a lane places its clips on the shared axis, a
        // clip places its bodies on its own local one.
        WidgetKind::Track { .. } | WidgetKind::Clip { .. } => {
            return place_on_time(area, widget, clip, space, ctx, me, out);
        }
        _ => return, // leaves have no children to place
    };
    let inner = area.inset(margin(flow, space));
    for (child, rect) in widget.children.iter().zip(child_rects(
        inner,
        widget.children.as_slice(),
        layout,
        flow,
        space,
    )) {
        place(rect, child, clip, space, ctx, Some(me), out, None);
    }
}

/// Places the contents of a **time container**: the one place a coordinate
/// system made of a visible window and a placement becomes rectangles.
///
/// A `track` puts each `clip` child at its `[offset, offset + dur]` span on the
/// group's window, inside the lane body (which starts at the axis' shared
/// indent, and reserves the lane's own ruler strip at the bottom). A `clip`
/// gives each of its own children the whole clip rect: its bodies **layer** —
/// a curve over the notes over the take — rather than dividing the box, and
/// each reads the clip's local axis `[0, dur]`, which is why a clip lifted into
/// another parent draws the same without re-deriving anything.
///
/// A clip outside the visible window is placed empty (zero width) rather than
/// skipped: the tree and the placement vector stay parallel, so `Placed::parent`
/// keeps meaning what it says.
fn place_on_time<'a>(
    area: Rect,
    widget: &'a Widget,
    clip: Option<Rect>,
    space: Space,
    ctx: &Ctx,
    me: usize,
    out: &mut Vec<Placed<'a>>,
) {
    let body = match &widget.kind {
        WidgetKind::Track { editor, .. } => {
            let ruler_on = editor.ruler != super::widget::Ruler::Off;
            crate::host::graphics::track::lane_body(
                area,
                ruler_on,
                ctx.indent(widget),
                &space.metrics,
            )
        }
        // A clip's own box is the coordinate system its bodies fill.
        _ => area,
    };
    let nav = match &widget.kind {
        WidgetKind::Track { editor, .. } => widget
            .id
            .and_then(|id| ctx.axis.nav(id, editor.link))
            .unwrap_or_else(|| crate::host::graphics::track::window_nav(widget)),
        // The clip's own axis: the slice of its span its rectangle shows,
        // handed down by the lane that placed it (its whole span when nothing
        // did — a clip outside a lane, or a measurement pass with no groups).
        WidgetKind::Clip { dur, .. } => space
            .time
            .unwrap_or_else(|| View::full(dur.ceil().max(1.0) as usize)),
        _ => return,
    };
    for child in &widget.children {
        let (rect, inner) = match (&widget.kind, &child.kind) {
            (WidgetKind::Track { .. }, WidgetKind::Clip { offset, dur, .. }) => {
                let rect =
                    match crate::host::graphics::track::clip_x_range(body, &nav, *offset, *dur) {
                        Some((x0, x1)) => crate::host::graphics::track::clip_rect(body, x0, x1),
                        None => Rect::new(body.x, body.y, 0.0, 0.0),
                    };
                // The lane hands the clip its own axis here, and that is the
                // last time the lane's window is mentioned: from the clip
                // inwards everything reads `(rect, time)`.
                let local =
                    crate::host::graphics::track::clip_local_view(body, &nav, *offset, *dur, rect);
                (rect, space.on_time(local))
            }
            // Anything else a time container holds fills its body: a clip's
            // layered bodies, and a lane's own non-clip chrome.
            _ => (body, space.on_time(nav)),
        };
        // The gutter is the *container's*: it was taken out of the lane's rect
        // to make this body, so a member drawn inside it must not take it
        // again. A clip's bodies carry no id and never asked for one; a heavy
        // view used as a lane's body does, and used to land a gutter's width
        // to the right of the clips it shares an axis with.
        place(rect, child, clip, inner, ctx, Some(me), out, Some(0.0));
    }
}

/// A container's inner margin in its space's coordinates: its own declared
/// `margin` when it names one, else the space's resolved role.
fn margin(flow: Flow, space: Space) -> f32 {
    flow.margin
        .map_or(space.metrics.margin, |m| space.px(m))
        .max(0.0)
}

/// Places a `scroll` container's children: they lay out into the **virtual
/// content area** (content units, origin at the content's top-left) by the
/// container's ordinary layout, then each rectangle is transformed through the
/// view — offset by the pan, scaled by the zoom — into the window's pixels, and the
/// whole subtree is clipped to the container's area. The transform applies to
/// the direct children's rectangles; their own subtrees then lay out in an
/// ordinary space at the accumulated zoom ([`Space::scrolled`]), so everything
/// inside a scrolled box — its declared lengths, its metric roles, its text —
/// carries that one factor.
///
/// The plane's own coordinates are **content units** ([`Space::plane`]): its
/// content extent, its pan and its children's declared positions are the units
/// the gesture machine pans in, not the window's logical ones.
#[allow(clippy::too_many_arguments)] // one recursion's state, all by value
fn place_scrolled<'a>(
    area: Rect,
    widget: &'a Widget,
    clip: Option<Rect>,
    space: Space,
    ctx: &Ctx,
    me: usize,
    out: &mut Vec<Placed<'a>>,
) {
    let WidgetKind::Scroll { layout, flow, view } = widget.kind else {
        return;
    };
    let metrics = ctx.metrics;
    let zoom = view.zoom(metrics);
    let (content_w, content_h) = scroll_content(widget, area, metrics);
    let slack = view.axis.slack();
    let vx = scroll::clamp_pan(view.view_x, area.w, zoom, content_w, slack);
    let vy = scroll::clamp_pan(view.view_y, area.h, zoom, content_h, slack);
    // Two spaces: the children are placed in the plane's content units, and
    // each one's subtree then lives in an ordinary space at the accumulated zoom.
    let plane = space.plane();
    let inner = Rect::new(0.0, 0.0, content_w, content_h).inset(margin(flow, plane));
    let inside = space.scrolled(zoom as f32);
    let clip = Some(clip.map_or(area, |c| c.intersect(area)));
    for (child, r) in widget.children.iter().zip(child_rects(
        inner,
        widget.children.as_slice(),
        layout,
        flow,
        plane,
    )) {
        let rect = Rect::new(
            area.x + ((r.x as f64 - vx) * zoom) as f32,
            area.y + ((r.y as f64 - vy) * zoom) as f32,
            (r.w as f64 * zoom) as f32,
            (r.h as f64 * zoom) as f32,
        );
        place(rect, child, clip, inside, ctx, Some(me), out, None);
    }
}

/// A `scroll` container's virtual content size in content units: the explicit
/// `content_w`/`content_h` when given; else, for the `free` layout, the
/// children's placement extents (`x + w` / `y + h`, plus the margin all
/// around); else the **viewport itself** (the workspace degenerates to a plain
/// panel). Pure, so the gesture layer clamps against the same size the layout
/// renders.
///
/// Every number here is a **content unit**, the viewport included: the pane's
/// pixels divided by `zoom`. Comparing a content extent against a pixel width
/// would place a plane's contents off by exactly that zoom — the bug that pushed
/// a centred patch graph into the corner once the zoom stopped defaulting to 1.
///
/// The conversion uses the plane's **natural** scale (its default zoom, the
/// window's density), never the zoom it is currently at — the content extent has
/// to be *constant under zooming*. A content that shrinks as the zoom grows
/// slides everything measured against it, so a wheel zoom stops holding the point
/// under the cursor: the pivot math is exact, but the plane it pivots in moves.
pub fn scroll_content(widget: &Widget, area: Rect, metrics: &Metrics) -> (f32, f32) {
    let WidgetKind::Scroll { layout, flow, view } = widget.kind else {
        return (area.w, area.h);
    };
    let margin = flow.margin.unwrap_or(metrics.margin).max(0.0);
    // The pane, in the plane's own units.
    let natural = metrics.ui_scale.max(0.01);
    let (visible_w, visible_h) = (area.w / natural, area.h / natural);
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
    // A child with an **intrinsic size** (a patcher, whose graph the host lays
    // out) drives the content: the workspace sizes to the graph but never below
    // the viewport, so a small graph centres in the window and a large one fills
    // the content and pans. An explicit `content_w`/`content_h` still overrides.
    if let Some((nw, nh)) = widget.children.iter().find_map(child_intrinsic_size) {
        return (
            view.content_w.unwrap_or(nw.max(visible_w)).max(1.0),
            view.content_h.unwrap_or(nh.max(visible_h)).max(1.0),
        );
    }
    let free = layout == Layout::Free;
    (
        view.content_w
            .or_else(|| free.then(|| extent(|p| p.x, |p| p.w)).flatten())
            .unwrap_or(visible_w)
            .max(1.0),
        view.content_h
            .or_else(|| free.then(|| extent(|p| p.y, |p| p.h)).flatten())
            .unwrap_or(visible_h)
            .max(1.0),
    )
}

/// A child's intrinsic content size, if it has one the host computes rather than
/// the wire declaring — today a patcher, whose graph the host lays out. Drives a
/// scroll workspace's content extent, asked of the element itself
/// ([`Element::content_size`](super::widget::Element::content_size)) rather than
/// derived from what it is.
fn child_intrinsic_size(widget: &Widget) -> Option<(f32, f32)> {
    match &widget.kind {
        WidgetKind::Custom(el) => el.content_size(),
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
) -> Vec<Rect> {
    if children.is_empty() {
        return Vec::new();
    }
    let gap = flow.gap.map_or(space.metrics.gap, |g| space.px(g)).max(0.0);
    match layout {
        Layout::Free => children
            .iter()
            .map(|c| free_rect(inner, c.place, space))
            .collect(),
        Layout::Row => strip(inner, children, gap, true, space),
        Layout::Col => strip(inner, children, gap, false, space),
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
fn strip(inner: Rect, children: &[Widget], gap: f32, horizontal: bool, space: Space) -> Vec<Rect> {
    let gaps = gap * (children.len() as f32 - 1.0);
    let main = if horizontal { inner.w } else { inner.h };
    let fixed_of = |c: &Widget| {
        let p = c.place;
        let explicit = if horizontal { p.w } else { p.h };
        explicit.map(|s| space.px(s)).or_else(|| {
            // An explicit weight overrides the natural size — "stretch this
            // button" stays expressible.
            p.weight.is_none().then(|| {
                // Measured in this space's own coordinates, at the scale its
                // text will draw at — one table, one scale.
                let (nw, nh) = c.natural_size(&space.metrics, space.unit);
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

    /// The application shell, without the number: a strip of controls under a
    /// work surface used to need an `h` nobody could derive, because a
    /// container did not measure what it held. With `hug` it takes exactly its
    /// content and the view keeps the rest — and the resolution order is
    /// unchanged, this is just another natural size.
    #[test]
    fn a_hugging_strip_takes_its_content_and_the_view_keeps_the_rest() {
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"signal","view":"trace","data":[]},
            {"id":6,"type":"layout","flow":"row","hug":1,"children":[
                {"id":7,"type":"menu","options":["time","samples","beats"],"label":"time axis"},
                {"id":8,"type":"toggle","label":"rulers"}]}]}"#,
        );
        let m = Metrics::default();
        let placed = layout(area(), &w, &m);
        let rect = |id: i32| {
            placed
                .iter()
                .find(|p| p.widget.id == Some(id))
                .unwrap()
                .rect
        };

        let strip = rect(6);
        let menu = w.children[1].children[0].hug_size(&m, 1.0).1.unwrap();
        assert_eq!(strip.h, menu + 2.0 * m.margin, "the strip is its content");
        // The view takes everything the strip did not.
        let view = rect(5);
        assert_eq!(view.h + m.gap + strip.h, area().h - 2.0 * m.margin);
        assert!(
            view.h > strip.h * 3.0,
            "and it dominates: {view:?} {strip:?}"
        );
        // Each control still lands inside the strip it sized.
        for id in [7, 8] {
            let c = rect(id);
            assert!(c.y >= strip.y && c.y + c.h <= strip.y + strip.h + 1e-3);
        }
    }

    #[test]
    fn a_stack_places_only_the_page_its_index_names() {
        // Three pages, one shown: the hidden ones get no rectangle at all, so
        // nothing draws them and nothing hits them — while staying in the tree,
        // which is where a heavy view's GPU slot is collected from.
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"layout","flow":"stack","index":1,"children":[
                {"id":10,"type":"label","text":"one"},
                {"id":11,"type":"label","text":"two"},
                {"id":12,"type":"label","text":"three"}]}]}"#,
        );
        let m = Metrics::default();
        let placed = layout(area(), &w, &m);
        let shown: Vec<i32> = placed.iter().filter_map(|p| p.widget.id).collect();
        assert_eq!(
            shown,
            vec![1, 5, 11],
            "the window, the stack and its page 1, nothing else"
        );

        // The page fills the stack, inset by the container's margin — a stack
        // has no arrangement to make, only a page to show.
        let stack = placed.iter().find(|p| p.widget.id == Some(5)).unwrap();
        let page = placed.iter().find(|p| p.widget.id == Some(11)).unwrap();
        assert_eq!(page.rect, stack.rect.inset(m.margin));
        let si = placed.iter().position(|p| p.widget.id == Some(5)).unwrap();
        assert_eq!(page.parent, Some(si), "the page hangs off the stack");
    }

    #[test]
    fn a_stack_index_outside_its_children_shows_nothing() {
        // A blank page, not a clamped one: a pager that runs off the end shows
        // nothing rather than silently showing the wrong child.
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"layout","flow":"stack","index":7,"children":[
                {"id":10,"type":"label","text":"one"}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let shown: Vec<i32> = placed.iter().filter_map(|p| p.widget.id).collect();
        assert_eq!(shown, vec![1, 5], "the window and an empty stack");
        // A negative index is the same blank page.
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"layout","flow":"stack","index":-1,"children":[
                {"id":10,"type":"label","text":"one"}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        assert_eq!(placed.iter().filter_map(|p| p.widget.id).count(), 2);
    }

    #[test]
    fn a_clip_is_placed_with_its_own_axis_and_keeps_it_when_it_is_half_off_screen() {
        // One lane, one clip spanning [0, 400] of a 400-long timeline.
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"field","children":[
                {"id":10,"type":"field","offset":0,"dur":400}]}]}"#,
        );
        let m = Metrics::default();

        // Fully visible: the clip's own window is its whole span.
        let placed = layout(area(), &w, &m);
        let clip = placed.iter().find(|p| p.widget.id == Some(10)).unwrap();
        let local = clip.time.expect("a placed clip carries its axis");
        assert!(local.start.abs() < 0.5 && (local.len - 400.0).abs() < 1.0);

        // Scrolled to the clip's second half: the rectangle shrinks to what is
        // on screen, and the axis says *which* half that is - the fact a body
        // needs to draw the right samples, resolved here instead of by each
        // renderer from the lane's window.
        let half = View {
            start: 200.0,
            len: 200.0,
        };
        let placed = layout_on(area(), &w, &m, &|_, _| Some(half));
        let clip = placed.iter().find(|p| p.widget.id == Some(10)).unwrap();
        let local = clip.time.unwrap();
        assert!((local.start - 200.0).abs() < 1.0 && (local.len - 200.0).abs() < 1.0);

        // The lane above it stays on the *group's* window, not the clip's.
        let lane = placed.iter().find(|p| p.widget.id == Some(5)).unwrap();
        assert_eq!(lane.time, None);
    }

    #[test]
    fn a_heavy_view_used_as_a_lanes_body_does_not_indent_by_the_gutter_twice() {
        // A lane with a header and, as its body, a spectrogram on the same
        // navigation group — a spectral lane in a multitrack. The lane reserves
        // the group's gutter for its header; the view drawn inside that body
        // must start at the body's own left edge, or its trace, its ruler and
        // its playhead land a gutter's width right of the clips it shares an
        // axis with.
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"field","label":"takes","link":7,"children":[
                {"id":6,"type":"field","offset":0,"dur":400}]},
            {"id":8,"type":"field","label":"spectrum","link":7,"children":[
                {"id":9,"type":"signal","view":"spectrogram","link":7,
                 "data":[0.0,1.0]}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let lane = placed.iter().find(|p| p.widget.id == Some(8)).unwrap();
        let view = placed.iter().find(|p| p.widget.id == Some(9)).unwrap();
        assert!(lane.indent > 0.0, "the lanes share a gutter for the header");
        assert_eq!(view.indent, 0.0, "the lane already took the gutter out");
        assert_eq!(
            view.rect.x,
            lane.rect.x + lane.indent,
            "the body starts where the clips of the sibling lane do"
        );
    }

    #[test]
    fn a_clips_bodies_are_placed_children_layered_on_its_own_axis() {
        // One clip carrying all three bodies at once: a take, a roll of events
        // and an automation curve. They **layer** — each fills the clip's whole
        // rectangle — rather than dividing it, which is what makes an envelope
        // over a take one clip instead of two.
        let w = tree(
            r#"{"type":"window","children":[
            {"id":5,"type":"field","children":[
                {"id":10,"type":"field","offset":0,"dur":400,"data":[0.0,1.0],
                 "notes":[0.0,100.0,60.0],"points":[0.0,0.5,1,0.0]}]}]}"#,
        );
        let m = Metrics::default();
        let placed = layout(area(), &w, &m);
        let ci = placed.iter().position(|p| p.widget.id == Some(10)).unwrap();
        let clip = placed[ci];

        // The bodies are the clip's children, in layering order.
        let bodies: Vec<_> = placed
            .iter()
            .filter(|p| p.parent == Some(ci))
            .cloned()
            .collect();
        assert_eq!(bodies.len(), 3, "a take, a roll and a curve");
        use crate::host::widget::element::BodyRole;
        assert!(bodies[0].widget.signal().is_some());
        assert_eq!(bodies[1].widget.kind.body_role(), Some(BodyRole::Notes));
        assert_eq!(bodies[2].widget.kind.body_role(), Some(BodyRole::Curve));

        for b in &bodies {
            assert_eq!(
                b.rect, clip.rect,
                "a body fills the clip, it does not share it"
            );
            assert_eq!(b.time, clip.time, "...and reads the clip's own axis");
            assert!(
                b.widget.id.is_none(),
                "a body is not addressed: the clip is"
            );
        }
    }

    #[test]
    fn col_splits_height_evenly() {
        // Two elastic surfaces: nothing knows its own height, so they share.
        let w = tree(
            r#"{"type":"window","flow":"col","children":[
            {"id":1,"type":"layout"},{"id":2,"type":"layout"}]}"#,
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
            r#"{"type":"window","flow":"row","children":[
            {"id":1,"type":"label","text":"a"},{"id":2,"type":"label","text":"b"}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert!(a.x < b.x, "row places side by side");
        assert!((a.y - b.y).abs() < 1e-3, "same y in a row");
    }

    #[test]
    fn single_child_fills_inset_area() {
        let w = tree(
            r#"{"type":"window","children":[{"id":12,"type":"signal","view":"trace","data":[]}]}"#,
        );
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
            r#"{"type":"window","flow":"grid","children":[
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
            r#"{"type":"window","flow":"row","margin":0,"gap":0,"children":[
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
            r#"{"type":"window","flow":"col","margin":0,"gap":0,"children":[
            {"id":1,"type":"button","label":"fixed","h":50},
            {"id":2,"type":"button","label":"stretched","weight":2},
            {"id":3,"type":"button","label":"natural"},
            {"id":4,"type":"layout"}]}"#,
        );
        let placed = layout(area(), &w, &m);
        let (fixed, weighted, natural, elastic) = (
            placed[1].rect,
            placed[2].rect,
            placed[3].rect,
            placed[4].rect,
        );
        assert_eq!(fixed.h, 50.0, "an explicit size is taken as given");
        let control_h = placed[3]
            .widget
            .kind
            .natural_size(&m, 1.0)
            .1
            .expect("a button knows its height");
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
            r#"{"type":"window","flow":"col","margin":0,"gap":0,"children":[
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
            r#"{"type":"window","flow":"row","margin":0,"gap":0,"children":[
            {"id":1,"type":"button","label":"a"},{"id":2,"type":"button","label":"b"}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        let (a, b) = (placed[1].rect, placed[2].rect);
        assert_eq!((a.w, b.w), (300.0, 300.0));
        assert_eq!(a.h, 400.0, "the cross axis fills");
    }

    #[test]
    fn the_density_moves_a_natural_row() {
        let json = r#"{"type":"window","flow":"col","margin":0,"gap":0,"children":[
            {"id":1,"type":"button","label":"a"},{"id":2,"type":"layout"}]}"#;
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
        let json = r#"{"type":"window","flow":"col","margin":0,"gap":10,"children":[
            {"id":11,"type":"layout","h":28},
            {"id":12,"type":"button","label":"natural"},
            {"id":13,"type":"layout"}]}"#;
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
        let json = r#"{"type":"window","flow":"free","margin":4,"children":[
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
            {"id":9,"type":"plane","margin":0,"content_w":2000,"content_h":2000,
             "children":[{"id":7,"type":"label","text":"a","x":100,"y":50,"w":80,"h":40}]}]}"#;
        let w = tree(json);
        let placed = layout(area(), &w, &Metrics::default().resolved(2.0));
        let child = placed.iter().find(|p| p.widget.id == Some(7)).unwrap();
        assert_eq!((child.rect.x, child.rect.y), (200.0, 100.0));
        assert_eq!((child.rect.w, child.rect.h), (160.0, 80.0));
        assert_eq!(child.scale, 2.0, "the plane's text rides its zoom");
    }

    /// A zoom is an **enlargement**: inside a scrolled box the declared lengths,
    /// the metric roles and the text all carry the same factor. The failure this
    /// pins is what a knob in a zoomed patcher looked like when only the text
    /// did — proportions inside the box came apart as soon as the zoom moved.
    #[test]
    fn a_scrolled_box_enlarges_whole() {
        let json = r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"plane","margin":0,"content_w":2000,"content_h":2000,
             "view_zoom":ZOOM,"children":[
               {"id":10,"type":"layout","flow":"col","x":0,"y":0,"w":300,"h":220,
                "children":[{"id":11,"type":"label","text":"node"},
                            {"id":12,"type":"knob","label":"amount"}]}]}]}"#;
        let m = Metrics::default();
        let at = |zoom: f32| {
            let w = tree(&json.replace("ZOOM", &zoom.to_string()));
            let placed = layout(area(), &w, &m);
            let of = |id: i32| {
                let p = placed.iter().find(|p| p.widget.id == Some(id)).unwrap();
                (p.rect, p.scale, p.metrics)
            };
            (of(10), of(11), of(12))
        };
        let (one, two) = (at(1.0), at(2.0));
        for (a, b) in [(one.0, two.0), (one.1, two.1), (one.2, two.2)] {
            let (ra, sa, ma) = a;
            let (rb, sb, mb) = b;
            assert_eq!(rb.w, ra.w * 2.0, "the box doubles");
            assert_eq!(rb.h, ra.h * 2.0);
            assert_eq!(sb, sa * 2.0, "and so does its text");
            assert_eq!(mb.pad, ma.pad * 2.0, "and its padding");
            assert_eq!(mb.knob_d, ma.knob_d * 2.0, "and its parts");
        }
        // The proportion that broke: the knob's box against the disc inside it.
        assert_eq!(two.2.0.h / one.2.0.h, two.2.2.knob_d / one.2.2.knob_d);
    }

    #[test]
    fn a_named_plane_zoom_is_literal_at_any_density() {
        // The script said one physical pixel per content unit, so that is what
        // it gets on a doubled display too — the default is the density, a
        // named number is the number.
        let json = r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"plane","margin":0,"view_zoom":1,
             "content_w":2000,"content_h":2000,
             "children":[{"id":7,"type":"label","text":"a","x":100,"y":50,"w":80,"h":40}]}]}"#;
        let w = tree(json);
        let placed = layout(area(), &w, &Metrics::default().resolved(2.0));
        let child = placed.iter().find(|p| p.widget.id == Some(7)).unwrap();
        assert_eq!((child.rect.x, child.rect.y), (100.0, 50.0));
        assert_eq!((child.rect.w, child.rect.h), (80.0, 40.0));
        assert_eq!(child.scale, 1.0);
    }

    /// The plane's content extent is measured in **content units**, the
    /// viewport included. Mixing in the pane's pixels put a graph that should
    /// centre itself into the corner instead, by exactly the zoom.
    #[test]
    fn a_small_graph_centres_at_any_density() {
        let json = r#"{"type":"window","margin":0,"children":[
            {"id":9,"type":"plane","margin":0,"children":[
              {"id":7,"type":"plane","boxes":[{"def":"src","outlets":["out"]}]}]}]}"#;
        let w = tree(json);
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        for scale in [1.0, 2.0] {
            let m = Metrics::default().resolved(scale);
            let placed = layout(area, &w, &m);
            let scroll = placed.iter().find(|p| p.widget.id == Some(9)).unwrap();
            let content = scroll_content(scroll.widget, scroll.rect, &m);
            // The content is never smaller than the pane *in its own units*.
            assert_eq!(content, (area.w / scale, area.h / scale), "at {scale}");
            // So the graph's own rect is the pane: it centres in the window
            // rather than starting off the right edge.
            let patch = placed.iter().find(|p| p.widget.id == Some(7)).unwrap();
            assert_eq!(patch.rect.w, area.w, "at {scale}");
            assert_eq!(patch.rect.h, area.h, "at {scale}");
        }
    }

    #[test]
    fn the_application_shell_lays_out() {
        // The acceptance shape: a col window with a fixed menu bar, a weighted
        // content area and a fixed status bar.
        let w = tree(
            r#"{"type":"window","flow":"col","margin":0,"gap":0,"children":[
            {"id":11,"type":"layout","flow":"row","h":28},
            {"id":12,"type":"layout"},
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
            r#"{"type":"window","flow":"col","margin":10,"gap":2,"children":[
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
            r#"{"type":"window","flow":"grid","cols":4,"children":[
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
            r#"{"type":"window","flow":"free","margin":0,"children":[
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
            r#"{"type":"window","flow":"row","margin":0,"gap":0,"children":[
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
            {"id":9,"type":"plane","margin":10,"children":[
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
            {"id":9,"type":"plane","content_w":2000,"content_h":1500,"children":[
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
            {"id":9,"type":"plane","flow":"col","children":[
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
            {"id":9,"type":"plane","margin":0,"content_w":2000,"content_h":2000,
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
            {"id":19,"type":"plane","margin":0,"view_zoom":2,"content_w":1000,"content_h":1000,
             "children":[
              {"id":11,"type":"label","text":"a","x":0,"y":0,"w":80,"h":40},
              {"id":12,"type":"plane","margin":0,"view_zoom":0.5,"x":100,"y":0,"w":200,"h":200,
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
            {"id":9,"type":"plane","margin":0,"axis":"x","zoom":0,
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
            {"id":9,"type":"plane","margin":0,"content_w":2000,"content_h":100,
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
            {"id":9,"type":"plane","margin":0,"content_w":1000,"content_h":1000,"children":[
              {"id":5,"type":"layout","flow":"row","margin":0,"gap":0,
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
            {"id":5,"type":"layout","children":[{"id":12,"type":"signal","view":"trace","data":[]}]}]}"#,
        );
        let placed = layout(area(), &w, &Metrics::default());
        // [window, panel, waveform]
        assert_eq!(placed.len(), 3);
        assert!(placed[2].widget.is_nav_signal());
        // The waveform sits inside the panel's rect.
        let (panel, wave) = (placed[1].rect, placed[2].rect);
        assert!(wave.x >= panel.x && wave.y >= panel.y);
    }
}
