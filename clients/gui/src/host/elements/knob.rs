//! `knob` — a value turned by how far you drag, not by where you point.
//!
//! The **incremental** drag, and the leaf that proves the pointer grab: a knob
//! turns further than the screen is tall, so the press asks the front to lock
//! the cursor and motion arrives as deltas. Whether it got the lock is the
//! front's answer — a page has no pointer lock — and the element does not have
//! to know: the machine routes positions or deltas accordingly, and the step is
//! the same either way. The math itself is [`super::control::Dial`]'s, shared
//! with `number`.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::controls;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::Range;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input};
use crate::host::widget::size::{Natural, knob_h};

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

    fn value(&self) -> Option<OscType> {
        control::value(&self.range)
    }

    fn info(&self) -> Vec<(String, Value)> {
        control::info(&self.range)
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        self.drag.press(control::body(&self.range, input).h, at)
    }

    fn drag(&mut self, at: (f64, f64), _input: &Input) -> Events {
        self.drag.drag(&mut self.range, at)
    }

    fn drag_relative(&mut self, delta: (f64, f64), _input: &Input) -> Events {
        self.drag.drag_relative(&mut self.range, delta)
    }

    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
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
            rect: Rect::new(0.0, 0.0, 60.0, 80.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            time: None,
        }
    }

    /// The press asks for the grab and reports nothing: turning has not started
    /// yet, and a knob that emitted on every click would send a value nobody
    /// changed.
    #[test]
    fn the_press_grabs_and_reports_nothing() {
        let m = Metrics::default();
        let mut k = from_props(&props(r#"{"value":0.5}"#));
        match k.press((30.0, 40.0), &input(&m)) {
            Claim::Take(Take { events, grab }) => {
                assert!(grab, "a knob wants the pointer");
                assert!(events.is_empty(), "and reports nothing yet");
            }
            other => panic!("expected a grabbing take, got {other:?}"),
        }
        assert_eq!(k.range.value, 0.5, "and moves nothing");
    }

    /// Dragging up raises the value and dragging down lowers it, whichever way
    /// the motion arrives — a locked pointer sends the delta, an unlocked one
    /// sends positions the element differences itself.
    #[test]
    fn positions_and_deltas_turn_it_the_same_way() {
        let m = Metrics::default();
        let by_position = {
            let mut k = from_props(&props(r#"{"value":0.5}"#));
            k.press((30.0, 40.0), &input(&m));
            k.drag((30.0, 30.0), &input(&m));
            k.drag((30.0, 20.0), &input(&m));
            k.range.value
        };
        let by_delta = {
            let mut k = from_props(&props(r#"{"value":0.5}"#));
            k.press((30.0, 40.0), &input(&m));
            k.drag_relative((0.0, -10.0), &input(&m));
            k.drag_relative((0.0, -10.0), &input(&m));
            k.range.value
        };
        assert!(by_position > 0.5, "up is more: {by_position}");
        assert_eq!(by_position, by_delta, "one step, two ways of arriving");
    }

    /// Re-anchoring every step is what removes the dead zone: pinned at an end,
    /// reversing direction moves it at once instead of unwinding a snapshot.
    #[test]
    fn a_pinned_knob_reverses_immediately() {
        let m = Metrics::default();
        let mut k = from_props(&props(r#"{"value":1.0}"#));
        k.press((30.0, 40.0), &input(&m));
        k.drag_relative((0.0, -500.0), &input(&m)); // far past the top
        assert_eq!(k.range.value, 1.0);
        k.drag_relative((0.0, 10.0), &input(&m));
        assert!(k.range.value < 1.0, "one step down, not five hundred");
    }

    /// Nothing turns without a press behind it, and the release drops the
    /// anchor.
    #[test]
    fn a_drag_without_a_press_turns_nothing() {
        let m = Metrics::default();
        let mut k = from_props(&props(r#"{"value":0.5}"#));
        assert!(k.drag_relative((0.0, -20.0), &input(&m)).is_empty());
        assert_eq!(k.range.value, 0.5);
        k.press((30.0, 40.0), &input(&m));
        k.release((30.0, 40.0), &input(&m));
        assert!(k.drag_relative((0.0, -20.0), &input(&m)).is_empty());
    }
}
