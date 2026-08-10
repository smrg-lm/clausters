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

/// Every timeline (waveform/spectrogram) widget id in the tree.
pub(super) fn timeline_ids(tree: &Widget) -> Vec<i32> {
    tree.descendants()
        .filter(|w| w.is_nav_signal())
        .filter_map(|w| w.id)
        .collect()
}

/// Every widget id in the tree that navigates a frequency axis of its own.
pub(super) fn freq_nav_ids(tree: &Widget) -> Vec<i32> {
    tree.descendants()
        .filter(|w| w.kind.freq_nav().is_some())
        .filter_map(|w| w.id)
        .collect()
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
    let before = group_view(host, id);
    let roots = host.pan_timeline(id, start - dx_fraction * len);
    if !group_view_moved(host, id, before) {
        return;
    }
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
    let mut moved = true;
    if let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    {
        moved = window_moved((editor.y_start, editor.y_len), (start, len));
        (editor.y_start, editor.y_len) = (start, len);
    }
    if !moved {
        return;
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

/// The **frequency axis** of a navigable spectrum: the body its curve is drawn
/// in, the surface the axis is grabbed on and the window it stands at.
///
/// A spectrum is not a timeline container — it is nobody's coordinate system,
/// so this is not a [`Coords`](super::super::interact::Coords) variant — and it
/// is in no navigation group: the window is the element's own
/// ([`EditorProps::x_view`](super::super::widget::EditorProps::x_view)), like
/// the vertical window of every other axis that measures something.
#[derive(Clone, Copy)]
pub(super) struct FreqAxis {
    /// Where the columns map, exactly what the renderer drew through.
    pub(super) body: Rect,
    /// Where the axis answers to the pointer: the body plus the hertz strip
    /// under it, which is the axis with the ticks drawn on it.
    pub(super) surface: Rect,
    pub(super) start: f64,
    pub(super) len: f64,
    /// The rate the axis is placed by, so a hertz the gesture resolves is the
    /// hertz the frame drew — and so the zoom knows the analysis' resolution.
    pub(super) sample_rate: f64,
}

/// The frequency axis of the widget a hit landed on, if that widget is a
/// navigable spectrum. Resolved through the renderer's own region split, so a
/// zoom anchors at the hertz the reader has the pointer on.
pub(super) fn freq_axis(host: &Host, ctx: &GestureCtx, hit: &interact::Hit) -> Option<FreqAxis> {
    let def_id = ctx.def_id;
    let el = hit.kind.freq_nav()?;
    let r = super::super::spectrum::regions(
        hit.rect,
        el.display.label.is_some(),
        el.editor.ruler != super::super::widget::Ruler::Off,
        el.editor.ruler_y != super::super::widget::RulerY::Off,
        (el.spectral.db_floor, el.spectral.db_ceil),
        host.metrics_for(def_id),
    );
    if r.body.w <= 0.0 || r.body.h <= 0.0 {
        return None;
    }
    let surface = match r.strip_x {
        Some(strip) => Rect::new(r.body.x, r.body.y, r.body.w, r.body.h + strip.h),
        None => r.body,
    };
    // What the axis is showing, not what was asked of it: a gesture anchors in
    // the picture the reader is pointing at.
    let (start, len) = el.freq_window(ctx.sample_rate);
    Some(FreqAxis {
        body: r.body,
        surface,
        start,
        len,
        sample_rate: ctx.sample_rate,
    })
}

/// How far a view window has to move to count as having moved.
///
/// Not a fudge but the resolution the question is asked at. A normalized window
/// spans a body of at most a few thousand pixels, so a billionth of it is a
/// millionth of a pixel; a timeline window is measured in whole samples, so a
/// billionth of one is nothing either. In both units this is float noise rather
/// than a movement — and it matters because a bound that is itself a function
/// of the window's position converges to it by last bits rather than landing on
/// it, and each of those last bits would otherwise be an event.
const VIEW_EPSILON: f64 = 1e-9;

/// Whether a view window actually moved, past [`VIEW_EPSILON`].
///
/// **A gesture that moves nothing says nothing.** An axis pressed against a
/// bound goes on receiving wheel steps and drag motion, and re-emitting the
/// window it already had fills a script's event stream with a view that never
/// changed — the reader turning the wheel at the end of an axis is not asking
/// anything, and the script should not be told they were.
fn window_moved(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() > VIEW_EPSILON || (a.1 - b.1).abs() > VIEW_EPSILON
}

/// The narrowest window spectrum `id`'s frequency axis may be **asked** for at
/// a window starting at `start`: the display width of a handful of its own
/// analysis bins.
///
/// A zoom needs it as a number, because a zoom that overshot the floor and was
/// clamped afterwards would have anchored a window narrower than the one it
/// ends up with, sliding the picture sideways at every step. Everyone else
/// wants the window it produces, which is
/// [`SignalElement::freq_window`](crate::host::signal::SignalElement::freq_window).
pub(super) fn freq_min_span(
    host: &Host,
    def_id: i32,
    id: i32,
    sample_rate: f64,
    start: f64,
) -> f64 {
    let Some(el) = host.widget_kind(def_id, id).and_then(WidgetKind::freq_nav) else {
        return crate::viewport::MIN_SPAN;
    };
    // Through the very geometry the curve and the ruler are drawn with, the
    // fallback rate included — the floor has to be the one the reader sees.
    let (nyquist, f_lo_norm) = super::super::spectrum::axis_geometry(el.freq_rate(sample_rate));
    super::super::spectrum::min_display_span(
        el.spectral.fft_size,
        nyquist * 2.0,
        el.spectral.freq_scale,
        f_lo_norm,
        start,
    )
}

/// The window spectrum `id` is **showing**: its request opened up to what the
/// analysis resolves there.
pub(super) fn freq_window(
    host: &Host,
    def_id: i32,
    id: i32,
    sample_rate: f64,
) -> Option<(f64, f64)> {
    host.widget_kind(def_id, id)?
        .signal()
        .map(|el| el.freq_window(sample_rate))
}

/// The length spectrum `id`'s frequency window was last **asked** for, which is
/// what a pan carries along: a pan moves an axis, and moving one is no reason
/// to spend the zoom the reader set on it.
fn asked_x_len(host: &Host, def_id: i32, id: i32) -> f64 {
    host.widget_kind(def_id, id)
        .and_then(WidgetKind::editor)
        .map_or(1.0, |e| e.x_view().1)
}

/// Pans spectrum `id`'s frequency window to `start`, keeping the length that
/// was asked for rather than the wider one the floor may be granting where the
/// window currently sits.
///
/// The distinction is the whole of why the two are kept apart: a pan down a log
/// axis has to open the window (four bins at 100 Hz are a quarter of the axis),
/// and writing that opening back would make the pan spend the zoom — the way
/// up would then arrive somewhere nobody asked to be, and one gesture would no
/// longer undo itself.
pub(super) fn pan_x_view(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    start: f64,
    sample_rate: f64,
) {
    let len = asked_x_len(host, def_id, id);
    set_x_view(host, out, def_id, id, start, len, sample_rate);
}

/// Writes spectrum `id`'s **frequency** window — the request, clamped through
/// the same normalized axis the vertical one uses — and emits the
/// `"view_x" start len` event carrying the window that request produces. The
/// horizontal sibling of [`set_y_view`], and deliberately not the group's
/// `"view"`: this window belongs to the element, so nothing else moves with it.
///
/// A request that shows the reader exactly what they are already looking at is
/// **not written down**. It is the wheel at the end of an axis: it asks for a
/// window the axis cannot give, so the one already there stands — and with it
/// the length the reader chose where it *was* available, which the axis will
/// hand back the moment the pan returns somewhere it fits.
pub(super) fn set_x_view(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    start: f64,
    len: f64,
    sample_rate: f64,
) {
    let mut axis = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
    axis.set_span(start, len);
    let (start, len) = axis.span();
    let shown = match (
        freq_window(host, def_id, id, sample_rate),
        host.widget_kind(def_id, id)
            .and_then(WidgetKind::signal)
            .map(|el| el.freq_window_of(sample_rate, start, len)),
    ) {
        (Some(before), Some(after)) if !window_moved(before, after) => return,
        (_, after) => after.unwrap_or((start, len)),
    };
    if let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    {
        (editor.x_start, editor.x_len) = (start, len);
    }
    emit(
        out,
        def_id,
        id,
        vec![
            OscType::String("view_x".into()),
            OscType::Float(shown.0 as f32),
            OscType::Float(shown.1 as f32),
        ],
    );
    out.push(GestureEffect::Redraw(def_id));
}

/// Anchor-preserving zoom of a spectrum's frequency axis: `anchor` is the
/// cursor's position across the body (0 = its left edge, 1 = its right), so
/// the frequency under the pointer stays under it.
pub(super) fn zoom_freq(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    axis: FreqAxis,
    cx: f64,
    factor: f64,
) {
    let sample_rate = axis.sample_rate;
    let anchor = ((cx - axis.body.x as f64) / axis.body.w.max(1.0) as f64).clamp(0.0, 1.0);
    let mut a = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
    // The floor belongs to the *zoom*, not only to the write: clamping it
    // afterwards would keep the anchor of a window narrower than the floor, so
    // every further step at the bottom would slide the picture sideways instead
    // of standing still.
    a.set_min_span(freq_min_span(host, def_id, id, sample_rate, axis.start));
    a.set_span(axis.start, axis.len);
    a.zoom(factor, anchor);
    let (start, len) = a.span();
    set_x_view(host, out, def_id, id, start, len, sample_rate);
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
    let before = group_view(host, id);
    let roots = host.zoom_timeline(id, factor, anchor);
    if !group_view_moved(host, id, before) {
        return;
    }
    emit_view(host, out, def_id, id);
    redraw_all(out, &roots);
}

/// Whether view `id`'s group window differs from the `before` snapshot — the
/// timeline sibling of [`window_moved`], in samples rather than normalized
/// units.
fn group_view_moved(host: &Host, id: i32, before: Option<(f64, f64, usize)>) -> bool {
    match (before, group_view(host, id)) {
        (Some(a), Some(b)) => window_moved((a.0, a.1), (b.0, b.1)),
        (a, b) => a.is_some() != b.is_some(),
    }
}
