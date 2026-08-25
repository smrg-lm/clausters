//! `knob` — a value turned by how far you drag, not by where you point.
//!
//! The drag **measured from the press**: a knob has no groove on screen to
//! point at, so what turns it is how far the cursor has travelled since it went
//! down — against the value the press found, never against the value as it
//! stands. That is what keeps it in phase with the hand without capturing the
//! pointer: leave the disc, cross the whole window, come back, and the value is
//! what the cursor's distance says it is. A per-step delta would not be, which
//! is the same defect a curve's bend had, and the same fix. The math itself is
//! [`super::control::Dial`]'s, shared with `number`.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::graphics::controls::knob_h;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::Range;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input};
use crate::host::widget::size::{Natural, body_inset, text_box};

use super::control::{self, Dial};

/// A continuous value over `min`..`max`, turned by a vertical drag.
#[derive(Debug, Clone)]
pub struct Knob {
    pub range: Range,
    drag: Dial,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Knob {
    Knob {
        range: Range::parse(props),
        drag: Dial::default(),
    }
}

impl Element for Knob {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        control::set(&mut self.range, key, v)
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::knob(d, &self.range, ctx.rect, self.range.text_size * ctx.scale);
    }

    /// A knob knows its height, not its width: the disc sizes itself to the
    /// shorter side of its body and centres there, so extra width is slack it
    /// absorbs while extra height would stack it under dead space. Elastic
    /// across, so a row of knobs still spreads.
    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        (None, Some(knob_h(&self.range, m, scale)))
    }

    /// Squeezed, a knob gives up its **label strip** and keeps disc and
    /// read-out: a knob with no caption is terser, a knob with no number is a
    /// control you cannot read, and a disc cut into is a control you cannot
    /// aim. Unlabelled it has nothing to give and floors where it stands.
    fn floor(&self, m: &Metrics, scale: f32) -> Natural {
        (
            None,
            Some(knob_h(&self.range, m, scale) - controls::label_give(&self.range, m, scale)),
        )
    }

    /// Asked to be fitted, it has a width after all, and it is the width of the
    /// **whole** control: a knob is a label strip over a disc over a read-out,
    /// all three drawn by one element into one cell, so fitting it to the disc
    /// alone would ellipsize the name and clip the number — parts of the widget
    /// being cut to fit the widget. The three terms, and the widest wins.
    ///
    /// That the natural size says `None` here is not a contradiction: elastic
    /// is the right answer to "how much of the row do you want" — a row of
    /// knobs spreads — and the wrong one to "how big are you".
    fn hug(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.range.text_size * scale;
        let label = self
            .range
            .label
            .as_deref()
            .map_or(0.0, |t| text_box(t, size, m));
        let w = (m.knob_d + body_inset(m))
            .max(label)
            .max(controls::readout_w(&self.range, size, m));
        (Some(w), Some(knob_h(&self.range, m, scale)))
    }

    fn value(&self) -> Option<OscType> {
        control::value(&self.range)
    }

    fn info(&self) -> Vec<(String, Value)> {
        control::info(&self.range)
    }

    /// **The dial is a disc, and only the disc turns.** A knob's cell is a
    /// label strip over the disc over a read-out, so its rectangle is taller
    /// than what is drawn round in it and wider whenever the row spread it: a
    /// press on the name, on the number, or on the paper in a corner was
    /// grabbing the value and turning it. The disc is read off the same
    /// `knob_disc` the drawing places it with, so the two cannot disagree.
    fn hit_area(&self, input: &Input) -> HitArea {
        let body = control::body(&self.range, input);
        let (cx, cy, r) =
            controls::knob_disc(body, self.range.text_size * input.scale, input.metrics);
        HitArea::Disc { cx, cy, r }
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let body_h = control::body(&self.range, input).h;
        self.drag.press(&self.range, body_h, at)
    }

    fn drag(&mut self, at: (f64, f64), _input: &Input) -> Events {
        self.drag.drag(&mut self.range, at)
    }

    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        self.drag.release();
        Events::none()
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::layout::Rect;
    use crate::host::widget::element::{Mods, Take};

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn input<'a>(m: &'a Metrics) -> Input<'a> {
        Input {
            metrics: m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 60.0, 80.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            time: None,
        }
    }

    /// The press takes the drag and reports nothing: turning has not started
    /// yet, and a knob that emitted on every click would send a value nobody
    /// changed. It asks for no pointer capture — the travel since the press is
    /// the gesture, and it is measured the same way on either front.
    #[test]
    fn the_press_takes_the_drag_and_reports_nothing() {
        let m = Metrics::default();
        let mut k = from_props(&props(r#"{"value":0.5}"#));
        match k.press((30.0, 40.0), &input(&m)) {
            Claim::Take(Take { events, .. }) => {
                assert!(events.is_empty(), "and reports nothing yet")
            }
            other => panic!("expected a take, got {other:?}"),
        }
        assert_eq!(k.range.value, 0.5, "and moves nothing");
    }

    /// Dragging up raises the value and dragging down lowers it, and what the
    /// value is depends only on **where the cursor is**, not on the path it
    /// took: the same position twice is the same value twice.
    #[test]
    fn a_position_has_one_answer_however_it_was_reached() {
        let m = Metrics::default();
        let straight = {
            let mut k = from_props(&props(r#"{"value":0.5}"#));
            k.press((30.0, 40.0), &input(&m));
            k.drag((30.0, 20.0), &input(&m));
            k.range.value
        };
        let wandering = {
            let mut k = from_props(&props(r#"{"value":0.5}"#));
            k.press((30.0, 40.0), &input(&m));
            k.drag((30.0, 30.0), &input(&m));
            k.drag((900.0, 500.0), &input(&m)); // off the disc, off the window
            k.drag((30.0, 20.0), &input(&m));
            k.range.value
        };
        assert!(straight > 0.5, "up is more: {straight}");
        assert_eq!(straight, wandering, "the hand's route is not the gesture");
    }

    /// Pinned at an end, the motion spent past it is **kept**: the anchor does
    /// not move, so coming back down is exactly as far as it says. It is the
    /// same rule the bend follows, and the reason a drag that left the widget
    /// does not come back out of phase.
    #[test]
    fn a_pinned_knob_comes_back_where_the_cursor_says() {
        let m = Metrics::default();
        let mut k = from_props(&props(r#"{"value":1.0}"#));
        k.press((30.0, 40.0), &input(&m));
        k.drag((30.0, -460.0), &input(&m)); // far past the top
        assert_eq!(k.range.value, 1.0);
        k.drag((30.0, 40.0), &input(&m)); // back where it started
        assert_eq!(
            k.range.value, 1.0,
            "the press's own value, not a wound-up one"
        );
        k.drag((30.0, 60.0), &input(&m));
        assert!(k.range.value < 1.0, "below the press is below the value");
    }

    /// Nothing turns without a press behind it, and the release drops the
    /// anchor.
    #[test]
    fn a_drag_without_a_press_turns_nothing() {
        let m = Metrics::default();
        let mut k = from_props(&props(r#"{"value":0.5}"#));
        assert!(k.drag((30.0, 20.0), &input(&m)).is_empty());
        assert_eq!(k.range.value, 0.5);
        k.press((30.0, 40.0), &input(&m));
        k.release((30.0, 40.0), true, &input(&m));
        assert!(k.drag((30.0, 20.0), &input(&m)).is_empty());
    }
}
