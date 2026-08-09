//! What a gesture *reads and moves*: the tree queries a gesture needs (which
//! widget is under the cursor, what a scroll plane or a timeline group is
//! showing) and the navigation writes it makes (pan, zoom, the selection, a
//! clip's placement).
//!
//! Split from the machine itself so the state machine reads as press -> drag ->
//! release, with the geometry it consults kept beside the effects it emits.

use clausters_core::osc::OscType;

use super::super::Host;
use super::super::interact::{self, Hit};
use super::super::layout::Rect;
use super::super::widget::{ScrollView, Widget, WidgetKind};
use super::effects::{emit, emit_clip, emit_view, redraw_all};
use super::{GestureCtx, GestureEffect};

/// The rectangle spanned by two corner points, whatever their order.
pub(crate) fn corner_rect(a: (f64, f64), b: (f64, f64)) -> Rect {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    Rect::new(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32)
}

/// The deepest widget under `(x, y)` and the containers over it — the lane
/// counts the vertical axes are panned through coming off the front's context.
pub(super) fn hit(host: &Host, ctx: &GestureCtx, x: f64, y: f64) -> Option<Hit> {
    interact::hit(host, ctx.def_id, ctx.fb_w, ctx.fb_h, x, y, &|id, kind| {
        ctx.lanes(id, kind)
    })
}

/// A `scroll` widget's **current** view state and configuration. A drag reads
/// it every step: the plane it is panning moves under it, so the chain's
/// press-time snapshot would be one frame stale by the second step.
pub(super) fn scroll_view(host: &Host, def_id: i32, id: i32) -> Option<ScrollView> {
    match host.widget_kind(def_id, id)? {
        WidgetKind::Scroll { view, .. } => Some(*view),
        _ => None,
    }
}

/// Applies a `scroll` view change (clamped through the shared door) and, when
/// it actually moved, emits the `"view" x y zoom` payload and repaints. Always
/// an event, never a bound forward: the view is view state, exactly as the
/// timeline views' `"view"` and the piano's `"range"`.
pub(super) fn set_scroll_view(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    area: Rect,
    next: (f64, f64, f64),
) -> bool {
    if let Some((x, y, zoom)) = interact::scroll_set_view(host, def_id, id, area, next) {
        emit(
            out,
            def_id,
            id,
            vec![
                OscType::String("view".into()),
                OscType::Float(x as f32),
                OscType::Float(y as f32),
                OscType::Float(zoom as f32),
            ],
        );
        out.push(GestureEffect::Redraw(def_id));
        return true;
    }
    false
}

/// The navigation window of timeline view `id`'s group:
/// `(start, len, total)` in timeline samples.
pub(super) fn group_view(host: &Host, id: i32) -> Option<(f64, f64, usize)> {
    host.timeline_nav(id)
        .map(|(nav, total)| (nav.start, nav.len, total))
}

/// A piano-roll note's current `(start, dur)` in the host tree.
pub(super) fn note_at(host: &Host, def_id: i32, id: i32, index: usize) -> Option<(f64, f64)> {
    match host.widget_kind(def_id, id)? {
        WidgetKind::PianoRoll { notes, .. } => notes.get(index).map(|n| (n.start, n.dur)),
        _ => None,
    }
}

/// The diatonic steps a vertical drag of `dy` pixels means on score `id`, whose
/// page is fitted into `rect`.
pub(super) fn score_steps(host: &Host, def_id: i32, id: i32, rect: Rect, dy: f64) -> Option<i32> {
    match host.widget_kind(def_id, id)? {
        WidgetKind::Score(data) => Some(data.steps_for(rect, dy as f32)),
        _ => None,
    }
}

/// Appends every timeline (waveform/spectrogram) widget id in the tree.
pub(super) fn collect_timeline_ids(widget: &Widget, out: &mut Vec<i32>) {
    if widget.is_nav_signal()
        && let Some(id) = widget.id
    {
        out.push(id);
    }
    for child in &widget.children {
        collect_timeline_ids(child, out);
    }
}

/// Writes the selection spanning samples `a..b` (any order, clamped to the
/// timeline) into view `id`'s navigation group — every member follows — and
/// emits **one** `"selection" start len` event, carrying the interacted
/// member's id.
pub(super) fn set_selection(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    a: f64,
    b: f64,
) {
    let Some((start, len, roots)) = host.select_timeline(id, a, b) else {
        return;
    };
    redraw_all(out, &roots);
    emit(
        out,
        def_id,
        id,
        vec![
            OscType::String("selection".into()),
            OscType::Float(start as f32),
            OscType::Float(len as f32),
        ],
    );
}

/// Locates the transport: the timeline position under the cursor becomes the
/// group's static cursor (drawn at once on every lane, so the click lands
/// where you see it) and leaves as `/gui_event <id> "locate" <position>` — the
/// script seeks its playhead there, which is what actually moves the music.
pub(super) fn locate_timeline(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    body: Rect,
    cx: f64,
) {
    let Some((start, len, _total)) = group_view(host, id) else {
        return;
    };
    let pos = interact::sample_at(start, len, body.x as f64, body.w as f64, cx).max(0.0);
    let roots = host.set_timeline_cursor(id, pos);
    emit(
        out,
        def_id,
        id,
        vec![OscType::String("locate".into()), OscType::Float(pos as f32)],
    );
    redraw_all(out, &roots);
    out.push(GestureEffect::Redraw(def_id));
}

pub(super) fn pan_timeline(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    start: f64,
    dx_fraction: f64,
) {
    let Some((_, len, _)) = group_view(host, id) else {
        return;
    };
    let roots = host.pan_timeline(id, start - dx_fraction * len);
    emit_view(host, out, def_id, id);
    redraw_all(out, &roots);
}

/// One in-flight clip drag, as the placement math needs it: the press-time
/// snapshot plus the lane geometry the cursor maps through.
#[derive(Clone, Copy)]
pub(super) struct ClipDrag {
    pub(super) id: i32,
    pub(super) lane: i32,
    pub(super) part: interact::ClipPart,
    pub(super) body_x: f64,
    pub(super) body_w: f64,
    pub(super) nav_start: f64,
    pub(super) nav_len: f64,
    pub(super) press_sample: f64,
    pub(super) orig_offset: f64,
    pub(super) orig_dur: f64,
    pub(super) grid: f64,
}

/// Applies a clip drag at cursor `cx`: maps the cursor to a timeline sample,
/// runs the shared placement math (move/resize against the press snapshot,
/// snapped and clamped), writes it and reports it.
///
/// The cursor maps through the group's **current** window, not the press-time
/// one — that is what lets the edge auto-scroll ([`super::Gestures::tick`]) carry the
/// clip: panning the view under a held cursor moves the sample beneath it, and
/// the clip follows. `press_sample` is already a timeline coordinate, so it
/// stays fixed while the window moves.
pub(super) fn apply_clip_drag(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    d: ClipDrag,
    cx: f64,
) {
    let (nav_start, nav_len) = group_view(host, d.lane)
        .map(|(start, len, _)| (start, len))
        .unwrap_or((d.nav_start, d.nav_len));
    let sample = interact::sample_at(nav_start, nav_len, d.body_x, d.body_w, cx);
    let (new_offset, new_dur) = interact::clip_drag_placement(
        d.part,
        sample,
        d.press_sample,
        d.orig_offset,
        d.orig_dur,
        d.grid,
    );
    interact::clip_set(host, def_id, d.id, Some(new_offset), Some(new_dur));
    // The lane's extent moved with the clip: re-register it, so the shared axis
    // grows when a clip is dragged past the end — keeping the window's length,
    // so the axis *scrolls* under the drag rather than zooming out from under
    // the cursor (a DAW scrolls at constant zoom; the refit is for content that
    // changes under a still view).
    host.sync_track_totals_keeping_view();
    emit_clip(host, out, def_id, d.id);
    out.push(GestureEffect::Redraw(def_id));
}

/// How near a lane body's edge (device pixels) a held clip drag starts pulling
/// the view along with it.
pub(super) const EDGE_MARGIN: f64 = 28.0;

/// How much of the visible window one second pinned against the edge scrolls.
/// Deliberately a *fraction of the window* rather than a pixel rate: zoomed in,
/// a clip must still travel at a usable speed, and zoomed out the same gesture
/// must not fly off the composition.
pub(super) const EDGE_SCROLL_PER_SEC: f64 = 0.9;

/// Writes timeline view `id`'s vertical display window (clamped) into its
/// editor props and emits the `"view_y" y_start y_len` event — the vertical
/// sibling of [`emit_view`]'s range.
pub(super) fn set_y_view(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    start: f64,
    len: f64,
) {
    let mut axis = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
    axis.set_span(start, len);
    let (start, len) = axis.span();
    if let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    {
        (editor.y_start, editor.y_len) = (start, len);
    }
    emit(
        out,
        def_id,
        id,
        vec![
            OscType::String("view_y".into()),
            OscType::Float(start as f32),
            OscType::Float(len as f32),
        ],
    );
    out.push(GestureEffect::Redraw(def_id));
}

/// Anchor-preserving vertical zoom of timeline view `id`: `anchor` in display
/// coordinates (0 = lane bottom, 1 = lane top).
pub(super) fn zoom_timeline_y(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    factor: f64,
    anchor: f64,
) {
    let Some((y0, ylen)) = host
        .widget_kind(def_id, id)
        .and_then(WidgetKind::editor)
        .map(|e| e.y_view())
    else {
        return;
    };
    let mut axis = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
    axis.set_span(y0, ylen);
    axis.zoom(factor, anchor);
    let (start, len) = axis.span();
    set_y_view(host, out, def_id, id, start, len);
}

pub(super) fn zoom_timeline(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    body: Rect,
    cx: f64,
    factor: f64,
) {
    let anchor = ((cx - body.x as f64) / body.w.max(1.0) as f64).clamp(0.0, 1.0);
    let roots = host.zoom_timeline(id, factor, anchor);
    emit_view(host, out, def_id, id);
    redraw_all(out, &roots);
}
