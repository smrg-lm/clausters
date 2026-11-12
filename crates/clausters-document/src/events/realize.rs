//! **The performance derived from what is written.**
//!
//! A sequence written as notation sounds as its page says: a staccato is
//! short, a slur that joins is one line, a dynamic is a level, a pedal holds.
//! What it sounds is still keys of its events -- `sustain`, the level,
//! `joins`, the curves of its channels -- and an edit of the page writes them
//! back ([`write_back`](super::writeback::write_back)) only for what it
//! changed. [`realize`] writes them all again from what is written: the
//! sequence is read into its page as its reading says, the page is rendered,
//! and each note takes what its own renders as.
//!
//! **What is written stays as it is.** A note keeps its onset, its written
//! length, its pitch and its notation keys; what changes is how long it is
//! held, how loud it is and whether it goes on from the one before. A curve
//! the render writes replaces the sequence's of the same lane, on the
//! channel the sequence plays that line on; a curve the render does not write
//! is the sequence's own and stays.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use clausters_core::event::render as keys_of;
use clausters_notation::Interpretation;

use super::reading::Transcription;
use super::score::render;
use super::transcription::read;
use super::writeback::{bare, by_item, lane_of, midi_of};
use super::{EventSequence, articulation};

/// The keys a render sounds a note by, which a derived performance writes.
const SOUNDING: [&str; 2] = ["sustain", "amp"];

/// The curves of a note a render writes from its page: the level under a
/// hairpin, the pitch under a glissando.
const NOTE_CURVES: [&str; 2] = ["hairpin", "glissando"];

/// **`sequence` with its performance derived from what is written** (see the
/// module), read as its own reading says and played as `interp` says.
///
/// # Errors
/// When the sequence cannot be read into a page, or the page rendered.
pub fn realize(sequence: &EventSequence, interp: &Interpretation) -> Result<EventSequence, String> {
    let page = read(sequence, &Transcription::of(sequence), interp)?;
    let rendered = render(&page.sheet, interp)?;
    let sounded = by_item(&rendered);
    let mut out = sequence.clone();

    // each event of the sequence with the note of the render it is: by what
    // it sounds, else in order
    let mut held: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (event, item) in &page.items {
        held.entry(*item).or_default().push(*event);
    }
    // the render's channel of a line, as the sequence's
    let mut channels: BTreeMap<i64, i64> = BTreeMap::new();
    for (item, mine) in &held {
        let Some(theirs) = sounded.get(item) else {
            continue;
        };
        let mut free: Vec<usize> = (0..theirs.len()).collect();
        for id in mine {
            let Some(at) = out.events.iter().position(|event| event.id == *id) else {
                continue;
            };
            let sounds = midi_of(&out.events[at]);
            let Some(pick) = free
                .iter()
                .position(|i| midi_of(theirs[*i]) == sounds)
                .or((!free.is_empty()).then_some(0))
            else {
                break;
            };
            let from = theirs[free.remove(pick)];
            let (said, mut keys) = (from.keys(), out.events[at].keys());
            for key in SOUNDING {
                if let Some(value) = said.get(key) {
                    keys_of::set_key(&mut keys, key, value.clone());
                }
            }
            let channel = |keys: &serde_json::Map<String, Value>| {
                keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64
            };
            channels.entry(channel(&said)).or_insert(channel(&keys));
            let event = &mut out.events[at];
            event.data = crate::Opaque(Value::Object(keys));
            let derived =
                |name: &Option<String>| name.as_deref().is_some_and(|n| NOTE_CURVES.contains(&n));
            event.automation.retain(|curve| !derived(&curve.name));
            event.automation.extend(
                from.automation
                    .iter()
                    .filter(|c| derived(&c.name))
                    .map(bare),
            );
        }
    }

    // the curves of the lines: the render's, on the sequence's channels
    let lanes: Vec<_> = rendered
        .automation
        .iter()
        .map(|curve| {
            let mut curve = bare(curve);
            if let Some(target) = curve.target.0.as_object_mut()
                && let Some(channel) = target.get("channel").and_then(Value::as_f64)
                && let Some(ours) = channels.get(&(channel as i64))
            {
                target.insert("channel".into(), json!(ours));
            }
            curve
        })
        .collect();
    // A derived lane keeps the id of the lane it replaces, so deriving again
    // changes nothing; a new one takes the sequence's next id, from the one
    // counter events and curves share: a roll names a curve by its id, so
    // two curves with one id are drawn as one and edited into each other.
    let mut replaced: BTreeMap<String, crate::NodeId> = BTreeMap::new();
    out.automation.retain(|curve| {
        let lane = lane_of(curve);
        let derived = lanes.iter().any(|derived| lane_of(derived) == lane);
        if derived {
            replaced.entry(lane).or_insert(curve.id);
        }
        !derived
    });
    out.hold();
    for mut curve in lanes {
        curve.id = match replaced.remove(&lane_of(&curve)) {
            Some(id) => id,
            None => crate::NodeId(out.mint()),
        };
        out.automation.push(curve);
    }

    // what goes on from what: as each slur of the page says
    let slurs: Vec<(u64, u64, bool)> = page
        .sheet
        .spanners
        .iter()
        .filter(|spanner| spanner.kind == "slur")
        .filter_map(|spanner| {
            let first = |item: u64| held.get(&item).and_then(|events| events.first().copied());
            Some((first(spanner.from)?, first(spanner.to)?, spanner.join?))
        })
        .collect();
    for (from, to, join) in slurs {
        articulation::joins_from_slur(&mut out, from, to, join);
    }
    out.hold();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;
    use clausters_core::ratio::Ratio;
    use clausters_notation::{Item, Marks, Pitch, Sheet, Spanner, Voice, voice_to_sheet};

    /// Four quarters: a staccato under `p`, a slur that joins the next two,
    /// and the pedal down over the last two.
    fn page() -> Sheet {
        let mut sheet: Sheet = voice_to_sheet(&[], "4/4", "G2", "C");
        let note = |id: u64, midi: i32, marks: Marks| Item::Note {
            id,
            pitches: vec![Pitch::from_midi(midi, false)],
            dur: Ratio::new(1, 4),
            tie: false,
            marks,
        };
        sheet.staves[0].voices = vec![Voice {
            items: vec![
                note(
                    1,
                    60,
                    Marks {
                        articulations: vec!["stacc".into()],
                        dynamic: Some("p".into()),
                        ..Marks::default()
                    },
                ),
                note(2, 62, Marks::default()),
                note(3, 64, Marks::default()),
                note(4, 65, Marks::default()),
            ],
        }];
        sheet.spanners = vec![
            Spanner {
                kind: "slur".into(),
                from: 2,
                to: 3,
                join: Some(true),
            },
            Spanner {
                kind: "pedal".into(),
                from: 3,
                to: 4,
                join: None,
            },
        ];
        sheet
    }

    /// What each note sounds by, in order.
    fn sounding(sequence: &EventSequence) -> Vec<Map<String, Value>> {
        sequence
            .events
            .iter()
            .map(|event| {
                let keys = event.keys();
                ["at", "sustain", "amp", "joins"]
                    .iter()
                    .filter_map(|key| {
                        let value = match *key {
                            "at" => json!(event.at.0),
                            "joins" => json!(keys.get("joins").is_some()),
                            _ => keys.get(*key)?.clone(),
                        };
                        Some((key.to_string(), value))
                    })
                    .collect()
            })
            .collect()
    }

    /// **A performance is derived from what is written**: a sequence whose
    /// sounding keys were edited away gets them back from its page -- the
    /// staccato short, the level the dynamic's, the slur's join, the pedal's
    /// curve -- and keeps what is written.
    #[test]
    fn the_performance_is_derived_from_what_is_written() {
        let interp = Interpretation::default();
        let written = render(&page(), &interp).unwrap();
        let mut played = written.clone();
        for event in &mut played.events {
            let mut keys = event.keys();
            let dur = keys["dur"].clone();
            keys.insert("sustain".into(), dur);
            keys.insert("amp".into(), json!(1.0));
            keys.remove("joins");
            event.data = crate::Opaque(Value::Object(keys));
        }
        played.automation.clear();
        assert_ne!(sounding(&played), sounding(&written));

        let derived = realize(&played, &interp).unwrap();
        assert_eq!(sounding(&derived), sounding(&written));
        assert_eq!(
            derived.automation.len(),
            written.automation.len(),
            "the pedal and the dynamics' curves come back"
        );
        for (a, b) in derived.events.iter().zip(&played.events) {
            let (a, b) = (a.keys(), b.keys());
            for key in ["midinote", "dur", "value", "pitches", "articulations"] {
                assert_eq!(a.get(key), b.get(key), "{key} is written, and stays");
            }
        }
        // what is already derived is derived again the same
        assert_eq!(realize(&derived, &interp).unwrap(), derived);
    }

    /// **A derived curve takes an id nothing else has**: events and curves
    /// draw on one counter, so the pedal is never given the id of a note or
    /// of a note's own curve: a roll names a curve by its id, and two
    /// curves under one name are drawn as one and edited into each other.
    #[test]
    fn a_derived_curve_takes_an_id_nothing_else_has() {
        let interp = Interpretation::default();
        let mut sequence = render(&page(), &interp).unwrap();
        sequence.automation.clear();
        let bend = json!({"id": 0, "target": {"bend": true}, "points": [
            {"at": 0.0, "value": 0.0}, {"at": 1.0, "value": 1.0}]});
        sequence.events[0]
            .automation
            .push(serde_json::from_value(bend).unwrap());
        sequence.hold();
        let ids = |s: &EventSequence| {
            let mut ids: Vec<u64> = s
                .events
                .iter()
                .map(|e| e.id)
                .chain(s.automation.iter().map(|a| a.id.0))
                .chain(
                    s.events
                        .iter()
                        .flat_map(|e| e.automation.iter().map(|a| a.id.0)),
                )
                .collect();
            ids.sort_unstable();
            ids
        };
        let derived = realize(&sequence, &interp).unwrap();
        assert!(!derived.automation.is_empty(), "the pedal is derived");
        let all = ids(&derived);
        let mut unique = all.clone();
        unique.dedup();
        assert_eq!(all, unique, "no id is two things");
        // deriving again keeps each derived curve's id
        assert_eq!(ids(&realize(&derived, &interp).unwrap()), all);
    }

    /// **A slur that does not join takes the joins over its span away**.
    #[test]
    fn a_slur_that_does_not_join_takes_its_joins_away() {
        let interp = Interpretation::default();
        let mut sheet = page();
        sheet.spanners[0].join = Some(false);
        let mut sequence = render(&sheet, &interp).unwrap();
        // the third note joined to the second by hand, against the slur
        let second = sequence.events[1].id;
        let mut keys = sequence.events[2].keys();
        keys.insert("joins".into(), json!(second));
        sequence.events[2].data = crate::Opaque(Value::Object(keys));
        let derived = realize(&sequence, &interp).unwrap();
        assert!(
            derived
                .events
                .iter()
                .all(|event| event.keys().get("joins").is_none())
        );
    }
}
