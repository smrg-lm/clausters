//! `number` — the same value a `knob` holds, read as a figure in a field.
//!
//! Its drag is a knob's, verbatim ([`super::control::Dial`]): the two differ in
//! the picture and in the height they ask for, which is the whole of what a
//! catalog entry is once the interaction lives in one place.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::controls;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::Range;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input};
use crate::host::widget::size::{Natural, field_h};

use super::control::{self, Dial};

/// A continuous value over `min`..`max`, shown as a number and dragged
/// vertically.
#[derive(Debug, Clone)]
pub struct Number {
    pub range: Range,
    drag: Dial,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Number {
    Number {
        range: Range::parse(props),
        drag: Dial::default(),
    }
}

impl Element for Number {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        control::set(&mut self.range, key, v)
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::number(d, &self.range, ctx.rect, self.range.text_size * ctx.scale);
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        (None, Some(field_h(&self.range, m, scale)))
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
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    /// The props and the drag are a knob's; what a `number` declares for itself
    /// is the field's height and the figure in it.
    #[test]
    fn it_is_a_knob_in_a_field() {
        let m = Metrics::default();
        let mut n = from_props(&props(r#"{"min":0,"max":10,"value":5}"#));
        assert_eq!(n.natural(&m, 1.0), (None, Some(field_h(&n.range, &m, 1.0))));

        let input = Input {
            metrics: &m,
            rect: Rect::new(0.0, 0.0, 80.0, 24.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            time: None,
        };
        n.press((40.0, 12.0), &input);
        n.drag_relative((0.0, -20.0), &input);
        assert!(n.range.value > 5.0, "up is more: {}", n.range.value);
        assert_eq!(n.value(), Some(OscType::Float(n.range.value)));
    }
}
