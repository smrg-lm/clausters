//! `number` -- the same value a `knob` holds, read as a figure in a field.
//!
//! Its drag is a field's ([`super::control::Scrub`]): a step a pixel of
//! travel, Shift for ten and Alt for a tenth, since a knob's drag spreads the
//! range over a few heights of a body that here is one line tall.
//!
//! **And it is typed into**, as a numeric field is: with the focus, a digit,
//! a point or a minus starts a figure in place of the value shown, Backspace
//! takes its last character back, and Enter, or the focus going elsewhere,
//! sets the value to it, within the range and on the step. An arrow drops
//! what was typed and steps the value. The field draws the figure alone, with
//! no level behind it: it is read, not watched.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::graphics::controls::field_h;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::Range;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input, Key, KeyInput};
use crate::host::widget::size::Natural;

use super::control::{self, Scrub};

/// A continuous value over `min`..`max`, shown as a number and dragged
/// vertically.
#[derive(Debug, Clone)]
pub struct Number {
    pub range: Range,
    /// The `stepper` prop: a pair of arrows at the field's right edge that
    /// move the value one step each.
    pub stepper: bool,
    drag: Scrub,
    /// The figure being typed, until Enter or the focus going elsewhere
    /// sets it.
    typed: Option<String>,
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
        drag: Scrub::default(),
        typed: None,
    }
}

impl Number {
    /// **Sets the value to the figure typed**, within the range and on the
    /// step, and reports it; nothing for no figure or one that is no number.
    fn commit(&mut self) -> Events {
        let Some(typed) = self.typed.take() else {
            return Events::none();
        };
        let Ok(value) = typed.parse::<f32>() else {
            return Events::none();
        };
        let (lo, hi) = (
            self.range.min.min(self.range.max),
            self.range.min.max(self.range.max),
        );
        let mut value = value.clamp(lo, hi);
        if self.range.step > 0.0 {
            value = (lo + ((value - lo) / self.range.step).round() * self.range.step).min(hi);
        }
        self.range.value = value;
        Events::value(OscType::Float(value))
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
            self.typed.as_deref(),
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
        self.drag.press(&self.range, at, input.mods)
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        self.drag.drag(&mut self.range, at, input.scale, input.mods)
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
        match key {
            Key::Char(c) if c.is_ascii_digit() || matches!(c, '.' | ',' | '-') => {
                let c = if *c == ',' { '.' } else { *c };
                self.typed.get_or_insert_with(String::new).push(c);
                Some(Events::none())
            }
            Key::Char(_) => None,
            Key::Backspace => {
                let typed = self
                    .typed
                    .get_or_insert_with(|| controls::fmt(self.range.value));
                typed.pop();
                Some(Events::none())
            }
            Key::Enter => Some(self.commit()),
            Key::Escape if self.typed.is_some() => {
                self.typed = None;
                Some(Events::none())
            }
            _ => {
                self.typed = None;
                control::key(&mut self.range, key, input.mods)
            }
        }
    }

    fn blur(&mut self) -> Events {
        self.commit()
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

    /// The props are a knob's; what a `number` declares for itself is the
    /// field's height and the figure in it.
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

    /// **A field drags a step a pixel**: ten pixels up take a tuning a
    /// hertz at a step of a tenth, Shift ten steps a pixel and Alt a tenth
    /// of one, and a modifier pressed mid-drag goes on from where the value
    /// is rather than jumping.
    #[test]
    fn a_field_drags_a_step_a_pixel() {
        let m = Metrics::default();
        let mut n = from_props(&props(r#"{"min":200,"max":1000,"step":0.1,"value":440}"#));
        let mut input = Input {
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
        n.drag((40.0, 2.0), &input);
        assert!((n.range.value - 441.0).abs() < 1e-3, "{}", n.range.value);
        input.mods.shift = true;
        n.drag((40.0, 2.0), &input);
        assert!(
            (n.range.value - 441.0).abs() < 1e-3,
            "no jump: {}",
            n.range.value
        );
        n.drag((40.0, -8.0), &input);
        assert!((n.range.value - 451.0).abs() < 1e-3, "{}", n.range.value);
        input.mods = Mods {
            alt: true,
            ..Mods::default()
        };
        n.drag((40.0, -8.0), &input);
        n.drag((40.0, -18.0), &input);
        assert!((n.range.value - 451.1).abs() < 1e-3, "{}", n.range.value);
        n.release((40.0, -18.0), true, &input);
        // at twice the scale, a logical pixel is two device ones
        input.mods = Mods::default();
        input.scale = 2.0;
        n.press((40.0, 12.0), &input);
        n.drag((40.0, -8.0), &input);
        assert!((n.range.value - 452.1).abs() < 1e-3, "{}", n.range.value);
    }

    /// **A number is typed into**: a figure replaces the value shown, Enter
    /// sets it within the range and on the step, the focus going elsewhere
    /// sets it too, and an arrow drops what was typed and steps the value.
    #[test]
    fn a_figure_typed_sets_the_value_on_enter_or_blur() {
        let mut clip = crate::host::clipboard::Clip::default();
        let mut n = from_props(&props(r#"{"min":200,"max":1000,"step":0.1,"value":440}"#));
        let mut type_in = |n: &mut Number, keys: &[Key]| {
            let mut input = KeyInput {
                mods: Mods::default(),
                clipboard: &mut clip,
                cursor: None,
            };
            keys.iter().map(|k| n.key(k, &mut input)).last().flatten()
        };
        let digits = |s: &str| s.chars().map(Key::Char).collect::<Vec<_>>();
        type_in(&mut n, &digits("432,57"));
        assert_eq!(n.range.value, 440.0, "nothing is set while typing");
        assert_eq!(n.typed.as_deref(), Some("432.57"));
        let set = type_in(&mut n, &[Key::Backspace, Key::Enter]).expect("consumed");
        assert!((n.range.value - 432.5).abs() < 1e-3, "{}", n.range.value);
        assert_eq!(set, Events::value(OscType::Float(n.range.value)));
        type_in(&mut n, &digits("5000"));
        n.blur();
        assert_eq!(n.range.value, 1000.0, "within the range");
        type_in(&mut n, &digits("3"));
        type_in(&mut n, &[Key::Down]);
        assert!(n.typed.is_none(), "an arrow drops the figure");
        assert!((n.range.value - 999.9).abs() < 1e-3, "{}", n.range.value);
        assert_eq!(
            type_in(&mut n, &[Key::Char('x')]),
            None,
            "a letter is the window's"
        );
    }
}
