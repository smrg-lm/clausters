//! `toggle` -- a boolean that flips where you click it.
//!
//! The click with no drag behind it: a press flips the state and reports it,
//! and everything after the press is nothing. Which is why it is two lines and
//! not a `Drag` variant.
//!
//! **The state is a boolean; the two values it sends need not be.** What is
//! drawn is a box that is filled or empty, and what is sent is `on` or `off` --
//! `1`/`0` unless the def named another pair, since a bypass lives at
//! `0.0`/`0.7` and a mode at `1`/`2` and neither is a span a widget could be
//! drawn over.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls::{self, ToggleView};
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, control_box, text_box};

use super::switch_value;

/// A boolean on/off control.
#[derive(Debug, Clone)]
pub struct Toggle {
    pub value: bool,
    pub label: Option<String>,
    pub text_size: f32,
    /// The two values the state stands for on the wire.
    pub on: f32,
    pub off: f32,
    /// The `view` prop: the picture -- a box by default, or a switch, or a
    /// button that stays pressed. The state and the values are the same.
    pub view: ToggleView,
    /// The `icon` prop: a glyph of the font drawn with the label.
    pub icon: Option<char>,
}

fn view_of(v: &Value) -> Option<ToggleView> {
    v.as_str().and_then(ToggleView::from_str)
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
        on: parse::number(props, "on", 1.0),
        off: parse::number(props, "off", 0.0),
        view: props.get("view").and_then(view_of).unwrap_or_default(),
        icon: props.get("icon").and_then(crate::host::menu::icon_of),
    }
}

impl Toggle {
    /// The value the state stands for.
    fn sent(&self) -> f32 {
        if self.value { self.on } else { self.off }
    }
}

impl Element for Toggle {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "value" => parse::truthy(v).map(|b| self.value = b).is_some(),
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            "on" => parse::set_f(&mut self.on, v),
            "off" => parse::set_f(&mut self.off, v),
            "view" => view_of(v).map(|view| self.view = view).is_some(),
            "icon" => {
                self.icon = crate::host::menu::icon_of(v);
                true
            }
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::toggle(
            d,
            self.value,
            self.label.as_deref(),
            self.icon,
            ctx.rect,
            self.view,
            ctx.hovered,
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

    /// The box, and the label beside it when there is one -- the row the drawing
    /// lays out, measured.
    fn hug(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        let h = control_box(size, m).max(m.box_side);
        let text = controls::button_text(self.label.as_deref().or(Some("")), self.icon);
        let text = text.trim_end();
        // Drawn as a button it is one: as wide as what it says.
        if self.view == ToggleView::Button {
            return (Some(text_box(text, size, m)), Some(h));
        }
        let side = m.box_side.min(h);
        let mark = match self.view {
            ToggleView::Switch => side * 1.8,
            _ => side,
        };
        // The label starts one pad past the box and gets one more at the right
        // edge, so a hugged toggle never draws its own text into an ellipsis.
        let label = if text.is_empty() {
            0.0
        } else {
            text_box(text, size, m)
        };
        (Some(mark + label), Some(h))
    }

    fn value(&self) -> Option<OscType> {
        Some(switch_value(self.sent()))
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![("value".into(), Value::from(self.value))]
    }

    /// **The box and its label, not the row they were placed in.** A toggle is
    /// a small square with a word beside it, and a layout that stretches the
    /// cell across a panel leaves the rest as air -- air that was flipping the
    /// value when it was clicked.
    fn hit_area(&self, input: &Input) -> HitArea {
        HitArea::Rect(controls::toggle_hit(
            input.rect,
            self.view,
            self.label.as_deref(),
            self.text_size * input.scale,
            input.metrics,
        ))
    }

    fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
        self.value = !self.value;
        Claim::value(switch_value(self.sent()))
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn reports_focus(&self) -> bool {
        false
    }

    /// Space or Enter flips it, as a click does.
    fn activate(&mut self, _input: &Input) -> Option<Events> {
        self.value = !self.value;
        Some(Events::value(switch_value(self.sent())))
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
            clicks: 1,
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
