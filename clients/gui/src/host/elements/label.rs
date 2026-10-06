//! `label` -- static text in a rect.
//!
//! The first leaf to move behind the trait, and the smallest complete one: four
//! props, one draw, a natural height, and the wheel falling through it. What
//! used to be an arm in `build`, `apply`, `size`, the frame's walk and the
//! bare-surface query is this file.

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::graphics::controls;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Ctx, Element, Needs};
use crate::host::widget::size::{Natural, line_box, text_box};
use crate::host::widget::{Align, parse};

/// Static text. `wrap` word-wraps it on the font's advance (off, a single line
/// clipped with an ellipsis); `align` places each line in the rect.
///
/// `live` makes the text a **reading**: something written many times a
/// second, as a clock's is. It is then drawn on the window's live layer, so a
/// write costs the label's own triangles and not the window's picture -- and
/// it sizes nothing, since a container that followed it would be laid out on
/// every write.
#[derive(Debug, Clone)]
pub struct Label {
    pub text: String,
    pub text_size: f32,
    pub wrap: bool,
    pub align: Align,
    pub live: bool,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `label` node carries, read once -- shared by the constructor
/// and by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Label {
    Label {
        text: props
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        text_size: parse::text_size(props),
        wrap: props.get("wrap").and_then(parse::truthy).unwrap_or(false),
        align: Align::parse(props),
        live: props.get("live").and_then(parse::truthy).unwrap_or(false),
    }
}

impl Label {
    fn paint(&self, d: &mut Draw, ctx: &Ctx) {
        controls::draw_label(
            d,
            &self.text,
            ctx.rect,
            self.text_size * ctx.scale,
            self.wrap,
            self.align,
        );
    }
}

impl Element for Label {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "text" => v.as_str().map(|s| self.text = s.to_string()).is_some(),
            "text_size" => v
                .as_f64()
                .filter(|n| *n > 0.0)
                .map(|n| self.text_size = n as f32)
                .is_some(),
            "wrap" => parse::truthy(v).map(|b| self.wrap = b).is_some(),
            "live" => parse::truthy(v).map(|b| self.live = b).is_some(),
            "align" => v
                .as_str()
                .and_then(Align::from_str)
                .map(|a| self.align = a)
                .is_some(),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        if !self.live {
            self.paint(d, ctx);
        }
    }

    /// A reading is the whole of what a live label draws: its rect is the
    /// window's picture, and the text over it is what a write changes.
    fn draw_live(&self, d: &mut Draw, ctx: &Ctx) {
        if self.live {
            self.paint(d, ctx);
        }
    }

    fn live_prop(&self, key: &str) -> bool {
        self.live && key == "text"
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        (
            None,
            // A wrapped label's line count follows its string, which is data:
            // it stays elastic and clips what does not fit.
            (!self.wrap).then(|| line_box(self.text_size * scale, m)),
        )
    }

    /// A label's `text` is a presentation prop resolved at a mutation point, so
    /// a container fitted to its content is as wide as the line it draws. A
    /// **wrapped** label is the exception on both axes: its width is what wraps
    /// it, so it has none of its own.
    fn hug(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        (
            // A reading sizes nothing: its text is written on a tick, and a
            // container fitted to it would be laid out on each one.
            (!self.wrap && !self.live).then(|| text_box(&self.text, size, m)),
            self.natural(m, scale).1,
        )
    }

    fn value(&self) -> Option<OscType> {
        None
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![("text".into(), Value::from(self.text.clone()))]
    }

    fn needs(&self) -> Needs {
        Needs {
            live: self.live,
            ..Needs::default()
        }
    }

    /// A label puts marks on its rect and navigates nothing: in a window with
    /// one navigation group, its pixels are that axis with something written
    /// on them, so the wheel means there what it means over a lane.
    fn is_bare_surface(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn text_props_parse_and_default() {
        let l = from_props(&props(
            r#"{"text":"hi","text_size":3.5,"wrap":1,"align":"center"}"#,
        ));
        assert_eq!(l.text, "hi");
        assert_eq!(l.text_size, 3.5);
        assert!(l.wrap);
        assert_eq!(l.align, Align::Center);

        let l = from_props(&props(r#"{"text":"hi"}"#));
        assert_eq!(l.text_size, crate::host::font::DEFAULT_SIZE);
        assert!(!l.wrap);
        assert_eq!(l.align, Align::Start);
    }

    /// **A reading is drawn on the live layer and sizes nothing.** A label
    /// that says `live` leaves its rect to the window's picture and draws its
    /// text over it, where a write of the text is the only thing redrawn; a
    /// container fitted to its content does not follow it.
    #[test]
    fn a_live_label_draws_its_text_on_the_live_layer() {
        use crate::host::layout::Rect;
        use crate::host::paint::Mesh;
        use crate::host::theme::Theme;
        use crate::host::world::World;

        let m = Metrics::default();
        let theme = Theme::default();
        let world = World::default();
        let ctx = Ctx {
            world: &world,
            metrics: &m,
            rect: Rect::new(0.0, 0.0, 200.0, 20.0),
            indent: 0.0,
            clip: None,
            scale: 1.0,
            time: None,
            focused: false,
            hovered: false,
            clock: 0.0,
        };
        let drawn = |label: &Label, live: bool| {
            let mut mesh = Mesh::new();
            let mut d = Draw::new(&mut mesh, &m, &theme);
            if live {
                label.draw_live(&mut d, &ctx);
            } else {
                label.draw(&mut d, &ctx);
            }
            !mesh.is_empty()
        };
        let plain = from_props(&props(r#"{"text":"12.3 s"}"#));
        assert!(drawn(&plain, false) && !drawn(&plain, true));
        assert!(!plain.live_prop("text") && !plain.needs().live);
        assert!(plain.hug(&m, 1.0).0.is_some());

        let reading = from_props(&props(r#"{"text":"12.3 s","live":true}"#));
        assert!(!drawn(&reading, false) && drawn(&reading, true));
        assert!(reading.live_prop("text") && reading.needs().live);
        assert!(!reading.live_prop("text_size"), "its size is the picture's");
        assert!(reading.hug(&m, 1.0).0.is_none(), "it sizes nothing");
    }

    /// A wrapped label's line count follows its string, which is data -- so it
    /// stays elastic and clips, while an unwrapped one knows its one line.
    #[test]
    fn only_an_unwrapped_label_knows_its_height() {
        let m = Metrics::default();
        let one = from_props(&props(r#"{"text":"hi"}"#));
        assert_eq!(one.natural(&m, 1.0).1, Some(line_box(one.text_size, &m)));
        assert_eq!(
            from_props(&props(r#"{"text":"hi","wrap":1}"#))
                .natural(&m, 1.0)
                .1,
            None
        );
    }

    #[test]
    fn a_set_lands_on_its_own_key_and_declines_the_rest() {
        let mut l = from_props(&props(r#"{"text":"hi"}"#));
        assert!(l.set("text", &Value::from("bye")));
        assert_eq!(l.text, "bye");
        assert!(l.set("align", &Value::from("end")));
        assert_eq!(l.align, Align::End);
        assert!(!l.set("nonesuch", &Value::from(1)));
    }
}
