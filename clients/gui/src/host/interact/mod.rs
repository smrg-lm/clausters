//! Pointer-interaction primitives over the widget tree — the value/hit logic
//! shared by both fronts.
//!
//! Hit-testing a point, reading and writing a control's value, flipping a toggle,
//! cycling a menu: all of it is pure work on the [`Host`]'s typed tree plus the
//! [`layout`] and [`controls`] math, with no platform dependency. The native
//! windowed front ([`super::gui`]) and the browser front (`super::web`) both
//! call these, so a turned knob updates the tree and decides bound-vs-event the
//! same way on either platform — only the event *source* (winit vs browser
//! pointer events) and the event *sink* (a socket vs the binding surface) differ.
//!
//! **Module layout.** The file was flat, and what a reader wants from it is
//! never "everything about a clip" but one of four questions, each asked at a
//! different moment of a gesture. So it is four children, and the question is
//! the boundary:
//!
//! - [`coords`] — *in what system?* The container vocabulary ([`Coords`],
//!   [`TimeAxis`](coords::TimeAxis), [`Frame`], [`Hit`]) and the arithmetic that
//!   inverts the renderer's maps. It never mentions the [`Host`], which is what
//!   keeps it the vocabulary rather than a fourth door.
//! - [`hit`](mod@hit) — *what is under the point?* The one layout pass, and the
//!   per-element hit-tests that read its answer finer.
//! - [`edit`] — *write it.* One door per element, both fronts through it.
//! - [`read`] — *what does it hold, and what does it report?* The live values a
//!   drag starts from, and the edit-back payloads a finished edit sends.
//!
//! What the fronts use is re-exported here, so a caller still says
//! `interact::clip_hit` and never names the child — the split is the
//! maintainer's map, not a new surface to learn. A name used only *within* the
//! module is not re-exported, which is what keeps the list below an honest
//! inventory of the door rather than of the file.
//!
//! [`Host`]: super::Host
//! [`layout`]: super::layout
//! [`controls`]: crate::host::graphics::controls

mod coords;
mod edit;
mod hit;
mod read;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

/// A lane header's part, re-exported so the gesture machine names one without
/// reaching into the lane's geometry module: the header is the lane's chrome,
/// and this is the door onto it.
pub(crate) use crate::host::graphics::track::HeaderPart;

pub(crate) use coords::{
    ClipPart, Coords, Frame, Hit, clip_drag_placement, local_time_of, plane_of, sample_at, time_of,
};
pub(crate) use edit::{clip_set, header_set, lane_resize, scroll_set_view};
pub(crate) use hit::{ClipHit, clip_hit, header_hit, hit, sole_time_axis};
pub(crate) use read::{clip_event_args, lane_event_args, plane_can_pan};
