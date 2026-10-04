//! `number` -- the same value a `knob` holds, read as a figure in a field.
//!
//! Its drag is a knob's, verbatim ([`super::control::Dial`]): the two differ in
//! the picture and in the height they ask for, which is the whole of what a
//! catalog entry is once the interaction lives in one place.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::graphics::controls::field_h;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::Range;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input, Key, KeyInput};
use crate::host::widget::size::Natural;

use super::control::{self, Dial};

/// A continuous value over `min`..`max`, shown as a number and dragged
/// vertically.
#[derive(Debug, Clone)]
pub struct Number {
    pub range: Range,
    /// The `stepper` prop: a pair of arrows at the field's right edge that
    /// move the value one step each.
    pub stepper: bool,
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
        stepper: props
            .get("stepper")
            .and_then(crate::host::widget::parse::truthy)
            .unwrap_or(false),
        drag: Dial::default(),
    }
}

impl Element for Number {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "stepper" => crate::host::widget::parse::truthy(v)
                .map(|b| self.stepper = b)
                .is_some(),
            _ => control::set(&mut self.range, key, v),
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::number(
            d,
            &self.range,
            ctx.rect,
            self.range.text_size * ctx.scale,
            self.stepper,
        );
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        (None, Some(field_h(&self.range, m, scale)))
    }

    /// Squeezed, a field gives up its **label strip** and keeps its box: the
    /// value is the whole widget, and a box shorter than the glyphs inside it
    /// is a drawing that lies.
    fn floor(&self, m: &Metrics, scale: f32) -> Natural {
        (
            None,
            Some(field_h(&self.range, m, scale) - controls::label_give(&self.range, m, scale)),
        )
    }

    fn value(&self) -> Option<OscType> {
        control::value(&self.range)
    }

    fn info(&self) -> Vec<(String, Value)> {
        control::info(&self.range)
    }

    /// **The field, not the cell.** A number is a strip under its label, and
    /// the run of the cell a row stretched around it is the window's: a drag
    /// begun on that air used to turn the value.
    fn hit_area(&self, input: &Input) -> HitArea {
        HitArea::Rect(control::body(&self.range, input))
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let body = control::body(&self.range, input);
        // An arrow of the stepper is a step, by the same rule the keys step
        // it; the rest of the field is the drag it always was.
        if self.stepper {
            let size = self.range.text_size * input.scale;
            let (up, down) = controls::stepper_cells(body, size, input.metrics);
            let key = if up.contains(at.0, at.1) {
                Some(Key::Up)
            } else if down.contains(at.0, at.1) {
                Some(Key::Down)
            } else {
                None
            };
            if let Some(key) = key {
                return Claim::events(
                    control::key(&mut self.range, &key, input.mods).unwrap_or_default(),
                );
            }
        }
        self.drag.press(&self.range, body.h, at)
    }

    fn drag(&mut self, at: (f64, f64), _input: &Input) -> Events {
        self.drag.drag(&mut self.range, at)
    }

    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        self.drag.release();
        Events::none()
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn reports_focus(&self) -> bool {
        false
    }

    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        control::key(&mut self.range, key, input.mods)
    }

    /// The wheel turns it, a notch a step, wherever the pointer is on it.
    fn wheel(&mut self, _at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        Some(control::wheel(&mut self.range, delta.1, input.mods))
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
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 80.0, 24.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            clicks: 1,
            time: None,
        };
        n.press((40.0, 12.0), &input);
        n.drag((40.0, -8.0), &input);
        assert!(n.range.value > 5.0, "up is more: {}", n.range.value);
        assert_eq!(n.value(), Some(OscType::Float(n.range.value)));
    }
}
