//! `menu` — an option list that opens over everything.
//!
//! The leaf whose state **is not a value**. What a menu holds between clicks is
//! its open list: where it was placed, and the fact that it is up at all. That
//! lived in the gesture machine (a `MenuOpen` beside the drag) *and* in the
//! frame's inputs, because a `WidgetKind` arm cannot own anything and the
//! renderer had to be told what to paint over the window.
//!
//! Here it is one `Option<Rect>` on the menu, and both passes read it through
//! the same declaration: the frame asks `overlay_rect` what to draw last, and
//! the press asks it who to route to first. The list is modal — it swallows a
//! press either way, picking an option on its own rows and closing anywhere
//! else — which is what a menu everywhere else does, and it needs no machine
//! state to be true.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::controls;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Input};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, body_inset, control_box, label_strip};

/// A one-of-several chooser: the options, which one is current, and — while it
/// is up — the list it opened.
#[derive(Debug, Clone)]
pub struct Menu {
    pub options: Vec<String>,
    pub index: usize,
    pub label: Option<String>,
    pub text_size: f32,
    /// The open list's rectangle in window pixels, resolved once at the press
    /// so the drawing and the click cannot disagree about where the rows are.
    open: Option<Rect>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Menu {
    let options = parse::options(props);
    let index = props.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    Menu {
        index: index.min(options.len().saturating_sub(1)),
        options,
        label: parse::label(props),
        text_size: parse::text_size(props),
        open: None,
    }
}

impl Menu {
    /// The option currently chosen (empty when there are none).
    fn current(&self) -> &str {
        self.options.get(self.index).map_or("", String::as_str)
    }

    /// The **body** the list hangs off — the field the chosen option is drawn
    /// in, not the whole cell, so the list lines up with what it replaces
    /// rather than with the label over it.
    fn body(&self, input: &Input) -> Rect {
        controls::body_rect_at(
            input.rect,
            self.label.is_some(),
            self.text_size * input.scale,
            input.metrics,
        )
    }
}

impl Element for Menu {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "index" => v
                .as_u64()
                .map(|n| self.index = (n as usize).min(self.options.len().saturating_sub(1)))
                .is_some(),
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::menu(
            d,
            self.current(),
            self.label.as_deref(),
            ctx.rect,
            self.text_size * ctx.scale,
        );
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        (
            None,
            Some(label_strip(self.label.is_some(), size, m) + body_inset(m) + control_box(size, m)),
        )
    }

    fn value(&self) -> Option<OscType> {
        Some(OscType::Int(self.index as i32))
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        // Already up: this press is the list's, wherever it landed. A row picks
        // that option; anywhere else just closes, and either way the press goes
        // no further.
        if let Some(popup) = self.open.take() {
            return match controls::menu_row_at(popup, self.options.len(), at.0, at.1) {
                Some(row) => {
                    self.index = row;
                    Claim::value(OscType::Int(row as i32))
                }
                None => Claim::take(),
            };
        }
        let size = self.text_size * input.scale;
        self.open = Some(controls::menu_popup(
            self.body(input),
            self.options.len(),
            size,
            input.viewport.1,
            input.metrics,
        ));
        Claim::take()
    }

    fn overlay_rect(&self) -> Option<Rect> {
        self.open
    }

    fn overlay(&self, d: &mut Draw, ctx: &Ctx) {
        let Some(popup) = self.open else {
            return;
        };
        // The row under the cursor highlights, read straight off the frame's
        // pointer — a hover is not a gesture.
        let hover = ctx
            .world
            .cursor
            .and_then(|(cx, cy)| controls::menu_row_at(popup, self.options.len(), cx, cy));
        controls::draw_menu_popup(
            d,
            popup,
            &self.options,
            self.index,
            hover,
            self.text_size * ctx.scale,
        );
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn input<'a>(m: &'a Metrics) -> Input<'a> {
        Input {
            metrics: m,
            rect: Rect::new(0.0, 0.0, 120.0, 24.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
        }
    }

    #[test]
    fn props_parse_and_the_index_clamps() {
        let m = from_props(&props(r#"{"options":["a","b","c"],"index":9}"#));
        assert_eq!(m.options.len(), 3);
        assert_eq!(m.index, 2, "an index past the end clamps to the last");
        assert_eq!(m.current(), "c");

        let empty = from_props(&props("{}"));
        assert_eq!(empty.index, 0);
        assert_eq!(empty.current(), "", "no options is not a panic");
    }

    /// The whole state machine: closed → open (nothing reported yet) → a row
    /// picks and closes. And the list is what the two passes read, so opening
    /// it declares an overlay and picking retires it.
    #[test]
    fn a_press_opens_the_list_and_the_next_one_picks_from_it() {
        let metrics = Metrics::default();
        let mut menu = from_props(&props(r#"{"options":["a","b","c"]}"#));
        assert_eq!(menu.overlay_rect(), None);

        assert_eq!(menu.press((10.0, 10.0), &input(&metrics)), Claim::take());
        let popup = menu.overlay_rect().expect("the list is up");
        assert!(popup.h > 0.0 && popup.w > 0.0);

        // The last row of the list, whatever the metrics made it.
        let last = (popup.x as f64 + 1.0, (popup.y + popup.h) as f64 - 1.0);
        assert_eq!(
            menu.press(last, &input(&metrics)),
            Claim::value(OscType::Int(2))
        );
        assert_eq!(menu.index, 2);
        assert_eq!(menu.overlay_rect(), None, "picking closes it");
    }

    /// A press anywhere else closes it and reports nothing — and is still
    /// taken, which is what keeps the click from also landing on whatever the
    /// list was covering.
    #[test]
    fn a_press_off_the_list_closes_it_and_is_still_taken() {
        let metrics = Metrics::default();
        let mut menu = from_props(&props(r#"{"options":["a","b"],"index":1}"#));
        menu.press((10.0, 10.0), &input(&metrics));
        assert_eq!(menu.press((999.0, 999.0), &input(&metrics)), Claim::take());
        assert_eq!(menu.overlay_rect(), None);
        assert_eq!(menu.index, 1, "and nothing was chosen");
    }

    /// The list is placed against the **window**, not against the cell: near
    /// the bottom edge it opens upward instead of off the screen.
    #[test]
    fn a_list_near_the_bottom_opens_upward() {
        let metrics = Metrics::default();
        let mut menu = from_props(&props(r#"{"options":["a","b","c","d"]}"#));
        let low = Input {
            rect: Rect::new(0.0, 180.0, 120.0, 24.0),
            viewport: (400.0, 200.0),
            ..input(&metrics)
        };
        menu.press((10.0, 190.0), &low);
        let popup = menu.overlay_rect().unwrap();
        assert!(popup.y + popup.h <= 204.0, "it fits: {popup:?}");
    }
}
