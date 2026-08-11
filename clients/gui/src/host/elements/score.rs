//! `score` — an engraved notation page: the leaf whose state is a **document
//! it does not own**.
//!
//! The host holds geometry, never the score: a client engraves and sends a
//! semantic display list, and everything here is a *reading* of that drawing —
//! which element is under the cursor, how far up the staff a drag has moved it,
//! where the playback cursor sits at a given millisecond. The page itself and
//! all of that reading stay in [`crate::host::score`], which is the model; this
//! file is only how the passes reach it.
//!
//! What the port collapses is the routing that model needed while the leaf was
//! an enum arm: four `interact` doors (`score_select`, `score_drag`,
//! `score_drag_end`, plus the payload readers), a `nav::score_steps` lookup and
//! a `Drag::ScoreStep` variant carrying the press-time origin the machine could
//! not put anywhere else. The press-time origin is a field here, and the drag's
//! displacement was always the page's own (`ScoreData::drag`), drawn as
//! notation while it happens.
//!
//! **An edit is an intent, and the preview outlives the gesture.** The release
//! reports `"transpose" <xml:id> <steps>` — the owner's units, not pixels — and
//! the displacement **stays drawn** until the client answers with a re-engraved
//! page, because dropping it first would show the old pitch for a frame. That
//! is why `display_list` replaces the drawing and keeps the chrome.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::paint::Draw;
use crate::host::score::{ScoreColors, ScoreData, ScoreDrag};
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input, Needs};
use crate::host::widget::parse;

/// An engraved page, plus the one thing a page does not carry: where the pitch
/// drag in flight was pressed.
#[derive(Debug, Clone)]
pub struct Score {
    pub data: ScoreData,
    /// The press this drag started from, in window pixels — the origin the
    /// step count is measured from, so it is absolute from the snapshot rather
    /// than accumulated. `None` when no drag is in flight.
    origin_y: Option<f64>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(Score {
        data: ScoreData::parse(props),
        origin_y: None,
    }))
}

impl Element for Score {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        let data = &mut self.data;
        match key {
            // Replace the engraved page in place — the answer to an edit, and
            // the reason a score does not have to be redefined to change. Only
            // the drawing travels: the chrome (playhead, selection) is the
            // host's own state and survives, so the note the user is editing
            // stays selected across the round trip. The drag preview is what
            // this page *is* now, so it retires here.
            "display_list" => match parse::as_props(v) {
                Some(props) => {
                    let page = ScoreData::parse(&props);
                    let keep = std::mem::replace(data, page);
                    data.playhead = keep.playhead;
                    data.playhead_at = keep.playhead_at;
                    data.playhead_loop_start = keep.playhead_loop_start;
                    data.playhead_loop_len = keep.playhead_loop_len;
                    data.sample_rate = keep.sample_rate;
                    data.selected = keep.selected;
                    // A re-engraved page carries only the drawing; whether the
                    // widget edits is the host's own state, like the chrome, so
                    // an editor stays an editor across the round trip.
                    data.editable = keep.editable;
                    true
                }
                None => false,
            },
            // Locate the static playback cursor; a negative time hides it.
            "playhead" => v.as_f64().map(|t| data.playhead = t as f32).is_some(),
            // Anchor score time 0 to a sample-clock value: the cursor then
            // sweeps on its own, one message per pass instead of per frame.
            "playhead_at" => v.as_f64().map(|t| data.playhead_at = t).is_some(),
            // Wrap the sweep inside a repeated passage (ms; <= 0 length = the
            // straight pass).
            "playhead_loop_start" => v
                .as_f64()
                .map(|t| data.playhead_loop_start = t as f32)
                .is_some(),
            "playhead_loop_len" => v
                .as_f64()
                .map(|t| data.playhead_loop_len = t as f32)
                .is_some(),
            "sample_rate" => v.as_f64().map(|r| data.sample_rate = r).is_some(),
            // Select an element by its MEI id; the empty string clears it.
            "selected" => v
                .as_str()
                .map(|s| data.selected = (!s.is_empty()).then(|| s.to_string()))
                .is_some(),
            // Turn editing on or off live (a view that becomes an editor, or
            // the reverse). A drag only transposes while this is true.
            "editable" => v.as_bool().map(|b| data.editable = b).is_some(),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        // Notation tessellates straight into the shared triangle mesh: a paper
        // panel under the engraving, glyphs and fills in ink, the playback
        // cursor over it in the playhead accent.
        let (mesh, _, theme) = d.parts();
        mesh.rect(ctx.rect, theme.panel);
        // The cursor sweeps off the engine clock while a pass plays
        // (`playhead_at`), so playback costs no messages per frame.
        let head = self
            .data
            .head_ms(ctx.world.sample_clock, ctx.world.sample_rate);
        let colors = ScoreColors {
            ink: theme.text,
            playhead: theme.playhead,
            selection: theme.selection,
        };
        self.data.render(mesh, ctx.rect, ctx.clip, head, colors);
    }

    fn needs(&self) -> Needs {
        Needs {
            // A score carries its **own** playhead anchor rather than a
            // navigation group's, so it is the widget itself that says its
            // cursor is sweeping — without this the window stops following the
            // clock and the cursor freezes where it was anchored.
            clock: self.data.playhead_at >= 0.0,
            ..Needs::default()
        }
    }

    fn info(&self) -> Vec<(String, Value)> {
        // The one prop a gesture changes: a click selects, and `/gui_set
        // selected` is how a script would reproduce it.
        vec![(
            "selected".into(),
            Value::from(self.data.selected.clone().unwrap_or_default()),
        )]
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        // A press names the engraved element under it by its MEI id — the same
        // id the client engraved from, so a driver resolves it in its own
        // score. Pressing blank paper clears the selection.
        let picked = self
            .data
            .hit(input.rect, at.0 as f32, at.1 as f32)
            .map(str::to_string);
        let changed = picked != self.data.selected;
        if changed {
            self.data.selected = picked.clone();
        }
        // ...and, on an editable score, holding it drags the element's pitch. A
        // press that does not move stays a plain selection: the release emits
        // nothing more. A read-only page (the default) still selects and
        // reports the element above, but a drag does nothing — the host holds
        // no score, so an edit the client will not apply is a gesture it cannot
        // fulfil.
        let dragging = self.data.editable && picked.is_some();
        if dragging {
            self.data.drag = Some(ScoreDrag {
                id: picked.clone().unwrap_or_default(),
                steps: 0,
            });
            self.origin_y = Some(at.1);
        }
        match (changed, dragging) {
            // Nothing selected, nothing to drag: the press was never this
            // page's, so it goes back to the chain.
            (false, false) => Claim::Decline,
            (true, _) => Claim::events(Events::message(vec![
                OscType::String("element".into()),
                OscType::String(picked.unwrap_or_default()),
            ])),
            (false, true) => Claim::take(),
        }
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        // Absolute from the press, quantized to whole steps: the page is
        // redrawn only when the drag crosses one, so the pixels between two
        // pitches cost nothing. Nothing is reported until the release — what
        // travels is the finished intent.
        let Some(origin_y) = self.origin_y else {
            return Events::none();
        };
        let steps = self.data.steps_for(input.rect, (at.1 - origin_y) as f32);
        if let Some(drag) = self.data.drag.as_mut() {
            drag.steps = steps;
        }
        Events::none()
    }

    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        self.origin_y = None;
        let Some(drag) = self.data.drag.as_ref() else {
            return Events::none();
        };
        // A drag that ended where it started retires here — there is nothing to
        // ask the client for. One that moved **keeps its displacement drawn**:
        // the host owns no notation, so it cannot re-engrave the page itself,
        // and dropping the preview now would show the old pitch until the
        // client's answer arrives. The page it sends back retires the preview
        // (see the `display_list` prop).
        if drag.steps == 0 {
            self.data.drag = None;
            return Events::none();
        }
        Events::message(vec![
            OscType::String("transpose".into()),
            OscType::String(drag.id.clone()),
            OscType::Int(drag.steps),
        ])
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::layout::Rect;
    use crate::host::metrics::Metrics;
    use crate::host::widget::element::Mods;

    /// A one-staff page with two identified noteheads, engraved the way the
    /// client sends one: a viewBox, one glyph outline, and placed primitives.
    fn page(editable: bool) -> Score {
        let props: Map<String, Value> = serde_json::from_str(&format!(
            r#"{{"vb":[1000,400],"step":90,"editable":{editable},
                "glyphs":{{"E0A4":"M0 0 L100 0 L100 -100 L0 -100 Z"}},
                "prims":[
                  {{"k":"glyph","cp":"E0A4","xf":[100,200,1,-1],"id":"n1"}},
                  {{"k":"glyph","cp":"E0A4","xf":[400,200,1,-1],"id":"n2"}}]}}"#
        ))
        .unwrap();
        Score {
            data: ScoreData::parse(&props),
            origin_y: None,
        }
    }

    fn input<'a>(m: &'a Metrics) -> Input<'a> {
        Input {
            metrics: m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 500.0, 200.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (600.0, 400.0),
            time: None,
        }
    }

    /// The window pixel a page point lands on, through the same fit the
    /// renderer draws with — so a test presses where the ink is.
    fn at(score: &Score, rect: Rect, px: f32, py: f32) -> (f64, f64) {
        let fit = score.data.fit(rect);
        let [x, y] = fit.apply(px, py);
        (x as f64, y as f64)
    }

    /// A click names the element under it and clears on blank paper — the
    /// inspection half, which is **not** gated by `editable`.
    #[test]
    fn a_press_selects_the_element_under_it() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(false);

        let claim = score.press(at(&score, input.rect, 150.0, 250.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("element".into()),
                OscType::String("n1".into()),
            ]))
        );
        assert_eq!(score.data.selected.as_deref(), Some("n1"));
        assert_eq!(
            score.info(),
            vec![("selected".into(), Value::from("n1"))],
            "a query answers what is selected now"
        );

        // Blank paper clears it, and says so.
        let claim = score.press(at(&score, input.rect, 900.0, 380.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("element".into()),
                OscType::String(String::new()),
            ]))
        );
        assert_eq!(score.data.selected, None);
        // ...and pressing blank paper *again* changes nothing, so the press
        // goes back to the chain instead of being swallowed.
        assert_eq!(
            score.press(at(&score, input.rect, 900.0, 380.0), &input),
            Claim::Decline
        );
    }

    /// A read-only page selects and reports, and drags nothing: the host holds
    /// no score, so an edit the client will not apply is a gesture it cannot
    /// fulfil.
    #[test]
    fn a_read_only_page_does_not_drag() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(false);
        score.press(at(&score, input.rect, 150.0, 250.0), &input);
        assert!(score.data.drag.is_none());
        let (x, y) = at(&score, input.rect, 150.0, 250.0);
        score.drag((x, y - 100.0), &input);
        assert!(score.data.drag.is_none());
        assert_eq!(score.release((x, y - 100.0), &input), Events::none());
    }

    /// An editable page displaces the element as the drag crosses whole
    /// diatonic steps, and the release reports the intent in the owner's units
    /// — steps up the staff, never pixels.
    #[test]
    fn an_editable_page_transposes_in_whole_steps() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(true);
        let press = at(&score, input.rect, 150.0, 250.0);
        score.press(press, &input);
        assert_eq!(score.data.drag.as_ref().map(|d| d.steps), Some(0));

        // Two steps up, in page units through the fit: dragging up is positive.
        let step_px = (score.data.step * score.data.fit(input.rect).sy) as f64;
        score.drag((press.0, press.1 - 2.0 * step_px), &input);
        assert_eq!(score.data.drag.as_ref().map(|d| d.steps), Some(2));

        let events = score.release((press.0, press.1 - 2.0 * step_px), &input);
        assert_eq!(
            events,
            Events::message(vec![
                OscType::String("transpose".into()),
                OscType::String("n1".into()),
                OscType::Int(2),
            ])
        );
        assert!(
            score.data.drag.is_some(),
            "the displacement stays drawn until the client's re-engraved page arrives"
        );
    }

    /// A drag that ends where it started asks for nothing and drops its
    /// preview — the press was a selection after all.
    #[test]
    fn a_drag_that_moved_nothing_retires_on_release() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(true);
        let press = at(&score, input.rect, 150.0, 250.0);
        score.press(press, &input);
        assert_eq!(score.release(press, &input), Events::none());
        assert!(score.data.drag.is_none());
    }

    /// A swept page declares that it reads the clock and a still one does not
    /// — the declaration that used to be a `live.rs` arm asking what kind of
    /// widget this was.
    #[test]
    fn a_swept_page_declares_that_it_reads_the_clock() {
        let mut score = page(false);
        assert!(!score.needs().clock);
        assert!(score.set("playhead_at", &Value::from(48_000.0)));
        assert!(score.needs().clock);
    }

    /// A re-engraved page keeps the host's own chrome — the selection the user
    /// is editing, the playhead — and retires the drag preview it answers.
    #[test]
    fn a_new_display_list_keeps_the_chrome_and_retires_the_preview() {
        let mut score = page(true);
        score.data.selected = Some("n2".into());
        score.data.playhead = 500.0;
        score.data.drag = Some(ScoreDrag {
            id: "n2".into(),
            steps: 3,
        });
        assert!(score.set(
            "display_list",
            &Value::from(r#"{"vb":[1000,400],"prims":[]}"#)
        ));
        assert_eq!(score.data.selected.as_deref(), Some("n2"));
        assert_eq!(score.data.playhead, 500.0);
        assert!(score.data.editable, "an editor stays an editor");
        assert!(score.data.drag.is_none());
        assert!(score.data.prims.is_empty(), "and the drawing was replaced");
    }
}
