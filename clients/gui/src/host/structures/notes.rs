//! **A note**, and the verbs a roll's hand has over a list of them.
//!
//! The structure and nothing about its picture: a note is a start, a length, a
//! pitch, a velocity and a channel, and what a hand does to a set of them is
//! move, resize, set the velocity, insert, remove, and the four that act on a
//! selection -- cut, join, quantize, and the clipboard block. Where a pitch is
//! drawn and which note a pixel is on stay with the roll that draws them
//! (`graphics::pianoroll`), which hands the numbers here.
//!
//! **A note is a box on a time axis**, so most of what a verb does to one is
//! [`boxes`]' and is not written again: the two `Placements`
//! impls are the whole of what a note has to say about that, and
//! [`Holder`](boxes::Holder) adds the two questions a verb asks of the list itself. The
//! identity of a second half -- the pitch, the velocity and the channel it
//! keeps -- is what stays here, because nothing general could state it.

use serde_json::Value;

use super::boxes::{self, Bounds, Contents, Part, Placement, Placements};

pub use super::boxes::{Limit, selection_after_removal, toggle_selected};

/// One note: its `start`/`dur` in timeline sample units (relative to the owning
/// region's offset), `pitch` as a MIDI note number (kept `f32` so a clip can map
/// it over an arbitrary `[min, max]` range), and the MIDI `velocity` (`0..127`)
/// and `channel` (`0..15`) that make it a real MIDI note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub start: f64,
    pub dur: f64,
    pub pitch: f32,
    pub velocity: i32,
    pub channel: i32,
}

/// The `notes` wire form of a note list: the flat `start dur pitch velocity
/// channel` quintuple array, as JSON.
///
/// The inverse of the `notes` prop's parse, so what a `/gui_query` reports is
/// what a `/gui_set` would take — which is the whole contract of reporting a
/// non-scalar as its own string carrier.
pub fn notes_json(notes: &[Note]) -> Value {
    let mut out = Vec::with_capacity(notes.len() * 5);
    for n in notes {
        out.push(Value::from(n.start));
        out.push(Value::from(n.dur));
        out.push(Value::from(n.pitch));
        out.push(Value::from(n.velocity));
        out.push(Value::from(n.channel));
    }
    Value::Array(out)
}

/// The `osc` wire form of a marker list: the flat `time label` pair array, as
/// JSON — the inverse of the `osc` prop's parse.
pub fn osc_json(marks: &[OscMark]) -> Value {
    let mut out = Vec::with_capacity(marks.len() * 2);
    for m in marks {
        out.push(Value::from(m.time));
        out.push(Value::from(m.label.clone().unwrap_or_default()));
    }
    Value::Array(out)
}

impl Note {
    /// A note with the default velocity (100) on channel 0 — the plain
    /// `(start, dur, pitch)` triple's reading.
    pub fn new(start: f64, dur: f64, pitch: f32) -> Self {
        Note {
            start,
            dur,
            pitch,
            velocity: 100,
            channel: 0,
        }
    }
}

/// One marker on the OSC lane: its `time` (timeline samples,
/// relative to the region offset) and an optional short `label` (an address or
/// tag) drawn beside the flag.
#[derive(Clone, Debug, PartialEq)]
pub struct OscMark {
    pub time: f64,
    pub label: Option<String>,
}

// --- Editing (pure, mapping-free) -----------------------------------------

/// **Indexed access to the note list**, so every block edit a note shares with
/// a clip is written once (`crate::host::structures::boxes`) and both call it.
///
/// A note's row is its pitch and its `Placement::start` is always zero: there
/// is no source behind a note to window, so an edge drag's trim has nowhere to
/// travel and the accessor drops it.
impl Placements for [Note] {
    fn len(&self) -> usize {
        <[Note]>::len(self)
    }

    fn placement(&self, i: usize) -> Placement {
        let n = self[i];
        Placement {
            offset: n.start,
            dur: n.dur,
            start: 0.0,
        }
    }

    fn set_placement(&mut self, i: usize, p: Placement) {
        self[i].start = p.offset;
        self[i].dur = p.dur;
    }

    fn row(&self, i: usize) -> f32 {
        self[i].pitch
    }

    fn set_row(&mut self, i: usize, r: f32) {
        self[i].pitch = r;
    }
}

/// **A growable list of notes is a list of boxes a verb can act on.**
///
/// The slice above answers where each note is, which is what a drag needs; a
/// verb needs the list itself, because cutting one note makes a second and
/// deleting one takes it away. Two methods, and both of them are what a note
/// *is* rather than what a box is: a second half keeps the pitch, the velocity
/// and the channel of the note it was cut out of, which is the identity nothing
/// general could have written.
impl Placements for Vec<Note> {
    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn placement(&self, i: usize) -> Placement {
        <[Note] as Placements>::placement(self, i)
    }

    fn set_placement(&mut self, i: usize, p: Placement) {
        <[Note] as Placements>::set_placement(self, i, p)
    }

    fn row(&self, i: usize) -> f32 {
        <[Note] as Placements>::row(self, i)
    }

    fn set_row(&mut self, i: usize, r: f32) {
        <[Note] as Placements>::set_row(self, i, r)
    }
}

impl boxes::Holder for Vec<Note> {
    fn duplicate(&mut self, i: usize) -> Option<usize> {
        let note = *self.get(i)?;
        Some(insert_note(self, note))
    }

    fn discard(&mut self, indices: &[usize]) {
        remove_notes(self, indices);
    }
}

/// Move the note at `index` to a new start (clamped into the bounds' domain,
/// tail included) and pitch (clamped into `[lo, hi]`, rounded to the nearest
/// semitone). The duration is kept.
pub fn move_note(
    notes: &mut [Note],
    index: usize,
    start: f64,
    pitch: f32,
    lo: f32,
    hi: f32,
    bounds: Bounds,
) {
    if index >= notes.len() {
        return;
    }
    let orig = notes.placement(index);
    let placed = boxes::drag(Part::Body, start, orig, Contents::default(), bounds);
    notes.set_placement(index, placed);
    notes.set_row(index, pitch.round().clamp(lo, hi));
}

/// Resize the note at `index` by dragging one edge to timeline-relative `t` —
/// the clip's edge drag, over a note.
///
/// `Start` moves the onset (keeping the end fixed), `End` moves the end; a note
/// never shrinks below the bounds' floor, and never grows past their domain.
pub fn resize_note(notes: &mut [Note], index: usize, part: Part, t: f64, bounds: Bounds) {
    if index >= notes.len() || part == Part::Body {
        return;
    }
    let orig = notes.placement(index);
    let placed = boxes::drag(part, t, orig, Contents::default(), bounds);
    notes.set_placement(index, placed);
}

/// Set the velocity (clamped `0..127`) of the note at `index`.
pub fn set_velocity(notes: &mut [Note], index: usize, velocity: i32) {
    if let Some(n) = notes.get_mut(index) {
        n.velocity = velocity.clamp(0, 127);
    }
}

/// Insert a note, returning its index (appended; the list is not kept sorted —
/// draw order is insertion order, matching the clip's).
pub fn insert_note(notes: &mut Vec<Note>, note: Note) -> usize {
    notes.push(note);
    notes.len() - 1
}

/// Remove the note at `index` (a no-op out of range).
pub fn remove_note(notes: &mut Vec<Note>, index: usize) {
    if index < notes.len() {
        notes.remove(index);
    }
}

// --- Multi-note selection and block edits (pure, mapping-free) -------------
//
// The selection is a set of note indices — view state, native-side. The
// marquee is the shared time selection restricted in pitch: dragging the empty
// grid keeps setting the linked views' time selection, and the notes inside
// the time × pitch rectangle become the selected set.

/// The indices of the notes intersecting the time span `[t0, t1)` whose row
/// touches the pitch band `[p_lo, p_hi]` — [`boxes::in_rect`] over the note
/// list, the same marquee a lane's clips answer.
pub fn notes_in_rect(notes: &[Note], t0: f64, t1: f64, p_lo: f32, p_hi: f32) -> Vec<usize> {
    boxes::in_rect(notes, t0, t1, p_lo, p_hi)
}

/// Move a block of notes rigidly from a press-time snapshot — the shared
/// [`boxes::move_block`], with the pitch window as the row bounds.
pub fn move_notes_from(
    notes: &mut [Note],
    orig: &[(usize, f64, f32)],
    dt: f64,
    dp: f32,
    lo: f32,
    hi: f32,
    limit: Limit,
) {
    boxes::move_block(notes, orig, dt, dp, (lo, hi), limit);
}

/// Remove a set of notes by index (any order, duplicates tolerated).
pub fn remove_notes(notes: &mut Vec<Note>, indices: &[usize]) {
    let mut sorted: Vec<usize> = indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    for i in sorted.into_iter().rev() {
        if i < notes.len() {
            notes.remove(i);
        }
    }
}

/// Nudge a block of velocities relatively from a press-time snapshot: `orig`
/// is `(index, velocity)` per selected note, `dv` the common delta — each note
/// clamps to `0..127` on its own (a saturated bar stays put, the rest keep
/// moving, and reversing restores the original spread).
pub fn nudge_velocities_from(notes: &mut [Note], orig: &[(usize, i32)], dv: i32) {
    for (i, v) in orig {
        if let Some(n) = notes.get_mut(*i) {
            n.velocity = (v + dv).clamp(0, 127);
        }
    }
}

/// Copy a selection of notes **as they stand** — the clipboard form
/// [`paste_notes`] re-places.
///
/// Absolute, not normalized to the block's first onset, because *where a block
/// lands* is the paste's question and not the copy's: a clip's block travels
/// the same way, and one carrier with two conventions is how the same gesture
/// comes to mean two things in two views. What the payload is, either way, is
/// exactly what a `/gui_set` of the prop would accept.
pub fn copy_notes(notes: &[Note], indices: &[usize]) -> Vec<Note> {
    indices
        .iter()
        .filter_map(|&i| notes.get(i).copied())
        .collect()
}

/// Paste a clipboard block with its first onset at `at`: the notes append
/// (original pitches and spread kept), and the new indices come back — the
/// pasted block becomes the selection, ready to drag into place.
///
/// Where each note lands is [`boxes::rebased`], which is the rule a pasted
/// block obeys wherever one is put down: the earliest goes to `at` and the rest
/// keep their distances from it, so a pasted block is the same block.
pub fn paste_notes(notes: &mut Vec<Note>, clip: &[Note], at: f64) -> Vec<usize> {
    let starts: Vec<f64> = clip.iter().map(|n| n.start).collect();
    let Some(placed) = boxes::rebased(&starts, at.max(0.0)) else {
        return Vec::new();
    };
    clip.iter()
        .zip(placed)
        .map(|(n, start)| {
            let mut n = *n;
            n.start = start;
            insert_note(notes, n)
        })
        .collect()
}

/// **Split notes at `at`** (a region-relative time): every named note the time
/// falls strictly inside becomes two, the second carrying the same pitch,
/// velocity and channel. `indices` picks the notes (the selection); empty
/// splits them all. Returns the selection the cut leaves — both halves of every
/// note that was cut — so the block stays in hand.
///
/// The clip's `e` verb, over notes. A clip asks its owner to cut, because the
/// owner holds the element; a roll holds its own notes and cuts them.
pub fn split_notes(notes: &mut Vec<Note>, indices: &[usize], at: f64) -> Vec<usize> {
    let all: Vec<usize>;
    let targets = if indices.is_empty() {
        all = (0..notes.len()).collect();
        &all[..]
    } else {
        indices
    };
    boxes::split(notes, targets, at)
}

/// **Join the named notes**: on each pitch, a run of notes that touch or
/// overlap becomes one, spanning from the first onset to the last end and
/// keeping the first note's velocity and channel. `indices` picks the notes;
/// empty joins over the whole list. Returns the selection that is left.
///
/// The clip's `j` verb, over notes — and the same reading of "juxtaposed": what
/// joins is what touches, so no second selection model is needed to say which
/// two. A pitch is what makes two notes the same voice, which is the roll's
/// answer to the lane a clip's join is confined to.
pub fn join_notes(notes: &mut Vec<Note>, indices: &[usize]) -> Vec<usize> {
    let mut targets: Vec<usize> = if indices.is_empty() {
        (0..notes.len()).collect()
    } else {
        let mut t = indices.to_vec();
        t.sort_unstable();
        t.dedup();
        t
    };
    // Earliest first within a pitch, so a run is walked in the order it sounds.
    targets.sort_by(|&a, &b| {
        let (na, nb) = (notes[a], notes[b]);
        na.pitch
            .total_cmp(&nb.pitch)
            .then(na.start.total_cmp(&nb.start))
    });
    let mut absorbed: Vec<usize> = Vec::new();
    let mut head: Option<usize> = None;
    for i in targets {
        match head {
            Some(h)
                if notes[h].pitch == notes[i].pitch
                    && boxes::adjacent(notes.placement(h), notes.placement(i), JOIN_TOL) =>
            {
                let joined = boxes::merge(notes.placement(h), notes.placement(i));
                notes.set_placement(h, joined);
                absorbed.push(i);
            }
            _ => head = Some(i),
        }
    }
    if absorbed.is_empty() {
        return indices.to_vec();
    }
    // The survivors, re-indexed after the absorbed ones leave the list.
    let left: Vec<usize> = (0..notes.len()).filter(|i| !absorbed.contains(i)).collect();
    let kept: Vec<usize> = indices
        .iter()
        .filter(|i| !absorbed.contains(i))
        .map(|&i| left.iter().position(|&j| j == i).unwrap_or(i))
        .collect();
    remove_notes(notes, &absorbed);
    kept
}

/// How near two notes' edges must be to count as touching — half a sample, the
/// same tolerance every other "did this actually move" question uses.
const JOIN_TOL: f64 = 0.5;

/// Quantize note onsets to the `grid` (timeline samples) — the shared
/// [`boxes::quantize`], which a lane's clips run the same way.
pub fn quantize_notes(notes: &mut [Note], indices: &[usize], grid: f64) -> bool {
    boxes::quantize(notes, indices, grid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::structures::boxes::Limit;

    /// The bounds of an edit inside a domain that ends at `l` — a clip's body.
    fn limited(l: f64) -> Bounds {
        Bounds {
            limit: Some(l),
            ..Bounds::default()
        }
    }

    /// The bounds of an edit with its own floor.
    fn floor(min_dur: f64, limit: Limit) -> Bounds {
        Bounds {
            grid: 0.0,
            min_dur,
            limit,
        }
    }

    #[test]
    fn move_clamps_pitch_and_start() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, -50.0, 200.7, 24.0, 96.0, Bounds::default());
        assert_eq!(notes[0].start, 0.0);
        assert_eq!(notes[0].pitch, 96.0); // clamped to hi, rounded
        assert_eq!(notes[0].dur, 200.0); // duration kept
    }

    #[test]
    fn a_move_inside_a_limit_keeps_the_whole_note_in() {
        // Unbounded (a roll's own view): the note goes where it is dropped, and
        // the roll's span grows with it.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, 5000.0, 60.0, 24.0, 96.0, Bounds::default());
        assert_eq!(notes[0].start, 5000.0);
        // Bounded (a clip's body): the **tail** stops at the edge, so the last
        // start is limit - dur and the note stays whole and visible.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, 5000.0, 60.0, 24.0, 96.0, limited(1000.0));
        assert_eq!((notes[0].start, notes[0].dur), (800.0, 200.0));
        // A note longer than the clip pins to the near edge: its tail cannot
        // fit, so the edge that can be honoured is zero.
        let mut notes = vec![Note::new(0.0, 400.0, 60.0)];
        move_note(&mut notes, 0, 300.0, 60.0, 24.0, 96.0, limited(200.0));
        assert_eq!(notes[0].start, 0.0);
    }

    #[test]
    fn resize_respects_min_dur_from_either_edge() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        // Drag the end back past the start → clamped to min_dur.
        resize_note(&mut notes, 0, Part::End, 50.0, floor(10.0, None));
        assert_eq!(notes[0].dur, 10.0);
        // Drag the start forward past the end → clamped.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)]; // end = 300
        resize_note(&mut notes, 0, Part::Start, 400.0, floor(10.0, None));
        assert_eq!(notes[0].start, 290.0);
        assert_eq!(notes[0].dur, 10.0);
    }

    #[test]
    fn a_resize_stops_the_tail_at_the_limit() {
        // The end edge dragged past the clip's own length stops there.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        resize_note(&mut notes, 0, Part::End, 5000.0, floor(10.0, Some(1000.0)));
        assert_eq!(notes[0].dur, 900.0);
        // The start edge holds the end still, so a note already inside stays
        // inside with no far edge of its own.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        resize_note(&mut notes, 0, Part::Start, 50.0, floor(10.0, Some(1000.0)));
        assert_eq!((notes[0].start, notes[0].dur), (50.0, 250.0));
    }

    /// A cut leaves two notes end to end, and joining them back leaves what
    /// was there — the roll's `e` and `j`, which are the clip's own two verbs
    /// over a list the host holds itself.
    #[test]
    fn a_split_and_a_join_are_inverses() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        let sel = split_notes(&mut notes, &[], 150.0);
        assert_eq!(sel, vec![0, 1]);
        assert_eq!((notes[0].start, notes[0].dur), (100.0, 50.0));
        assert_eq!((notes[1].start, notes[1].dur), (150.0, 150.0));
        assert_eq!(notes[1].pitch, 60.0, "the second half is the same note");
        let sel = join_notes(&mut notes, &sel);
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].start, notes[0].dur), (100.0, 200.0));
        assert_eq!(sel, vec![0]);
    }

    /// A cut on an edge is not a cut, and neither is one outside the note.
    #[test]
    fn a_cut_that_would_leave_nothing_is_no_cut() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        assert!(split_notes(&mut notes, &[], 100.0).is_empty());
        assert!(split_notes(&mut notes, &[], 300.0).is_empty());
        assert!(split_notes(&mut notes, &[], 50.0).is_empty());
        assert_eq!(notes.len(), 1);
    }

    /// **A pitch is what makes two notes one voice**, which is the roll's
    /// answer to the lane a clip's join is confined to: notes that touch join,
    /// notes on another pitch do not, and neither do notes with a gap.
    #[test]
    fn a_join_takes_what_touches_on_the_same_pitch() {
        let mut notes = vec![
            Note::new(0.0, 100.0, 60.0),
            Note::new(100.0, 100.0, 60.0), // touches the first
            Note::new(100.0, 100.0, 64.0), // another voice
            Note::new(400.0, 100.0, 60.0), // a gap
        ];
        let sel = join_notes(&mut notes, &[]);
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].start, notes[0].dur), (0.0, 200.0));
        assert!(
            sel.is_empty(),
            "nothing was selected, nothing is left selected"
        );
        assert!(notes.iter().any(|n| n.pitch == 64.0 && n.dur == 100.0));
        assert!(notes.iter().any(|n| n.start == 400.0));
    }

    #[test]
    fn insert_and_remove() {
        let mut notes = vec![Note::new(0.0, 100.0, 60.0)];
        let i = insert_note(&mut notes, Note::new(200.0, 50.0, 64.0));
        assert_eq!(i, 1);
        assert_eq!(notes.len(), 2);
        remove_note(&mut notes, 0);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 64.0);
        remove_note(&mut notes, 9); // out of range, no-op
        assert_eq!(notes.len(), 1);
    }

    // --- selection + block edits ---

    fn three_notes() -> Vec<Note> {
        vec![
            Note::new(0.0, 100.0, 60.0),
            Note::new(200.0, 100.0, 64.0),
            Note::new(400.0, 100.0, 72.0),
        ]
    }

    #[test]
    fn a_marquee_selects_by_time_and_pitch_and_tolerates_reversed_ranges() {
        let notes = three_notes();
        // The middle note only: its time span, its pitch band.
        assert_eq!(notes_in_rect(&notes, 150.0, 350.0, 62.0, 66.0), vec![1]);
        // A reversed drag selects the same.
        assert_eq!(notes_in_rect(&notes, 350.0, 150.0, 66.0, 62.0), vec![1]);
        // The full time span but a pitch band excluding the top note.
        assert_eq!(notes_in_rect(&notes, 0.0, 500.0, 58.0, 65.0), vec![0, 1]);
        // A note intersecting the span's edge is in (its tail crosses t0).
        assert_eq!(notes_in_rect(&notes, 50.0, 60.0, 59.0, 61.0), vec![0]);
        // An empty rect selects nothing.
        assert!(notes_in_rect(&notes, 120.0, 130.0, 60.0, 60.0).is_empty());
    }

    #[test]
    fn a_block_move_is_rigid_and_clamps_as_one() {
        // Free move: both notes shift by the same delta.
        let mut notes = three_notes();
        let orig = vec![(0, 0.0, 60.0f32), (1, 200.0, 64.0f32)];
        move_notes_from(&mut notes, &orig, 50.0, 2.4, 24.0, 96.0, None);
        assert_eq!((notes[0].start, notes[0].pitch), (50.0, 62.0));
        assert_eq!((notes[1].start, notes[1].pitch), (250.0, 66.0));
        // Clamped at time zero: the whole block stops, keeping the spread.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, -80.0, 0.0, 24.0, 96.0, None);
        assert_eq!((notes[0].start, notes[1].start), (0.0, 200.0));
        // Clamped at the pitch top: the highest note pins the block.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, 0.0, 40.0, 24.0, 96.0, None);
        assert_eq!((notes[0].pitch, notes[1].pitch), (92.0, 96.0));
        // Durations are never touched.
        assert_eq!(notes[0].dur, 100.0);
    }

    #[test]
    fn a_block_wider_than_the_pitch_window_does_not_fold() {
        let mut notes = vec![Note::new(0.0, 10.0, 20.0), Note::new(0.0, 10.0, 100.0)];
        let orig = vec![(0, 0.0, 20.0f32), (1, 0.0, 100.0f32)];
        move_notes_from(&mut notes, &orig, 0.0, 5.0, 24.0, 96.0, None);
        // The rigid pitch move is refused; the pitches only clamp into range.
        assert_eq!((notes[0].pitch, notes[1].pitch), (24.0, 96.0));
    }

    #[test]
    fn a_block_move_stops_its_last_tail_at_the_limit() {
        // The block's far end is its last note's **tail** (400 + 100 = 500), so
        // inside a 600-long clip it may only move 100 further, spread intact.
        let mut notes = three_notes();
        let orig = vec![(0, 0.0, 60.0f32), (2, 400.0, 72.0f32)];
        move_notes_from(&mut notes, &orig, 5000.0, 0.0, 24.0, 96.0, Some(600.0));
        assert_eq!((notes[0].start, notes[2].start), (100.0, 500.0));
        assert_eq!(notes[2].dur, 100.0); // durations are never touched
        // A block wider than the clip pins to zero: the near edge is applied
        // last, the same choice a single over-long note makes.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, 50.0, 0.0, 24.0, 96.0, Some(200.0));
        assert_eq!((notes[0].start, notes[2].start), (0.0, 400.0));
    }

    #[test]
    fn a_block_removal_takes_any_order_and_duplicates() {
        let mut notes = three_notes();
        remove_notes(&mut notes, &[2, 0, 0]);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 64.0);
    }

    #[test]
    fn a_velocity_nudge_is_relative_and_saturates_per_note() {
        let mut notes = three_notes();
        notes[0].velocity = 120;
        notes[1].velocity = 60;
        let orig = vec![(0, 120), (1, 60)];
        nudge_velocities_from(&mut notes, &orig, 20);
        assert_eq!((notes[0].velocity, notes[1].velocity), (127, 80));
        // Reversing from the same snapshot restores the original spread.
        nudge_velocities_from(&mut notes, &orig, -20);
        assert_eq!((notes[0].velocity, notes[1].velocity), (100, 40));
    }

    #[test]
    fn a_block_travels_as_it_stands_and_the_paste_places_it() {
        let notes = three_notes();
        // Copy the last two: the block travels with the onsets it had.
        let clip = copy_notes(&notes, &[1, 2]);
        assert_eq!(clip.len(), 2);
        assert_eq!((clip[0].start, clip[0].pitch), (notes[1].start, 64.0));
        assert_eq!(clip[1].start - clip[0].start, 200.0, "and its own spread");
        // Paste at 1000: appended with the spread kept, new indices returned.
        let mut notes = three_notes();
        let sel = paste_notes(&mut notes, &clip, 1000.0);
        assert_eq!(sel, vec![3, 4]);
        assert_eq!((notes[3].start, notes[4].start), (1000.0, 1200.0));
        assert_eq!(notes[4].pitch, 72.0);
        // A negative paste point clamps to the timeline start.
        let sel = paste_notes(&mut notes, &clip, -50.0);
        assert_eq!(notes[sel[0]].start, 0.0);
        // Copying nothing yields an empty clipboard.
        assert!(copy_notes(&notes, &[]).is_empty());
    }

    #[test]
    fn quantize_snaps_the_selection_or_everything_and_reports_movement() {
        // The selection only: the third note keeps its offbeat start.
        let mut notes = vec![
            Note::new(90.0, 50.0, 60.0),
            Note::new(260.0, 50.0, 64.0),
            Note::new(430.0, 50.0, 67.0),
        ];
        assert!(quantize_notes(&mut notes, &[0, 1], 100.0));
        assert_eq!(
            (notes[0].start, notes[1].start, notes[2].start),
            (100.0, 300.0, 430.0)
        );
        // No selection: everything snaps; durations never move.
        assert!(quantize_notes(&mut notes, &[], 100.0));
        assert_eq!(notes[2].start, 400.0);
        assert_eq!(notes[2].dur, 50.0);
        // Already on the grid (or no grid): nothing to report.
        assert!(!quantize_notes(&mut notes, &[], 100.0));
        assert!(!quantize_notes(&mut notes, &[], 0.0));
    }

    #[test]
    fn the_selection_follows_a_single_removal_and_toggles() {
        assert_eq!(selection_after_removal(&[0, 1, 2], 1), vec![0, 1]);
        assert_eq!(selection_after_removal(&[2], 2), Vec::<usize>::new());
        let mut sel = vec![0];
        toggle_selected(&mut sel, 2);
        assert_eq!(sel, vec![0, 2]);
        toggle_selected(&mut sel, 0);
        assert_eq!(sel, vec![2]);
    }
}
