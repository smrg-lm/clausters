//! The **coordinate systems** a container gives its contents, and the arithmetic
//! that maps a cursor through one.
//!
//! This is the vocabulary the other three modules speak in: what a container's
//! system *is* ([`Coords`]), the time axis a timeline view draws through
//! ([`TimeAxis`]/[`YAxis`]), the chain of containers over a point ([`Frame`],
//! [`Hit`]) and the readers that pick one system out of a chain ([`plane_of`],
//! [`time_of`], [`local_time_of`]). Beside them sits the small arithmetic that
//! inverts the renderer's maps — a pixel back to a sample ([`sample_at`]) — and
//! the placement one drag step produces ([`clip_drag_placement`]).
//!
//! **Nothing here mentions the [`Host`]**, which is the line that keeps this
//! module the vocabulary rather than a fourth door: it is geometry and types,
//! testable on their own, and every question that needs the tree is asked in
//! [`hit`], [`read`] or [`edit`].
//!
//! [`Host`]: super::super::Host
//! [`hit`]: mod@super::hit
//! [`read`]: super::read
//! [`edit`]: super::edit

use super::super::layout::Rect;
use super::super::widget::{GestureMap, ScrollView, WidgetKind};
use crate::host::graphics::track;
use crate::host::metrics::Metrics;
use crate::viewport::View;

/// The coordinate system a container gives its contents.
///
/// A widget's geometry means nothing on its own: `x: 400` is a window pixel
/// inside a `panel` and a content coordinate inside a `scroll`, and a clip's
/// `offset` is a *sample* on its lane's time axis. The system is the
/// container's property, not the child's, so it is named here once and read off
/// the chain rather than re-derived by each gesture from the kind it happens to
/// have hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Coords {
    /// Rows, columns and grids of the window's own pixels: `window`, `panel`.
    Layout,
    /// A pannable, zoomable plane in content units: `scroll`.
    Plane(ScrollView),
    /// Time along x: every timeline view — a `track` lane placing its clips by
    /// *when* they are, but equally a `waveform`, a `spectrogram`, a
    /// `pianoroll` or a free-standing `timeruler`, whose contents are drawn on
    /// the same axis rather than laid out on it. A view is its own time
    /// container: the axis is the surface the pan, the selection and the locate
    /// all measure against.
    Time(TimeAxis),
    /// A **clip's own span**: its rectangle and the slice of `[0, dur]` that
    /// rectangle shows, resolved by the layout ([`Placed::time`]). A clip is a
    /// coordinate system — everything inside one is placed, drawn and hit
    /// through this alone — but it is *not* a navigable axis: it is not a
    /// navigation-group member, and a pan or a locate started over a clip still
    /// measures against the lane under it. So it is a variant of its own, and
    /// [`time_of`] keeps meaning the axis the groups move.
    ///
    /// [`Placed::time`]: super::super::layout::Placed::time
    Local(TimeAxis),
}

/// The time axis a timeline container gives its contents: where its samples
/// land, the window they are seen through, and — when the view has one — the
/// vertical axis beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TimeAxis {
    /// The rectangle the samples map onto, exactly what the renderer drew
    /// through: a lane minus its header and ruler strip, a heavy view minus its
    /// rulers, a piano-roll's note grid.
    pub body: Rect,
    /// The window of the navigation group the body is seen at.
    pub nav: View,
    /// The vertical axis, when the view has a surface for it.
    pub y: Option<YAxis>,
}

impl TimeAxis {
    /// Whether the cursor is over the axis at all: within the body's **x**
    /// span, whatever its height. The strips stacked under a body — a lane's
    /// time ruler, a roll's velocity and OSC lanes — are on the same axis and
    /// read the same position; a lane's header, beside it, is on no position at
    /// all, which is why a locate or a sweep declines there.
    pub fn spans(&self, cx: f64) -> bool {
        cx >= self.body.x as f64 && cx <= (self.body.x + self.body.w) as f64
    }
}

/// A timeline view's vertical axis: the strip that is its gesture surface (a
/// y-ruler, a roll's keyboard gutter), the display window it stands at, and the
/// pixels one window's worth spans.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct YAxis {
    /// The band left of the body — a press there pans the axis, a wheel over it
    /// zooms it.
    pub strip: Rect,
    /// The visible window (`EditorProps::y_view`) at the press.
    pub start: f64,
    pub len: f64,
    /// How many pixels one window's worth spans: a **lane's** height, since one
    /// vertical window is shared by every channel lane of a stacked view.
    pub lane_h: f64,
}

/// One container over a hit, with the rectangle its coordinate system occupies
/// on screen — the chain's link.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    /// The container's widget id, when it has one (a `/gui_set` target).
    pub id: Option<i32>,
    /// Where the container sits, in window pixels.
    pub rect: Rect,
    /// What a press on this container does, by modifier — the container's own
    /// table, or the default its kind carries.
    pub map: GestureMap,
    pub coords: Coords,
}

/// What a press, a wheel or a move landed on: the deepest interactive widget
/// under the point, plus the **chain** of containers over it — outermost first,
/// ending with the widget itself when it is one.
///
/// The chain is the point of doing this in one pass. A gesture needs more than
/// the widget it hit: the plane to pan, the axis to zoom, the transform its
/// coordinates mean something in. Each of those used to be a second search of
/// the tree by id, which asks the layout to place the whole window again to
/// recover containment the first pass already had.
pub(crate) struct Hit {
    pub id: i32,
    pub rect: Rect,
    /// Where the widget's navigation group starts its body inside `rect`
    /// ([`super::super::layout::Placed::indent`]) — the group's answer, so a
    /// press on a member lands on the same pixels the frame painted.
    pub indent: f32,
    /// The accumulated workspace zoom ([`super::super::layout::Placed::scale`], which the control
    /// hit-math shares with the drawing).
    pub scale: f32,
    pub kind: WidgetKind,
    pub chain: Vec<Frame>,
}

/// The innermost plane in `chain`: its id, its rectangle and its view state.
/// The wheel and the fall-through pan address the workspace itself, whether the
/// point is over its empty area (the `scroll` is the hit) or over a child that
/// consumed nothing (the `scroll` is over it).
pub(crate) fn plane_of(chain: &[Frame]) -> Option<(i32, Rect, ScrollView)> {
    chain.iter().rev().find_map(|f| match f.coords {
        Coords::Plane(view) => Some((f.id?, f.rect, view)),
        _ => None,
    })
}

/// The innermost time axis in `chain`: the container's id and the axis itself.
/// Every gesture on a timeline — locating, panning, selecting, grabbing a clip
/// — measures against this one, so they cannot drift from each other or from
/// the frame the renderer drew.
pub(crate) fn time_of(chain: &[Frame]) -> Option<(i32, TimeAxis)> {
    chain.iter().rev().find_map(|f| match f.coords {
        Coords::Time(axis) => Some((f.id?, axis)),
        _ => None,
    })
}

/// The innermost **clip** span in `chain`: the clip's id and its own axis. What
/// a clip's contents are drawn and edited through — the lane's window is not
/// mentioned past this point.
pub(crate) fn local_time_of(chain: &[Frame]) -> Option<(i32, TimeAxis)> {
    chain.iter().rev().find_map(|f| match f.coords {
        Coords::Local(axis) => Some((f.id?, axis)),
        _ => None,
    })
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

/// Which part of a clip spanning pixels `[x0, x1]` the pointer x fell on —
/// **the strips the grips are drawn on**, and only the ends that carry one.
///
/// It reads the same [`track::clip_grips`] the renderer draws, so the pixels
/// that light up are the pixels that resize; `ends` is which of the clip's own
/// ends are on screen, since an end that is not cannot be grabbed (the
/// rectangle's edge there is the window's, not the clip's). The width is the
/// `grip_w` role: it was a literal in device pixels, which halved the grab zone
/// on a HiDPI screen — a clip was hardest to resize exactly where its edge was
/// thinnest.
pub(crate) fn clip_part(rect: Rect, ends: (bool, bool), m: &Metrics, x: f32) -> ClipPart {
    let (start, end) = track::clip_grips(rect, ends, m);
    if start.is_some_and(|r| x >= r.x && x <= r.x + r.w) {
        ClipPart::Start
    } else if end.is_some_and(|r| x >= r.x && x <= r.x + r.w) {
        ClipPart::End
    } else {
        ClipPart::Body
    }
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
