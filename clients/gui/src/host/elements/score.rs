//! `score` -- an engraved notation page: the leaf whose state is a **document
//! it does not own**.
//!
//! The host holds geometry, never the score: a client engraves and sends a
//! semantic display list, and everything here is a *reading* of that drawing --
//! which element is under the cursor, how far up the staff a drag has moved it,
//! where the playback cursor sits at a given millisecond. The page itself and
//! all of that reading stay in [`crate::host::graphics::score`], which is the model; this
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
//! reports `"transpose" <xml:id> <position>` -- the diatonic staff position the
//! note reaches, in the owner's units rather than pixels, and **absolute** so
//! that a resend cannot move the note twice and a page re-engraved under the
//! gesture needs no rebasing -- and
//! the displacement **stays drawn** until the client answers with a re-engraved
//! page, because dropping it first would show the old pitch for a frame. That
//! is why `display_list` replaces the drawing and keeps the chrome.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::score::{ScoreColors, ScoreData, ScoreDrag, TextEditing};
use crate::host::graphics::textedit;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input, Key, KeyInput, Needs};
use crate::host::widget::parse;

/// An engraved page, plus the one thing a page does not carry: where the pitch
/// drag in flight was pressed.
#[derive(Debug, Clone)]
pub struct Score {
    pub data: ScoreData,
    /// The press this drag started from, in window pixels -- the origin the
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

impl Score {
    /// **The text being typed over is done**: what was written is reported
    /// as `"text" <id> <string>` -- the page's element and what it now says,
    /// for whoever owns the score to write -- unless nothing changed, and the
    /// page goes back to drawing what it engraved either way. The owner
    /// answers with a page that says it.
    fn finish(&mut self) -> Events {
        let Some(edit) = self.data.editing.take() else {
            return Events::none();
        };
        if self.data.text_of(&edit.id) == Some(edit.value.as_str()) {
            return Events::none();
        }
        Events::message(vec![
            OscType::String("text".into()),
            OscType::String(edit.id),
            OscType::String(edit.value),
        ])
    }
}

impl Element for Score {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        let data = &mut self.data;
        match key {
            // Replace the engraved page in place -- the answer to an edit, and
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
                    // an editor stays an editor across the round trip. Note
                    // entry is the same kind of state and was being dropped
                    // here: the page took one insertion and then silently
                    // stopped taking any, because answering the first one
                    // replaced the display list.
                    data.editable = keep.editable;
                    data.entry = keep.entry;
                    // a text being typed over stays so while the page that
                    // came still draws it
                    data.editing = keep.editing.filter(|edit| data.text_of(&edit.id).is_some());
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
            "selected" => {
                data.selected = crate::host::graphics::score::selection(v);
                true
            }
            // Turn editing on or off live (a view that becomes an editor, or
            // the reverse). A drag only transposes while this is true.
            "editable" => v.as_bool().map(|b| data.editable = b).is_some(),
            "entry" => v.as_bool().map(|b| data.entry = b).is_some(),
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
        let head = self.data.head_ms(ctx.clock, ctx.world.sample_rate);
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
            // cursor is sweeping -- without this the window stops following the
            // clock and the cursor freezes where it was anchored.
            clock: self.data.playhead_at >= 0.0,
            ..Needs::default()
        }
    }

    fn info(&self) -> Vec<(String, Value)> {
        // The one prop a gesture changes: a click selects, and `/gui_set
        // selected` is how a script would reproduce it.
        // One element reads as its id, as it always has; several as the list.
        let selected = match self.data.selected.as_slice() {
            [] => Value::from(""),
            [one] => Value::from(one.clone()),
            many => Value::from(many.to_vec()),
        };
        vec![("selected".into(), selected)]
    }

    /// An editor's page is where its keys point: it takes the focus so that a
    /// text typed over on it has a keyboard, and says nothing about it.
    fn accepts_focus(&self) -> bool {
        self.data.editable
    }

    fn reports_focus(&self) -> bool {
        false
    }

    /// Only while a text is being typed over: every other key of a page is a
    /// command.
    fn takes_text(&self) -> bool {
        self.data.editing.is_some()
    }

    /// **The keys of a text being typed over.** Enter writes it, Escape
    /// leaves it as it was, and the rest edit the line as a field's do. With
    /// no text in hand a page has no key of its own.
    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        let edit = self.data.editing.as_mut()?;
        match key {
            Key::Enter => Some(self.finish()),
            Key::Escape => {
                self.data.editing = None;
                Some(Events::none())
            }
            // Tab is the ring's, and moving the focus writes the text
            Key::Tab => None,
            // a key that edits nothing -- a chord that is the window's -- goes on
            _ => super::text::edit_key(&mut edit.value, &mut edit.caret, false, key, input)
                .map(|_| Events::none()),
        }
    }

    fn blur(&mut self) -> Events {
        self.finish()
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        // A press names the engraved element under it by its MEI id -- the same
        // id the client engraved from, so a driver resolves it in its own
        // score. Pressing blank paper clears the selection.
        let picked = self
            .data
            .hit(input.rect, at.0 as f32, at.1 as f32, input.metrics.hit_slop)
            .map(str::to_string);
        // **A press while a text is being typed over ends it**, written: the
        // hand went elsewhere on the page, and that is all this press says --
        // it selects nothing and writes no note, unless it is the double
        // click that takes up another text.
        let typing = self.data.editing.is_some();
        let written = self.finish();
        // **A double click on a text of the page types over it, where it is
        // drawn**: the whole of it selected, as a field's is when it is
        // entered. Only on a page that edits, and only a text -- a title, a
        // name, a footnote -- which is what the page draws as a string.
        if self.data.editable
            && input.clicks >= 2
            && let Some(id) = picked.as_deref()
            && let Some(text) = self.data.text_of(id)
        {
            let value = text.to_string();
            let mut caret = textedit::Caret::default();
            textedit::select_all(&value, &mut caret);
            self.data.editing = Some(TextEditing {
                id: id.to_string(),
                value,
                caret,
            });
            return Claim::events(written);
        }
        if typing {
            return Claim::events(written);
        }
        // **On a page that takes note entry, a staff line is a place.** The hit
        // test answers with a sounding element where there is one and with the
        // tightest box otherwise, and the tightest box on an engraved page is a
        // staff *line* -- a hairline the width of the system, thinner than any
        // notehead, carrying the staff's own id. So a press aimed at a line
        // rather than at a space was answered with the engraver's own drawing
        // and spent on a selection: measured over one sitting, 64 presses wrote
        // a quarter and 10 came back as `"element"`.
        //
        // Selecting a staff is not the wrong answer -- writing is what a press
        // on the staff is *for*, and being a pixel onto a line is not a way to
        // ask for something else. So on a page that takes entry, a press on the
        // staff's own drawing is the place that drawing is at, and selecting a
        // staff needs its own way to be asked for (see the plan's "A selected
        // staff is edited by its line count").
        //
        // **The staff's lines and nothing else.** The first cut of this asked
        // whether the pick *sounds*, which is a different question with a
        // different answer: a slur, a hairpin, a dynamic and a beam sound
        // nothing and are elements of the score all the same, so that rule made
        // them unselectable and turned every press on one into a note. What the
        // page can say for itself is which primitives draw the staves
        // (`ScoreData::staff_ids`, derived by the same geometry the staves are),
        // and that is exactly the furniture this is allowed to reach.
        let picked = match &picked {
            Some(id) if self.data.entry && self.data.staff_ids.contains(id) => None,
            _ => picked,
        };
        // **A modified press adds to the selection rather than replacing it**:
        // Ctrl toggles the element in it, Shift extends it to the element --
        // the field's two conventions. What a range *is* (the notes in time
        // between the two, across the staves between them) is the owner's to
        // say, since only the model knows time; the host keeps the two ends
        // until the owner answers with the whole range.
        let mode = if input.mods.ctrl {
            "toggle"
        } else if input.mods.shift {
            "extend"
        } else {
            ""
        };
        let next: Vec<String> = match (&picked, mode) {
            (Some(id), "toggle") => {
                let mut next = self.data.selected.clone();
                match next.iter().position(|s| s == id) {
                    Some(at) => {
                        next.remove(at);
                    }
                    None => next.push(id.clone()),
                }
                next
            }
            (Some(id), "extend") => {
                let mut next = self.data.selected.clone();
                if !next.contains(id) {
                    next.push(id.clone());
                }
                next
            }
            (Some(id), _) => vec![id.clone()],
            // a modified press on paper keeps the selection it would have added to
            (None, "toggle" | "extend") => self.data.selected.clone(),
            (None, _) => Vec::new(),
        };
        let changed = next != self.data.selected;
        self.data.selected = next;
        // ...and, on an editable score, holding it drags the element's pitch. A
        // press that does not move stays a plain selection: the release emits
        // nothing more. A read-only page (the default) still selects and
        // reports the element above, but a drag does nothing -- the host holds
        // no score, so an edit the client will not apply is a gesture it cannot
        // fulfil. **Only what has a pitch drags** (the core's `admits`): a
        // slur, a time signature, a rest or a staff dragged like a notehead
        // and grew ledger lines, and where those sit is the engraver's.
        let dragging = self.data.editable
            && mode.is_empty()
            && picked
                .as_deref()
                .is_some_and(|id| self.data.admits(id).pitch);
        if dragging {
            self.data.drag = Some(ScoreDrag {
                id: picked.clone().unwrap_or_default(),
                steps: 0,
            });
            self.origin_y = Some(at.1);
        }
        // A press on blank paper, on a page that takes note entry, reports
        // *where* it landed rather than only that nothing is there. The host
        // names a place -- the staff, how far up it, the element it would follow
        // -- and nothing else: a staff position is not a pitch until something
        // knows the clef and the key, and a duration is a choice nobody made by
        // clicking. Both are the client's, which is the line every other score
        // gesture already draws.
        if picked.is_none()
            && mode.is_empty()
            && self.data.entry
            && let Some(entry) = self.data.entry_at(input.rect, at.0 as f32, at.1 as f32)
        {
            return Claim::events(Events::message(vec![
                OscType::String("insert".into()),
                OscType::String(entry.after.unwrap_or_default()),
                OscType::Int(entry.position),
                OscType::Int(entry.staff as i32),
            ]));
        }
        match (changed, dragging) {
            // Nothing selected, nothing to drag: the press was never this
            // page's, so it goes back to the chain.
            (false, false) => Claim::Decline,
            (true, _) => {
                let mut report = vec![
                    OscType::String("element".into()),
                    OscType::String(picked.unwrap_or_default()),
                ];
                // a plain press says only what it picked, as it always has
                if !mode.is_empty() {
                    report.push(OscType::String(mode.into()));
                }
                Claim::events(Events::message(report))
            }
            (false, true) => Claim::take(),
        }
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        // Absolute from the press, quantized to whole steps: the page is
        // redrawn only when the drag crosses one, so the pixels between two
        // pitches cost nothing. Nothing is reported until the release -- what
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

    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        self.origin_y = None;
        let Some(drag) = self.data.drag.as_ref() else {
            return Events::none();
        };
        // A drag that ended where it started retires here -- there is nothing to
        // ask the client for. One that moved **keeps its displacement drawn**:
        // the host owns no notation, so it cannot re-engrave the page itself,
        // and dropping the preview now would show the old pitch until the
        // client's answer arrives. The page it sends back retires the preview
        // (see the `display_list` prop).
        if drag.steps == 0 {
            self.data.drag = None;
            return Events::none();
        }
        // Absolute, not the displacement: the position the note *reaches*, so
        // the edit is idempotent and needs no rebasing against a page that
        // moved under it. A page with no measurable staff for this element has
        // no position to name, and a displacement would be worse than silence.
        let Some(from) = self.data.staff_position(&drag.id) else {
            self.data.drag = None;
            return Events::none();
        };
        Events::message(vec![
            OscType::String("transpose".into()),
            OscType::String(drag.id.clone()),
            OscType::Int(from + drag.steps),
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
        // Five staff lines one space (two diatonic steps) apart, because a
        // pitch edit is reported as a position *on a staff* and a page without
        // one has no position to name. The noteheads are placed two steps below
        // the top line, so the position they report cannot be confused with the
        // displacement of the drag that reaches it.
        let props: Map<String, Value> = serde_json::from_str(&format!(
            r#"{{"vb":[1000,1000],"step":90,"editable":{editable},
                "glyphs":{{"E0A4":"M0 0 L100 0 L100 -100 L0 -100 Z"}},
                "prims":[
                  {{"k":"line","pts":[[0,20],[1000,20]],"w":4}},
                  {{"k":"line","pts":[[0,200],[1000,200]],"w":4}},
                  {{"k":"line","pts":[[0,380],[1000,380]],"w":4}},
                  {{"k":"line","pts":[[0,560],[1000,560]],"w":4}},
                  {{"k":"line","pts":[[0,740],[1000,740]],"w":4}},
                  {{"k":"glyph","cp":"E0A4","xf":[100,200,1,-1],"id":"n1"}},
                  {{"k":"glyph","cp":"E0A4","xf":[400,200,1,-1],"id":"n2"}}],
                "kinds":{{"n1":"note","n2":"note"}}}}"#
        ))
        .unwrap();
        Score {
            data: ScoreData::parse(&props),
            origin_y: None,
        }
    }

    /// The same page **as the engraver actually draws it**: the staff lines
    /// carry the staff's own id, which the fixture above leaves off and which
    /// is the whole of why a press on a line had somewhere else to go.
    fn engraved_staff() -> Score {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,1000],"step":90,"editable":true,
                "glyphs":{"E0A4":"M0 0 L100 0 L100 -100 L0 -100 Z"},
                "prims":[
                  {"k":"line","pts":[[0,20],[1000,20]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,200],[1000,200]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,380],[1000,380]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,560],[1000,560]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,740],[1000,740]],"w":4,"id":"staff1"},
                  {"k":"glyph","cp":"E0A4","xf":[100,200,1,-1],"id":"n1"},
                  {"k":"glyph","cp":"E0A4","xf":[400,200,1,-1],"id":"n2"}],
                "kinds":{"n1":"note","n2":"note","staff1":"staff"}}"#,
        )
        .unwrap();
        Score {
            data: ScoreData::parse(&props),
            origin_y: None,
        }
    }

    /// The same page, taking note entry, with the client's element list on it.
    fn entry_page() -> Score {
        let mut score = page(true);
        score.data.entry = true;
        score.data.elements = ["n1", "n2"].iter().map(|s| s.to_string()).collect();
        score
    }

    /// A page that edits, with a title drawn as a text under its id.
    fn titled() -> Score {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,1000],"step":90,"editable":true,"glyphs":{},
                "prims":[{"k":"text","s":"A title","x":500,"y":300,"size":100,
                          "anchor":"middle","id":"t-title"}],
                "kinds":{"t-title":"rend"}}"#,
        )
        .unwrap();
        Score {
            data: ScoreData::parse(&props),
            origin_y: None,
        }
    }

    /// The keys `keys`, typed at `score` one after another: what the last
    /// answered.
    fn typed(score: &mut Score, keys: &[Key]) -> Option<Events> {
        let mut clipboard = crate::host::clipboard::Clip::default();
        let mut last = None;
        for key in keys {
            let mut input = KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: None,
            };
            last = score.key(key, &mut input);
        }
        last
    }

    /// **A double click on a text of the page types over it, where it is
    /// drawn.** The whole of it is selected, so the first key replaces it;
    /// Enter reports what it now says under the element's id, which is the
    /// intent whoever owns the score writes; and the page goes back to
    /// drawing what it engraved until that owner answers.
    #[test]
    fn a_double_click_on_a_text_types_over_it_and_enter_writes_it() {
        let metrics = Metrics::default();
        let mut once = input(&metrics);
        let mut score = titled();
        assert!(score.accepts_focus() && !score.takes_text());
        let on_title = at(&score, once.rect, 500.0, 270.0);
        // one press selects it, as it selects any element
        assert!(matches!(score.press(on_title, &once), Claim::Take(_)));
        assert!(score.data.editing.is_none());
        // the second of a double click takes it up, all of it selected
        once.clicks = 2;
        score.press(on_title, &once);
        let edit = score
            .data
            .editing
            .clone()
            .expect("the title is being typed over");
        assert_eq!(
            (edit.id.as_str(), edit.value.as_str()),
            ("t-title", "A title")
        );
        assert_eq!(edit.caret.selection(), Some((0, 7)));
        assert!(score.takes_text(), "its keys are characters now");

        // typing replaces what was selected; the keys are a field's
        let answered = typed(
            &mut score,
            &[Key::Char('N'), Key::Char('o'), Key::Backspace],
        );
        assert_eq!(
            answered,
            Some(Events::none()),
            "nothing is said until it is done"
        );
        assert_eq!(score.data.editing.as_ref().unwrap().value, "N");
        assert_eq!(
            typed(&mut score, &[Key::Char('e'), Key::Char('w'), Key::Enter]),
            Some(Events::message(vec![
                OscType::String("text".into()),
                OscType::String("t-title".into()),
                OscType::String("New".into()),
            ]))
        );
        assert!(score.data.editing.is_none() && !score.takes_text());
        // with no text in hand a page has no key of its own
        assert_eq!(typed(&mut score, &[Key::Char('x')]), None);
    }

    /// **Escape leaves the text as it was, and going elsewhere writes it**:
    /// the focus leaving the page, or a press somewhere else on it -- which is
    /// spent on ending the edit and selects nothing. A text left as it was is
    /// not reported at all.
    #[test]
    fn a_text_typed_over_is_written_when_the_hand_goes_elsewhere() {
        let metrics = Metrics::default();
        let mut twice = input(&metrics);
        twice.clicks = 2;
        let mut score = titled();
        let on_title = at(&score, twice.rect, 500.0, 270.0);
        let written = |text: &str| {
            Events::message(vec![
                OscType::String("text".into()),
                OscType::String("t-title".into()),
                OscType::String(text.into()),
            ])
        };

        score.press(on_title, &twice);
        typed(&mut score, &[Key::Char('X'), Key::Escape]);
        assert!(score.data.editing.is_none(), "Escape leaves it");
        assert_eq!(score.blur(), Events::none(), "and nothing was written");

        // the focus goes: what was typed is written
        score.press(on_title, &twice);
        typed(&mut score, &[Key::Char('B')]);
        assert_eq!(score.blur(), written("B"));

        // a press elsewhere on the page: written, and that is all it says
        score.press(on_title, &twice);
        typed(&mut score, &[Key::Char('C')]);
        let once = input(&metrics);
        let elsewhere = at(&score, once.rect, 100.0, 900.0);
        assert_eq!(score.press(elsewhere, &once), Claim::events(written("C")));

        // taken up and left untouched, it says nothing
        score.press(on_title, &twice);
        assert_eq!(typed(&mut score, &[Key::Enter]), Some(Events::none()));

        // and a page that does not edit takes up no text
        let mut reading = titled();
        reading.data.editable = false;
        reading.press(on_title, &twice);
        assert!(reading.data.editing.is_none() && !reading.accepts_focus());
    }

    /// A text being typed over is drawn as it stands, with its caret -- and a
    /// re-engraved page keeps it in hand only while it still draws that text.
    #[test]
    fn the_text_in_hand_is_drawn_and_survives_a_page_that_still_has_it() {
        use crate::host::graphics::score::ScoreColors;
        use crate::host::paint::Mesh;

        let metrics = Metrics::default();
        let mut twice = input(&metrics);
        twice.clicks = 2;
        let mut score = titled();
        let rect = twice.rect;
        let colors = ScoreColors {
            ink: [1.0; 4],
            playhead: [1.0; 4],
            selection: [1.0; 4],
        };
        let drawn = |score: &Score| {
            let mut mesh = Mesh::new();
            score.data.render(&mut mesh, rect, None, -1.0, colors);
            mesh.vertex_count()
        };
        let plain = drawn(&score);
        score.press(at(&score, rect, 500.0, 270.0), &twice);
        assert!(drawn(&score) > plain, "the selection band and the caret");

        // the owner's answer to another edit: the same page again
        let again = serde_json::json!({"vb": [1000, 1000], "step": 90, "glyphs": {},
            "prims": [{"k": "text", "s": "A title", "x": 500, "y": 300, "size": 100,
                       "anchor": "middle", "id": "t-title"}]});
        assert!(score.set("display_list", &again));
        assert!(score.data.editing.is_some());
        // a page without that text lets go of it
        let gone = serde_json::json!({"vb": [1000, 1000], "step": 90, "glyphs": {}, "prims": []});
        assert!(score.set("display_list", &gone));
        assert!(score.data.editing.is_none());
    }

    fn input<'a>(m: &'a Metrics) -> Input<'a> {
        Input {
            metrics: m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 500.0, 200.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (600.0, 400.0),
            clicks: 1,
            time: None,
        }
    }

    /// The window pixel a page point lands on, through the same fit the
    /// renderer draws with -- so a test presses where the ink is.
    fn at(score: &Score, rect: Rect, px: f32, py: f32) -> (f64, f64) {
        let fit = score.data.fit(rect);
        let [x, y] = fit.apply(px, py);
        (x as f64, y as f64)
    }

    /// A click names the element under it and clears on blank paper -- the
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
        assert_eq!(score.data.selected.first().map(String::as_str), Some("n1"));
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
        assert!(score.data.selected.is_empty());
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
        assert_eq!(score.release((x, y - 100.0), true, &input), Events::none());
    }

    /// **Ctrl adds or removes, Shift extends, and neither drags.** A plain
    /// press replaces the selection and says only what it picked; a modified
    /// one keeps the others and says how it changed them, so the owner can
    /// answer a Shift with the whole range between the two.
    #[test]
    fn a_modified_press_adds_to_the_selection_and_says_how() {
        let metrics = Metrics::default();
        let mut score = page(true);
        let plain = input(&metrics);
        score.press(at(&score, plain.rect, 150.0, 250.0), &plain);
        assert_eq!(score.data.selected, vec!["n1".to_string()]);
        score.release((0.0, 0.0), true, &plain);

        let mut ctrl = input(&metrics);
        ctrl.mods.ctrl = true;
        let claim = score.press(at(&score, ctrl.rect, 450.0, 250.0), &ctrl);
        assert_eq!(
            score.data.selected,
            vec!["n1".to_string(), "n2".to_string()]
        );
        assert!(score.data.drag.is_none(), "a modified press does not drag");
        let Claim::Take(take) = claim else {
            panic!("the press is the page's")
        };
        assert_eq!(
            take.events,
            Events::message(vec![
                OscType::String("element".into()),
                OscType::String("n2".into()),
                OscType::String("toggle".into()),
            ])
        );
        // the same again takes it out
        score.press(at(&score, ctrl.rect, 450.0, 250.0), &ctrl);
        assert_eq!(score.data.selected, vec!["n1".to_string()]);

        let mut shift = input(&metrics);
        shift.mods.shift = true;
        score.press(at(&score, shift.rect, 450.0, 250.0), &shift);
        assert_eq!(
            score.data.selected,
            vec!["n1".to_string(), "n2".to_string()]
        );
        // a list set from the owner is drawn whole, and read back as the list
        assert!(score.set("selected", &serde_json::json!(["n1", "n2"])));
        assert_eq!(
            score.info(),
            vec![("selected".into(), serde_json::json!(["n1", "n2"]))]
        );
        assert!(score.set("selected", &Value::from(r#"["n2"]"#)));
        assert_eq!(score.data.selected, vec!["n2".to_string()]);
    }

    /// **Only what has a pitch drags.** A slur on an editable page is selected
    /// like any element and a drag on it moves nothing: no displacement, no
    /// ledger lines, and nothing reported -- where a slur sits is the
    /// engraver's, and its ends are notes.
    #[test]
    fn a_drag_on_what_has_no_pitch_does_nothing() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(true);
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,1000],"step":90,"editable":true,
                "prims":[{"k":"fill","d":"M0 0 L400 0 L400 40 L0 40 Z","xf":[300,850,1,1],"id":"s1"}],
                "kinds":{"s1":"slur"}}"#,
        )
        .unwrap();
        score.data = ScoreData::parse(&props);
        let (x, y) = at(&score, input.rect, 500.0, 870.0);
        let pressed = score.press((x, y), &input);
        assert_eq!(
            score.data.selected.first().map(String::as_str),
            Some("s1"),
            "it is selected"
        );
        assert!(!matches!(pressed, Claim::Decline), "and says so");
        assert!(score.data.drag.is_none(), "but nothing is held");
        score.drag((x, y - 100.0), &input);
        assert!(score.data.drag.is_none());
        assert_eq!(score.release((x, y - 100.0), true, &input), Events::none());
    }

    /// A page that names no kinds -- a client from before them -- offers no
    /// drag at all, rather than guessing which ids have a pitch.
    #[test]
    fn a_page_naming_no_kinds_does_not_drag() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(true);
        score.data.kinds.clear();
        score.press(at(&score, input.rect, 150.0, 250.0), &input);
        assert_eq!(score.data.selected.first().map(String::as_str), Some("n1"));
        assert!(score.data.drag.is_none());
    }

    /// An editable page displaces the element as the drag crosses whole
    /// diatonic steps, and the release reports the intent in the owner's units
    /// -- the staff position it lands on, never pixels and never a displacement.
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

        // The note is engraved two steps below the staff's top line, so a drag
        // of two steps up lands it *on* that line: the payload is the position
        // reached (0), not the displacement (2). The two differ here on
        // purpose -- with the note engraved on the line they would coincide and
        // a relative payload would pass this test.
        assert_eq!(score.data.staff_position("n1"), Some(-2));
        let events = score.release((press.0, press.1 - 2.0 * step_px), true, &input);
        assert_eq!(
            events,
            Events::message(vec![
                OscType::String("transpose".into()),
                OscType::String("n1".into()),
                OscType::Int(0),
            ])
        );
        assert!(
            score.data.drag.is_some(),
            "the displacement stays drawn until the client's re-engraved page arrives"
        );
    }

    /// A drag that ends where it started asks for nothing and drops its
    /// preview -- the press was a selection after all.
    #[test]
    fn a_drag_that_moved_nothing_retires_on_release() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(true);
        let press = at(&score, input.rect, 150.0, 250.0);
        score.press(press, &input);
        assert_eq!(score.release(press, true, &input), Events::none());
        assert!(score.data.drag.is_none());
    }

    /// A swept page declares that it reads the clock and a still one does not
    /// -- the declaration that used to be a `live.rs` arm asking what kind of
    /// widget this was.
    #[test]
    fn a_swept_page_declares_that_it_reads_the_clock() {
        let mut score = page(false);
        assert!(!score.needs().clock);
        assert!(score.set("playhead_at", &Value::from(48_000.0)));
        assert!(score.needs().clock);
    }

    /// A re-engraved page keeps the host's own chrome -- the selection the user
    /// is editing, the playhead -- and retires the drag preview it answers.
    #[test]
    fn a_new_display_list_keeps_the_chrome_and_retires_the_preview() {
        let mut score = page(true);
        score.data.selected = vec!["n2".into()];
        score.data.playhead = 500.0;
        score.data.drag = Some(ScoreDrag {
            id: "n2".into(),
            steps: 3,
        });
        assert!(score.set(
            "display_list",
            &Value::from(r#"{"vb":[1000,400],"prims":[]}"#)
        ));
        assert_eq!(score.data.selected.first().map(String::as_str), Some("n2"));
        assert_eq!(score.data.playhead, 500.0);
        assert!(score.data.editable, "an editor stays an editor");
        assert!(score.data.drag.is_none());
        assert!(score.data.prims.is_empty(), "and the drawing was replaced");
    }
    /// **A press on blank paper reports a place, on a page that asked for one.**
    /// It names the staff, how far up it, and the element the note would follow
    /// -- and nothing else. A staff position is not a pitch until something
    /// knows the clef and the key, and the host knows neither.
    #[test]
    fn a_page_taking_note_entry_reports_where_a_press_landed() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = entry_page();

        // To the right of both notes, on the staff: it follows the second. The
        // top line is y=20 and a step is 90, so the middle line at y=380 is
        // four steps below it.
        let claim = score.press(at(&score, input.rect, 700.0, 380.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("insert".into()),
                OscType::String("n2".into()),
                OscType::Int(-4),
                OscType::Int(0),
            ]))
        );
    }

    /// **A press on a staff line, on a page that takes entry, writes.**
    ///
    /// The defect (found 2026-09-07 by the user, by eye, `notation/
    /// score_editor`): the engraver gives its staff lines the staff's own id,
    /// and a staff line is a hairline the width of the system -- the tightest
    /// box on the page, thinner than any notehead. With nothing sounding under
    /// the pointer the tightest box decides alone, so a press aimed at a line
    /// rather than at a space came back as `"element"` naming the staff, and a
    /// press meant to write was spent on a selection. It looked random from the
    /// window; it is exactly the presses that land on a line. Measured over one
    /// sitting: 64 wrote a quarter, 10 answered with the drawing.
    #[test]
    fn a_press_on_a_staff_line_writes_rather_than_selecting_the_staff() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = engraved_staff();
        score.data.entry = true;
        score.data.elements = ["n1", "n2"].iter().map(|s| s.to_string()).collect();

        // y = 380 is the middle line, and 700 is clear of both noteheads: dead
        // on the furniture, and the one press that used to answer with it.
        let claim = score.press(at(&score, input.rect, 700.0, 380.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("insert".into()),
                OscType::String("n2".into()),
                OscType::Int(-4),
                OscType::Int(0),
            ])),
            "the line is a place to write, not a thing to select"
        );

        // A note still answers as itself: the rule reaches the furniture only.
        score.press(at(&score, input.rect, 450.0, 250.0), &input);
        assert_eq!(
            score.data.selected.first().map(String::as_str),
            Some("n2"),
            "a press on a notehead is still the note's"
        );
    }

    /// **A slur, a dynamic and anything else that does not sound are still
    /// selected by pointing at them**, on a page that takes entry.
    ///
    /// The regression this pins, shipped and caught by eye within the hour: the
    /// first cut of the rule above asked whether the pick *sounds*, and
    /// `elements` names notes and rests. A slur, a hairpin, a `p` and a beam
    /// sound nothing and are elements of the score all the same, so every press
    /// on one became a note and none of them could be selected --
    /// *"no es posible seleccionar ligaduras, p, mp y otros elementos, ahora
    /// siempre agrega notas"*. Sounding and *being the staff's own drawing* are
    /// different questions; only the second is this fix's business.
    #[test]
    fn a_slur_or_a_dynamic_is_still_selected_on_a_page_that_takes_entry() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,1000],"step":90,"editable":true,"entry":true,
                "glyphs":{"E0A4":"M0 0 L100 0 L100 -100 L0 -100 Z"},
                "prims":[
                  {"k":"line","pts":[[0,20],[1000,20]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,200],[1000,200]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,380],[1000,380]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,560],[1000,560]],"w":4,"id":"staff1"},
                  {"k":"line","pts":[[0,740],[1000,740]],"w":4,"id":"staff1"},
                  {"k":"glyph","cp":"E0A4","xf":[100,200,1,-1],"id":"n1"},
                  {"k":"glyph","cp":"E0A4","xf":[700,900,1,-1],"id":"dyn1"}]}"#,
        )
        .unwrap();
        let mut score = Score {
            data: ScoreData::parse(&props),
            origin_y: None,
        };
        // Only the notehead sounds; the dynamic is drawn and named like any
        // other element of the score.
        score.data.elements = ["n1"].iter().map(|s| s.to_string()).collect();
        assert!(
            score.data.staff_ids.contains("staff1"),
            "the staff's own lines were recognized"
        );
        assert!(
            !score.data.staff_ids.contains("dyn1"),
            "and a glyph is not one of them"
        );

        score.press(at(&score, input.rect, 750.0, 950.0), &input);
        assert_eq!(
            score.data.selected.first().map(String::as_str),
            Some("dyn1"),
            "a press on the dynamic selects it rather than writing a note"
        );
    }

    /// ...and a page that is **not** taking entry still selects the staff,
    /// because selecting one is a legitimate thing a score editor does. What
    /// the fix refuses is a *write* being spent on it.
    #[test]
    fn a_read_only_page_still_selects_the_staff_under_the_pointer() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = engraved_staff();
        score.data.elements = ["n1", "n2"].iter().map(|s| s.to_string()).collect();
        score.press(at(&score, input.rect, 700.0, 380.0), &input);
        assert_eq!(
            score.data.selected.first().map(String::as_str),
            Some("staff1"),
            "no entry on this page: the press has nothing else to be"
        );
    }

    /// **A staff is named by its place in its system, not down the page.** The
    /// two differ the moment a score wraps, and a two-staff score whose third
    /// system reported staff 4 was naming a staff no model has.
    #[test]
    fn a_wrapped_score_names_the_staff_of_its_system() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        // Two systems of two staves each, and the client's own reading of which
        // staves belong to which system.
        let props: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,5000],"step":90,"entry":true,"elements":["n1"],
                "systems":[[20,1740],[2500,4520]],
                "glyphs":{"E0A4":"M0 0 L100 0 L100 -100 L0 -100 Z"},
                "prims":[
                  {"k":"line","pts":[[0,20],[1000,20]],"w":4},
                  {"k":"line","pts":[[0,200],[1000,200]],"w":4},
                  {"k":"line","pts":[[0,380],[1000,380]],"w":4},
                  {"k":"line","pts":[[0,560],[1000,560]],"w":4},
                  {"k":"line","pts":[[0,740],[1000,740]],"w":4},
                  {"k":"line","pts":[[0,1020],[1000,1020]],"w":4},
                  {"k":"line","pts":[[0,1200],[1000,1200]],"w":4},
                  {"k":"line","pts":[[0,1380],[1000,1380]],"w":4},
                  {"k":"line","pts":[[0,1560],[1000,1560]],"w":4},
                  {"k":"line","pts":[[0,1740],[1000,1740]],"w":4},
                  {"k":"line","pts":[[0,2500],[1000,2500]],"w":4},
                  {"k":"line","pts":[[0,2680],[1000,2680]],"w":4},
                  {"k":"line","pts":[[0,2860],[1000,2860]],"w":4},
                  {"k":"line","pts":[[0,3040],[1000,3040]],"w":4},
                  {"k":"line","pts":[[0,3220],[1000,3220]],"w":4},
                  {"k":"line","pts":[[0,3800],[1000,3800]],"w":4},
                  {"k":"line","pts":[[0,3980],[1000,3980]],"w":4},
                  {"k":"line","pts":[[0,4160],[1000,4160]],"w":4},
                  {"k":"line","pts":[[0,4340],[1000,4340]],"w":4},
                  {"k":"line","pts":[[0,4520],[1000,4520]],"w":4},
                  {"k":"glyph","cp":"E0A4","xf":[100,200,1,-1],"id":"n1"}]}"#,
        )
        .unwrap();
        let mut score = Score {
            data: ScoreData::parse(&props),
            origin_y: None,
        };
        assert_eq!(score.data.staves.len(), 4, "four staves drawn");
        // A press on the *second* system's lower staff is staff 1 of the score
        // -- it is the fourth staff down the page, and that is not its name.
        let claim = score.press(at(&score, input.rect, 700.0, 4340.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("insert".into()),
                OscType::String(String::new()),
                OscType::Int(-6),
                OscType::Int(1),
            ]))
        );
    }

    /// **Note entry survives a re-engrave**, which is not a detail: answering
    /// an insertion *is* a new display list, so a page that lost the flag there
    /// took one note and then silently stopped taking any.
    #[test]
    fn a_re_engraved_page_still_takes_note_entry() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = entry_page();
        let page: Map<String, Value> = serde_json::from_str(
            r#"{"vb":[1000,1000],"step":90,
                "glyphs":{"E0A4":"M0 0 L100 0 L100 -100 L0 -100 Z"},
                "elements":["n1"],
                "prims":[
                  {"k":"line","pts":[[0,20],[1000,20]],"w":4},
                  {"k":"line","pts":[[0,200],[1000,200]],"w":4},
                  {"k":"line","pts":[[0,380],[1000,380]],"w":4},
                  {"k":"line","pts":[[0,560],[1000,560]],"w":4},
                  {"k":"line","pts":[[0,740],[1000,740]],"w":4},
                  {"k":"glyph","cp":"E0A4","xf":[100,200,1,-1],"id":"n1"}]}"#,
        )
        .unwrap();
        assert!(Element::set(
            &mut score,
            "display_list",
            &Value::Object(page)
        ));
        let claim = score.press(at(&score, input.rect, 700.0, 380.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("insert".into()),
                OscType::String("n1".into()),
                OscType::Int(-4),
                OscType::Int(0),
            ]))
        );
    }

    /// **The gesture is opt-in, and not a second meaning for `editable`.** On
    /// every other page a press on blank paper clears the selection, so a page
    /// that never asked for note entry must keep doing exactly that.
    #[test]
    fn a_page_that_did_not_ask_for_note_entry_only_clears_the_selection() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = page(true);
        score.data.elements = ["n1", "n2"].iter().map(|s| s.to_string()).collect();
        score.press(at(&score, input.rect, 150.0, 200.0), &input);
        assert_eq!(score.data.selected.first().map(String::as_str), Some("n1"));

        let claim = score.press(at(&score, input.rect, 700.0, 380.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("element".into()),
                OscType::String(String::new()),
            ])),
            "the selection is cleared, as it always was"
        );
    }

    /// A press that lands *on* an element is a selection, whatever the page
    /// takes: note entry is what happens where there is nothing.
    #[test]
    fn note_entry_does_not_take_over_a_press_on_a_note() {
        let metrics = Metrics::default();
        let input = input(&metrics);
        let mut score = entry_page();
        let claim = score.press(at(&score, input.rect, 150.0, 200.0), &input);
        assert_eq!(
            claim,
            Claim::events(Events::message(vec![
                OscType::String("element".into()),
                OscType::String("n1".into()),
            ]))
        );
    }
}
