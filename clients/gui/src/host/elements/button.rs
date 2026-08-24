//! `button` — a push whose **press is the event**, in the two shapes a control
//! signal comes in: a gate that is held, or one message and nothing after it.
//!
//! The smallest thing that has to know it is **being pressed**. Its held state
//! used to travel the other way round the world — into the gesture machine as a
//! `Drag::Button`, and back down into the frame as a `FrameInputs` field — for
//! no reason but that a `WidgetKind` arm is data the host matches on and cannot
//! own anything. Here it is a `bool` on the thing that is held.
//!
//! **Press and release are the primitives**, and everything else a pointer does
//! to a button is composed from them: a click is a press and a release that
//! landed inside, a double click is two of those inside a window. Those are
//! *gestures*, they belong to the gesture machine, and none of them is a mode
//! here — what a mode says is only which of the two primitives reaches the
//! server.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, control_box, text_box};

use super::switch_value;

/// **When a button emits** — the whole of what separates the two control
/// signals one element serves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// `on` at the press, `off` when it is let go: the value lasts exactly as
    /// long as the button is held. What an envelope's gate reads, and what a
    /// trigger control ignores the tail of by definition.
    #[default]
    Gate,
    /// `on` at the press and nothing after it: one message, the bang.
    ///
    /// **A widget cannot make a value instantaneous.** What is sent is held by
    /// whoever receives it, so this is a bang only against something that
    /// returns to zero on its own: a `tr` control, which the server resets
    /// after one block, or a script, for which one message *is* an event. On a
    /// plain control it leaves `on` standing, which is why the clients refuse
    /// to build that pair.
    Press,
}

fn mode_from(v: &Value) -> Option<Mode> {
    match v.as_str()? {
        "gate" => Some(Mode::Gate),
        "press" => Some(Mode::Press),
        _ => None,
    }
}

/// A push button.
#[derive(Debug, Clone)]
pub struct Button {
    pub label: Option<String>,
    pub text_size: f32,
    pub mode: Mode,
    /// What the press sends, and what the release sends under [`Mode::Gate`].
    /// `1`/`0` unless the def named another pair.
    pub on: f32,
    pub off: f32,
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
        mode: props.get("mode").and_then(mode_from).unwrap_or_default(),
        on: parse::number(props, "on", 1.0),
        off: parse::number(props, "off", 0.0),
        held: false,
    }
}

impl Element for Button {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            "mode" => mode_from(v).map(|m| self.mode = m).is_some(),
            "on" => parse::set_f(&mut self.on, v),
            "off" => parse::set_f(&mut self.off, v),
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

    /// A button is momentary either way: the press *is* the event, so what it
    /// reports between presses is the `on` it last sent rather than a state it
    /// keeps.
    fn value(&self) -> Option<OscType> {
        Some(switch_value(self.on))
    }

    fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
        self.held = true;
        Claim::value(switch_value(self.on))
    }

    /// The one line the mode changes. A gate closes; a press already said
    /// everything it had to say, and the button only stops being drawn held.
    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        self.held = false;
        match self.mode {
            Mode::Gate => Events::value(switch_value(self.off)),
            Mode::Press => Events::none(),
        }
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

    /// The bang: one message at the press, and a release that reports nothing
    /// while still letting the box back up.
    #[test]
    fn a_press_button_reports_the_press_and_nothing_after_it() {
        let m = Metrics::default();
        let mut b = from_props(&props(r#"{"label":"go","mode":"press"}"#));
        assert_eq!(
            b.press((10.0, 10.0), &input(&m)),
            Claim::value(OscType::Int(1))
        );
        assert!(b.held);
        assert_eq!(b.release((10.0, 10.0), &input(&m)), Events::none());
        assert!(!b.held, "drawn up again even though it said nothing");
    }

    /// The two values are the def's to name: a button that drives an amplitude
    /// sends that amplitude, not the `1` OSC has instead of a bool.
    #[test]
    fn it_sends_the_pair_it_was_given() {
        let m = Metrics::default();
        let mut b = from_props(&props(r#"{"on":0.7,"off":0.0}"#));
        assert_eq!(
            b.press((10.0, 10.0), &input(&m)),
            Claim::value(OscType::Float(0.7))
        );
        assert_eq!(
            b.release((10.0, 10.0), &input(&m)),
            Events::value(OscType::Int(0)),
            "a whole number stays the int every reader already parses"
        );
    }

    /// Both live, since a mode is a prop like any other.
    #[test]
    fn the_mode_and_the_pair_are_set_live() {
        let mut b = from_props(&props(r#"{}"#));
        assert!(b.set("mode", &Value::from("press")));
        assert_eq!(b.mode, Mode::Press);
        assert!(!b.set("mode", &Value::from("click")), "not a mode here");
        assert!(b.set("on", &Value::from(2.0)));
        assert_eq!(b.on, 2.0);
    }
}
