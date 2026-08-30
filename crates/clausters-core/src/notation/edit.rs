//! The edit verbs: what a hand does to one item.
//!
//! The same family as the algebra — a function from a sheet to a sheet, crossing
//! as data — and the same reason they are here rather than in a client: a
//! standalone host opened on a saved session has no client language in the
//! process, and the score it opens has to be editable there. So the arithmetic,
//! the validation and the refusals are all on this side, and a client
//! contributes the name it calls them by and nothing else.
//!
//! **An edit names its item by id, never by index.** An index moves the moment
//! anything before it is inserted or removed, so a caller holding one would be
//! addressing a different note after every edit but its own. The id is minted
//! once and travels with the item through every operation above.
//!
//! **Deleting and silencing are different acts** and are two verbs, because
//! confusing them is how time goes missing: [`delete`] takes the item out and
//! everything after it moves earlier by its value; [`silence`] leaves a rest of
//! the same length, so nothing moves at all. A caller who wanted the second and
//! got the first has a piece that is shorter than it was and no obvious sign of
//! where.
//!
//! **Nothing here reaches the engraver.** An edit rewrites the model and the
//! page is engraved again from it, which is why the editing surface owes nothing
//! to what a layout engine's own editor can express — inserting a measure and
//! splitting a voice are ordinary operations here and are not in that vocabulary
//! at all.

use super::model::{Header, Item, Marks, Pitch, Sheet, Spanner};
use crate::ratio::Ratio;

/// Where a new item goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    /// Before everything in the voice.
    Start,
    /// Straight after the item with this id, in that item's own voice.
    After(u64),
}

/// The item `id` is not in this sheet — the one refusal every verb here shares.
fn missing(id: u64) -> String {
    format!("no item with id {id} is in this score")
}

/// Write a new note, chord or rest into a voice.
///
/// Everything after it moves later by its value: inserting adds time, exactly
/// as writing a note into a bar of finished music does. An empty `pitches` is a
/// rest, which is the same distinction [`super::model::Item`] draws.
///
/// `staff` and `voice` say where a [`At::Start`] insertion goes; after an item,
/// the item's own voice is the answer and they are ignored.
///
/// # Errors
/// When the value is not positive, when the item to follow is not in the sheet,
/// or when the named staff or voice is not there.
pub fn insert(
    mut sheet: Sheet,
    at: At,
    pitches: Vec<Pitch>,
    dur: Ratio,
    staff: usize,
    voice: usize,
) -> Result<Sheet, String> {
    if !dur.is_positive() {
        return Err(format!("{dur} is not a length a written item can have"));
    }
    sheet.assign_ids();
    let id = sheet.mint();
    let item = if pitches.is_empty() {
        Item::Rest { id, dur }
    } else {
        Item::Note {
            id,
            pitches,
            dur,
            tie: false,
            marks: Marks::default(),
        }
    };
    let (si, vi, index) = match at {
        At::Start => (staff, voice, 0),
        At::After(after) => {
            let (si, vi, ii) = sheet.locate(after).ok_or_else(|| missing(after))?;
            (si, vi, ii + 1)
        }
    };
    let voice = sheet
        .staves
        .get_mut(si)
        .and_then(|s| s.voices.get_mut(vi))
        .ok_or_else(|| format!("this score has no voice {vi} on staff {si}"))?;
    voice.items.insert(index, item);
    Ok(sheet)
}

/// Take an item out. Everything after it in that voice moves earlier by its
/// value — see [`silence`] for the other thing this could mean.
pub fn delete(mut sheet: Sheet, id: u64) -> Result<Sheet, String> {
    sheet.assign_ids();
    let (si, vi, ii) = sheet.locate(id).ok_or_else(|| missing(id))?;
    sheet.staves[si].voices[vi].items.remove(ii);
    // A slur that ended on it goes with it: the alternative is a score that
    // cannot be engraved because of a note the caller meant to remove.
    super::algebra::prune_spanners(&mut sheet);
    Ok(sheet)
}

/// Turn an item into a rest of the same length. Nothing moves.
pub fn silence(mut sheet: Sheet, id: u64) -> Result<Sheet, String> {
    sheet.assign_ids();
    let (si, vi, ii) = sheet.locate(id).ok_or_else(|| missing(id))?;
    let item = &mut sheet.staves[si].voices[vi].items[ii];
    *item = item.silenced();
    Ok(sheet)
}

/// Give an item a different written value. Everything after it in that voice
/// moves by the difference, and the measures it now falls across are worked out
/// when the page is written.
pub fn set_dur(mut sheet: Sheet, id: u64, dur: Ratio) -> Result<Sheet, String> {
    if !dur.is_positive() {
        return Err(format!("{dur} is not a length a written item can have"));
    }
    sheet.assign_ids();
    let (si, vi, ii) = sheet.locate(id).ok_or_else(|| missing(id))?;
    let item = &mut sheet.staves[si].voices[vi].items[ii];
    *item = item.with_dur(dur);
    Ok(sheet)
}

/// Give an item different pitches — one for a note, several for a chord, none
/// to make it a rest (which is [`silence`], reached the other way).
///
/// The value and the id are kept, so this is the same item newly spelled rather
/// than a replacement: a caller holding the id still holds it.
pub fn set_pitches(mut sheet: Sheet, id: u64, pitches: Vec<Pitch>) -> Result<Sheet, String> {
    sheet.assign_ids();
    let (si, vi, ii) = sheet.locate(id).ok_or_else(|| missing(id))?;
    let item = &mut sheet.staves[si].voices[vi].items[ii];
    let dur = item.dur();
    *item = if pitches.is_empty() {
        Item::Rest { id, dur }
    } else {
        match item {
            Item::Note { tie, marks, .. } => Item::Note {
                id,
                pitches,
                dur,
                tie: *tie,
                marks: marks.clone(),
            },
            Item::Rest { .. } => Item::Note {
                id,
                pitches,
                dur,
                tie: false,
                marks: Marks::default(),
            },
        }
    };
    Ok(sheet)
}

/// Tie an item into the one after it, or untie it.
///
/// This is the tie a caller *writes* — the note goes on sounding through the
/// next item — and it is stored. The ties an emitter adds where a value crosses
/// a barline are made from the projection and never stored, so the two compose
/// instead of overwriting each other.
///
/// # Errors
/// When the item is a rest (nothing sounds through a silence) or is the last in
/// its voice (there is nothing to tie into).
pub fn tie(mut sheet: Sheet, id: u64, tied: bool) -> Result<Sheet, String> {
    sheet.assign_ids();
    let (si, vi, ii) = sheet.locate(id).ok_or_else(|| missing(id))?;
    let items = &mut sheet.staves[si].voices[vi].items;
    if ii + 1 == items.len() && tied {
        return Err(format!(
            "item {id} is the last in its voice, so there is nothing after it \
             to tie into"
        ));
    }
    match &mut items[ii] {
        Item::Note { tie: t, .. } => *t = tied,
        Item::Rest { .. } => {
            return Err(format!("item {id} is a rest, and a rest ties into nothing"));
        }
    }
    Ok(sheet)
}

/// Give an item the marks it carries: articulations, a dynamic, an ornament,
/// whether it is a grace note, a forced stem, and how long it sounds as against
/// how long it is written.
///
/// It replaces rather than merges, which is the honest shape for something a
/// caller holds whole: reading the marks, changing one and sending them back is
/// two calls and no ambiguity, where a merge would leave no way to *remove* a
/// mark at all.
///
/// # Errors
/// When the item is not in the sheet, or is a rest — a rest has no pitch to
/// articulate and nothing to say about how long it sounds.
pub fn set_marks(mut sheet: Sheet, id: u64, marks: Marks) -> Result<Sheet, String> {
    sheet.assign_ids();
    let (si, vi, ii) = sheet.locate(id).ok_or_else(|| missing(id))?;
    match &mut sheet.staves[si].voices[vi].items[ii] {
        Item::Note { marks: m, .. } => *m = marks,
        Item::Rest { .. } => {
            return Err(format!(
                "item {id} is a rest, and a rest carries no articulation, dynamic \
                 or sounding length"
            ));
        }
    }
    Ok(sheet)
}

/// Write something between two notes: a slur, a crescendo, a diminuendo.
///
/// It cannot go on an item because it has two ends, so it goes on the sheet
/// beside the staves. Adding the same one twice changes nothing, which is what
/// makes a caller resending its state harmless.
///
/// # Errors
/// When either end is not in the sheet. A spanner pointing at a note that is
/// not there would be a mark that never appears and no reason why.
pub fn add_spanner(mut sheet: Sheet, kind: &str, from: u64, to: u64) -> Result<Sheet, String> {
    sheet.assign_ids();
    for id in [from, to] {
        if sheet.locate(id).is_none() {
            return Err(missing(id));
        }
    }
    if from == to {
        return Err(
            "a slur or a hairpin runs between two notes, not from one to \
                    itself"
                .to_string(),
        );
    }
    let spanner = Spanner {
        kind: kind.to_string(),
        from,
        to,
    };
    if !sheet.spanners.contains(&spanner) {
        sheet.spanners.push(spanner);
    }
    Ok(sheet)
}

/// Take back what [`add_spanner`] wrote. Removing one that is not there changes
/// nothing rather than refusing, since the state a caller asked for holds.
pub fn remove_spanner(mut sheet: Sheet, kind: &str, from: u64, to: u64) -> Result<Sheet, String> {
    sheet
        .spanners
        .retain(|s| !(s.kind == kind && s.from == from && s.to == to));
    Ok(sheet)
}

/// Move items to another voice on the same staff, leaving rests where they were.
///
/// How two lines written as one come apart. The items keep their ids and their
/// place in time — a rest of each one's length holds the gap open in the voice
/// it left — so nothing before or after either line moves, and a caller's ids
/// still name the same notes.
///
/// The target voice is created when it is not there yet, and is padded with a
/// rest so the moved items land at the moment they were already sounding at.
///
/// # Errors
/// When any of the ids is missing, or when they are not all in one voice: a
/// move that started from two places would have two answers for what to leave
/// behind.
pub fn to_voice(mut sheet: Sheet, ids: &[u64], target: usize) -> Result<Sheet, String> {
    sheet.assign_ids();
    if ids.is_empty() {
        return Ok(sheet);
    }
    let mut located = Vec::new();
    for &id in ids {
        located.push((id, sheet.locate(id).ok_or_else(|| missing(id))?));
    }
    let (si, vi, _) = located[0].1;
    if located.iter().any(|(_, (s, v, _))| *s != si || *v != vi) {
        return Err(
            "these items are not all in one voice, so there is no single voice \
             for them to leave"
                .to_string(),
        );
    }
    if target == vi {
        return Err(format!("voice {target} is the voice these items are in"));
    }

    // Where each moved item sounds, so the target voice can be padded to it.
    let mut onset = Ratio::ZERO;
    let mut moving: Vec<(Ratio, Item)> = Vec::new();
    let source = &mut sheet.staves[si].voices[vi];
    for item in &mut source.items {
        let dur = item.dur();
        if ids.contains(&item.id()) {
            moving.push((onset, item.clone()));
            *item = item.silenced();
        }
        onset = onset + dur;
    }

    let staff = &mut sheet.staves[si];
    while staff.voices.len() <= target {
        staff.voices.push(super::model::Voice::default());
    }
    let mut next = sheet.next_id;
    let mut mint = move || {
        next += 1;
        next - 1
    };
    let into = &mut staff.voices[target];
    for (at, item) in moving {
        let have = into.items.iter().fold(Ratio::ZERO, |acc, i| acc + i.dur());
        if have < at {
            into.items.push(Item::Rest {
                id: mint(),
                dur: at - have,
            });
        }
        into.items.push(item);
    }
    sheet.next_id = mint();
    Ok(sheet)
}

/// Write what is above the music: the title, and who wrote it.
///
/// It **replaces** rather than merges, as [`set_marks`] does and for the same
/// reason: with a merge there would be no way to clear a field at all, since an
/// omitted one and an emptied one look identical on the wire.
///
/// # Errors
/// Never — a header is text and there is nothing to refuse.
pub fn set_header(mut sheet: Sheet, header: Header) -> Result<Sheet, String> {
    sheet.header = header;
    Ok(sheet)
}

/// Give a measure a right barline other than the ordinary single one.
///
/// `measure` is 1-based, as everywhere a caller names one. `single` removes the
/// override rather than storing one, so the state a caller asks for is the
/// state it gets and a sheet never carries a note saying "ordinary".
///
/// # Errors
/// When the measure number is 0, or the kind is not one MEI draws.
pub fn set_barline(mut sheet: Sheet, measure: usize, kind: &str) -> Result<Sheet, String> {
    let index = measure_index(measure)?;
    const KINDS: [&str; 7] = [
        "single", "end", "rptstart", "rptend", "rptboth", "dbl", "invis",
    ];
    if !KINDS.contains(&kind) {
        return Err(format!(
            "there is no barline called {kind}; it is one of {}",
            KINDS.join(", ")
        ));
    }
    sheet.grid.barlines.retain(|(m, _)| *m != index);
    if kind != "single" {
        sheet.grid.barlines.push((index, kind.to_string()));
        sheet.grid.barlines.sort();
    }
    Ok(sheet)
}

/// Break the system or the page before a measure.
///
/// This is layout, and it is an edit for the reason a forced stem is one: the
/// engraver breaks lines wherever they fit, and a break somebody *chose* is a
/// statement about the page. `none` takes it back.
///
/// # Errors
/// When the measure number is 0, or the kind is not `system`, `page` or `none`.
pub fn set_break(mut sheet: Sheet, measure: usize, kind: &str) -> Result<Sheet, String> {
    let index = measure_index(measure)?;
    if !["system", "page", "none"].contains(&kind) {
        return Err(format!(
            "there is no break called {kind}; it is system, page or none"
        ));
    }
    sheet.grid.breaks.retain(|(m, _)| *m != index);
    if kind != "none" {
        sheet.grid.breaks.push((index, kind.to_string()));
        sheet.grid.breaks.sort();
    }
    Ok(sheet)
}

/// A measure a caller named, as the grid indexes them.
fn measure_index(measure: usize) -> Result<usize, String> {
    measure
        .checked_sub(1)
        .ok_or_else(|| "measures are numbered from 1, so there is no measure 0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::model::{Staff, Step, Voice};

    fn c4() -> Pitch {
        Pitch {
            step: Step::C,
            alter: 0,
            octave: 4,
            forced: false,
        }
    }

    /// Three quarter notes, ids 1..3.
    fn three() -> Sheet {
        let mut sheet = Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice {
                    items: (0..3)
                        .map(|_| Item::Note {
                            id: 0,
                            pitches: vec![c4()],
                            dur: Ratio::new(1, 4),
                            tie: false,
                            marks: Marks::default(),
                        })
                        .collect(),
                }],
            }],
            ..Default::default()
        };
        sheet.assign_ids();
        sheet
    }

    fn items(sheet: &Sheet) -> &[Item] {
        &sheet.staves[0].voices[0].items
    }

    #[test]
    fn deleting_takes_time_out_and_silencing_does_not() {
        let sheet = three();
        let before = sheet.len();
        let id = items(&sheet)[1].id();

        let gone = delete(sheet.clone(), id).unwrap();
        assert_eq!(items(&gone).len(), 2);
        assert_eq!(gone.len(), before - Ratio::new(1, 4));

        let quiet = silence(sheet, id).unwrap();
        assert_eq!(items(&quiet).len(), 3);
        assert_eq!(quiet.len(), before);
        // and it is the same item, so a caller's id still names it
        assert_eq!(items(&quiet)[1].id(), id);
        assert!(items(&quiet)[1].pitches().is_empty());
    }

    #[test]
    fn inserting_writes_a_new_item_and_pushes_what_follows() {
        let sheet = three();
        let first = items(&sheet)[0].id();
        let out = insert(sheet, At::After(first), vec![c4()], Ratio::new(1, 8), 0, 0).unwrap();
        assert_eq!(items(&out).len(), 4);
        assert_eq!(items(&out)[1].dur(), Ratio::new(1, 8));
        // the new item has an id of its own, and it is nobody else's
        let ids: Vec<u64> = items(&out).iter().map(Item::id).collect();
        let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len());

        // and one written at the start goes first
        let out = insert(three(), At::Start, vec![], Ratio::new(1, 4), 0, 0).unwrap();
        assert!(items(&out)[0].pitches().is_empty());
    }

    #[test]
    fn a_value_and_a_spelling_change_keep_the_item() {
        let sheet = three();
        let id = items(&sheet)[0].id();
        let longer = set_dur(sheet.clone(), id, Ratio::new(1, 2)).unwrap();
        assert_eq!(items(&longer)[0].dur(), Ratio::new(1, 2));
        assert_eq!(items(&longer)[0].id(), id);

        let respelled = set_pitches(
            sheet,
            id,
            vec![Pitch {
                step: Step::B,
                alter: 1,
                octave: 3,
                forced: false,
            }],
        )
        .unwrap();
        // B#3 and C4 are one sound and two notes -- the point of storing both
        assert_eq!(items(&respelled)[0].pitches()[0].midi(), c4().midi());
        assert_eq!(items(&respelled)[0].pitches()[0].step, Step::B);
        assert_eq!(items(&respelled)[0].id(), id);
    }

    #[test]
    fn a_tie_is_written_and_refused_where_it_would_mean_nothing() {
        let sheet = three();
        let first = items(&sheet)[0].id();
        let last = items(&sheet)[2].id();
        let tied = tie(sheet.clone(), first, true).unwrap();
        assert!(matches!(items(&tied)[0], Item::Note { tie: true, .. }));
        // untying is the same verb
        assert!(matches!(
            items(&tie(tied, first, false).unwrap())[0],
            Item::Note { tie: false, .. }
        ));
        // nothing follows the last item
        let err = tie(sheet.clone(), last, true).unwrap_err();
        assert!(err.contains("nothing after it"), "{err}");
        // and a rest ties into nothing
        let quiet = silence(sheet, first).unwrap();
        let err = tie(quiet, first, true).unwrap_err();
        assert!(err.contains("rest"), "{err}");
    }

    #[test]
    fn moving_items_to_another_voice_leaves_the_time_they_took() {
        let sheet = three();
        let second = items(&sheet)[1].id();
        let out = to_voice(sheet, &[second], 1).unwrap();
        assert_eq!(out.staves[0].voices.len(), 2);
        // the item left a rest of its own length behind, so nothing slid
        assert_eq!(out.staves[0].voices[0].items.len(), 3);
        assert!(out.staves[0].voices[0].items[1].pitches().is_empty());
        // and it landed in the new voice at the moment it was sounding at
        let moved = &out.staves[0].voices[1].items;
        assert_eq!(moved[0].dur(), Ratio::new(1, 4)); // the pad
        assert!(moved[0].pitches().is_empty());
        assert_eq!(moved[1].id(), second);
    }

    #[test]
    fn every_verb_refuses_an_item_that_is_not_there() {
        let sheet = three();
        for err in [
            delete(sheet.clone(), 999).unwrap_err(),
            silence(sheet.clone(), 999).unwrap_err(),
            set_dur(sheet.clone(), 999, Ratio::new(1, 4)).unwrap_err(),
            set_pitches(sheet.clone(), 999, vec![c4()]).unwrap_err(),
            tie(sheet.clone(), 999, true).unwrap_err(),
            to_voice(sheet.clone(), &[999], 1).unwrap_err(),
        ] {
            assert!(err.contains("999"), "{err}");
        }
        // and a value that is not a length
        let err = set_dur(sheet, 1, Ratio::ZERO).unwrap_err();
        assert!(err.contains("not a length"), "{err}");
    }

    #[test]
    fn an_id_survives_a_sheet_that_never_had_one() {
        // A sheet written by hand carries no ids at all; the first operation
        // gives it them, so an edit can name any note in it.
        let json = r#"{"staves": [{"clef": "G2", "voices": [{"items": [
            {"kind": "note", "pitches": [{"step": "c", "octave": 4}], "dur": [1, 4]},
            {"kind": "rest", "dur": [1, 4]}]}]}]}"#;
        let mut sheet: Sheet = serde_json::from_str(json).unwrap();
        assert_eq!(items(&sheet)[0].id(), 0);
        sheet.assign_ids();
        assert_eq!(items(&sheet)[0].id(), 1);
        assert_eq!(items(&sheet)[1].id(), 2);
        assert!(delete(sheet, 2).is_ok());
    }
}
