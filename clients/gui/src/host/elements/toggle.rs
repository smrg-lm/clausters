//! `toggle` — a boolean that flips where you click it.
//!
//! The click with no drag behind it: a press flips the state and reports it,
//! and everything after the press is nothing. Which is why it is two lines and
//! not a `Drag` variant.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Input};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, control_box, text_box};

/// A boolean on/off control.
#[derive(Debug, Clone)]
pub struct Toggle {
    pub value: bool,
    pub label: Option<String>,
    pub text_size: f32,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Toggle {
    Toggle {
        value: props.get("value").and_then(parse::truthy).unwrap_or(false),
        label: parse::label(props),
        text_size: parse::text_size(props),
    }
}

impl Element for Toggle {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "value" => parse::truthy(v).map(|b| self.value = b).is_some(),
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::toggle(
            d,
            self.value,
            self.label.as_deref(),
            ctx.rect,
            self.text_size * ctx.scale,
        );
    }

    /// A toggle owns its cell: the box and its label sit on one row, so the
    /// box's own side is the floor its height cannot go under.
    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        (
            None,
            Some(control_box(self.text_size * scale, m).max(m.box_side)),
        )
    }

    /// The box, and the label beside it when there is one — the row the drawing
    /// lays out, measured.
    fn hug(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        let h = control_box(size, m).max(m.box_side);
        let side = m.box_side.min(h);
        // The label starts one pad past the box and gets one more at the right
        // edge, so a hugged toggle never draws its own text into an ellipsis.
        let label = self.label.as_deref().map_or(0.0, |t| text_box(t, size, m));
        (Some(side + label), Some(h))
    }

    fn value(&self) -> Option<OscType> {
        Some(OscType::Int(self.value as i32))
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![("value".into(), Value::from(self.value))]
    }

    fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
        self.value = !self.value;
        Claim::value(OscType::Int(self.value as i32))
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

    #[test]
    fn a_press_flips_it_and_reports_the_new_state() {
        let m = Metrics::default();
        let input = Input {
            metrics: &m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 80.0, 24.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            time: None,
        };
        let mut t = from_props(&props(r#"{"value":1}"#));
        assert_eq!(t.value(), Some(OscType::Int(1)));
        assert_eq!(t.press((10.0, 10.0), &input), Claim::value(OscType::Int(0)));
        assert!(!t.value);
        assert_eq!(t.press((10.0, 10.0), &input), Claim::value(OscType::Int(1)));
    }

    #[test]
    fn a_set_lands_on_its_own_key_and_declines_the_rest() {
        let mut t = from_props(&props("{}"));
        assert!(t.set("value", &Value::from(1)));
        assert!(t.value);
        assert!(t.set("label", &Value::from("on")));
        assert!(!t.set("nonesuch", &Value::from(1)));
    }
}
