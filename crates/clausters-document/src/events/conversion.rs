//! **A performance turned into notation**: the sequence rewritten as the page
//! it reads as.
//!
//! The roll's notation mode edits a sequence written as notation: its notes
//! stand where they are written, each says its staff, its voice and its
//! written value, and what it sounds is derived from them
//! ([`realize`](super::realize::realize)). A performance -- a take, a MIDI
//! file, a voice rendered from a pattern -- is written as no notation, and
//! [`to_notation`] is the step between the two: the sequence is read into a
//! page with a reading and a writing given ([`read`]), and the page rendered
//! back into it ([`render`]), so its events are the page's notes and its
//! `notation` section the page's own. What a page does not hold is kept: what
//! plays each note (its `instrument`, every key that is neither time, pitch,
//! level nor notation, and its own curves on a target the page writes none
//! on), the tempo map, the parts, the curves the page does not write.
//!
//! **And what goes on without an attack.** A note held on into its own
//! pitch is one note on the page, whose ties the page writes where its metre
//! cuts it; a join between different notes is a slur that joins
//! (`super::articulation::held_on`, `carry_joins`).
//!
//! **And the ids it had.** A note of the page that is a note of the
//! performance keeps that note's id, a curve keeps the id of the curve it is,
//! and what the page adds is numbered from the sequence's counter, so an id a
//! client holds names the same thing after the conversion as before it.
//!
//! **It is a conversion, not a reading**: the times as played become the
//! written ones, so it is one edit, undone as one, and a caller that wants the
//! performance kept converts a copy.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;

use clausters_notation::Interpretation;

use super::EventSequence;
use super::articulation::{carry_joins, held_on};
use super::part::Writing;
use super::reading::Transcription;
use super::score::render;
use super::transcription::read;
use super::writeback::{by_item, lane_of, midi_of};

/// The keys a page writes or derives: what a note's time, pitch, level and
/// notation are. Every other key of a performance's note is what plays it,
/// and the converted note keeps it.
const PAGE_KEYS: &[&str] = &[
    "midinote",
    "freq",
    "degree",
    "note",
    "octave",
    "root",
    "scale",
    "stretch",
    "ctranspose",
    "mtranspose",
    "gtranspose",
    "harmonic",
    "detune",
    "dur",
    "sustain",
    "legato",
    "amp",
    "velocity",
    "db",
    "channel",
    "staff",
    "voice",
    "value",
    "pitches",
    "articulations",
    "dynamic",
    "ornament",
    "grace",
    "stem",
    "tie",
    "joins",
    "spelling",
    "accidental",
];

/// **`sequence` written as the page it reads as** (see the module), read as
/// `reading` says, with every part written as `writing` where one is given,
/// and played as `interp` says.
///
/// # Errors
/// When the sequence cannot be read into a page, or the page rendered.
pub fn to_notation(
    sequence: &EventSequence,
    reading: &Transcription,
    writing: Option<Writing>,
    interp: &Interpretation,
) -> Result<EventSequence, String> {
    let mut source = sequence.clone();
    // a note held on into its own pitch is one note on the page, its ties
    // the page's own
    held_on(&mut source);
    if let Some(writing) = writing {
        let mut parts = source.parts_read();
        for part in &mut parts {
            part.writing = writing;
        }
        source.parts = parts;
    }
    let page = read(&source, reading, interp)?;
    let mut out = render(&page.sheet, interp)?;

    // what plays each note, from the performance's notes of its item, matched
    // by what they sound
    let mut held: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (event, item) in &page.items {
        held.entry(*item).or_default().push(*event);
    }
    let sounded: BTreeMap<u64, Vec<u64>> = by_item(&out)
        .into_iter()
        .map(|(item, events)| (item, events.iter().map(|e| e.id).collect()))
        .collect();
    // the page's event of each of the performance's notes, by id
    let mut was: HashMap<u64, u64> = HashMap::new();
    for (item, theirs) in &sounded {
        let mut mine: Vec<u64> = held.get(item).cloned().unwrap_or_default();
        for id in theirs {
            let Some(at) = out.events.iter().position(|e| e.id == *id) else {
                continue;
            };
            let sounds = midi_of(&out.events[at]);
            let pick = mine
                .iter()
                .position(|m| source.get(*m).is_some_and(|e| midi_of(e) == sounds))
                .or((!mine.is_empty()).then_some(0));
            let Some(from) = pick.map(|i| mine.remove(i)).and_then(|m| source.get(m)) else {
                continue;
            };
            was.insert(*id, from.id);
            let mut keys = out.events[at].keys();
            for (key, value) in from.keys() {
                if !PAGE_KEYS.contains(&key.as_str()) {
                    keys.entry(key).or_insert(value);
                }
            }
            out.events[at].data = crate::Opaque(Value::Object(keys));
            // and the note's own curves, where the page writes none on their
            // target: a bend a performance played is still the note's
            let page = &out.events[at].automation;
            let kept: Vec<_> = from
                .automation
                .iter()
                .filter(|curve| page.iter().all(|written| written.target != curve.target))
                .cloned()
                .collect();
            out.events[at].automation.extend(kept);
        }
    }

    // a staff of a part played on channels is played on them
    let parts = source.parts.clone();
    if !parts.is_empty() {
        let mut first = 0;
        let mut of_staff: BTreeMap<usize, i64> = BTreeMap::new();
        for part in &parts {
            if !part.channels.is_empty() {
                for rank in 0..part.staves.max(1) {
                    of_staff.insert(
                        first + rank,
                        part.channels[rank.min(part.channels.len() - 1)],
                    );
                }
            }
            first += part.staves.max(1);
        }
        for event in &mut out.events {
            let mut keys = event.keys();
            let staff = keys
                .get("staff")
                .and_then(Value::as_u64)
                .map(|s| s as usize);
            if let Some(channel) = staff.and_then(|s| of_staff.get(&s)) {
                keys.insert("channel".into(), Value::from(*channel));
                event.data = crate::Opaque(Value::Object(keys));
            }
        }
    }

    // what the page does not hold: the tempo map, the parts, the curves the
    // page does not write, the specification
    // the times are the sequence's beats, so its tempo map is what plays
    // them: a performance with none is played at a beat a second, as before
    out.tempo_map.clone_from(&sequence.tempo_map);
    out.parts = parts;
    number_as_the_sequence(&mut out, sequence, &was);
    let written: BTreeSet<String> = out.automation.iter().map(lane_of).collect();
    for curve in &sequence.automation {
        if !written.contains(&lane_of(curve)) {
            out.automation.push(curve.clone());
        }
    }
    // a join between different notes is a slur that joins
    carry_joins(&mut out, &source);
    out.midi = sequence.midi.or(out.midi);
    // the reading it was read with is the one it is read with from now on
    out.set_reading(Some(
        serde_json::to_value(reading).map_err(|e| e.to_string())?,
    ))?;
    Ok(out)
}

/// **The page numbered as the sequence it was read from** (see `ids`): the
/// render numbers its events and curves from one, so ids the sequence gave out
/// would come back naming other things. A note of the page that is a note of
/// the performance (`was`, the page's event to the sequence's) keeps the
/// performance's id, and a curve keeps the id of the same curve it was (the
/// note's curve on the same target, the sequence's lane on the same channel);
/// everything else is minted from the sequence's counter, which climbs past
/// both. What names an event is renamed with it.
fn number_as_the_sequence(
    out: &mut EventSequence,
    sequence: &EventSequence,
    was: &HashMap<u64, u64>,
) {
    out.next_id = out.next_id.max(sequence.next_id);
    let mut renamed: HashMap<u64, u64> = HashMap::new();
    for event in &out.events {
        let id = match was.get(&event.id) {
            Some(theirs) => *theirs,
            None => {
                out.next_id += 1;
                out.next_id
            }
        };
        renamed.insert(event.id, id);
    }
    out.rename_events(&renamed);
    let lanes: HashMap<String, crate::NodeId> = sequence
        .automation
        .iter()
        .map(|curve| (lane_of(curve), curve.id))
        .collect();
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    let mint = |next_id: &mut u64| {
        *next_id += 1;
        crate::NodeId(*next_id)
    };
    for i in 0..out.automation.len() {
        let id = match lanes.get(&lane_of(&out.automation[i])) {
            Some(id) if taken.insert(id.0) => *id,
            _ => mint(&mut out.next_id),
        };
        out.automation[i].id = id;
    }
    for i in 0..out.events.len() {
        let theirs = sequence.get(out.events[i].id);
        for k in 0..out.events[i].automation.len() {
            let target = out.events[i].automation[k].target.clone();
            let same = theirs.and_then(|e| e.automation.iter().find(|c| c.target == target));
            let id = match same {
                Some(curve) if taken.insert(curve.id.0) => curve.id,
                _ => mint(&mut out.next_id),
            };
            out.events[i].automation[k].id = id;
        }
    }
    // what is left to check is only that the page says each id once
    out.hold();
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::events::EventsIntent;

    /// A take played a little off the beat, with what plays it.
    fn take() -> EventSequence {
        serde_json::from_value(json!({"events": [
            {"at": 0.02, "data": {"midinote": 60, "dur": 0.97, "instrument": "bell", "pan": -0.5}},
            {"at": 1.01, "data": {"midinote": 64, "dur": 0.49, "instrument": "bell"}},
            {"at": 2.0, "data": {"midinote": 67, "dur": 2.03, "instrument": "bell"}},
        ], "tempo_map": [{"beats": 0.0, "tempo": 2.0}]}))
        .expect("a sequence")
    }

    /// **A performance converted is the page it reads as**: its notes stand
    /// where they are written and say their staff, voice and value; what
    /// plays them and the tempo map are kept; the reading is stored; and it
    /// is one edit, undone as one.
    #[test]
    fn a_take_converted_is_its_page_and_keeps_what_plays_it() {
        let mut seq = take();
        assert!(!seq.written_as_notation());
        let before = seq.clone();
        seq.edit(EventsIntent::ToNotation {
            reading: Some(json!({"meter": "3/4", "division": 8})),
            writing: None,
            interpretation: None,
        })
        .unwrap();
        assert!(seq.written_as_notation());
        let at: Vec<f64> = seq.events.iter().map(|e| e.at.0).collect();
        assert_eq!(at, vec![0.0, 1.0, 2.0], "where they are written");
        let first = seq.events[0].keys();
        assert_eq!(first["instrument"], json!("bell"));
        assert_eq!(first["pan"], json!(-0.5));
        assert_eq!(first["staff"], json!(0));
        assert_eq!(first["value"], json!([1, 4]));
        assert_eq!(seq.tempo_map, before.tempo_map);
        assert_eq!(
            Transcription::of(&seq).meter.as_deref(),
            Some("3/4"),
            "the reading it was read with"
        );
        // the copy changes nothing
        let mut copy = before.clone();
        let answer: serde_json::Value = serde_json::from_str(&crate::events::call_json(
            &mut copy,
            r#"{"verb": "notated", "reading": {"meter": "3/4", "division": 8}}"#,
        ))
        .unwrap();
        assert_eq!(copy, before);
        let notated: EventSequence = serde_json::from_value(answer["sequence"].clone()).unwrap();
        assert_eq!(notated.events.len(), 3);
    }

    /// **A performance converted keeps its ids**: a note of the page that is
    /// a note of the performance has the id it had, its curves too, a note the
    /// page adds (the second half of a note across a barline) takes an id the
    /// sequence never gave out, and everything that names an event names one
    /// that is there.
    #[test]
    fn a_take_converted_keeps_the_ids_it_had() {
        let mut seq: EventSequence = serde_json::from_value(json!({
            "events": [
                {"id": 7, "at": 0.0, "data": {"midinote": 60, "dur": 1.0},
                 "automation": [{"id": 30, "target": {"bend": true},
                                 "points": [{"at": 0.0, "value": 0.0}, {"at": 1.0, "value": 1.0}]}]},
                {"id": 8, "at": 1.0, "data": {"midinote": 64, "dur": 1.0}},
                {"id": 9, "at": 2.0, "data": {"midinote": 67, "dur": 2.0}}
            ],
            "automation": [{"id": 20, "target": {"cc": 1},
                            "points": [{"at": 0.0, "value": 0.0}]}],
            "next_id": 30
        }))
        .unwrap();
        seq.edit(EventsIntent::ToNotation {
            reading: Some(json!({"meter": "3/4", "division": 4})),
            writing: None,
            interpretation: None,
        })
        .unwrap();
        let ids: Vec<u64> = seq.events.iter().map(|e| e.id).collect();
        for kept in [7, 8, 9] {
            assert!(ids.contains(&kept), "{kept} in {ids:?}");
        }
        assert!(
            ids.iter().all(|id| [7, 8, 9].contains(id) || *id > 30),
            "a note the page adds is new: {ids:?}"
        );
        let first = seq.get(7).unwrap();
        assert_eq!(
            first.automation[0].id,
            crate::NodeId(30),
            "its bend is its bend"
        );
        assert!(seq.automation.iter().any(|c| c.id == crate::NodeId(20)));
        let mut all: Vec<u64> = ids.clone();
        all.extend(seq.automation.iter().map(|c| c.id.0));
        all.extend(
            seq.events
                .iter()
                .flat_map(|e| e.automation.iter().map(|c| c.id.0)),
        );
        let mut once = all.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(once.len(), all.len(), "{all:?}");
        // what names an event names one that is there
        let held = |id: &serde_json::Value| id.as_u64().is_some_and(|id| ids.contains(&id));
        let section = seq.notation.clone().unwrap();
        for entry in section["items"].as_array().unwrap() {
            assert!(held(&entry[0]), "item {entry}");
        }
        for spanner in section["spanners"].as_array().into_iter().flatten() {
            assert!(
                held(&spanner["from"]) && held(&spanner["to"]),
                "spanner {spanner}"
            );
        }
        for event in &seq.events {
            if let Some(joins) = event.keys().get("joins") {
                assert!(held(joins), "a join from {}", event.id);
            }
        }
    }

    /// **What the page adds is numbered past what the sequence gave out**, and
    /// what names a page's event names it under its new id: a render numbers
    /// from one, so its ids would otherwise name the performance's other
    /// things.
    #[test]
    fn a_page_s_own_events_are_numbered_from_the_sequence_s_counter() {
        let performance: EventSequence = serde_json::from_value(json!({
            "events": [
                {"id": 7, "at": 0.0, "data": {"midinote": 60}},
                {"id": 9, "at": 2.0, "data": {"midinote": 64}}
            ],
            "automation": [{"id": 20, "target": {"cc": 64, "channel": 0}}],
            "next_id": 20
        }))
        .unwrap();
        // the page: its event 1 is the performance's 7, 3 is its 9, and 2 is
        // a note of the page alone, which 3 goes on from and a slur ends on
        let mut page: EventSequence = serde_json::from_value(json!({
            "events": [
                {"id": 1, "at": 0.0, "data": {"midinote": 60}},
                {"id": 2, "at": 1.0, "data": {"midinote": 62}},
                {"id": 3, "at": 2.0, "data": {"midinote": 64, "joins": 2}}
            ],
            "automation": [{"id": 4, "target": {"cc": 64, "channel": 0}},
                           {"id": 5, "target": {"cc": 7, "channel": 0}}],
            "notation": {"items": [[1, 1], [2, 2], [3, 3]],
                         "spanners": [{"kind": "slur", "from": 1, "to": 2}],
                         "marks": [[2, {"fingering": "3"}]]}
        }))
        .unwrap();
        let was = HashMap::from([(1, 7), (3, 9)]);
        number_as_the_sequence(&mut page, &performance, &was);
        let ids: Vec<u64> = page.events.iter().map(|e| e.id).collect();
        assert_eq!(ids[0], 7);
        assert_eq!(ids[2], 9);
        assert!(ids[1] > 20, "the page's own note is new: {ids:?}");
        let added = ids[1];
        assert_eq!(page.events[2].keys()["joins"], json!(added));
        let section = page.notation.clone().unwrap();
        assert_eq!(section["items"], json!([[7, 1], [added, 2], [9, 3]]));
        assert_eq!(section["spanners"][0]["to"], json!(added));
        assert_eq!(section["marks"][0][0], json!(added));
        assert_eq!(page.automation[0].id, crate::NodeId(20), "the same lane");
        assert!(page.automation[1].id.0 > 20, "a lane of the page's own");
        assert!(page.automation[1].id.0 != added);
    }

    /// **What goes on without an attack is written as a page writes it**: a
    /// note held on into its own pitch is one note of the summed value, and
    /// a join between different notes is a slur that joins them.
    #[test]
    fn a_take_s_joins_are_one_note_or_a_slur_that_joins() {
        let mut seq: EventSequence = serde_json::from_value(json!({
            "events": [
                {"id": 1, "at": 0.0, "data": {"midinote": 60, "dur": 1.0, "sustain": 1.0}},
                {"id": 2, "at": 1.0, "data": {"midinote": 64, "dur": 1.0, "sustain": 1.0,
                                              "joins": 1}},
                {"id": 3, "at": 2.0, "data": {"midinote": 64, "dur": 1.0, "sustain": 1.0,
                                              "joins": 2}},
                {"id": 4, "at": 3.0, "data": {"midinote": 67, "dur": 1.0, "sustain": 1.0}}
            ],
            "next_id": 4
        }))
        .unwrap();
        seq.edit(EventsIntent::ToNotation {
            reading: Some(json!({"meter": "4/4", "division": 4})),
            writing: None,
            interpretation: None,
        })
        .unwrap();
        let ids: Vec<u64> = seq.events.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 2, 4], "64 held on is one note");
        assert_eq!(seq.get(2).unwrap().keys()["value"], json!([1, 2]));
        assert_eq!(seq.get(2).unwrap().keys()["joins"], json!(1));
        let spanners = seq.notation.as_ref().unwrap()["spanners"].clone();
        assert_eq!(
            spanners,
            json!([{"kind": "slur", "from": 1, "to": 2, "join": true}])
        );
    }

    /// **A writing given writes every part with it**, stating the parts.
    #[test]
    fn a_writing_given_is_every_parts() {
        let mut seq = take();
        seq.edit(EventsIntent::ToNotation {
            reading: None,
            writing: Some(Writing {
                polyphony: crate::events::Polyphony::Ties,
                voices_per_staff: 1,
            }),
            interpretation: None,
        })
        .unwrap();
        assert_eq!(seq.parts.len(), 1);
        assert_eq!(
            seq.parts[0].writing.polyphony,
            crate::events::Polyphony::Ties
        );
    }
}
