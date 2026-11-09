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
//! plays each note (its `instrument`, and every key that is neither time,
//! pitch, level nor notation), the tempo map, the parts, the curves the page
//! does not write.
//!
//! **It is a conversion, not a reading**: the times as played become the
//! written ones, so it is one edit, undone as one, and a caller that wants the
//! performance kept converts a copy.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use clausters_core::notation::Interpretation;

use super::EventSequence;
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
            let mut keys = out.events[at].keys();
            for (key, value) in from.keys() {
                if !PAGE_KEYS.contains(&key.as_str()) {
                    keys.entry(key).or_insert(value);
                }
            }
            out.events[at].data = crate::Opaque(Value::Object(keys));
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
    let written: BTreeSet<String> = out.automation.iter().map(lane_of).collect();
    let mut next = out.automation.iter().map(|c| c.id.0).max().unwrap_or(0);
    for curve in &sequence.automation {
        if !written.contains(&lane_of(curve)) {
            next += 1;
            let mut kept = curve.clone();
            kept.id = crate::NodeId(next);
            out.automation.push(kept);
        }
    }
    out.midi = sequence.midi.or(out.midi);
    // the reading it was read with is the one it is read with from now on
    out.set_reading(Some(
        serde_json::to_value(reading).map_err(|e| e.to_string())?,
    ))?;
    Ok(out)
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
