//! The stateful, editable score: a document held open so it can be edited and
//! re-engraved against the same ids.
//!
//! [`engrave_svg`](crate::engrave_svg) is the one-shot form — load, draw,
//! discard. This is the other one: the toolkit stays open, so an edit lands on
//! the very document the display list was drawn from. The MEI `xml:id`s survive
//! editing, which is what lets a host keep its selection across the round trip —
//! the id the user clicked still names the same note afterwards.

use clausters_core::notation::{
    Cursor, DisplayList, TimemapEntry, cursor_track, svg_to_display_list,
};
use serde::Serialize;

use crate::verovio::{EngraveError, EngraveOptions, Toolkit, default_resource_path, ffi_lock};

/// verovio's `keyDown` codes for the arrow keys (`vrvdef.h`): what moves a note
/// one diatonic step along the staff.
const KEY_UP: i32 = 38;
const KEY_DOWN: i32 = 40;

/// Snapshots the undo stack keeps. An MEI page is small, but not free.
const UNDO_LIMIT: usize = 64;

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
pub struct Score {
    tk: Toolkit,
    undo: Vec<String>,
    redo: Vec<String>,
    /// Whether the page has been drawn since the last load. Editing a document
    /// that was loaded but never rendered **segfaults** (the editor reaches
    /// through drawing state the load does not build), and since every edit
    /// reloads, two edits in a row would hit exactly that.
    drawn: bool,
}

// SAFETY: every public method brackets its whole verovio sequence with
// `ffi_lock`, so no two calls — on this score or any other — ever run in the
// library at once. That is the same discipline `engrave_svg` follows, and it is
// what makes handing a handle to another thread (a GUI client's usual shape)
// sound; the toolkit pointer itself is never shared.
unsafe impl Send for Score {}

impl Score {
    /// Load `data` (a score in any format verovio auto-detects) and lay it out,
    /// keeping the document open. `opts.page` is ignored here — the page to draw
    /// is chosen per call in [`Score::display_list`].
    pub fn open(data: &str, opts: &EngraveOptions) -> Result<Self, EngraveError> {
        let options = crate::verovio::options_json(opts);
        let resources = opts.resource_path.clone().or_else(default_resource_path);

        let _guard = ffi_lock();
        let tk = Toolkit::new(resources.as_deref())?;
        tk.set_options(&options)?;
        if !tk.load_data(data)? {
            return Err(EngraveError::Load);
        }
        Ok(Self {
            tk,
            undo: Vec::new(),
            redo: Vec::new(),
            drawn: false,
        })
    }

    /// This score engraved into a [`Page`] — from the live document, so it
    /// reflects every edit applied so far.
    pub fn display_list(&mut self, page: i32) -> Page {
        let _guard = ffi_lock();
        self.page_locked(page)
    }

    /// The score as MEI, ids and all — the format to persist, and what the undo
    /// stack is made of.
    pub fn mei(&self) -> String {
        let _guard = ffi_lock();
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
        let _guard = ffi_lock();
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
        let _guard = ffi_lock();
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
        let _guard = ffi_lock();
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
    /// (`clausters_core::notation::DisplayList::staff_position`), so a host
    /// naming a position and this resolving one cannot disagree about what it
    /// means.
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
        let _guard = ffi_lock();
        self.apply_locked(&[action])
    }

    // -- internals: every one of these assumes `ffi_lock` is already held ------

    /// Run `actions` as one undo step, then make every derived structure agree
    /// with the result. Rolls back if verovio rejects any of them, so a failed
    /// edit is not a half-edited score.
    fn apply_locked(&mut self, actions: &[String]) -> bool {
        self.ensure_drawn_locked();
        let before = self.mei_locked();
        let mut ok = true;
        for action in actions {
            ok = self.tk.edit(action).unwrap_or(false) && ok;
        }
        // `commit` is what re-runs the layout; an action alone leaves it stale.
        let _ = self.tk.edit(r#"{"action":"commit"}"#);
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
        self.tk.load_data(mei).unwrap_or(false)
    }

    /// Draw the page if it has not been drawn since the last load, so the editor
    /// never reaches through drawing state that is not there. One render either
    /// way: the common path draws the page anyway, to send it.
    fn ensure_drawn_locked(&mut self) {
        if !self.drawn {
            self.tk.render_svg(1);
            self.drawn = true;
        }
    }

    fn mei_locked(&self) -> String {
        self.tk.mei("{}").unwrap_or_default()
    }

    fn page_locked(&mut self, page: i32) -> Page {
        let draw = svg_to_display_list(&self.tk.render_svg(page));
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
        let json = self
            .tk
            .render_timemap(r#"{"includeMeasures":false}"#)
            .unwrap_or_default();
        serde_json::from_str(&json).unwrap_or_default()
    }

    /// The score's sounding events, read from the same layout the page was drawn
    /// from. Silent elements (a rest, a tied continuation) carry no pitch and
    /// place no event.
    fn note_events_locked(&self, timemap: &[TimemapEntry]) -> Vec<NoteEvent> {
        let mut events = Vec::new();
        for entry in timemap {
            for id in &entry.on {
                let Ok(json) = self.tk.midi_values(id) else {
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
    use clausters_core::notation::{Slot, voice_to_mei};

    /// The two-bar fixture: a C major scale in quarter notes, written by the
    /// core's own encoder. Going in through MEI rather than through typed score
    /// text pins the pitches exactly, so what comes back out of the engraver is
    /// comparable note for note.
    fn phrase() -> String {
        let voice: Vec<Slot> = [60, 62, 64, 65, 67, 69, 71, 72]
            .into_iter()
            .map(|midi| Slot::Note {
                midis: vec![midi],
                ticks: 8, // a quarter, at 32 ticks to the whole note
            })
            .collect();
        voice_to_mei(&voice, "4/4", "G2", "C")
    }

    fn score() -> Score {
        Score::open(&phrase(), &EngraveOptions::default()).expect("opens")
    }

    /// The ids of the eight noteheads, in draw order.
    fn note_ids(page: &Page) -> Vec<String> {
        page.notes.iter().map(|n| n.id.clone()).collect()
    }

    #[test]
    fn a_page_carries_the_drawing_the_cursors_and_the_notes() {
        let page = score().display_list(1);
        assert!(page.draw.vb[0] > 0.0, "a sized page");
        assert_eq!(page.notes.len(), 8, "eight noteheads across two bars");
        assert_eq!(page.cursors.len(), 8, "one cursor per onset");
        // C major scale from middle C up: MIDI 60..72 diatonically.
        let pitches: Vec<i32> = page.notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 62, 64, 65, 67, 69, 71, 72]);
        // Every cursor lands on a drawn position inside the page.
        for c in &page.cursors {
            assert!(c.x > 0.0 && c.x < page.draw.vb[0], "x within the page");
            assert!(c.y0 < c.y1, "a span, top above bottom");
        }
    }

    #[test]
    fn the_cursor_times_follow_the_note_onsets() {
        let page = score().display_list(1);
        let onsets: Vec<f64> = page.notes.iter().map(|n| n.t).collect();
        let cursors: Vec<f64> = page.cursors.iter().map(|c| c.t).collect();
        assert_eq!(onsets, cursors, "one cursor per onset, same times");
    }

    #[test]
    fn transposing_a_note_moves_only_that_note() {
        let mut sc = score();
        let before = sc.display_list(1);
        let ids = note_ids(&before);
        assert!(sc.transpose(&ids[0], 1), "verovio accepts the step");
        let after = sc.display_list(1);
        let pitches: Vec<i32> = after.notes.iter().map(|n| n.pitch).collect();
        assert_eq!(
            pitches,
            vec![62, 62, 64, 65, 67, 69, 71, 72],
            "the first note is a step up, the rest untouched"
        );
    }

    /// The absolute form lands on the position it names, whatever the note's
    /// own is — the property that lets an edit cross a wire.
    #[test]
    fn transposing_to_a_position_lands_on_it() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        let target = sc.display_list(1).draw.staff_position(&ids[0]).unwrap() + 2;
        assert!(sc.transpose_to(&ids[0], target, 1), "verovio accepts it");
        assert_eq!(
            sc.display_list(1).draw.staff_position(&ids[0]),
            Some(target),
            "the note is at the position that was named"
        );
    }

    /// The acceptance the absolute form exists for: a resend must be harmless.
    /// A relative payload applied twice would move the note twice.
    #[test]
    fn transposing_to_the_same_position_twice_moves_the_note_once() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        let target = sc.display_list(1).draw.staff_position(&ids[0]).unwrap() + 3;
        assert!(sc.transpose_to(&ids[0], target, 1));
        let once: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();

        assert!(
            sc.transpose_to(&ids[0], target, 1),
            "already there is success, not a refusal -- the requested state holds"
        );
        let twice: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(once, twice, "the second application changed nothing");
    }

    #[test]
    fn an_unplaceable_transpose_target_is_refused_rather_than_guessed() {
        let mut sc = score();
        assert!(!sc.transpose_to("no-such-id", 0, 1));
    }

    #[test]
    fn an_edit_is_one_undo_step_and_comes_back() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        assert!(!sc.can_undo(), "a fresh score has nothing to undo");
        sc.transpose(&ids[0], 2);
        assert!(sc.can_undo() && !sc.can_redo());

        assert!(sc.undo(), "steps back");
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[0], 60, "back to the original pitch");
        assert!(!sc.can_undo() && sc.can_redo());

        assert!(sc.redo(), "steps forward again");
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[0], 64, "two diatonic steps up from C is E");
    }

    #[test]
    fn undo_and_redo_on_an_empty_stack_are_false_not_a_crash() {
        let mut sc = score();
        assert!(!sc.undo());
        assert!(!sc.redo());
    }

    #[test]
    fn a_new_edit_drops_the_redo_branch() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        sc.transpose(&ids[0], 1);
        sc.undo();
        assert!(sc.can_redo());
        sc.transpose(&ids[0], -1);
        assert!(!sc.can_redo(), "the undone branch is gone");
    }

    #[test]
    fn transposing_by_zero_steps_is_not_an_edit() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        assert!(!sc.transpose(&ids[0], 0));
        assert!(!sc.can_undo(), "nothing was pushed");
    }

    #[test]
    fn a_rejected_edit_leaves_the_score_untouched() {
        let mut sc = score();
        let before = sc.mei();
        assert!(
            !sc.transpose("no-such-element", 1),
            "verovio rejects an unknown id"
        );
        assert!(!sc.can_undo(), "a rejected edit is not an undo step");
        assert_eq!(sc.mei(), before, "rolled back to the snapshot");
    }

    #[test]
    fn the_raw_edit_hatch_reaches_the_editor() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        let param = format!(r#"{{"elementId": "{}", "key": {}}}"#, ids[0], KEY_UP);
        assert!(
            sc.edit("keyDown", &param),
            "the same action transpose sends"
        );
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[0], 62);
    }

    #[test]
    fn two_edits_in_a_row_survive_the_reload() {
        // Every edit reloads, and editing an undrawn document segfaults, so this
        // is the case `ensure_drawn` exists for.
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        assert!(sc.transpose(&ids[0], 1));
        assert!(sc.transpose(&ids[1], 1));
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[..2], [62, 64]);
    }

    #[test]
    fn the_mei_round_trips_through_a_second_score() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        sc.transpose(&ids[0], 1);
        let mei = sc.mei();
        let mut reopened = Score::open(&mei, &EngraveOptions::default()).expect("MEI reloads");
        let pitches: Vec<i32> = reopened
            .display_list(1)
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        assert_eq!(pitches[0], 62, "the edit persisted through MEI");
    }

    #[test]
    fn a_page_serializes_to_the_layers_a_client_sends() {
        let json = serde_json::to_value(score().display_list(1)).expect("serializes");
        for key in ["vb", "glyphs", "prims", "step", "cursors", "notes"] {
            assert!(json.get(key).is_some(), "the `{key}` layer");
        }
    }
}
