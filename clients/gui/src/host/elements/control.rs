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
//! - **incremental** ([`Dial`]) — a delta, re-anchored every step against the
//!   value as it stands now. A control pinned at an end has no dead zone:
//!   reversing direction moves it at once instead of sticking and jumping,
//!   which a press-time snapshot would have to unwind.
//!
//! The third family — snapshotted, a press-time origin plus a container's axis
//! and a snap — belongs to the leaves placed on a time axis and lands with
//! them.

use clausters_core::osc::OscType;
use serde_json::Value;

use crate::host::controls;
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

/// The **incremental** drag: the height the delta is scaled against and the
/// cursor's last position, re-anchored every step.
#[derive(Debug, Clone, Default)]
pub(super) struct Dial(Option<(f64, f32)>);

impl Dial {
    /// Takes the press and asks for the pointer grab: a knob turns further than
    /// the screen is tall, and locking the cursor is the front's job, not the
    /// widget's. Nothing is reported — the press alone changes no value.
    pub(super) fn press(&mut self, body_h: f32, at: (f64, f64)) -> Claim {
        self.0 = Some((at.1, body_h));
        Claim::take().grabbing()
    }

    /// One step from a cursor position: the delta since the last step, which is
    /// then the new anchor.
    pub(super) fn drag(&mut self, r: &mut Range, at: (f64, f64)) -> Events {
        let Some((last_y, body_h)) = self.0 else {
            return Events::none();
        };
        self.0 = Some((at.1, body_h));
        self.step(r, at.1 - last_y, body_h)
    }

    /// One step from a **delta**, which is what a grabbed pointer sends: the
    /// cursor is not travelling, so there is no position to anchor against.
    pub(super) fn drag_relative(&mut self, r: &mut Range, delta: (f64, f64)) -> Events {
        let Some((_, body_h)) = self.0 else {
            return Events::none();
        };
        self.step(r, delta.1, body_h)
    }

    pub(super) fn release(&mut self) {
        self.0 = None;
    }

    /// Adds `dy`'s worth to the value **as it stands now** (not to a press-time
    /// snapshot), so a control pinned at an end has no dead zone.
    fn step(&self, r: &mut Range, dy: f64, body_h: f32) -> Events {
        let t = (r.fraction() + controls::drag_fraction_delta(dy, body_h)).clamp(0.0, 1.0);
        r.set_fraction(t);
        Events::value(OscType::Float(r.value))
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
