//! `button` — a momentary push: `1` while it is held, `0` when it is let go.
//!
//! The smallest thing that has to know it is **being pressed**. Its held state
//! used to travel the other way round the world — into the gesture machine as a
//! `Drag::Button`, and back down into the frame as a `FrameInputs` field — for
//! no reason but that a `WidgetKind` arm is data the host matches on and cannot
//! own anything. Here it is a `bool` on the thing that is held.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, control_box, text_box};

/// A momentary push button.
#[derive(Debug, Clone)]
pub struct Button {
    pub label: Option<String>,
    pub text_size: f32,
    /// Whether it is being held right now — drawn pressed.
    held: bool,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Button {
    Button {
        label: parse::label(props),
        text_size: parse::text_size(props),
        held: false,
    }
}

impl Element for Button {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::button(
            d,
            self.label.as_deref(),
            ctx.rect,
            self.held,
            self.text_size * ctx.scale,
        );
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        (None, Some(control_box(self.text_size * scale, m)))
    }

    /// A button *is* its box, and its caption is a prop: fitted to its content
    /// it is as wide as the text it centres, padded on both sides.
    fn hug(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        (
            Some(text_box(self.label.as_deref().unwrap_or("BUTTON"), size, m)),
            Some(control_box(size, m)),
        )
    }

    /// A button is momentary: the press *is* the event, so what it reports
    /// between presses is the `1` it last sent rather than a state it keeps.
    fn value(&self) -> Option<OscType> {
        Some(OscType::Int(1))
    }

    fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
        self.held = true;
        Claim::value(OscType::Int(1))
    }

    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        self.held = false;
        Events::value(OscType::Int(0))
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

    fn input<'a>(m: &'a Metrics) -> Input<'a> {
        Input {
            metrics: m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 80.0, 24.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            time: None,
        }
    }

    /// One press, two events, and in between the button knows it is held —
    /// which is the whole of what used to be a drag variant and a frame input.
    #[test]
    fn it_holds_itself_down_between_the_one_and_the_zero() {
        let m = Metrics::default();
        let mut b = from_props(&props(r#"{"label":"go"}"#));
        assert!(!b.held);
        assert_eq!(
            b.press((10.0, 10.0), &input(&m)),
            Claim::value(OscType::Int(1))
        );
        assert!(b.held, "drawn pressed while it is");
        assert_eq!(
            b.release((10.0, 10.0), &input(&m)),
            Events::value(OscType::Int(0))
        );
        assert!(!b.held);
    }
}
