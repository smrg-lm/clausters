//! What the three continuous controls share: the [`Range`] props and the two
//! ways a pointer moves one.
//!
//! `slider`, `knob` and `number` are one value over one range, differing in the
//! picture and in **how a drag reaches the value** — which is exactly the split
//! the gesture seam makes an element's own business. Two of the three families
//! the trait was designed against are here, side by side, so the difference
//! between them is a few lines rather than two enum variants and two arms in
//! each phase:
//!
//! - **absolute** ([`Track`]) — a position inside a rectangle *is* the value.
//!   The rectangle is snapshotted at the press, because the groove a drag is
//!   measured against may not move under it.
//! - **offset from the press** ([`Dial`]) — how far the cursor has travelled
//!   *since the press*, against the value the press found. There is no groove
//!   on screen to point at, so the gesture is a distance rather than a
//!   position — but it is measured from one fixed anchor, which is what a
//!   curve's bend already does, and for the same two reasons: a per-step delta
//!   drifts out of phase once the pointer leaves the element, and the clamp at
//!   an end eats the motion spent past it.
//!
//! The third family — snapshotted, a press-time origin plus a container's axis
//! and a snap — belongs to the leaves placed on a time axis and lands with
//! them.

use clausters_core::osc::OscType;
use serde_json::Value;

use crate::host::graphics::controls;
use crate::host::layout::Rect;
use crate::host::widget::element::{Claim, Events, Input};
use crate::host::widget::{Range, parse};

/// Applies one `/gui_set` key to a control's range — the props all three share.
pub(super) fn set(r: &mut Range, key: &str, v: &Value) -> bool {
    match key {
        "value" => parse::set_f(&mut r.value, v),
        "min" => parse::set_f(&mut r.min, v),
        "max" => parse::set_f(&mut r.max, v),
        "label" => parse::set_label(&mut r.label, v),
        "text_size" => parse::set_size(&mut r.text_size, v),
        _ => false,
    }
}

/// A control's current value, as `/gui_event` and `/gui_query` carry it.
pub(super) fn value(r: &Range) -> Option<OscType> {
    Some(OscType::Float(r.value))
}

/// What a drag left behind, under the key that sets it — the whole of what a
/// ranged control's `/gui_query` has to correct, since `min`, `max` and the
/// label are the script's and are already current in the document.
pub(super) fn info(r: &Range) -> Vec<(String, Value)> {
    vec![("value".into(), Value::from(r.value))]
}

/// The **absolute** drag: the groove the press landed in, held for as long as
/// the press is. `None` when nothing is being dragged.
#[derive(Debug, Clone, Default)]
pub(super) struct Track(Option<(Rect, bool)>);

impl Track {
    /// Takes the press: the value jumps to where it landed and the groove is
    /// held. `body` is the track the *renderer* drew, so the grab and the
    /// picture cannot disagree about where the value is.
    pub(super) fn press(
        &mut self,
        r: &mut Range,
        body: Rect,
        vertical: bool,
        at: (f64, f64),
    ) -> Claim {
        self.0 = Some((body, vertical));
        r.set_fraction(fraction(body, vertical, at));
        Claim::value(OscType::Float(r.value))
    }

    /// Follows the cursor. Nothing when no press is held (a stray motion).
    pub(super) fn drag(&self, r: &mut Range, at: (f64, f64)) -> Events {
        let Some((body, vertical)) = self.0 else {
            return Events::none();
        };
        r.set_fraction(fraction(body, vertical, at));
        Events::value(OscType::Float(r.value))
    }

    /// Ends it: the groove is only meaningful while the press is held.
    pub(super) fn release(&mut self) {
        self.0 = None;
    }
}

/// Where `at` falls in `body`, as a 0..1 fraction along the control's axis.
fn fraction(body: Rect, vertical: bool, at: (f64, f64)) -> f32 {
    if vertical {
        controls::slider_fraction_v(body, at.1)
    } else {
        controls::slider_fraction(body, at.0)
    }
}

/// The drag **measured from the press**: where the cursor went down, the
/// fraction the value stood at there, and the height the travel is scaled
/// against. `None` when nothing is being dragged.
#[derive(Debug, Clone, Default)]
pub(super) struct Dial(Option<Anchor>);

/// What the press fixes, and what every step of the drag is read against.
#[derive(Debug, Clone, Copy)]
pub(super) struct Anchor {
    /// Where the press landed.
    y: f64,
    /// The value's fraction at that moment.
    t: f32,
    /// The control body's height at the press: how far a full range is.
    body_h: f32,
}

impl Dial {
    /// Takes the press and anchors on it. Nothing is reported — the press alone
    /// changes no value, and a knob that emitted on every click would send a
    /// value nobody turned.
    pub(super) fn press(&mut self, r: &Range, body_h: f32, at: (f64, f64)) -> Claim {
        self.0 = Some(Anchor {
            y: at.1,
            t: r.fraction(),
            body_h,
        });
        Claim::take()
    }

    /// One step: the value the press found, moved by how far the cursor has
    /// travelled since. A given cursor position has **one** answer, so leaving
    /// the element and coming back leaves the value where the pointer says it
    /// is — the same rule a curve's bend follows, and the reason neither needs
    /// the pointer captured to stay in phase with the hand.
    pub(super) fn drag(&mut self, r: &mut Range, at: (f64, f64)) -> Events {
        let Some(a) = self.0 else {
            return Events::none();
        };
        let t = (a.t + controls::drag_fraction_delta(at.1 - a.y, a.body_h)).clamp(0.0, 1.0);
        r.set_fraction(t);
        Events::value(OscType::Float(r.value))
    }

    pub(super) fn release(&mut self) {
        self.0 = None;
    }
}

/// The control body of a placement, at the size table and text size the
/// renderer drew it with — the geometry every one of these presses measures
/// against.
pub(super) fn body(r: &Range, input: &Input) -> Rect {
    controls::body_rect_at(
        input.rect,
        r.label.is_some(),
        r.text_size * input.scale,
        input.metrics,
    )
}
