//! `choice` -- one of several, picked from a list that opens over the window.
//!
//! A combobox with no typing: a field showing the chosen option, and a list of
//! all of them under it. What it holds is the options and which one is
//! current; **the list is not its own**. It asks the host to open one
//! ([`Events::and_popup`]) and is told which row was picked
//! ([`Element::picked`]), so where the list goes, how it scrolls and which
//! keys walk it are the popup layer's ([`crate::host::popup`]) and the same
//! for every list in the window.
//!
//! It was called `menu`, which is the name of the other thing: a tree of
//! entries that report verbs ([`crate::host::menu`]). This one reports a
//! value -- the index of the option -- like every other control.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::choice::{self, Part, View};
use crate::host::graphics::controls;
use crate::host::layout::Rect;
use crate::host::menu::Entry;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{
    Claim, Ctx, Element, Events, HitArea, Input, Key, KeyInput, PopupRequest,
};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, body_inset, label_strip, text_box};

/// A one-of-several chooser: the options, which one is current, and how it is
/// presented.
#[derive(Debug, Clone)]
pub struct Choice {
    pub options: Vec<String>,
    pub index: usize,
    pub label: Option<String>,
    pub text_size: f32,
    /// The `view` prop: the picture -- a combobox by default, or a radio
    /// group, a segmented control, tabs, a pager, a list
    /// ([`crate::host::graphics::choice`]). The data, the value and the event
    /// are the same in all of them.
    pub view: View,
    /// The option the first row of the open list stands for: `0` for a
    /// combobox's, the first gathered tab for a row of tabs that did not fit.
    list_from: usize,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn view_of(v: &Value) -> Option<View> {
    v.as_str().and_then(View::from_str)
}

fn from_props(props: &Map<String, Value>) -> Choice {
    let options = parse::options(props);
    let index = props.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    Choice {
        index: index.min(options.len().saturating_sub(1)),
        options,
        label: parse::label(props),
        text_size: parse::text_size(props),
        view: props.get("view").and_then(view_of).unwrap_or_default(),
        list_from: 0,
    }
}

impl Choice {
    /// The option currently chosen (empty when there are none).
    #[cfg(test)]
    fn current(&self) -> &str {
        self.options.get(self.index).map_or("", String::as_str)
    }

    /// The **body** the parts are drawn in -- the cell minus its label strip
    /// and inset, the same rectangle the renderer draws them in.
    fn body(&self, input: &Input) -> Rect {
        controls::body_rect_at(
            input.rect,
            self.label.is_some(),
            self.text_size * input.scale,
            input.metrics,
        )
    }

    fn parts(&self, input: &Input) -> Vec<(Part, Rect)> {
        choice::parts(
            self.view,
            &self.options,
            self.body(input),
            self.text_size * input.scale,
            input.metrics,
        )
    }

    /// The list a press opens under `anchor`: the options from `from` on, the
    /// chosen one marked.
    fn list(&mut self, from: usize, anchor: Rect, size: f32) -> PopupRequest {
        self.list_from = from;
        PopupRequest {
            entries: self
                .options
                .iter()
                .enumerate()
                .skip(from)
                .map(|(i, o)| Entry::option(o, i == self.index))
                .collect(),
            anchor,
            text_size: size,
        }
    }

    /// Moves the choice to `to` and reports it -- nothing, when it is already
    /// there.
    fn choose(&mut self, to: usize) -> Events {
        if to == self.index || to >= self.options.len() {
            return Events::none();
        }
        self.index = to;
        Events::value(OscType::Int(to as i32))
    }
}

impl Element for Choice {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "index" => v
                .as_u64()
                .map(|n| self.index = (n as usize).min(self.options.len().saturating_sub(1)))
                .is_some(),
            "options" => {
                let mut props = Map::new();
                props.insert("options".into(), v.clone());
                self.options = parse::options(&props);
                self.index = self.index.min(self.options.len().saturating_sub(1));
                true
            }
            "view" => view_of(v).map(|view| self.view = view).is_some(),
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let size = self.text_size * ctx.scale;
        // The part under the pointer, read off the same parts the press reads.
        let hover = ctx
            .hovered
            .then_some(ctx.world.cursor)
            .flatten()
            .and_then(|(x, y)| {
                let body =
                    controls::body_rect_at(ctx.rect, self.label.is_some(), size, ctx.metrics);
                let parts = choice::parts(self.view, &self.options, body, size, ctx.metrics);
                choice::part_at(&parts, x, y)
            });
        choice::draw(
            d,
            self.view,
            &self.options,
            self.index,
            self.label.as_deref(),
            ctx.rect,
            size,
            hover,
        );
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        (
            None,
            Some(
                label_strip(self.label.is_some(), size, m)
                    + body_inset(m)
                    + choice::body_h(self.view, self.options.len(), size, m),
            ),
        )
    }

    /// Squeezed, a chooser gives up its **label strip** and keeps its body --
    /// the caption names the choice, the body *is* it.
    fn floor(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        (
            None,
            Some(body_inset(m) + choice::body_h(self.view, self.options.len(), size, m)),
        )
    }

    /// The **options**, not the chosen one: the set is a prop and the choice is
    /// a value, so a hugged chooser is as wide as its options need and does
    /// not resize when the reader picks a shorter one. A label over it may be
    /// wider still.
    fn hug(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        let body = choice::body_w(self.view, &self.options, size, m) + 2.0 * m.pad;
        let label = self.label.as_deref().map_or(0.0, |t| text_box(t, size, m));
        (Some(body.max(label)), self.natural(m, scale).1)
    }

    fn value(&self) -> Option<OscType> {
        Some(OscType::Int(self.index as i32))
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![("index".into(), Value::from(self.index))]
    }

    /// **The body the parts are drawn in** -- never the label strip over it or
    /// the cell around it.
    fn hit_area(&self, input: &Input) -> HitArea {
        HitArea::Rect(self.body(input))
    }

    /// A press on a part does what the part is: an option is chosen, a field
    /// or the gathered tabs open their list, an arrow steps. Between the parts
    /// -- the air beside a short row of tabs -- it is still this control's and
    /// does nothing.
    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let size = self.text_size * input.scale;
        let parts = self.parts(input);
        let Some(part) = choice::part_at(&parts, at.0, at.1) else {
            return Claim::take();
        };
        let rect = parts
            .iter()
            .find(|(p, _)| *p == part)
            .map_or(input.rect, |(_, r)| *r);
        Claim::events(match part {
            Part::Option(i) => self.choose(i),
            Part::Prev => self.choose(self.index.saturating_sub(1)),
            Part::Next => self.choose(self.index + 1),
            Part::Field if self.options.is_empty() => Events::none(),
            Part::Field => Events::none().and_popup(self.list(0, rect, size)),
            Part::More(from) => Events::none().and_popup(self.list(from, rect, size)),
        })
    }

    /// The row a hand picked is the option at that position, counted from the
    /// one the list began at.
    fn picked(&mut self, path: &[usize], _input: &Input) -> Events {
        match path {
            [row] => self.choose(self.list_from + row),
            _ => Events::none(),
        }
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn reports_focus(&self) -> bool {
        false
    }

    /// The arrows move the choice, Home and End take it to the first and the
    /// last option -- in every view, the list of a combobox closed.
    fn key(&mut self, key: &Key, _input: &mut KeyInput) -> Option<Events> {
        let last = self.options.len().checked_sub(1)?;
        let to = match key {
            Key::Up | Key::Left => self.index.saturating_sub(1),
            Key::Down | Key::Right => (self.index + 1).min(last),
            Key::Home => 0,
            Key::End => last,
            _ => return None,
        };
        Some(self.choose(to))
    }

    /// Space or Enter opens a combobox's list, as a press on its field does.
    fn activate(&mut self, input: &Input) -> Option<Events> {
        if self.view != View::Combo || self.options.is_empty() {
            return None;
        }
        let size = self.text_size * input.scale;
        Some(Events::none().and_popup(self.list(0, self.body(input), size)))
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
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 120.0, 24.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            clicks: 1,
            time: None,
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

    /// A press asks for the list and changes nothing; the pick that comes back
    /// is what moves the index and what is reported.
    #[test]
    fn a_press_asks_for_the_list_and_a_pick_chooses_from_it() {
        let metrics = Metrics::default();
        let mut choice = from_props(&props(r#"{"options":["a","b","c"],"index":1}"#));
        let Claim::Take(mut take) = choice.press((10.0, 10.0), &input(&metrics)) else {
            panic!("a chooser takes its press");
        };
        let request = take.events.take_popup().expect("it asks for its list");
        assert_eq!(request.entries.len(), 3);
        assert!(request.entries[1].marked(), "the chosen option is marked");
        assert!(!request.entries[0].marked());
        assert_eq!(request.anchor, choice.body(&input(&metrics)));
        assert_eq!(choice.index, 1, "nothing was chosen by opening it");

        assert_eq!(
            choice.picked(&[2], &input(&metrics)),
            Events::value(OscType::Int(2))
        );
        assert_eq!(choice.index, 2);
        // A row past the options, or a path into a submenu it never had.
        assert_eq!(choice.picked(&[9], &input(&metrics)), Events::none());
        assert_eq!(choice.picked(&[0, 0], &input(&metrics)), Events::none());
        assert_eq!(choice.index, 2);
    }

    #[test]
    fn a_chooser_with_no_options_opens_nothing() {
        let metrics = Metrics::default();
        let mut choice = from_props(&props("{}"));
        assert_eq!(choice.press((10.0, 10.0), &input(&metrics)), Claim::take());
    }

    #[test]
    fn the_options_can_be_set_live_and_the_index_stays_inside_them() {
        let mut choice = from_props(&props(r#"{"options":["a","b","c"],"index":2}"#));
        assert!(choice.set("options", &serde_json::json!(["x"])));
        assert_eq!(choice.options, vec!["x".to_string()]);
        assert_eq!(choice.index, 0);
    }

    fn choice(view: &str) -> Choice {
        from_props(&props(&format!(
            r#"{{"options":["one","two","three"],"index":0,"view":"{view}"}}"#
        )))
    }

    /// The same data and the same event in every view: a press on an option
    /// chooses it.
    #[test]
    fn a_press_on_an_option_chooses_it_in_every_view_that_shows_them() {
        let metrics = Metrics::default();
        for view in ["radio", "segmented", "tabs", "list"] {
            let mut c = choice(view);
            let tall = Input {
                rect: Rect::new(0.0, 0.0, 300.0, 120.0),
                ..input(&metrics)
            };
            let parts = c.parts(&tall);
            let (_, r) = parts[2];
            let at = ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64);
            assert_eq!(
                c.press(at, &tall),
                Claim::value(OscType::Int(2)),
                "view {view}"
            );
            assert_eq!(c.index, 2);
            // The chosen one again is no news.
            assert_eq!(c.press(at, &tall), Claim::take(), "view {view}");
        }
    }

    #[test]
    fn a_pager_steps_and_stops_at_its_ends() {
        let metrics = Metrics::default();
        let mut c = choice("pager");
        let parts = c.parts(&input(&metrics));
        let mid = |r: Rect| ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64);
        let (prev, next) = (mid(parts[0].1), mid(parts[1].1));
        assert_eq!(
            c.press(prev, &input(&metrics)),
            Claim::take(),
            "already first"
        );
        assert_eq!(
            c.press(next, &input(&metrics)),
            Claim::value(OscType::Int(1))
        );
        c.press(next, &input(&metrics));
        assert_eq!(
            c.press(next, &input(&metrics)),
            Claim::take(),
            "already last"
        );
        assert_eq!(c.index, 2);
    }

    /// Tabs that do not fit gather under a last one, whose list holds them: a
    /// pick there counts from the first gathered tab.
    #[test]
    fn gathered_tabs_open_a_list_and_a_pick_counts_from_the_first_of_them() {
        let metrics = Metrics::default();
        let mut c = from_props(&props(
            r#"{"view":"tabs","options":["first tab","second tab","third tab","fourth tab"]}"#,
        ));
        let narrow = Input {
            rect: Rect::new(0.0, 0.0, 150.0, 30.0),
            ..input(&metrics)
        };
        let parts = c.parts(&narrow);
        let (part, r) = *parts.last().unwrap();
        let Part::More(from) = part else {
            panic!("the tabs fit: {parts:?}");
        };
        let at = ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64);
        let Claim::Take(mut take) = c.press(at, &narrow) else {
            panic!("the gathering tab takes its press");
        };
        let request = take.events.take_popup().expect("it lists the rest");
        assert_eq!(request.entries.len(), 4 - from);
        assert_eq!(
            c.picked(&[1], &narrow),
            Events::value(OscType::Int((from + 1) as i32))
        );
    }

    #[test]
    fn a_stacked_view_is_a_row_per_option_tall_and_the_others_are_one_line() {
        let m = Metrics::default();
        let line = choice("combo").natural(&m, 1.0).1.unwrap();
        assert_eq!(choice("tabs").natural(&m, 1.0).1, Some(line));
        assert_eq!(choice("pager").natural(&m, 1.0).1, Some(line));
        assert!(choice("radio").natural(&m, 1.0).1.unwrap() > 2.0 * line);
        // The view is a prop, live like any other.
        let mut c = choice("combo");
        assert!(c.set("view", &serde_json::json!("list")));
        assert_eq!(c.view, View::List);
        assert!(!c.set("view", &serde_json::json!("carousel")));
    }

    #[test]
    fn the_arrows_move_the_choice_and_stop_at_its_ends() {
        let mut c = choice("segmented");
        let mut clip = crate::host::clipboard::Clip::default();
        let mut keys = KeyInput {
            mods: Mods::default(),
            clipboard: &mut clip,
            cursor: None,
        };
        assert_eq!(c.key(&Key::Left, &mut keys), Some(Events::none()));
        assert_eq!(
            c.key(&Key::Right, &mut keys),
            Some(Events::value(OscType::Int(1)))
        );
        assert_eq!(
            c.key(&Key::End, &mut keys),
            Some(Events::value(OscType::Int(2)))
        );
        assert_eq!(c.key(&Key::Enter, &mut keys), None);
    }
}
