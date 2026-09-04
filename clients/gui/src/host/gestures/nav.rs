//! What a gesture *reads and moves*: the tree queries a gesture needs (which
//! widget is under the cursor, what a scroll plane or a timeline group is
//! showing) and the navigation writes it makes (pan, zoom, the selection, a
//! clip's placement).
//!
//! Split from the machine itself so the state machine reads as press -> drag ->
//! release, with the geometry it consults kept beside the effects it emits.

use clausters_core::osc::OscType;

use super::super::Host;
use super::super::bands::Bands;
use super::super::graphics::track;
use super::super::interact::{self, Hit};
use super::super::layout::Rect;
use super::super::placement;
use super::super::widget::element::{FreqAxis, ValueAxis};
use super::super::widget::{ScrollView, Widget, WidgetKind};
use super::effects::{emit, emit_view, redraw_all};
use super::{GestureCtx, GestureEffect};

/// The rectangle spanned by two corner points, whatever their order — the
/// marquee's own geometry, and the one every sweep is drawn from.
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
            host,
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

/// Every timeline (waveform/spectrogram) widget id in the tree.
pub(super) fn timeline_ids(tree: &Widget) -> Vec<i32> {
    tree.descendants()
        // **Everything on the shared axis**, not only the signal views: a lane
        // and a time ruler are on it too, and a multitrack whose clips carry
        // notes, curves or nothing at all has no signal element anywhere. Asked
        // the narrower question this found no view to reset in such a window
        // and did nothing at all — the wheel zoomed the axis and the key that
        // undoes that was inert, which is a worse answer than not offering it.
        .filter(|w| w.is_timeline())
        .filter_map(|w| w.id)
        .collect()
}

/// Every widget id in the tree that navigates a frequency axis of its own.
pub(super) fn freq_nav_ids(tree: &Widget) -> Vec<i32> {
    tree.descendants()
        .filter(|w| w.kind.navigates_freq())
        .filter_map(|w| w.id)
        .collect()
}

/// Writes the selection spanning samples `a..b` (any order, clamped to the
/// timeline) into view `id`'s navigation group — every member follows — and
/// emits **one** `"selection" start len` event, carrying the interacted
/// member's id.
///
/// `value` is the sweep's second axis, already ordered and clamped to the
/// element's domain ([`timeline::value_span`](crate::host::timeline::value_span)),
/// or `None` for a sweep on one axis. It is written to the **widget** rather
/// than to the group (the group is one time axis over views that measure
/// different things vertically) and it rides on the same event, appended: a
/// selection that is only a span is exactly the two numbers this has always
/// sent, so a reader of the old form keeps working and one that understands the
/// second axis is told when there is one. Passing `None` clears any range the
/// widget carried — a new sweep replaces the old selection whole, rather than
/// leaving a restriction from a gesture the hand has finished with.
/// Hands the element the run the hand is holding, or takes it back.
///
/// Returns whether the element is one that can hold one — a press that cannot
/// place its pending has nothing to draw and declines, rather than starting a
/// drag whose feedback would be invisible.
pub(super) fn set_pending(
    host: &mut Host,
    def_id: i32,
    id: i32,
    held: Option<crate::host::widget::element::PendingEdit>,
) -> bool {
    host.window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .is_some_and(|w| w.kind.set_pending_edit(held))
}

/// Extends the stroke the element is holding to cover `[from, to]`, writing a
/// straight ramp between the two values.
///
/// **The samples between two motion events are what this is for.** A pointer
/// reports where it is, not where it went, so a fast stroke arrives as a few
/// widely spaced positions; writing only those would leave the contents combed
/// with holes. Filling them by interpolation is what makes the stroke a stroke.
///
/// The run stays contiguous and may grow either way — a stroke that doubles
/// back keeps one run rather than two, which is what makes it one intent.
pub(super) fn extend_stroke(
    host: &mut Host,
    def_id: i32,
    id: i32,
    channel: usize,
    from: (usize, f32),
    to: (usize, f32),
) {
    let Some(mut held) = host
        .window_def(def_id)
        .and_then(|t| t.find(id))
        .and_then(|w| w.kind.pending_edit())
        .cloned()
    else {
        return;
    };
    let (lo, hi) = (from.0.min(to.0), from.0.max(to.0));
    // Grow the run to cover the new reach, asking the element what each newly
    // covered sample *was* — that is what keeps the intent invertible.
    let read = |frame: usize| -> f32 {
        host.window_def(def_id)
            .and_then(|t| t.find(id))
            .and_then(|w| w.kind.sample_value(channel, frame))
            .unwrap_or(0.0)
    };
    while held.start > lo {
        held.start -= 1;
        held.values.insert(0, read(held.start));
        held.previous.insert(0, read(held.start));
    }
    while held.end() <= hi {
        let frame = held.end();
        held.values.push(read(frame));
        held.previous.push(read(frame));
    }
    // The ramp itself, over the span this motion covered.
    let span = to.0 as f32 - from.0 as f32;
    for frame in lo..=hi {
        let t = if span == 0.0 {
            1.0
        } else {
            (frame as f32 - from.0 as f32) / span
        };
        let v = from.1 + (to.1 - from.1) * t;
        if let Some(slot) = frame
            .checked_sub(held.start)
            .and_then(|i| held.values.get_mut(i))
        {
            *slot = v;
        }
    }
    set_pending(host, def_id, id, Some(held));
}

/// **What a marquee caught**, asked of whichever holds the contents: the
/// element under it, or the lanes of the stack it is sweeping down.
///
/// One call, so the patcher's rectangle and the multitrack's are the same
/// gesture and not two — which is the whole point of there being one
/// [`Drag::Marquee`](super::Drag::Marquee). A rectangle of no size covers
/// nothing, so this is also what a press does, and what makes a click let go.
pub(super) fn marquee_caught(
    host: &mut Host,
    ctx: &GestureCtx,
    at: Option<super::element::At>,
    lanes: Option<&MarqueeLanes>,
    from: (f64, f64),
    to: (f64, f64),
) {
    if let Some(at) = at {
        sweep_element(host, ctx, at, from, to);
    }
    let Some(l) = lanes else {
        return;
    };
    // Against the group's **current** window: the axis may have moved under the
    // sweep, exactly as it may under a span's.
    let (start, len) = group_view(host, l.id).map_or((l.nav_start, l.nav_len), |(s, n, _)| (s, n));
    let sample = |x: f64| interact::sample_at(start, len, l.body.x as f64, l.body.w as f64, x);
    let crossed = match l.stack.across(from.1.min(to.1), from.1.max(to.1)) {
        found if found.is_empty() => vec![l.id],
        found => found,
    };
    interact::select_clips_in(host, ctx.def_id, &crossed, sample(from.0), sample(to.0));
}

/// **What the rectangle caught, of an element's own contents** — a patcher's
/// boxes, a roll's notes — and the band of its second axis it covered, where it
/// has one.
pub(super) fn sweep_element(
    host: &mut Host,
    ctx: &GestureCtx,
    at: super::element::At,
    from: (f64, f64),
    to: (f64, f64),
) -> Option<(f64, f64)> {
    super::element::swept(host, ctx, at, from, to).band
}

pub(super) fn set_selection(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    a: f64,
    b: f64,
    value: Option<(f64, f64)>,
) {
    let Some((start, len, roots)) = host.select_timeline(id, a, b) else {
        return;
    };
    if let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    {
        // The cleared case is the empty pair the reader already means by "no
        // restriction", so there is one convention rather than a second flag.
        (editor.sel_min, editor.sel_max) = value.unwrap_or((0.0, 0.0));
    }
    redraw_all(out, &roots);
    let mut args = vec![
        OscType::String("selection".into()),
        OscType::Float(start as f32),
        OscType::Float(len as f32),
    ];
    if let Some((min, max)) = value {
        args.push(OscType::Float(min as f32));
        args.push(OscType::Float(max as f32));
    }
    emit(host, out, def_id, id, args);
}

/// **Puts the monitor's transport where the hand put the cursor**, and makes
/// the selection the span it loops inside.
///
/// `place` is what separates the two moments a sweep speaks to the transport,
/// and getting it wrong is audible. The **loop follows the drag live** — a span
/// can be redrawn while the take repeats inside it, and setting one never moves
/// the piece, so the sound goes on from where it is and simply wraps somewhere
/// else. The **head is placed once**, by the press: locating on every frame of
/// a drag makes it chase the pointer, which a rolling transport hears as a
/// retrigger per frame rather than as a selection being drawn.
///
/// **Two conditions, and neither is "something is playing".** The host must be
/// the one that bound the governed group (`Host::owns_transport`) — a script
/// owns its own transport, and a sweep in a window it happens to be drawing is
/// not a request to seek it. And the view must draw **contents**: the
/// transport's position is in frames of the piece, so a sweep on a lane
/// measuring beats would send a number that means something else on an axis it
/// does not belong to.
///
/// Notably *not* conditioned on the monitor being loaded, which is where this
/// started and was wrong by use: the head is drawn from the moment the window
/// opens, and a cursor you cannot move until you have played once is a cursor
/// that does not work when you first reach for it.
pub(super) fn transport_follows_selection(
    host: &mut Host,
    def_id: i32,
    id: i32,
    start: f64,
    len: f64,
    place: bool,
) {
    if !host.owns_transport() || host.buffer_of(def_id, id).is_none() {
        return;
    }
    let start = start.max(0.0) as u64;
    if place {
        host.locate(start);
    }
    host.set_loop((len > 0.0).then(|| (start, start + len as u64)));
}

/// Locates the transport: the timeline position under the cursor becomes the
/// group's static cursor (drawn at once on every lane, so the click lands
/// where you see it) and leaves as `/gui_event <id> "locate" <position>` — the
/// script seeks its playhead there, which is what actually moves the music.
/// **The marker a press at `cx` landed on**, over the strip it was drawn in —
/// asked of the group's *current* window, since the axis may have moved since
/// the markers were set.
pub(super) fn marker_under(
    host: &Host,
    id: i32,
    strip: Rect,
    markers: &[crate::host::widget::Marker],
    cx: f64,
) -> Option<usize> {
    let (start, len, _) = group_view(host, id)?;
    let nav = crate::viewport::View { start, len };
    crate::host::frame::marker_at(strip, &nav, markers, cx)
}

/// Writes the widget's markers and reports them: the flat `time label color`
/// list, the same shape a `/gui_set markers` takes and a `/gui_query` gives
/// back — **the time and the text are what the owner is handed**, and what it
/// stores against its own document.
pub(super) fn set_markers(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    markers: Vec<crate::host::widget::Marker>,
) {
    let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    else {
        return;
    };
    editor.markers = markers;
    let args = std::iter::once(OscType::String("markers".into()))
        .chain(
            host.widget_kind(def_id, id)
                .and_then(|k| k.editor().map(|e| e.markers.clone()))
                .unwrap_or_default()
                .iter()
                .flat_map(|m| {
                    [
                        OscType::Double(m.time),
                        OscType::String(m.label.clone()),
                        OscType::String(m.color.clone().unwrap_or_default()),
                    ]
                }),
        )
        .collect();
    emit(host, out, def_id, id, args);
    out.push(GestureEffect::Redraw(def_id));
}

/// The transport's cursor at an **exact** position, rather than at the one a
/// pixel names: what a click on a marker means, since a marker is the moment it
/// was placed at and not the pixel it is drawn on.
pub(super) fn locate_at(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    pos: f64,
) {
    let roots = host.set_timeline_cursor(id, pos);
    emit(
        host,
        out,
        def_id,
        id,
        vec![OscType::String("locate".into()), OscType::Float(pos as f32)],
    );
    redraw_all(out, &roots);
    out.push(GestureEffect::Redraw(def_id));
}

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
    locate_at(host, out, def_id, id, pos);
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

/// **The lane stack a clip can be dragged across**: the lanes sharing the
/// dragged clip's navigation group, top to bottom, as their widget ids and the
/// [`Bands`] their rectangles make.
///
/// A clip changes lane by the same call a note changes row —
/// [`Bands::index_at`] — which is the whole point of there being one vertical
/// axis: the cross-band logic is written once. The stack shares the time axis,
/// so a lane in another navigation group is not somewhere this clip can go: its
/// x is a different window and the drop would land at a position the hand never
/// pointed at.
///
/// Read **once, at the press**: the lanes do not move while a clip is dragged
/// over them, and re-laying the window out per drag step to re-derive them
/// would be the search the hit chain exists to avoid.
#[derive(Clone, Debug, Default)]
pub(super) struct LaneStack {
    /// The lanes' widget ids, top to bottom.
    pub(super) ids: Vec<i32>,
    /// Where the first lane's rectangle starts, in window pixels.
    pub(super) top: f32,
    /// The bands the lanes make, measured from `top`. A gap between two lanes
    /// (a ruler strip, a `gap` in the column) belongs to the lane above it, so
    /// a drop between lanes lands on one rather than on nothing.
    pub(super) bands: Bands,
}

impl LaneStack {
    /// The lane a cursor y falls on, when it falls on one.
    pub(super) fn at(&self, cy: f64) -> Option<i32> {
        self.ids
            .get(self.bands.index_at(cy as f32 - self.top)?)
            .copied()
    }

    /// The lanes a **vertical span** touches, top to bottom — what a marquee
    /// sweeping down the stack catches.
    ///
    /// [`Bands::window`] is the same call a roll makes for the semitone rows a
    /// rectangle crosses, which is the point of one vertical axis: a lane and a
    /// row are one structure, so sweeping across either is one piece of code.
    /// A span that touches nothing (a stack that was never read, a sweep above
    /// the first lane) catches nothing.
    pub(super) fn across(&self, y0: f64, y1: f64) -> Vec<i32> {
        let range = self
            .bands
            .window(y0 as f32 - self.top, y1 as f32 - self.top);
        self.ids.get(range).map(<[i32]>::to_vec).unwrap_or_default()
    }
}

/// The stack `lane_id` belongs to: every `track` in the window on the same
/// navigation group, ordered by where it was placed.
pub(super) fn lane_stack(host: &Host, ctx: &GestureCtx, lane_id: i32) -> LaneStack {
    let Some(placed) = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h) else {
        return LaneStack::default();
    };
    let group = host.timeline_key(lane_id);
    let mut lanes: Vec<(f32, f32, i32)> = placed
        .iter()
        .filter(|p| matches!(p.widget.kind, WidgetKind::Track { .. }))
        .filter_map(|p| Some((p.rect.y, p.rect.h, p.widget.id?)))
        .filter(|(_, _, id)| group.is_none() || host.timeline_key(*id) == group)
        .collect();
    lanes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(&(top, _, _)) = lanes.first() else {
        return LaneStack::default();
    };
    // Each band runs to the next lane's top, so nothing between two lanes is
    // outside the stack; the last band is the last lane's own height.
    let heights: Vec<f32> = (0..lanes.len())
        .map(|i| match lanes.get(i + 1) {
            Some((next_top, _, _)) => next_top - lanes[i].0,
            None => lanes[i].1,
        })
        .collect();
    LaneStack {
        ids: lanes.iter().map(|(_, _, id)| *id).collect(),
        top,
        bands: Bands::table(heights),
    }
}

/// **The press-time snapshot of the clips one lane holds**, in the shape
/// [`placement::move_block`] moves: `(index, offset, row)` per held clip.
pub(super) type HeldClips = Vec<(usize, f64, f32)>;

/// **A block of held clips, per lane** — what one hand carries when it grabs
/// one of a selection a marquee took across the stack.
pub(super) type ClipBlock = Vec<(i32, HeldClips)>;

/// **The stack a marquee is sweeping over**, and the axis it measures time on:
/// what a multitrack needs to answer "which clips did this rectangle cover".
///
/// Read at the press, like a clip drag's stack, for the same reason: the lanes
/// do not move while a hand sweeps over them.
#[derive(Clone)]
pub(super) struct MarqueeLanes {
    /// The lane the press landed on — where the gesture happened, and the
    /// widget the rectangle is drawn over.
    pub(super) id: i32,
    pub(super) body: Rect,
    pub(super) nav_start: f64,
    pub(super) nav_len: f64,
    pub(super) stack: LaneStack,
}

/// One in-flight clip drag, as the placement math needs it: the press-time
/// snapshot plus the lane geometry the cursor maps through.
#[derive(Clone)]
pub(super) struct ClipDrag {
    pub(super) id: i32,
    pub(super) lane: i32,
    pub(super) part: interact::Part,
    pub(super) body_x: f64,
    pub(super) body_w: f64,
    pub(super) nav_start: f64,
    pub(super) nav_len: f64,
    pub(super) press_sample: f64,
    /// The placement the press found: where the clip sat, how long it was, and
    /// which part of its contents it showed.
    pub(super) orig: interact::Placement,
    /// What the contents behind it allows — how many frames there are, and
    /// whether the window loops off them.
    pub(super) contents: interact::Contents,
    pub(super) grid: f64,
    /// The block this drag moves, when the grabbed clip was selected: **per
    /// lane**, the press-time `(index, offset, row)` of every selected clip on
    /// it, the grabbed clip's own lane first and the grabbed clip first in it.
    ///
    /// A selection is not one lane's -- a marquee down the stack takes clips of
    /// several -- and neither is the block that moves it, which is the
    /// patcher's rule for a set of boxes: what the hand grabbed is the whole of
    /// what it holds.
    pub(super) block: ClipBlock,
    /// The lanes this clip can be dragged across, read at the press.
    pub(super) stack: LaneStack,
}

/// Applies a clip drag at cursor `cx`: maps the cursor to a timeline sample,
/// runs the shared placement math (move/resize against the press snapshot,
/// snapped and clamped), writes it and reports it.
///
/// `cy` is the cursor's height, or `None` where the caller has none — the edge
/// scroll, which pans the axis under a held cursor and knows only how far along
/// it is. With no height there is no lane to ask for, so the clip stays on the
/// one it is on.
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
    cy: Option<f64>,
) -> i32 {
    let (nav_start, nav_len) = group_view(host, d.lane)
        .map(|(start, len, _)| (start, len))
        .unwrap_or((d.nav_start, d.nav_len));
    let sample = interact::sample_at(nav_start, nav_len, d.body_x, d.body_w, cx);
    let placed =
        interact::clip_drag_placement(d.part, sample, d.press_sample, d.orig, d.contents, d.grid);
    let mut lane = d.lane;
    if d.block.is_empty() {
        // **The clip can change lane, by the call a note changes row with.**
        // One vertical axis, one `index_at`, one place the cross-band logic is
        // written. A body drag only: an edge trim is a length and says nothing
        // about which lane the clip is on, and a **block** stays on its lane
        // because moving several clips across a stack is several reparents with
        // one snapshot of indices behind them -- the snapshot is what would go
        // stale, and a wrong index moves the wrong clip.
        if d.part == interact::Part::Body
            && let Some(to) = cy.and_then(|cy| d.stack.at(cy)).filter(|to| *to != d.lane)
            && reparent_clip(host, def_id, d.id, d.lane, to)
        {
            lane = to;
        }
        interact::clip_set(host, def_id, d.id, placed);
    } else {
        // **The block moves rigidly by the grabbed clip's own delta**, and the
        // core clamps the whole of it as one — the same call, over the same
        // snapshot shape, that moves a block of notes in a roll. The grabbed
        // clip snapped to the grid; every other clip keeps its distance from
        // it, which is what makes the block a block and not a set of clips that
        // each round differently.
        //
        // **Rigid across lanes too**, which is why the near edge is clamped
        // here rather than left to each lane: `move_block` stops its own
        // snapshot at zero, so a block spanning three lanes would have the
        // lowest clip of each one stop separately and the block would fold as
        // it reached the start. Clamped once against the earliest clip of the
        // whole set, every lane then moves by a delta that is already legal
        // and the per-lane clamp never fires.
        let earliest = d
            .block
            .iter()
            .flat_map(|(_, clips)| clips.iter().map(|(_, offset, _)| *offset))
            .fold(f64::INFINITY, f64::min);
        let dt = (placed.offset - d.orig.offset).max(-earliest);
        for (lane_id, clips) in &d.block {
            if let Some(w) = host
                .window_def_mut(def_id)
                .and_then(|tree| tree.find_mut(*lane_id))
            {
                let row = 0.0;
                let mut lane_clips = track::LaneClips::of(w, row);
                placement::move_block(&mut lane_clips, clips, dt, 0.0, (row, row), None);
            }
        }
    }
    // The lane's extent moved with the clip: re-register it, so the shared axis
    // grows when a clip is dragged past the end — keeping the window's length,
    // so the axis *scrolls* under the drag rather than zooming out from under
    // the cursor (a DAW scrolls at constant zoom; the refit is for content that
    // changes under a still view).
    host.sync_track_totals_keeping_view();
    // **Nothing is emitted here.** One gesture is one edit: the clip follows
    // the hand because the host moved it, and what the hand did on the way is
    // the picture's business rather than the owner's -- the same rule
    // `Drag::Draw` and `Drag::Sample` already state at their own release. A
    // value per frame instead means a document edit per frame: an undo history
    // of a hundred steps for one drag, and a hundred round trips whose
    // acknowledgements the next frame outruns.
    out.push(GestureEffect::Redraw(def_id));
    lane
}

/// **Moves a clip widget from one lane to another**, keeping its own id and
/// everything it holds. Returns whether it moved.
///
/// The picture has to change while the hand is still holding it — a clip that
/// only jumped lanes on release would be drawn on a lane it is not over — so
/// this is the drag's mutation, exactly as writing the offset is. What the
/// **owner** does about it leaves once, at the release (`"lane"`), because one
/// gesture is one edit.
fn reparent_clip(host: &mut Host, def_id: i32, clip: i32, from: i32, to: i32) -> bool {
    let Some(tree) = host.window_def_mut(def_id) else {
        return false;
    };
    let Some(source) = tree.find_mut(from) else {
        return false;
    };
    let Some(at) = source.children.iter().position(|c| c.id == Some(clip)) else {
        return false;
    };
    let widget = source.children.remove(at);
    match tree.find_mut(to) {
        Some(target) => {
            target.children.push(widget);
            true
        }
        None => {
            // The target went away between the press and this step: put the
            // clip back where it was rather than dropping it out of the tree.
            if let Some(source) = tree.find_mut(from) {
                source.children.insert(at, widget);
            }
            false
        }
    }
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
        host,
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

/// The [`FreqAxis`] of the widget a hit landed on, if it navigates one — the
/// widget's own answer, since where the picture sits inside its rectangle is
/// its region split and not the machine's.
pub(super) fn freq_axis(host: &Host, ctx: &GestureCtx, hit: &interact::Hit) -> Option<FreqAxis> {
    hit.kind
        .freq_axis(hit.rect, host.metrics_for(ctx.def_id), ctx.sample_rate)
}

/// The [`ValueAxis`] a sweep on `frame` may restrict itself on: the hit
/// widget's own answer, and only where that widget *is* the container being
/// swept.
///
/// The guard is what keeps the second axis honest rather than approximately
/// right. A view addressed directly measures what it draws, so the value under
/// the pointer is its own; a lane of clips is a container whose contents each
/// have an axis, and which of them a marquee across three clips would be
/// restricting is a question with no answer yet. So a lane's sweep stays the
/// one-axis sweep it was, and says so by declining rather than by guessing.
pub(super) fn value_axis(
    host: &Host,
    ctx: &GestureCtx,
    frame: &interact::Frame,
    hit: &interact::Hit,
) -> Option<ValueAxis> {
    if frame.id != Some(hit.id) {
        return None;
    }
    hit.kind.value_axis(
        hit.rect,
        hit.indent,
        host.metrics_for(ctx.def_id),
        ctx.lanes(hit.id, &hit.kind),
    )
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
/// [`SignalElement::freq_window`](crate::host::elements::signal::SignalElement::freq_window).
pub(super) fn freq_min_span(
    host: &Host,
    def_id: i32,
    id: i32,
    sample_rate: f64,
    start: f64,
) -> f64 {
    host.widget_kind(def_id, id)
        .and_then(|kind| kind.freq_min_span(sample_rate, start))
        .unwrap_or(crate::viewport::MIN_SPAN)
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
        .freq_window_of(sample_rate, None)
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
            .and_then(|kind| kind.freq_window_of(sample_rate, Some((start, len)))),
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
        host,
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
