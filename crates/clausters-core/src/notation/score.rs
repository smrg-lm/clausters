//! The stateful, editable score: a document held open so it can be edited and
//! re-engraved against the same ids — and the **engraver port** it drives.
//!
//! Engraving one-shot (load, draw, discard) needs no state. This is the other
//! one: the engraver stays open, so an edit lands on the very document the
//! display list was drawn from. The MEI `xml:id`s survive editing, which is what
//! lets a host keep its selection across the round trip — the id the user
//! clicked still names the same note afterwards.
//!
//! **Why this lives in the core and not beside the engraver.** Everything here
//! is *logic*: which calls an edit is made of and in what order, when the
//! document has to be reloaded, what an undo step is and where the stack lives.
//! There is one of each of those, and both clients drive it — a native caller
//! over libverovio, a page over the same engraver compiled to wasm. The part
//! that genuinely differs is only *how a call reaches verovio*, and that is the
//! whole of [`Engraver`].
//!
//! The port is deliberately **verovio's toolkit surface** rather than an
//! abstract notion of engraving: both implementations drive the same C wrapper
//! (`tools/c_wrapper.h` natively, the identical exports of the Emscripten build
//! in a page), so an abstraction over it would be a third vocabulary nobody
//! speaks.

use serde::Serialize;

use super::{Cursor, DisplayList, TimemapEntry, cursor_track, svg_to_display_list};

/// verovio's `keyDown` codes for the arrow keys (`vrvdef.h`): what moves a note
/// one diatonic step along the staff.
const KEY_UP: i32 = 38;
const KEY_DOWN: i32 = 40;

/// Snapshots the undo stack keeps. An MEI page is small, but not free.
const UNDO_LIMIT: usize = 64;

/// What a [`Score`] needs from an engraver, and the whole of what differs
/// between a native caller and a page.
///
/// Each method is one verovio toolkit call, named as the C wrapper names it.
/// An implementation does the crossing and nothing else: no ordering, no
/// caching, no recovery — those are [`Score`]'s, once.
///
/// **Failure is a value, not an error type.** A crossing can fail for reasons
/// that are the caller's (unloadable data, an action verovio refuses) and for
/// reasons that are the binding's (a null toolkit, a JS exception), and the
/// state machine treats both the same way: it rolls back. Each implementation
/// keeps its own richer error where it has one.
pub trait Engraver {
    /// What [`Engraver::lock`] hands back — a guard for the native library's
    /// FFI lock, or `()` where the engraver cannot be reached concurrently at
    /// all (a page has one thread).
    type Guard;

    /// Taken for the whole of one public [`Score`] operation, which is what
    /// makes a sequence of calls one atomic use of the engraver. A native
    /// binding returns its FFI lock's guard here — libverovio has process-wide
    /// state, so two scores must not be inside it at once.
    fn lock(&self) -> Self::Guard;

    /// `vrvToolkit_loadData`: parse and lay out a score in any format the
    /// engraver auto-detects. `false` when it could not be read.
    fn load_data(&self, data: &str) -> bool;

    /// `vrvToolkit_renderToSVG`: the page as an SVG of glyph outlines and
    /// engraving strokes.
    fn render_svg(&self, page: i32) -> String;

    /// `vrvToolkit_getMEI`: the document as MEI, ids and all.
    fn mei(&self) -> String;

    /// `vrvToolkit_edit`: one editor action, as the JSON object verovio takes.
    /// `false` when it refused.
    fn edit(&self, action: &str) -> bool;

    /// `vrvToolkit_renderToTimemap`: the timemap JSON for the loaded document.
    fn timemap(&self, options: &str) -> String;

    /// `vrvToolkit_getMIDIValuesForElement`: what one element sounds as, or
    /// `None` when the engraver had nothing to say about it.
    fn midi_values(&self, xml_id: &str) -> Option<String>;
}

/// The engraver's options for one page, as the JSON object it is configured
/// with — `scale` (staff size), `page_width` (the page units a score wraps into
/// systems at) and whatever `extra` a caller merges over them.
///
/// Here rather than in a binding because these are what a page *looks like*,
/// and two clients configuring their engravers differently would draw the same
/// score two ways — a display list that cannot be compared across clients, which
/// is the one thing the shared engraver was for. A caller passing non-object
/// JSON as `extra` has it ignored rather than refused.
pub fn engrave_options(scale: i32, page_width: i32, extra: Option<&str>) -> String {
    use serde_json::{Map, Value, json};
    let mut map: Map<String, Value> = json!({
        "scale": scale,
        "adjustPageHeight": true,
        "svgViewBox": true,
        "breaks": "auto",
        "pageWidth": page_width,
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    if let Some(extra) = extra
        && let Ok(Value::Object(m)) = serde_json::from_str::<Value>(extra)
    {
        map.extend(m);
    }
    Value::Object(map).to_string()
}

/// One engraving, in the three layers a client sends and plays.
///
/// The drawing layers are flattened in, so the serialized object is exactly the
/// display list plus `cursors` and `notes`: one JSON value carries the whole
/// page. All three come out of a single engraving because they must — the
/// engraver mints fresh `xml:id`s on every load, so ids from two engravings do
/// not line up.
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    /// What the host draws: `vb`, `glyphs`, `prims`, `step`.
    #[serde(flatten)]
    pub draw: DisplayList,
    /// Where the playhead goes: the timemap folded into page geometry.
    pub cursors: Vec<Cursor>,
    /// What sounds: one event per note, in milliseconds and MIDI pitch. The
    /// client's own layer — a driver plays it, the host never sees it.
    pub notes: Vec<NoteEvent>,
}

/// One sounding note: onset `t` and `dur` in milliseconds, MIDI `pitch`, and the
/// MEI `id` that ties it to the primitive drawn for it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteEvent {
    pub t: f64,
    pub dur: f64,
    pub pitch: i32,
    pub id: String,
}

/// A loaded score, kept alive so it can be **edited** and re-engraved.
///
/// Every edit runs the same three steps, because verovio needs all of them: the
/// editor action, then `commit` (which is what re-runs the layout — an action
/// alone changes the document but leaves the drawing stale), then a reload of the
/// edited MEI. That last step looks redundant and is not: the MIDI/timemap cache
/// is *not* invalidated by an edit, so without it a transposed note keeps
/// sounding at its old pitch.
///
/// Undo is ours, not verovio's — a stack of MEI snapshots. Reloading the document
/// to refresh those caches resets the editor's own undo stack, so its stack could
/// not survive the cycle anyway; and its `canUndo`/`canRedo` are unreliable (a
/// successful edit can leave `canUndo` false) while `undo` on an empty stack
/// crashes the process. Owning the stack sidesteps all three.
pub struct Score<E: Engraver> {
    engraver: E,
    undo: Vec<String>,
    redo: Vec<String>,
    /// Whether the page has been drawn since the last load. Editing a document
    /// that was loaded but never rendered **segfaults** natively (the editor
    /// reaches through drawing state the load does not build), and since every
    /// edit reloads, two edits in a row would hit exactly that.
    drawn: bool,
}

impl<E: Engraver> Score<E> {
    /// Load `data` into `engraver` and keep the document open, or `None` when
    /// the engraver could not read it.
    ///
    /// Constructing the engraver — a resource path, options, whatever the
    /// binding needs — happens before this and belongs to the binding.
    pub fn open(engraver: E, data: &str) -> Option<Self> {
        {
            let _guard = engraver.lock();
            if !engraver.load_data(data) {
                return None;
            }
        }
        Some(Self {
            engraver,
            undo: Vec::new(),
            redo: Vec::new(),
            drawn: false,
        })
    }

    /// The engraver underneath, for a binding's own escape hatch (verovio's
    /// `editInfo`, its version string) — never for driving an edit, which is
    /// what this type is for.
    pub fn engraver(&self) -> &E {
        &self.engraver
    }

    /// This score engraved into a [`Page`] — from the live document, so it
    /// reflects every edit applied so far.
    pub fn display_list(&mut self, page: i32) -> Page {
        let _guard = self.engraver.lock();
        self.page_locked(page)
    }

    /// The score as MEI, ids and all — the format to persist, and what the undo
    /// stack is made of.
    pub fn mei(&self) -> String {
        let _guard = self.engraver.lock();
        self.mei_locked()
    }

    /// Whether there is an edit to step back over.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is an undone edit to step forward into.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Step back one edit. `false` (never a crash) when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let _guard = self.engraver.lock();
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        let current = self.mei_locked();
        self.redo.push(current);
        self.load_locked(&previous)
    }

    /// Step forward again after [`Score::undo`]; `false` when there is nothing to
    /// redo.
    pub fn redo(&mut self) -> bool {
        let _guard = self.engraver.lock();
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = self.mei_locked();
        self.undo.push(current);
        self.load_locked(&next)
    }

    /// Move the note `element_id` by `steps` **diatonic** steps along the staff —
    /// up when positive — as one undo step.
    ///
    /// This is the pitch edit as **verovio** expresses it, and it is in steps
    /// rather than in a position because verovio's coordinate-taking `drag`
    /// reads an absolute page y in a frame that does not line up with the
    /// display list's (passing a note its own drawn y moves it six steps), so a
    /// caller would have to carry an unexplained offset. Steps are exact.
    ///
    /// It is **not** the shape an edit travels in: a displacement made against
    /// a page that has since been re-engraved has to be rebased, which is why
    /// the wire carries a position and [`Score::transpose_to`] is what applies
    /// one. Reach for this directly only when the delta is what you actually
    /// have.
    pub fn transpose(&mut self, element_id: &str, steps: i32) -> bool {
        if steps == 0 {
            return false;
        }
        let key = if steps > 0 { KEY_UP } else { KEY_DOWN };
        let action = serde_json::json!({
            "action": "keyDown",
            "param": { "elementId": element_id, "key": key },
        })
        .to_string();
        let actions = vec![action; steps.unsigned_abs() as usize];
        let _guard = self.engraver.lock();
        self.apply_locked(&actions)
    }

    /// Move the note `element_id` **to** the diatonic staff position
    /// `position` on `page` — whole steps from its staff's top line, positive
    /// upward — as one undo step.
    ///
    /// This is the pitch edit as it **travels**: absolute, so applying it twice
    /// leaves the note where it is and a page re-engraved under the gesture
    /// needs no rebasing. The relative call underneath is verovio's
    /// requirement, not the wire's, and the delta is computed here against the
    /// engraving rather than carried from wherever the gesture happened —
    /// which is the whole point, since the two can differ.
    ///
    /// Both sides read the position off the same drawing
    /// ([`DisplayList::staff_position`]), so a host naming a position and this
    /// resolving one cannot disagree about what it means.
    ///
    /// Returns whether the note is now at `position`: **true when it was
    /// already there**, since the requested state holds and a resend must be
    /// harmless. False when the element is not on that page, the page has no
    /// staff to measure against, or verovio refused the move.
    pub fn transpose_to(&mut self, element_id: &str, position: i32, page: i32) -> bool {
        let Some(from) = self.display_list(page).draw.staff_position(element_id) else {
            return false;
        };
        from == position || self.transpose(element_id, position - from)
    }

    /// Apply one raw verovio editor action (`set`, `insert`, `delete`, ...) as a
    /// single undo step — the escape hatch for what [`Score::transpose`] does not
    /// cover. `param` is the action's parameter object as a JSON string (`"{}"`
    /// for none). Returns whether verovio accepted it; a rejected action leaves
    /// the score untouched.
    pub fn edit(&mut self, action: &str, param: &str) -> bool {
        let param: serde_json::Value = serde_json::from_str(param).unwrap_or(serde_json::json!({}));
        let action = serde_json::json!({ "action": action, "param": param }).to_string();
        let _guard = self.engraver.lock();
        self.apply_locked(&[action])
    }

    // -- internals: every one of these assumes the engraver's lock is held ----

    /// Run `actions` as one undo step, then make every derived structure agree
    /// with the result. Rolls back if verovio rejects any of them, so a failed
    /// edit is not a half-edited score.
    fn apply_locked(&mut self, actions: &[String]) -> bool {
        self.ensure_drawn_locked();
        let before = self.mei_locked();
        let mut ok = true;
        for action in actions {
            ok = self.engraver.edit(action) && ok;
        }
        // `commit` is what re-runs the layout; an action alone leaves it stale.
        let _ = self.engraver.edit(r#"{"action":"commit"}"#);
        if !ok {
            self.load_locked(&before);
            return false;
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.drain(..self.undo.len() - UNDO_LIMIT);
        }
        self.redo.clear();
        // Reload our own edited MEI: the layout is fresh after `commit`, but the
        // MIDI/timemap cache is not, and `notes` is read from it.
        let edited = self.mei_locked();
        self.load_locked(&edited)
    }

    fn load_locked(&mut self, mei: &str) -> bool {
        self.drawn = false;
        self.engraver.load_data(mei)
    }

    /// Draw the page if it has not been drawn since the last load, so the editor
    /// never reaches through drawing state that is not there. One render either
    /// way: the common path draws the page anyway, to send it.
    fn ensure_drawn_locked(&mut self) {
        if !self.drawn {
            self.engraver.render_svg(1);
            self.drawn = true;
        }
    }

    fn mei_locked(&self) -> String {
        self.engraver.mei()
    }

    fn page_locked(&mut self, page: i32) -> Page {
        let draw = svg_to_display_list(&self.engraver.render_svg(page));
        self.drawn = true;
        let timemap = self.timemap_locked();
        let cursors = cursor_track(&draw, &timemap);
        let notes = self.note_events_locked(&timemap);
        Page {
            draw,
            cursors,
            notes,
        }
    }

    fn timemap_locked(&self) -> Vec<TimemapEntry> {
        let json = self.engraver.timemap(r#"{"includeMeasures":false}"#);
        serde_json::from_str(&json).unwrap_or_default()
    }

    /// The score's sounding events, read from the same layout the page was drawn
    /// from. Silent elements (a rest, a tied continuation) carry no pitch and
    /// place no event.
    fn note_events_locked(&self, timemap: &[TimemapEntry]) -> Vec<NoteEvent> {
        let mut events = Vec::new();
        for entry in timemap {
            for id in &entry.on {
                let Some(json) = self.engraver.midi_values(id) else {
                    continue;
                };
                let Ok(midi) = serde_json::from_str::<serde_json::Value>(&json) else {
                    continue;
                };
                let pitch = midi.get("pitch").and_then(|v| v.as_i64()).unwrap_or(0);
                if pitch == 0 {
                    continue;
                }
                events.push(NoteEvent {
                    t: number(&midi, "time").unwrap_or(entry.tstamp.unwrap_or(0.0)),
                    dur: number(&midi, "duration").unwrap_or(0.0),
                    pitch: pitch as i32,
                    id: id.clone(),
                });
            }
        }
        events.sort_by(|a, b| a.t.total_cmp(&b.t));
        events
    }
}

fn number(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key)?.as_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// An engraver that is not one: it holds a string, records the actions it
    /// was handed and refuses whatever it is told to refuse.
    ///
    /// It exists because the *state machine* is what this module owns, and none
    /// of it is about verovio: that a rejected action leaves the document as it
    /// was, that an undo step is the MEI before it, that the document is drawn
    /// before an edit reaches it. The real engraver's own tests live beside the
    /// binding (`clausters-notation`), where there is a C++ library to be right
    /// about.
    #[derive(Default)]
    struct Fake {
        mei: RefCell<String>,
        actions: RefCell<Vec<String>>,
        renders: RefCell<Vec<i32>>,
        refuse: RefCell<bool>,
        loads: RefCell<usize>,
    }

    impl Fake {
        fn with(mei: &str) -> Self {
            Self {
                mei: RefCell::new(mei.to_string()),
                ..Default::default()
            }
        }
        fn actions(&self) -> Vec<String> {
            self.actions.borrow().clone()
        }
    }

    impl Engraver for &Fake {
        type Guard = ();
        fn lock(&self) -> Self::Guard {}
        fn load_data(&self, data: &str) -> bool {
            if data.is_empty() {
                return false;
            }
            *self.loads.borrow_mut() += 1;
            *self.mei.borrow_mut() = data.to_string();
            true
        }
        fn render_svg(&self, page: i32) -> String {
            self.renders.borrow_mut().push(page);
            String::new()
        }
        fn mei(&self) -> String {
            self.mei.borrow().clone()
        }
        fn edit(&self, action: &str) -> bool {
            self.actions.borrow_mut().push(action.to_string());
            if action.contains("commit") {
                return true;
            }
            if *self.refuse.borrow() {
                return false;
            }
            // An accepted edit is *something* happening to the document, so the
            // snapshots a caller compares are not all one string.
            self.mei.borrow_mut().push('!');
            true
        }
        fn timemap(&self, _options: &str) -> String {
            "[]".to_string()
        }
        fn midi_values(&self, _xml_id: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn unreadable_data_opens_nothing() {
        let fake = Fake::default();
        assert!(Score::open(&fake, "").is_none());
    }

    #[test]
    fn an_edit_is_one_undo_step_and_comes_back() {
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        assert!(!score.can_undo());

        assert!(score.edit("set", "{}"));
        assert!(score.can_undo());
        assert_eq!(score.mei(), "<mei/>!");

        assert!(score.undo());
        assert_eq!(score.mei(), "<mei/>");
        assert!(!score.can_undo());
        assert!(score.can_redo());

        assert!(score.redo());
        assert_eq!(score.mei(), "<mei/>!");
    }

    #[test]
    fn undo_and_redo_on_an_empty_stack_are_false_not_a_crash() {
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        assert!(!score.undo());
        assert!(!score.redo());
    }

    #[test]
    fn a_new_edit_drops_the_redo_branch() {
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        score.edit("set", "{}");
        score.undo();
        assert!(score.can_redo());
        score.edit("set", "{}");
        assert!(!score.can_redo());
    }

    #[test]
    fn a_refused_action_leaves_the_document_where_it_was() {
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        *fake.refuse.borrow_mut() = true;
        assert!(!score.edit("set", "{}"));
        assert_eq!(score.mei(), "<mei/>");
        assert!(!score.can_undo());
    }

    #[test]
    fn the_document_is_drawn_before_an_edit_reaches_it() {
        // Editing a document that was loaded and never rendered segfaults in the
        // real engraver, and every edit reloads -- so two edits in a row must
        // draw twice, not once.
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        score.edit("set", "{}");
        score.edit("set", "{}");
        assert_eq!(*fake.renders.borrow(), vec![1, 1]);
    }

    #[test]
    fn an_edit_commits_and_reloads_the_document_it_produced() {
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        let loads_before = *fake.loads.borrow();
        score.edit("set", "{}");
        let actions = fake.actions();
        assert_eq!(actions.len(), 2, "the action, then the commit");
        assert!(actions[0].contains("\"set\""));
        assert!(actions[1].contains("commit"));
        assert_eq!(
            *fake.loads.borrow() - loads_before,
            1,
            "the edited MEI is loaded back, or the timemap stays stale",
        );
    }

    #[test]
    fn a_transpose_is_one_key_press_per_step_in_one_undo_step() {
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        assert!(score.transpose("note-1", 3));
        let actions = fake.actions();
        assert_eq!(actions.len(), 4, "three key presses and one commit");
        assert_eq!(
            actions.iter().filter(|a| a.contains("\"key\":38")).count(),
            3
        );
        assert!(score.can_undo());
        assert!(score.undo());
        assert!(!score.can_undo(), "the three presses are one step");

        assert!(score.transpose("note-1", -1));
        assert!(fake.actions().last().unwrap().contains("commit"));
        assert_eq!(
            fake.actions()
                .iter()
                .filter(|a| a.contains("\"key\":40"))
                .count(),
            1,
        );
    }

    #[test]
    fn a_transpose_of_no_steps_is_not_an_edit() {
        let fake = Fake::with("<mei/>");
        let mut score = Score::open(&fake, "<mei/>").expect("opens");
        assert!(!score.transpose("note-1", 0));
        assert!(fake.actions().is_empty());
        assert!(!score.can_undo());
    }
}
