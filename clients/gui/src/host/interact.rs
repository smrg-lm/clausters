//! Pointer-interaction primitives over the widget tree — the value/hit logic
//! shared by both fronts.
//!
//! Hit-testing a point, reading and writing a control's value, flipping a toggle,
//! cycling a menu: all of it is pure work on the [`Host`]'s typed tree plus the
//! [`layout`] and [`controls`] math, with no platform dependency. The native
//! windowed front ([`super::gui`]) and the browser front ([`super::web`]) both
//! call these, so a turned knob updates the tree and decides bound-vs-event the
//! same way on either platform — only the event *source* (winit vs browser
//! pointer events) and the event *sink* (a socket vs the binding surface) differ.

use clausters_core::osc::OscType;

use super::bpf;
use super::layout::{self, Rect};
use super::pianoroll;
use super::track;
use super::widget::{Widget, WidgetKind};
use super::{Host, controls};
use crate::viewport::View;

/// The deepest interactive widget under `(x, y)` in window `def_id`: its id, its
/// laid-out rect, its accumulated workspace zoom ([`Placed::scale`], which the
/// control hit-math shares with the drawing) and a clone of its kind. Containers
/// (`window`/`panel`) are not hit targets — except `scroll`, whose empty area is
/// the pan gesture's surface (its children, laid out through its view transform,
/// still win over it). A widget scrolled out of its container's window (outside
/// its clip) is not hit. `fb_w`/`fb_h` is the window's framebuffer size in
/// device pixels.
///
/// [`Placed::scale`]: super::layout::Placed::scale
pub(crate) fn hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
) -> Option<(i32, Rect, f32, WidgetKind)> {
    let tree = host.window_def(def_id)?;
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    let mut found = None;
    for p in layout::layout(area, tree) {
        if p.rect.contains(x, y)
            && p.clip.is_none_or(|c| c.contains(x, y))
            && let Some(id) = p.widget.id
            && !matches!(
                p.widget.kind,
                WidgetKind::Window { .. } | WidgetKind::Panel { .. }
            )
        {
            found = Some((id, p.rect, p.scale, p.widget.kind.clone()));
        }
    }
    found
}

/// The innermost `scroll` container under `(x, y)`: its id and laid-out rect.
/// The wheel and the empty-area pan drag address the workspace itself even
/// when the cursor sits over a scrolled child that consumed nothing.
pub(crate) fn scroll_at(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
) -> Option<(i32, Rect)> {
    let tree = host.window_def(def_id)?;
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    let mut found = None;
    for p in layout::layout(area, tree) {
        if p.rect.contains(x, y)
            && p.clip.is_none_or(|c| c.contains(x, y))
            && matches!(p.widget.kind, WidgetKind::Scroll { .. })
            && let Some(id) = p.widget.id
        {
            found = Some((id, p.rect));
        }
    }
    found
}

/// Sets a `scroll`'s view state (clamped against its content in `area`),
/// returning the clamped `(view_x, view_y, view_zoom)` when something actually
/// moved — the one door every scroll navigation goes through, so a gesture and
/// a `/gui_set` clamp identically.
pub(crate) fn scroll_set_view(
    host: &mut Host,
    def_id: i32,
    id: i32,
    area: Rect,
    (vx, vy, zoom): (f64, f64, f64),
) -> Option<(f64, f64, f64)> {
    let tree = host.window_def_mut(def_id)?;
    let content = layout::scroll_content(tree.find(id)?, area);
    let w = tree.find_mut(id)?;
    let WidgetKind::Scroll { view, .. } = &mut w.kind else {
        return None;
    };
    let zoom = super::scroll::clamp_zoom(zoom);
    let slack = view.axis.slack();
    let next = (
        super::scroll::clamp_pan(vx, area.w, zoom, content.0, slack),
        super::scroll::clamp_pan(vy, area.h, zoom, content.1, slack),
        zoom,
    );
    if next == (view.view_x, view.view_y, view.view_zoom) {
        return None;
    }
    (view.view_x, view.view_y, view.view_zoom) = next;
    Some(next)
}

/// The current 0..1 fraction of a continuous control (slider/knob/number) in the
/// host tree — the live value used to drive an incremental drag.
pub(crate) fn fraction_of(host: &Host, def_id: i32, widget_id: i32) -> Option<f32> {
    fn walk(w: &Widget, id: i32) -> Option<f32> {
        if w.id == Some(id) {
            return match &w.kind {
                WidgetKind::Slider { range: r, .. }
                | WidgetKind::Knob(r)
                | WidgetKind::Number(r) => Some(r.fraction()),
                _ => None,
            };
        }
        w.children.iter().find_map(|c| walk(c, id))
    }
    walk(host.window_def(def_id)?, widget_id)
}

/// Sets a continuous control's value from a 0..1 fraction, in the host tree.
pub(crate) fn set_fraction(host: &mut Host, def_id: i32, widget_id: i32, t: f32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(widget_id)
    {
        match &mut w.kind {
            WidgetKind::Slider { range: r, .. } | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                r.set_fraction(t)
            }
            _ => {}
        }
    }
}

/// Runs `f` over a `text` field's `(value, caret)` in the host tree — the one
/// door every keystroke and click goes through, so the fronts never unpack the
/// variant themselves (the sibling of [`set_fraction`]/[`bpf_edit`]). `f`'s
/// return value is passed through (`None` when the widget is gone or not a
/// `text` field).
pub(crate) fn text_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut String, &mut super::textedit::Caret, bool) -> R,
) -> Option<R> {
    let w = host.window_def_mut(def_id)?.find_mut(widget_id)?;
    match &mut w.kind {
        WidgetKind::Text {
            value,
            caret,
            multiline,
            ..
        } => Some(f(value, caret, *multiline)),
        _ => None,
    }
}

/// The caret byte offset a click at `(cx, cy)` lands on in the `text` field
/// `widget_id` (its `rect` and workspace `scale` as the front hit-tested them).
/// `None` when the widget is gone or not a text field.
pub(crate) fn text_caret_at(
    host: &Host,
    def_id: i32,
    widget_id: i32,
    rect: Rect,
    scale: f32,
    cx: f64,
    cy: f64,
) -> Option<usize> {
    let w = host.window_def(def_id)?.find(widget_id)?;
    match &w.kind {
        WidgetKind::Text {
            value,
            label,
            text_size,
            multiline,
            caret,
        } => Some(controls::caret_at(
            rect,
            value,
            label.is_some(),
            *text_size * scale,
            *multiline,
            *caret,
            cx,
            cy,
        )),
        _ => None,
    }
}

/// Flips a `toggle`'s boolean state in the host tree.
pub(crate) fn flip_toggle(host: &mut Host, def_id: i32, id: i32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(id)
        && let WidgetKind::Toggle { value, .. } = &mut w.kind
    {
        *value = !*value;
    }
}

/// Advances a `menu`'s selected option (wrapping) in the host tree.
pub(crate) fn cycle_menu(host: &mut Host, def_id: i32, id: i32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(id)
        && let WidgetKind::Menu { index, options, .. } = &mut w.kind
        && !options.is_empty()
    {
        *index = (*index + 1) % options.len();
    }
}

/// The current event value of widget `id` in `tree` (what a `/gui_event` or a
/// bound forward carries).
pub(crate) fn value_of(tree: &Widget, id: i32) -> Option<OscType> {
    fn walk(w: &Widget, id: i32) -> Option<OscType> {
        if w.id == Some(id) {
            return w.kind.event_value();
        }
        w.children.iter().find_map(|c| walk(c, id))
    }
    walk(tree, id)
}

/// Runs `f` over a `bpf` widget's model in the host tree — the one door every
/// envelope edit goes through, so the fronts never unpack the variant
/// themselves. `f` gets the breakpoints and the display mapping (the time
/// domain, the value range and the `exp` scale); its return value is passed
/// through (`None` when the widget is not a `bpf`). Both fronts reach these
/// helpers through the shared gesture machine ([`super::gestures`]).
pub(crate) fn bpf_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<bpf::BpfPoint>, f64, f32, f32, bool) -> R,
) -> Option<R> {
    let w = host.window_def_mut(def_id)?.find_mut(widget_id)?;
    match &mut w.kind {
        WidgetKind::Bpf {
            points,
            min,
            max,
            duration,
            exp,
            ..
        } => Some(f(points, *duration, *min, *max, *exp)),
        _ => None,
    }
}

/// A break-point curve's edit-back payload: the `"points"` tag plus the flat
/// breakpoint list (`t v shape curve` per point) — what a `/gui_event` carries
/// to the script, and what a bound editor forwards to the audio server. Shared
/// by the `bpf` view and the **automation clip**, whose curve is the same model
/// placed on a lane: one payload, so a script (or an `Automation`) consumes an
/// edit without caring which view drew it.
pub(crate) fn bpf_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let points = match &tree.find(id)?.kind {
        WidgetKind::Bpf { points, .. } => points,
        WidgetKind::Clip { points, .. } if !points.is_empty() => points,
        _ => return None,
    };
    let mut args = vec![OscType::String("points".into())];
    args.extend(bpf::points_args(points));
    Some(args)
}

/// Completes a rewiring drag on a `graph` patch: the dragged port (`member` and
/// its control) is pointed at the bus under the cursor — or at **none**, when the
/// release lands on empty space, which unwires it. Writes the patch in the host
/// tree and returns the edit as `(member, control, bus)` (an empty bus = unwired)
/// — the payload of the flat `"wire"` event a script re-renders from. `None`
/// when there was nothing to rewire.
#[allow(clippy::too_many_arguments)] // one drop: the widget, the port, the place
pub(crate) fn wire_set(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    port: (usize, usize),
    area: Rect,
    cx: f64,
    cy: f64,
    scale: f32,
) -> Option<(usize, String, String)> {
    let w = host.window_def_mut(def_id)?.find_mut(widget_id)?;
    let WidgetKind::Graph { graph, .. } = &mut w.kind else {
        return None;
    };
    let (member, index) = port;
    let control = graph.members.get(member)?.ports.get(index)?.clone();
    let bus = super::graph::bus_hit(area, graph, cx, cy, scale)
        .and_then(|b| graph.buses.get(b).map(|bus| bus.name.clone()))
        .unwrap_or_default();

    // One wire per (member, control): a control touches one bus, or none.
    graph
        .wires
        .retain(|wire| !(wire.member == member && wire.control == control));
    if !bus.is_empty() {
        graph.wires.push(super::graph::Wire {
            member,
            control: control.clone(),
            bus: bus.clone(),
        });
    }
    Some((member, control, bus))
}

/// Sets a `graph` patch's selected set (the click/marquee gestures' write).
pub(crate) fn graph_select(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    set: Vec<(super::graph::BoxKind, usize)>,
) {
    if let Some(w) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(widget_id))
        && let WidgetKind::Graph { selected, .. } = &mut w.kind
    {
        *selected = set;
    }
}

/// Moves `graph` boxes to explicit canvas positions (the move drag's write):
/// each `(kind, index, x, y)` lands on the member's/bus's `x`/`y`, making an
/// auto-placed box's position explicit from its first drag.
pub(crate) fn graph_move(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    moves: &[(super::graph::BoxKind, usize, f32, f32)],
) {
    let Some(w) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(widget_id))
    else {
        return;
    };
    let WidgetKind::Graph { graph, .. } = &mut w.kind else {
        return;
    };
    for &(kind, i, x, y) in moves {
        match kind {
            super::graph::BoxKind::Member => {
                if let Some(m) = graph.members.get_mut(i) {
                    (m.x, m.y) = (Some(x), Some(y));
                }
            }
            super::graph::BoxKind::Bus => {
                if let Some(b) = graph.buses.get_mut(i) {
                    (b.x, b.y) = (Some(x), Some(y));
                }
            }
        }
    }
}

/// Sets a `graph`'s selection to the boxes intersecting the marquee between
/// `a` and `b` (device pixels) — the box-selection drag, live on every move.
pub(crate) fn graph_marquee(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    area: Rect,
    a: (f64, f64),
    b: (f64, f64),
    scale: f32,
) {
    let Some(w) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(widget_id))
    else {
        return;
    };
    let WidgetKind::Graph {
        graph, selected, ..
    } = &mut w.kind
    else {
        return;
    };
    let sel = super::gestures::corner_rect(a, b);
    let overlaps = |r: Rect| {
        r.x < sel.x + sel.w && sel.x < r.x + r.w && r.y < sel.y + sel.h && sel.y < r.y + r.h
    };
    let mut set = Vec::new();
    for i in 0..graph.members.len() {
        if overlaps(super::graph::member_rect(area, graph, i, scale)) {
            set.push((super::graph::BoxKind::Member, i));
        }
    }
    for bidx in 0..graph.buses.len() {
        if overlaps(super::graph::bus_rect(area, graph, bidx, scale)) {
            set.push((super::graph::BoxKind::Bus, bidx));
        }
    }
    *selected = set;
}

/// Runs `f` over an automation clip's break-points in the host tree — the one
/// door a curve edit on a lane goes through (the clip sibling of `bpf_edit`).
fn clip_curve<R>(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    f: impl FnOnce(&mut Vec<bpf::BpfPoint>, f32, f32, bool) -> R,
) -> Option<R> {
    let w = host.window_def_mut(def_id)?.find_mut(clip_id)?;
    match &mut w.kind {
        WidgetKind::Clip {
            points,
            points_min,
            points_max,
            exp,
            ..
        } => Some(f(points, *points_min, *points_max, *exp)),
        _ => None,
    }
}

/// Moves break-point `index` of an automation clip to the cursor: the time maps
/// back through the **shared axis** (a clip's curve lives on the timeline, not on
/// a widget-local domain) and the value through the clip's range, then the point
/// is placed with the `bpf` model's own monotonic clamp. Returns whether it moved.
#[allow(clippy::too_many_arguments)] // one display mapping, all scalars
pub(crate) fn clip_point_move(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    index: usize,
    rect: Rect,
    body: Rect,
    nav: &View,
    offset: f64,
    cx: f64,
    cy: f64,
) -> bool {
    let Some(widget) = host.window_def(def_id).and_then(|t| t.find(clip_id)) else {
        return false;
    };
    let WidgetKind::Clip { dur, .. } = widget.kind else {
        return false;
    };
    let t = track::curve_time_at(body, nav, offset, cx).min(dur);
    clip_curve(host, def_id, clip_id, |points, min, max, exp| {
        let value = track::curve_value_at(rect, min, max, exp, cy);
        bpf::place_point(points, index, t, value, dur.max(t));
    })
    .is_some()
}

/// Adds a break-point at the cursor on an automation clip (Ctrl+click), or
/// removes the one under it — the `bpf` view's gesture, on a lane.
#[allow(clippy::too_many_arguments)] // one display mapping, all scalars
pub(crate) fn clip_point_edit(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    hit: Option<usize>,
    rect: Rect,
    body: Rect,
    nav: &View,
    offset: f64,
    cx: f64,
    cy: f64,
) -> bool {
    let t = track::curve_time_at(body, nav, offset, cx);
    clip_curve(host, def_id, clip_id, |points, min, max, exp| match hit {
        Some(i) => bpf::remove_point(points, i),
        None => {
            let value = track::curve_value_at(rect, min, max, exp, cy);
            bpf::insert_point(points, t, value);
            true
        }
    })
    .unwrap_or(false)
}

/// Which part of a clip a press landed on: its body (move) or one of its edges
/// (resize). The edge zone is a few pixels at each end; a clip too narrow for
/// two edge zones is all body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ClipPart {
    Body,
    Start,
    End,
}

/// A clip press: the clip id, its current placement (`offset`/`dur`), the lane
/// body and the shared navigation window the drag maps through (so the front
/// turns cursor pixels into timeline samples), and which part was hit.
pub(crate) struct ClipHit {
    pub id: i32,
    pub offset: f64,
    pub dur: f64,
    pub body: Rect,
    /// The clip's own rectangle (the body the curve of an automation clip is
    /// drawn in, so its point edits map onto the pixels drawn).
    pub rect: Rect,
    pub nav: View,
    pub part: ClipPart,
    /// The break-point under the cursor on an automation clip: a point wins over
    /// the clip's body (as it wins over a segment in the `bpf` view), so the
    /// curve is edited in place while the clip still moves by its empty space.
    pub point: Option<usize>,
}

/// The clip edge hit zone, device pixels.
const CLIP_EDGE_PX: f32 = 6.0;

/// The topmost `clip` under `(x, y)`, if the point is over a track's lane body
/// (not its header) and inside a clip. Reconstructs the shared time axis
/// ([`track::window_nav`]) so it hit-tests against the same geometry the
/// renderer drew. Native-only, like the other edit-back gestures.
pub(crate) fn clip_hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
) -> Option<ClipHit> {
    let tree = host.window_def(def_id)?;
    let full = track::window_nav(tree);
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    for p in layout::layout(area, tree) {
        let WidgetKind::Track { editor, .. } = &p.widget.kind else {
            continue;
        };
        if !p.rect.contains(x, y) {
            continue;
        }
        // The lane's *group* window — the same one the renderer drew through, so
        // a zoomed or panned axis hit-tests where it looks.
        let nav = p
            .widget
            .id
            .and_then(|id| host.timeline_nav(id))
            .map_or(full, |(nav, _total)| nav);
        // The same body the renderer drew (its ruler strip reserved), so the
        // pixels a clip occupies are the pixels it is grabbed by.
        let body = track::lane_body(p.rect, editor.ruler != super::widget::Ruler::Off);
        if !body.contains(x, y) {
            return None; // over the header or the ruler strip, not a clip
        }
        // Topmost clip wins: later children draw over earlier ones.
        for c in p.widget.children.iter().rev() {
            if let WidgetKind::Clip { offset, dur, .. } = c.kind
                && let Some((x0, x1)) = track::clip_x_range(body, &nav, offset, dur)
                && (x as f32) >= x0
                && (x as f32) <= x1
                && let Some(id) = c.id
            {
                let rect = track::clip_rect(body, x0, x1);
                let point = track::clip_draw(c).and_then(|clip| {
                    (!clip.points.is_empty())
                        .then(|| track::curve_hit(&clip, rect, body, &nav, x, y))
                        .flatten()
                });
                return Some(ClipHit {
                    id,
                    offset,
                    dur,
                    body,
                    rect,
                    nav,
                    part: clip_part(x0, x1, x as f32),
                    point,
                });
            }
        }
    }
    None
}

/// Which part of a clip spanning pixels `[x0, x1]` the pointer x fell on.
fn clip_part(x0: f32, x1: f32, x: f32) -> ClipPart {
    if x1 - x0 < 2.0 * CLIP_EDGE_PX {
        return ClipPart::Body; // too narrow to grab an edge
    }
    if x - x0 <= CLIP_EDGE_PX {
        ClipPart::Start
    } else if x1 - x <= CLIP_EDGE_PX {
        ClipPart::End
    } else {
        ClipPart::Body
    }
}

/// Writes a clip's placement (`offset`/`dur`, each clamped `>= 0`) in the host
/// tree — the drag's mutation, the sibling of [`set_fraction`].
pub(crate) fn clip_set(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    new_offset: Option<f64>,
    new_dur: Option<f64>,
) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(clip_id)
        && let WidgetKind::Clip { offset, dur, .. } = &mut w.kind
    {
        if let Some(o) = new_offset {
            *offset = o.max(0.0);
        }
        if let Some(d) = new_dur {
            *dur = d.max(0.0);
        }
    }
}

/// A clip's edit-back payload: the `"clip"` tag plus the new `offset`/`dur` —
/// what a `/gui_event` carries to the script (and what a bound clip would
/// forward). Flat OSC primitives, the same pattern as the `bpf` `"points"`
/// payload.
pub(crate) fn clip_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    match &tree.find(id)?.kind {
        WidgetKind::Clip { offset, dur, .. } => Some(vec![
            OscType::String("clip".into()),
            OscType::Float(*offset as f32),
            OscType::Float(*dur as f32),
        ]),
        _ => None,
    }
}

// --- Piano-roll interaction (native gestures) -----------------------------

/// Which region of a `pianoroll` a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrRegion {
    Grid,
    Velocity,
    Osc,
}

/// A piano-roll press: the widget id, the reconstructed grid rect and shared
/// navigation window the drag maps through, the visible pitch window, the note
/// snap grid, which region was hit, and the note/OSC-marker under the cursor (if
/// any). The renderer's geometry is reconstructed here so a note is grabbed by
/// the pixels it is drawn on, exactly as `clip_hit` does for a clip.
pub(crate) struct PianoRollHit {
    pub grid: Rect,
    /// The rect of the region that was hit (the grid, the velocity lane or the
    /// OSC lane) — the velocity drag maps the cursor's height through it.
    pub region_rect: Rect,
    pub nav: View,
    pub lo: f32,
    pub hi: f32,
    pub snap: f64,
    pub region: PrRegion,
    pub note: Option<pianoroll::NoteHit>,
    pub osc_index: Option<usize>,
}

/// The pitch window `[lo, hi]` a piano-roll draws through — its `[min, max]`
/// axis sliced by the vertical display window (`y_start`/`y_len`), the same math
/// the renderer's `pitch_window` uses so the hit-test matches the pixels.
fn pitch_window(editor: &super::widget::EditorProps, min: f32, max: f32) -> (f32, f32) {
    let (y0, yl) = editor.y_view();
    let span = (max - min) as f64;
    (
        (min as f64 + y0 * span) as f32,
        (min as f64 + (y0 + yl) * span) as f32,
    )
}

/// The content extent (samples) of a piano-roll's notes and OSC events — the
/// fallback navigation window when the widget is in no group yet.
fn pianoroll_span(notes: &[pianoroll::Note], osc: &[pianoroll::OscMark]) -> f64 {
    let mut span = 0.0f64;
    for n in notes {
        span = span.max(n.start + n.dur);
    }
    for m in osc {
        span = span.max(m.time);
    }
    span
}

/// Hit-test a press against the `pianoroll` under `(x, y)`, reconstructing the
/// same regions and navigation window the renderer drew. Native-only, the
/// edit-back gesture posture.
pub(crate) fn pianoroll_hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
) -> Option<PianoRollHit> {
    let tree = host.window_def(def_id)?;
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    for p in layout::layout(area, tree) {
        let WidgetKind::PianoRoll {
            notes,
            osc,
            min,
            max,
            snap,
            velocity_lane,
            osc_lane,
            editor,
            ..
        } = &p.widget.kind
        else {
            continue;
        };
        if !p.rect.contains(x, y) {
            continue;
        }
        let id = p.widget.id?;
        let ruler_on = editor.ruler != super::widget::Ruler::Off;
        let r = pianoroll::regions(p.rect, ruler_on, *osc_lane, *velocity_lane);
        let nav = host
            .timeline_nav(id)
            .map(|(nav, _)| nav)
            .unwrap_or_else(|| View::full(pianoroll_span(notes, osc).ceil().max(1.0) as usize));
        let (lo, hi) = pitch_window(editor, *min, *max);
        let (fx, fy) = (x as f32, y as f32);
        let (region, note, osc_index) = if *osc_lane && r.osc.contains(x, y) {
            (PrRegion::Osc, None, nearest_osc(r.osc, &nav, osc, fx))
        } else if *velocity_lane && r.velocity.contains(x, y) {
            // A velocity-lane press picks the note whose bar it is nearest; the
            // hit rides in `note` as a body hit so the caller reads its index.
            let picked =
                nearest_note(r.velocity, &nav, notes, fx).map(|index| pianoroll::NoteHit {
                    index,
                    part: pianoroll::NotePart::Body,
                });
            (PrRegion::Velocity, picked, None)
        } else {
            let note = pianoroll::note_hit(r.grid, &nav, 0.0, notes, lo, hi, fx, fy);
            (PrRegion::Grid, note, None)
        };
        let region_rect = match region {
            PrRegion::Grid => r.grid,
            PrRegion::Velocity => r.velocity,
            PrRegion::Osc => r.osc,
        };
        return Some(PianoRollHit {
            grid: r.grid,
            region_rect,
            nav,
            lo,
            hi,
            snap: *snap,
            region,
            note,
            osc_index,
        });
    }
    None
}

/// The index of the note whose start is nearest the cursor x (within a small
/// pixel tolerance) — the velocity lane's bar picker.
fn nearest_note(lane: Rect, nav: &View, notes: &[pianoroll::Note], x: f32) -> Option<usize> {
    let to_x = |s: f64| lane.x + ((s - nav.start) / nav.len.max(1.0) * lane.w as f64) as f32;
    notes
        .iter()
        .enumerate()
        .map(|(i, n)| (i, (to_x(n.start) - x).abs()))
        .filter(|(_, d)| *d <= 5.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The index of the OSC marker whose time is nearest the cursor x.
fn nearest_osc(lane: Rect, nav: &View, marks: &[pianoroll::OscMark], x: f32) -> Option<usize> {
    let to_x = |s: f64| lane.x + ((s - nav.start) / nav.len.max(1.0) * lane.w as f64) as f32;
    marks
        .iter()
        .enumerate()
        .map(|(i, m)| (i, (to_x(m.time) - x).abs()))
        .filter(|(_, d)| *d <= 5.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Mutate a piano-roll's note list in the host tree (the drag's write path, the
/// sibling of [`bpf_edit`]). Returns `None` when the widget is gone or not a
/// piano-roll.
pub(crate) fn pianoroll_notes_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<pianoroll::Note>) -> R,
) -> Option<R> {
    match &mut host.window_def_mut(def_id)?.find_mut(widget_id)?.kind {
        WidgetKind::PianoRoll { notes, .. } => Some(f(notes)),
        _ => None,
    }
}

/// Mutate a piano-roll's notes **and** its multi-note selection together (the
/// block edits' write path: a marquee fills the selection, a block move/delete/
/// velocity nudge reads it while rewriting the notes).
pub(crate) fn pianoroll_state_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<pianoroll::Note>, &mut Vec<usize>) -> R,
) -> Option<R> {
    match &mut host.window_def_mut(def_id)?.find_mut(widget_id)?.kind {
        WidgetKind::PianoRoll {
            notes, selected, ..
        } => Some(f(notes, selected)),
        _ => None,
    }
}

/// Mutate a piano-roll's OSC-event list in the host tree.
pub(crate) fn pianoroll_osc_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<pianoroll::OscMark>) -> R,
) -> Option<R> {
    match &mut host.window_def_mut(def_id)?.find_mut(widget_id)?.kind {
        WidgetKind::PianoRoll { osc, .. } => Some(f(osc)),
        _ => None,
    }
}

/// A piano-roll's notes edit-back payload: the `"notes"` tag plus the flat
/// quintuple list (`start dur pitch velocity channel` per note) — the wire form
/// the `pianoroll` and `clip` share. A `/gui_event` carries it to the script; a
/// bound editor forwards it (minus the tag) to the audio server.
pub(crate) fn notes_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let notes = match &tree.find(id)?.kind {
        WidgetKind::PianoRoll { notes, .. } => notes,
        WidgetKind::Clip { notes, .. } if !notes.is_empty() => notes,
        _ => return None,
    };
    let mut args = vec![OscType::String("notes".into())];
    for n in notes {
        args.push(OscType::Float(n.start as f32));
        args.push(OscType::Float(n.dur as f32));
        args.push(OscType::Float(n.pitch));
        args.push(OscType::Int(n.velocity));
        args.push(OscType::Int(n.channel));
    }
    Some(args)
}

/// A piano-roll's OSC-events edit-back payload: the `"osc"` tag plus the flat
/// `time label` pairs (an empty string when a marker has no label).
pub(crate) fn osc_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let WidgetKind::PianoRoll { osc, .. } = &tree.find(id)?.kind else {
        return None;
    };
    let mut args = vec![OscType::String("osc".into())];
    for m in osc {
        args.push(OscType::Float(m.time as f32));
        args.push(OscType::String(m.label.clone().unwrap_or_default()));
    }
    Some(args)
}

/// Mark a piano key held (the press/glissando write path). `true` when the key
/// was not already held.
pub(crate) fn piano_press_key(host: &mut Host, def_id: i32, widget_id: i32, pitch: i32) -> bool {
    piano_state(host, def_id, widget_id, |pressed| {
        if pressed.contains(&pitch) {
            false
        } else {
            pressed.push(pitch);
            true
        }
    })
    .unwrap_or(false)
}

/// Mark a piano key released. `true` when it was held.
pub(crate) fn piano_release_key(host: &mut Host, def_id: i32, widget_id: i32, pitch: i32) -> bool {
    piano_state(host, def_id, widget_id, |pressed| {
        let before = pressed.len();
        pressed.retain(|&p| p != pitch);
        pressed.len() != before
    })
    .unwrap_or(false)
}

/// Run `f` over a piano's held-key set in the host tree.
fn piano_state<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<i32>) -> R,
) -> Option<R> {
    match &mut host.window_def_mut(def_id)?.find_mut(widget_id)?.kind {
        WidgetKind::Piano { pressed, .. } => Some(f(pressed)),
        _ => None,
    }
}

/// Write a piano's visible range (the pan/zoom write path): the min white-snaps,
/// held keys that left the window drop. Returns the applied range when it
/// actually changed (`None` for a no-op or a non-piano widget).
pub(crate) fn piano_set_range(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    new_min: i32,
    new_max: i32,
) -> Option<(i32, i32)> {
    let w = host.window_def_mut(def_id)?.find_mut(widget_id)?;
    let WidgetKind::Piano {
        min, max, pressed, ..
    } = &mut w.kind
    else {
        return None;
    };
    let nm = super::piano::snap_white_down(new_min.clamp(0, 127).min(new_max));
    let nx = new_max.clamp(0, 127).max(nm);
    if (nm, nx) == (*min, *max) {
        return None;
    }
    *min = nm;
    *max = nx;
    pressed.retain(|p| (nm..=nx).contains(p));
    Some((nm, nx))
}

/// Whether a piano key is inside the widget's active (non-grayed) range — a
/// press outside it is inert.
pub(crate) fn piano_key_active(host: &Host, def_id: i32, widget_id: i32, pitch: i32) -> bool {
    match host.window_def(def_id).and_then(|t| t.find(widget_id)) {
        Some(w) => match &w.kind {
            WidgetKind::Piano {
                active_min,
                active_max,
                ..
            } => (*active_min..=*active_max).contains(&pitch),
            _ => false,
        },
        None => false,
    }
}

/// A piano note event's payload — the MIDI-shaped
/// `"note" pitch velocity state channel` flat list (state 1 = on, 0 = off): a
/// `/gui_event` carries it to the script; a bound piano forwards it (minus the
/// tag) to the audio server; a future MIDI consumer translates it 1:1 to
/// note-on/note-off.
pub(crate) fn piano_note_args(pitch: i32, velocity: i32, state: i32, channel: i32) -> Vec<OscType> {
    vec![
        OscType::String("note".into()),
        OscType::Int(pitch),
        OscType::Int(velocity),
        OscType::Int(state),
        OscType::Int(channel),
    ]
}

/// Snaps a timeline sample value to a drag grid: to the nearest multiple of
/// `grid` when it is positive, else to a whole sample.
pub(crate) fn snap(v: f64, grid: f64) -> f64 {
    if grid > 0.0 {
        (v / grid).round() * grid
    } else {
        v.round()
    }
}

/// Maps a cursor x within a view's body strip to a timeline sample through the
/// shared navigation window — the inverse of the renderer's sample→pixel map,
/// used by every timeline gesture (select, locate, clip/note/marker drags).
pub(crate) fn sample_at(nav_start: f64, nav_len: f64, body_x: f64, body_w: f64, x: f64) -> f64 {
    nav_start + nav_len * ((x - body_x) / body_w.max(1.0))
}

/// The clip placement one drag step produces, against the press-time snapshot
/// (`press_sample`, `orig_offset`, `orig_dur`) so a clamped edge never drifts:
/// a body drag moves the offset, an edge drag resizes — the end never crosses
/// the start, the start stays within `[0, end]` — snapped to `grid`.
pub(crate) fn clip_drag_placement(
    part: ClipPart,
    sample: f64,
    press_sample: f64,
    orig_offset: f64,
    orig_dur: f64,
    grid: f64,
) -> (f64, f64) {
    let delta = sample - press_sample;
    let end = orig_offset + orig_dur;
    match part {
        ClipPart::Body => (snap(orig_offset + delta, grid), orig_dur),
        ClipPart::End => {
            let new_end = snap(end + delta, grid).max(orig_offset);
            (orig_offset, new_end - orig_offset)
        }
        ClipPart::Start => {
            let new_off = snap(orig_offset + delta, grid).clamp(0.0, end);
            (new_off, end - new_off)
        }
    }
}

/// The 0..1 fraction a slider press/drag at `(cx, cy)` maps to, by orientation:
/// the cursor x along a horizontal track, or y (bottom = 0, top = 1) on a
/// `vertical` one.
pub(crate) fn slider_t(body: Rect, cx: f64, cy: f64, vertical: bool) -> f32 {
    if vertical {
        controls::slider_fraction_v(body, cy)
    } else {
        controls::slider_fraction(body, cx)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    use super::super::{ClientId, GUI_DEF};
    use super::*;

    fn from() -> ClientId {
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        )))
    }

    /// A window (id 1) with one track (id 5) holding two abutting clips: A
    /// (id 10) over [0, 400), B (id 11) over [400, 400), grid 100.
    fn track_host() -> Host {
        let json = r#"{"type":"window","children":[
            {"id":5,"type":"track","snap":100.0,"children":[
                {"id":10,"type":"clip","offset":0.0,"dur":400.0},
                {"id":11,"type":"clip","offset":400.0,"dur":400.0}
            ]}
        ]}"#;
        let mut host = Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(json.into())],
            }),
            from(),
        );
        host
    }

    /// The lane body and shared nav of the one track, computed the same way the
    /// renderer and hit-test do — so the test hits real pixels.
    fn geometry(host: &Host, fb_w: u32, fb_h: u32) -> (Rect, View) {
        let tree = host.window_def(1).unwrap();
        let nav = track::window_nav(tree);
        let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        let track_rect = layout::layout(area, tree)
            .into_iter()
            .find(|p| matches!(p.widget.kind, WidgetKind::Track { .. }))
            .unwrap()
            .rect;
        (track::lane_body(track_rect, false), nav)
    }

    #[test]
    fn snap_rounds_to_the_grid_or_to_whole_samples() {
        assert_eq!(snap(437.0, 100.0), 400.0);
        assert_eq!(snap(451.0, 100.0), 500.0);
        assert_eq!(snap(12.4, 0.0), 12.0); // no grid: whole samples
    }

    #[test]
    fn sample_at_inverts_the_body_pixel_map() {
        // A 1000-sample window over a 500 px body starting at x = 100.
        assert_eq!(sample_at(0.0, 1000.0, 100.0, 500.0, 100.0), 0.0);
        assert_eq!(sample_at(0.0, 1000.0, 100.0, 500.0, 600.0), 1000.0);
        assert_eq!(sample_at(2000.0, 1000.0, 100.0, 500.0, 350.0), 2500.0);
        // A degenerate body never divides by zero.
        assert!(sample_at(0.0, 1000.0, 100.0, 0.0, 300.0).is_finite());
    }

    #[test]
    fn clip_drag_placement_moves_and_resizes_from_the_snapshot() {
        // Body: the offset follows the delta, snapped; the duration is kept.
        let (off, dur) = clip_drag_placement(ClipPart::Body, 730.0, 500.0, 400.0, 300.0, 100.0);
        assert_eq!((off, dur), (600.0, 300.0));
        // End: resizing never crosses the start (duration floors at 0).
        let (off, dur) = clip_drag_placement(ClipPart::End, 0.0, 690.0, 400.0, 300.0, 100.0);
        assert_eq!(off, 400.0);
        assert!(dur >= 0.0);
        // Start: the onset stays within [0, end], the end fixed.
        let (off, dur) = clip_drag_placement(ClipPart::Start, 0.0, 900.0, 400.0, 300.0, 100.0);
        assert_eq!((off, dur), (0.0, 700.0));
        let (off, dur) = clip_drag_placement(ClipPart::Start, 950.0, 400.0, 400.0, 300.0, 100.0);
        assert_eq!((off, dur), (700.0, 0.0));
    }

    #[test]
    fn clip_part_splits_body_from_edges() {
        // A wide clip: edges at each end, body in the middle.
        assert_eq!(clip_part(100.0, 300.0, 102.0), ClipPart::Start);
        assert_eq!(clip_part(100.0, 300.0, 297.0), ClipPart::End);
        assert_eq!(clip_part(100.0, 300.0, 200.0), ClipPart::Body);
        // Too narrow to grab an edge: all body.
        assert_eq!(clip_part(100.0, 108.0, 101.0), ClipPart::Body);
    }

    #[test]
    fn clip_hit_finds_the_clip_and_the_part_under_the_cursor() {
        let host = track_host();
        let (fb_w, fb_h) = (1000, 200);
        let (body, nav) = geometry(&host, fb_w, fb_h);
        let (ax0, ax1) = track::clip_x_range(body, &nav, 0.0, 400.0).unwrap();
        let midy = (body.y + body.h / 2.0) as f64;

        // The body of clip A → a move on id 10.
        let h = clip_hit(&host, 1, fb_w, fb_h, ((ax0 + ax1) / 2.0) as f64, midy).unwrap();
        assert_eq!((h.id, h.part), (10, ClipPart::Body));
        // Its left/right edges → resize.
        let h = clip_hit(&host, 1, fb_w, fb_h, (ax0 + 2.0) as f64, midy).unwrap();
        assert_eq!((h.id, h.part), (10, ClipPart::Start));
        let h = clip_hit(&host, 1, fb_w, fb_h, (ax1 - 2.0) as f64, midy).unwrap();
        assert_eq!((h.id, h.part), (10, ClipPart::End));
        // Deeper into the lane → clip B.
        let (bx0, bx1) = track::clip_x_range(body, &nav, 400.0, 400.0).unwrap();
        let h = clip_hit(&host, 1, fb_w, fb_h, ((bx0 + bx1) / 2.0) as f64, midy).unwrap();
        assert_eq!(h.id, 11);
        // Over the header strip → no clip.
        assert!(clip_hit(&host, 1, fb_w, fb_h, (body.x - 10.0) as f64, midy).is_none());
    }

    #[test]
    fn clip_set_and_event_args_move_and_report() {
        let mut host = track_host();
        clip_set(&mut host, 1, 10, Some(150.0), Some(250.0));
        // A negative offset clamps to 0.
        clip_set(&mut host, 1, 11, Some(-5.0), None);
        let tree = host.window_def(1).unwrap();
        let args = clip_event_args(tree, 10).unwrap();
        assert_eq!(args[0], OscType::String("clip".into()));
        assert_eq!(args[1], OscType::Float(150.0));
        assert_eq!(args[2], OscType::Float(250.0));
        assert_eq!(clip_event_args(tree, 11).unwrap()[1], OscType::Float(0.0));
    }

    /// A window (id 1) with one `pianoroll` (id 5): a single note spanning the
    /// whole roll at MIDI 60 over the pitch window [48, 72], velocity lane on.
    fn pianoroll_host() -> Host {
        let json = r#"{"type":"window","children":[
            {"id":5,"type":"pianoroll","min":48.0,"max":72.0,"snap":100.0,
             "notes":[0.0,1000.0,60.0,100,0]}
        ]}"#;
        let mut host = Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(json.into())],
            }),
            from(),
        );
        host
    }

    #[test]
    fn pianoroll_hit_finds_a_note_and_the_edit_reports_it() {
        let host = pianoroll_host();
        let (fb_w, fb_h) = (800u32, 400u32);
        // Reconstruct the grid the renderer draws (the default time ruler on, no
        // osc lane, velocity on) so the test aims at real pixels, then hit MIDI
        // 60's row center.
        let tree = host.window_def(1).unwrap();
        let rect = layout::layout(Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32), tree)
            .into_iter()
            .find(|p| matches!(p.widget.kind, WidgetKind::PianoRoll { .. }))
            .unwrap()
            .rect;
        let r = pianoroll::regions(rect, true, false, true);
        let cy = pianoroll::pitch_to_y(60.0, 48.0, 72.0, r.grid) as f64;
        let cx = (r.grid.x + r.grid.w * 0.5) as f64;

        let h = pianoroll_hit(&host, 1, fb_w, fb_h, cx, cy).unwrap();
        assert_eq!(h.region, PrRegion::Grid);
        assert_eq!(h.note.unwrap().index, 0);
        // A press in the velocity lane picks the note under it (its bar sits at
        // the note's start, x ~ grid.x).
        let vy = (r.velocity.y + r.velocity.h * 0.5) as f64;
        let hv = pianoroll_hit(&host, 1, fb_w, fb_h, (r.grid.x + 1.0) as f64, vy).unwrap();
        assert_eq!(hv.region, PrRegion::Velocity);
        assert_eq!(hv.note.unwrap().index, 0);
    }

    #[test]
    fn pianoroll_edit_back_reports_the_notes_as_quintuples() {
        let mut host = pianoroll_host();
        pianoroll_notes_edit(&mut host, 1, 5, |notes| {
            pianoroll::set_velocity(notes, 0, 42)
        });
        let tree = host.window_def(1).unwrap();
        let args = notes_event_args(tree, 5).unwrap();
        assert_eq!(args[0], OscType::String("notes".into()));
        assert_eq!(args.len(), 6); // the tag + one quintuple
        assert_eq!(args[3], OscType::Float(60.0)); // pitch
        assert_eq!(args[4], OscType::Int(42)); // the velocity we set
        assert_eq!(args[5], OscType::Int(0)); // channel
    }

    #[test]
    fn pianoroll_osc_edit_adds_and_reports_a_marker() {
        let mut host = pianoroll_host();
        pianoroll_osc_edit(&mut host, 1, 5, |osc| {
            osc.push(pianoroll::OscMark {
                time: 500.0,
                label: Some("/cue".into()),
            });
        });
        let tree = host.window_def(1).unwrap();
        let args = osc_event_args(tree, 5).unwrap();
        assert_eq!(args[0], OscType::String("osc".into()));
        assert_eq!(args[1], OscType::Float(500.0));
        assert_eq!(args[2], OscType::String("/cue".into()));
    }

    /// A window (id 1) with a `graph` patch (id 7): a source and a sink wired
    /// through the internal bus `mix`, the sink's output on `OUT`.
    fn graph_host() -> Host {
        let json = r#"{"type":"window","children":[
            {"id":7,"type":"graph","label":"chain",
             "members":[{"name":"gsrc","ports":["out"]},
                        {"name":"gsink","ports":["in","out"]}],
             "buses":["mix","OUT"],
             "wires":[0,"out","mix", 1,"in","mix", 1,"out","OUT"]}
        ]}"#;
        let mut host = Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(json.into())],
            }),
            from(),
        );
        host
    }

    fn patch(host: &Host) -> super::super::graph::GraphDraw {
        match &host.window_def(1).unwrap().find(7).unwrap().kind {
            WidgetKind::Graph { graph, .. } => graph.clone(),
            other => panic!("expected a graph, got {other:?}"),
        }
    }

    #[test]
    fn a_wire_dropped_on_a_bus_rewires_the_control_and_reports_it() {
        let mut host = graph_host();
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        let g = patch(&host);
        // Drag the source's `out` (member 0, port 0) onto the `OUT` bus (index 1).
        let bus = super::super::graph::bus_rect(area, &g, 1, 1.0);
        let edit = wire_set(
            &mut host,
            1,
            7,
            (0, 0),
            area,
            bus.x as f64 + 4.0,
            bus.y as f64 + 4.0,
            1.0,
        )
        .unwrap();
        assert_eq!(edit, (0, "out".to_string(), "OUT".to_string()));

        let g = patch(&host);
        let src: Vec<_> = g.wires.iter().filter(|w| w.member == 0).collect();
        assert_eq!(src.len(), 1, "a control touches one bus at a time");
        assert_eq!(src[0].bus, "OUT", "and it is the one it was dropped on");
    }

    #[test]
    fn a_wire_dropped_on_empty_space_unwires_it() {
        let mut host = graph_host();
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        // Released over the middle of the patch: no bus there.
        let edit = wire_set(&mut host, 1, 7, (1, 0), area, 300.0, 380.0, 1.0).unwrap();
        assert_eq!(
            edit,
            (1, "in".to_string(), String::new()),
            "an empty bus = unwired"
        );
        assert!(
            !patch(&host)
                .wires
                .iter()
                .any(|w| w.member == 1 && w.control == "in"),
            "the wire is gone from the patch"
        );
    }
}
