//! **The models**: what a visual thing is shaped like, how it is drawn, and
//! where a click on that drawing lands.
//!
//! A leaf of the widget catalog is two files, and the split is by *who asks*.
//! The [`elements`](crate::host::elements) half is the surface the passes see: an
//! `impl Element` answering `set`, `needs`, `natural`, `slot`, `body_role` —
//! one method per question the frame, the gesture machine or a `/gui_query`
//! puts to it at a given moment. This half is what that element **knows**: the
//! data shape, the geometry, the drawing over a [`Draw`],
//! and the hit-test primitives that invert that drawing.
//!
//! The boundary is stated negatively, and it is what makes the half worth
//! separating: nothing here mentions [`Host`](super::Host), the
//! [`Element`](crate::host::widget::element::Element) trait, a props map, OSC or a
//! GPU device. Every module in it is unit-testable without a window, which is
//! the property the whole crate leans on for its coverage.
//!
//! The dependency runs **one way** — an element reads its model, never the
//! reverse — and the cardinality is why this is a parallel tree rather than a
//! file beside each element: a model may serve several elements (the standard
//! controls are one drawing for eight of them), may serve an element *and* a
//! container's body (a piano roll draws the `notes` leaf and a clip's inside),
//! and may serve no element at all (a `track` is a container, and its lane is
//! still drawn here).

pub mod bpf;
pub mod controls;
pub mod meters;
pub mod nodetree;
#[cfg(feature = "patcher")]
pub mod patch;
pub mod piano;
pub mod pianoroll;
#[cfg(feature = "notation")]
pub mod score;
pub mod signal;
pub mod textedit;
pub mod track;

use crate::host::font;
use crate::host::layout::Rect;
use crate::host::paint::Draw;

/// A read-out in a body's **top-right corner** — the slot a scope's
/// `lock`/`free` state, a spectral view's scale tag and the frame's own overlay
/// all put a short string in.
///
/// It sits here rather than in whichever model happened to write it first: four
/// callers across three modules and the frame's draw pass share one corner, and
/// a helper with that reach is the models' own vocabulary, not one widget's.
pub(crate) fn corner_text(d: &mut Draw, s: &str, body: Rect) {
    let (mesh, m, theme) = d.parts();
    let w = font::width(s, m.text_scale);
    let x = (body.x + body.w - w - m.pad).max(body.x);
    font::text(mesh, s, x, body.y + m.pad, m.text_scale, theme.text);
}
