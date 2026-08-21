//! The libverovio binding of the editable score: what an [`Engraver`] call is,
//! natively.
//!
//! The score model itself — the order an edit is made in, when the layout is
//! re-run and reloaded, the undo stack of MEI snapshots — is
//! [`clausters_core::notation::Score`], because all of it is logic and both
//! clients run it. What is here is the crossing: [`Toolkit`] implements the
//! [`Engraver`] port over libverovio's C wrapper, and [`open`] builds one with
//! a resource path and options before handing it over.
//!
//! The tests below are the other half of that split: they need a real C++
//! engraver to be right about, which is why they are here and not in the core,
//! whose own tests drive the state machine over a fake.

use clausters_core::notation::{Engraver, Score as CoreScore};

use crate::verovio::{
    EngraveError, EngraveOptions, Toolkit, default_resource_path, ffi_lock, options_json,
};

pub use clausters_core::notation::{NoteEvent, Page};

/// An editable score over the native engraver — the type the C ABI hands out.
pub type Score = CoreScore<Toolkit>;

impl Engraver for Toolkit {
    /// libverovio has process-wide state, so the guard is the crate's FFI lock
    /// and it covers a whole operation rather than a call. That is the same
    /// discipline [`crate::engrave_svg`] follows, and it is what makes handing a
    /// score to another thread sound.
    type Guard = std::sync::MutexGuard<'static, ()>;

    fn lock(&self) -> Self::Guard {
        ffi_lock()
    }

    fn load_data(&self, data: &str) -> bool {
        Toolkit::load_data(self, data).unwrap_or(false)
    }

    fn render_svg(&self, page: i32) -> String {
        Toolkit::render_svg(self, page)
    }

    fn mei(&self) -> String {
        Toolkit::mei(self, "{}").unwrap_or_default()
    }

    fn edit(&self, action: &str) -> bool {
        Toolkit::edit(self, action).unwrap_or(false)
    }

    fn timemap(&self, options: &str) -> String {
        Toolkit::render_timemap(self, options).unwrap_or_default()
    }

    fn midi_values(&self, xml_id: &str) -> Option<String> {
        Toolkit::midi_values(self, xml_id).ok()
    }
}

/// Open `data` (a score in any format verovio auto-detects) on a fresh toolkit
/// and keep the document open. `opts.page` is ignored — the page to draw is
/// chosen per call in [`Score::display_list`].
///
/// The constructor is here rather than on [`Score`] because building the
/// engraver is exactly what a binding owns: the resource path libverovio needs,
/// and the options JSON it is configured with.
pub fn open(data: &str, opts: &EngraveOptions) -> Result<Score, EngraveError> {
    let options = options_json(opts);
    let resources = opts.resource_path.clone().or_else(default_resource_path);

    let toolkit = {
        let _guard = ffi_lock();
        let tk = Toolkit::new(resources.as_deref())?;
        tk.set_options(&options)?;
        tk
    };
    Score::open(toolkit, data).ok_or(EngraveError::Load)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// verovio's `keyDown` code for the up arrow (`vrvdef.h`), as the core's
    /// `transpose` sends it — spelled out here because the raw-hatch test is
    /// what proves the two reach the same editor action.
    const KEY_UP: i32 = 38;
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
        open(&phrase(), &EngraveOptions::default()).expect("opens")
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
        let mut reopened = open(&mei, &EngraveOptions::default()).expect("MEI reloads");
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
