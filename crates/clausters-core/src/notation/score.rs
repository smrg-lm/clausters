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

use super::{
    Cursor, DisplayList, Op, Sheet, TimemapEntry, apply, cursor_track, mei_to_sheet, sheet_to_mei,
    svg_to_display_list,
};

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
    /// The document as the **model**, read at open and kept in step with it.
    ///
    /// `None` when the document could not be read into one at all, which is a
    /// state and not a failure: the page still draws and still plays, and only
    /// the model's verbs are unavailable on it. A caller learns which it has
    /// from [`Score::sheet`].
    sheet: Option<Sheet>,
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
        let sheet = {
            let _guard = engraver.lock();
            if !engraver.load_data(data) {
                return None;
            }
            // The engraver normalizes whatever it loaded, so this reads one
            // encoding whatever the caller typed -- which is what makes a score
            // opened as ABC editable by the same verbs as one built in code.
            mei_to_sheet(&engraver.mei()).ok()
        };
        Some(Self {
            engraver,
            sheet,
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
        let ok = self.load_locked(&previous);
        self.resync_locked();
        ok
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
        let ok = self.load_locked(&next);
        self.resync_locked();
        ok
    }

    /// This score as the **model**, or `None` when the document could not be
    /// read into one.
    ///
    /// It is the live model: every edit made through [`Score::apply`] is in it,
    /// and so is every one made through the engraver escape hatch, since the
    /// document is re-read after each.
    pub fn sheet(&self) -> Option<&Sheet> {
        self.sheet.as_ref()
    }

    /// Apply one **model** operation as a single undo step, and re-engrave.
    ///
    /// This is the edit path, and the reason it is not verovio's editor: there
    /// is one implementation of what an edit to a score means, it is the
    /// vocabulary every client already binds, and a standalone host holding a
    /// sheet performs the same operation through the same code. Driving the
    /// engraver's editor instead would be a second answer to the same question,
    /// reachable only from a process that has an engraver in it.
    ///
    /// Returns whether it was applied. `false` when the document has no model,
    /// when the operation was refused (the sheet is untouched, since it crossed
    /// by value), or when the engraver could not load what the operation
    /// produced.
    pub fn apply(&mut self, op: &Op) -> bool {
        let Some(sheet) = self.sheet.clone() else {
            return false;
        };
        let Ok(edited) = apply(sheet, op) else {
            return false;
        };
        let Ok(mei) = sheet_to_mei(&edited) else {
            return false;
        };
        let _guard = self.engraver.lock();
        let before = self.mei_locked();
        if !self.load_locked(&mei) {
            self.load_locked(&before);
            return false;
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.drain(..self.undo.len() - UNDO_LIMIT);
        }
        self.redo.clear();
        // Re-read rather than trusting `edited`: what the engraver holds is
        // what the page draws and what the next edit starts from, and the two
        // must not be allowed to differ.
        self.sheet = mei_to_sheet(&self.mei_locked()).ok();
        true
    }

    /// Move the note `element_id` by `steps` **diatonic** steps along the staff —
    /// up when positive — as one undo step.
    ///
    /// It is the **model's** move where the page named a model item — the note
    /// takes the key signature's alteration for the letter it lands on, which
    /// is what reading in a key means — and falls back to the engraver's editor
    /// only for an element this layer did not write, which is the one case
    /// there is no item to move.
    ///
    /// It is in steps rather than in a position because verovio's
    /// coordinate-taking `drag` reads an absolute page y in a frame that does
    /// not line up with the display list's (passing a note its own drawn y
    /// moves it six steps), so a caller would have to carry an unexplained
    /// offset. Steps are exact.
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
        match item_id(element_id) {
            // The ordinary path: the page named an item of the model, and the
            // move is the model's own verb.
            Some(id) => self.apply(&Op::MoveSteps { id, steps }),
            // A document with no model behind it, or an element this layer did
            // not write. The engraver's editor is what is left, and it is why
            // the escape hatch stays.
            None => {
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
        }
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
        let ok = self.load_locked(&edited);
        self.resync_locked();
        ok
    }

    fn load_locked(&mut self, mei: &str) -> bool {
        self.drawn = false;
        self.engraver.load_data(mei)
    }

    /// Read the model back out of whatever the engraver now holds.
    ///
    /// Called after every path that changes the document without going through
    /// [`Score::apply`] — an undo, a redo, a raw editor action — because a
    /// model that had drifted from the page would apply the next operation to a
    /// score nobody is looking at.
    fn resync_locked(&mut self) {
        self.sheet = mei_to_sheet(&self.mei_locked()).ok();
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

/// The **model** item an engraved element belongs to, or `None` when the
/// element was not written from one.
///
/// The page names elements the way the emitter wrote them: `n7` is the item,
/// `n7-2` a piece of it split across a barline, `n7-p1` one pitch of a chord.
/// All three are the same item, which is what lets a gesture anywhere on a note
/// reach the note. An id of any other shape belongs to a document this layer
/// did not write, and there is nothing in the model to move.
///
/// It is public because a **client** needs it for the same reason this module
/// does: the page reports the element under a gesture, and every model verb
/// names an item. Working it out in a client would be a second answer to a
/// question the emitter already answered, and the two would disagree the first
/// time the emitter spelled a split or a chord differently.
pub fn item_id(element_id: &str) -> Option<u64> {
    element_id
        .strip_prefix('n')?
        .split('-')
        .next()?
        .parse()
        .ok()
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
    pub(super) struct Fake {
        mei: RefCell<String>,
        actions: RefCell<Vec<String>>,
        renders: RefCell<Vec<i32>>,
        refuse: RefCell<bool>,
        loads: RefCell<usize>,
    }

    impl Fake {
        pub(super) fn with(mei: &str) -> Self {
            Self {
                mei: RefCell::new(mei.to_string()),
                ..Default::default()
            }
        }
        pub(super) fn actions(&self) -> Vec<String> {
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

#[cfg(test)]
mod model_tests {
    use super::tests::Fake;
    use super::*;
    use crate::notation::{Item, Slot, voice_to_sheet};

    /// A real score, as MEI, so the model path has something to read.
    fn mei_of(n: usize) -> String {
        let voice: Vec<Slot> = (0..n).map(|_| Slot::note(vec![60], 8)).collect();
        sheet_to_mei(&voice_to_sheet(&voice, "4/4", "G2", "C")).unwrap()
    }

    #[test]
    fn a_score_opened_from_a_document_carries_the_model_behind_it() {
        let fake = Fake::with(&mei_of(4));
        let score = Score::open(&fake, &mei_of(4)).unwrap();
        let sheet = score.sheet().expect("the document read into a model");
        assert_eq!(sheet.staves[0].voices[0].items.len(), 4);
    }

    #[test]
    fn an_edit_goes_through_the_models_verbs_and_not_the_engravers_editor() {
        let fake = Fake::with(&mei_of(2));
        let mut score = Score::open(&fake, &mei_of(2)).unwrap();
        assert!(score.transpose("n1", 1));
        // The move is the model's, so the engraver was handed a *document* and
        // never an editor action: there is one implementation of what an edit
        // means, and a standalone with no engraver performs the same one.
        assert_eq!(fake.actions(), Vec::<String>::new());
        let moved = &score.sheet().unwrap().staves[0].voices[0].items[0];
        assert_eq!(moved.pitches()[0].step, crate::notation::Step::D);
        assert_eq!(moved.pitches()[0].octave, 4);
    }

    #[test]
    fn an_operation_is_applied_re_engraved_and_undone_as_one_step() {
        let fake = Fake::with(&mei_of(4));
        let mut score = Score::open(&fake, &mei_of(4)).unwrap();
        assert!(score.apply(&Op::Transpose {
            semitones: 12,
            steps: None,
            span: Default::default(),
        }));
        let up = score.sheet().unwrap().staves[0].voices[0].items[0].pitches()[0];
        assert_eq!(up.octave, 5);
        assert!(score.undo());
        // Undo puts the model back too, not only the page: the next edit starts
        // from what is drawn.
        let back = score.sheet().unwrap().staves[0].voices[0].items[0].pitches()[0];
        assert_eq!(back.octave, 4);
    }

    #[test]
    fn a_refused_operation_changes_neither_the_page_nor_the_model() {
        let fake = Fake::with(&mei_of(2));
        let mut score = Score::open(&fake, &mei_of(2)).unwrap();
        let before = score.mei();
        assert!(!score.apply(&Op::MoveSteps { id: 99, steps: 1 }));
        assert_eq!(score.mei(), before);
        assert!(!score.can_undo(), "a refusal is not an undo step");
    }

    #[test]
    fn an_element_this_layer_did_not_write_still_moves_through_the_engraver() {
        // A document with no model behind it: the escape hatch is what is left,
        // and it is why it stays.
        let fake = Fake::with("not a score");
        let mut score = Score::open(&fake, "not a score").unwrap();
        assert!(score.sheet().is_none());
        assert!(score.transpose("abc123", 1));
        assert!(fake.actions().iter().any(|a| a.contains("keyDown")));
    }

    #[test]
    fn a_chords_pitch_and_a_split_piece_both_name_the_note_they_belong_to() {
        assert_eq!(item_id("n7"), Some(7));
        assert_eq!(item_id("n7-2"), Some(7));
        assert_eq!(item_id("n7-p1"), Some(7));
        assert_eq!(item_id("m1ocu09p"), None);
    }

    #[test]
    fn a_rest_says_it_has_no_pitch_rather_than_moving_nothing() {
        let sheet = voice_to_sheet(&[Slot::rest(8)], "4/4", "G2", "C");
        let id = sheet.staves[0].voices[0].items[0].id();
        let err = crate::notation::move_steps(sheet, id, 1).unwrap_err();
        assert!(err.contains("rest"), "{err}");
    }

    #[test]
    fn a_note_dragged_onto_a_letter_the_armature_alters_arrives_altered() {
        // In E flat, dragging a note onto B gives B flat -- which is what
        // reading in a key means, and what nobody should have to say.
        let sheet = voice_to_sheet(&[Slot::note(vec![69], 8)], "4/4", "G2", "Eb");
        let id = sheet.staves[0].voices[0].items[0].id();
        let moved = crate::notation::move_steps(sheet, id, 1).unwrap();
        let Item::Note { pitches, .. } = &moved.staves[0].voices[0].items[0] else {
            panic!("a note");
        };
        assert_eq!(pitches[0].step, crate::notation::Step::B);
        assert_eq!(pitches[0].alter, -1);
    }
}
