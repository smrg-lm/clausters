//! The **coordinate systems** a container gives its contents, and the arithmetic
//! that maps a cursor through one.
//!
//! This is the vocabulary the other three modules speak in: what a container's
//! system *is* ([`Coords`]), the time axis a timeline view draws through
//! ([`TimeAxis`]/[`YAxis`]), the chain of containers over a point ([`Frame`],
//! [`Hit`]) and the readers that pick one system out of a chain ([`plane_of`],
//! [`time_of`], [`local_time_of`]). Beside them sits the small arithmetic that
//! inverts the renderer's maps — a pixel back to a sample ([`sample_at`]), to a
//! value ([`value_at`]) — and the placement one
//! drag step produces ([`clip_drag_placement`]).
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
use super::super::pianoroll;
use super::super::widget::{GestureMap, ScrollView, WidgetKind};
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
    /// A `patch`'s own canvas: boxes and cords placed in canvas units, seen
    /// through the workspace `scale` the frame carries. Its elements are drawn,
    /// not laid out, so the canvas is what a marquee sweeps.
    Canvas,
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
/// y-ruler, a piano-roll's keyboard gutter), the display window it stands at,
/// and the pixels one window's worth spans.
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
    /// The visible slice of the axis **in its own units**, when the axis has a
    /// domain to measure in: a piano-roll's pitch window. A selection swept on
    /// such an axis is a rectangle (time x value), not just a time span.
    pub window: Option<(f64, f64)>,
}

/// One container over a hit, with the rectangle its coordinate system occupies
/// on screen — the chain's link.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    /// The container's widget id, when it has one (a `/gui_set` target).
    pub id: Option<i32>,
    /// Where the container sits, in window pixels.
    pub rect: Rect,
    /// The accumulated workspace zoom the container is drawn at
    /// ([`super::super::layout::Placed::scale`]), which its own contents' geometry is
    /// measured at.
    pub scale: f32,
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

/// The clip edge hit zone, device pixels.
const CLIP_EDGE_PX: f32 = 6.0;

/// Which part of a clip spanning pixels `[x0, x1]` the pointer x fell on.
pub(crate) fn clip_part(x0: f32, x1: f32, x: f32) -> ClipPart {
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

/// The value a timeline container's **vertical** axis reads under the cursor,
/// on the window `[lo, hi]` it is seen at. Discrete today (a piano-roll's
/// pitch, whose rows are centred on whole semitones), which is the only ranged
/// vertical axis there is; a continuous one lands here beside it.
pub(crate) fn value_at(body: Rect, lo: f64, hi: f64, cy: f64) -> f64 {
    pianoroll::y_to_pitch(cy as f32, lo as f32, hi as f32, body) as f64
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

/// **A cursor on a patch's canvas**: the area the patcher was drawn in, the
/// workspace `scale` it was drawn at, and where the pointer is on it.
///
/// The three travel together through every canvas question — a box's rectangle,
/// a port's pin, a cord's drop — because none of them means anything without
/// the other two: an area without a scale places nothing, and a cursor without
/// an area is not on the canvas at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CanvasAt {
    pub area: Rect,
    pub scale: f32,
    pub cx: f64,
    pub cy: f64,
}
