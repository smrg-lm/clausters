//! **The notation keys**: what an event says about the page it is written on.
//!
//! An event says what a note does in the air -- its pitch, its level, its
//! length, in the families the module above keeps coherent. It may also say
//! what the note is **on a page**, and those keys are these: one table, read
//! by every client and by the books' reference, in which each key names one
//! fact, in one unit, and says which way it is read.
//!
//! Two of them are the written form of something the event also sounds, and
//! the table says which is the source:
//!
//! - `pitches` is the note as it is written -- a letter, an alteration, an
//!   octave -- where `midinote` is a number that two written notes share. An
//!   event that states no sounding pitch sounds its written one
//!   ([`written_midinote`]); one that states both keeps both, and the page
//!   writes `pitches`.
//! - `value` is the written value, an exact fraction of a whole note, where
//!   `dur` is time in beats. A triplet eighth is `[1, 12]` and its `dur` a
//!   third of a beat that no float holds; a grace note has a value and takes
//!   no time of the bar at all.
//!
//! The names are the score model's own (`pitches` is its field, `value` the
//! word its editor writes a note with), so what a client reads off a sheet and
//! what it writes on an event are spelled alike. None of them reaches a synth
//! as a control ([`super::render::RESERVED`]).
//!
//! **What is not any note's** is not a key. A slur, a hairpin, the meter and
//! its changes, the key, the clefs, the breaks, the page and its text belong
//! to the sequence, in its `notation` section
//! (`clausters_document::events::EventSequence`), where what has two ends
//! names them by event id.

use serde::Serialize;
use serde_json::Value;

/// One notation key: what it holds and how it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Key {
    /// The key, as an event spells it.
    pub name: &'static str,
    /// What its value is.
    pub holds: &'static str,
    /// The unit, or the vocabulary, the value is in.
    pub unit: &'static str,
    /// Which way it is read, and against which sounding key where it has one.
    pub reads: &'static str,
}

/// **The table.**
pub const KEYS: &[Key] = &[
    Key {
        name: "pitches",
        holds: "a list of written pitches, each {step, alter, octave} and forced where the \
                accidental is to be printed",
        unit: "step a letter c to b, alter in semitones, octave scientific (4 holds middle C)",
        reads: "the source of the pitch on a page; an event that states no freq, midinote or \
                degree sounds the first of them, and one with none of these is written from \
                midinote, spelled by `spelling`",
    },
    Key {
        name: "value",
        holds: "the written value, [numerator, denominator]",
        unit: "whole notes, exact: [1, 4] a quarter, [3, 8] a dotted quarter, [1, 12] a \
               triplet eighth",
        reads: "the source of the value on a page; `dur` and `sustain` are its performance \
                in beats, and an event without it is written from `dur`",
    },
    Key {
        name: "staff",
        holds: "the staff it is written on",
        unit: "an index from the top, from zero",
        reads: "where it is written; it sounds nothing",
    },
    Key {
        name: "voice",
        holds: "the voice of its staff it is written in",
        unit: "an index from zero",
        reads: "where it is written; a voice is one line, and renders on a channel of its own",
    },
    Key {
        name: "articulations",
        holds: "a list of articulations",
        unit: "MEI names: stacc, stacciss, spicc, acc, marc, ten, stress, upbow, dnbow, \
               harm, snap, open, stop",
        reads: "the mark is the fact; a shortened `sustain` or a raised level beside it is \
                what an interpretation made of it",
    },
    Key {
        name: "dynamic",
        holds: "a dynamic written at this note",
        unit: "its name: pp, p, mp, mf, f, ff, sf, fp",
        reads: "the mark is the source of the level and `amp`, `velocity` or `db` its \
                performance, derived again when the mark is edited",
    },
    Key {
        name: "ornament",
        holds: "an ornament on this note",
        unit: "its name: trill, mordent, turn, fermata",
        reads: "the mark is the fact; the notes it is played as are the interpretation's",
    },
    Key {
        name: "grace",
        holds: "that this is a grace note, and of which kind",
        unit: "acc, an appoggiatura, or unacc, an acciaccatura",
        reads: "on a page it takes no time of the bar and stands before the next note of \
                its voice; its `dur` and `sustain` are how it was played, and its `value` \
                what it is written as",
    },
    Key {
        name: "stem",
        holds: "a stem direction the writer forced",
        unit: "up or down",
        reads: "the page's alone; it sounds nothing",
    },
    Key {
        name: "tie",
        holds: "that this note ties into the next of its pitch",
        unit: "true or false",
        reads: "the two are one sound on a page; each event keeps its own length",
    },
    Key {
        name: "spelling",
        holds: "which accidental a pitch given as a number is written with",
        unit: "sharp or flat",
        reads: "a preference, read only where the event has no `pitches`",
    },
    Key {
        name: "accidental",
        holds: "whether the accidental is printed where the key or the measure implies it",
        unit: "written or sounding",
        reads: "read only where the event has no `pitches`, whose own `forced` says it",
    },
];

/// The names of the notation keys, in the table's order.
#[must_use]
pub fn names() -> Vec<&'static str> {
    KEYS.iter().map(|key| key.name).collect()
}

/// The table as JSON: a list of `{name, holds, unit, reads}`.
#[must_use]
pub fn keys_json() -> String {
    serde_json::to_string(KEYS).unwrap_or_else(|_| "[]".into())
}

/// The semitones of a step's letter above C.
fn step_semitones(step: &str) -> Option<f64> {
    Some(match step {
        "c" | "C" => 0.0,
        "d" | "D" => 2.0,
        "e" | "E" => 4.0,
        "f" | "F" => 5.0,
        "g" | "G" => 7.0,
        "a" | "A" => 9.0,
        "b" | "B" => 11.0,
        _ => return None,
    })
}

/// **The MIDI note a `pitches` value sounds**: that of its first pitch -- the
/// letter, altered, in its octave; middle C, `c4`, is 60 -- or `None` when the
/// value is not a list holding one. A written pitch may be altered by a
/// fraction of a semitone, as a degree's `alter` may.
#[must_use]
pub fn written_midinote(pitches: &Value) -> Option<f64> {
    let first = pitches.as_array()?.first()?;
    let step = step_semitones(first.get("step")?.as_str()?)?;
    let octave = first.get("octave")?.as_f64()?;
    let alter = first.get("alter").and_then(Value::as_f64).unwrap_or(0.0);
    Some((octave + 1.0) * 12.0 + step + alter)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::event::render::RESERVED;

    #[test]
    fn every_notation_key_is_reserved_and_named_once() {
        let names = names();
        for name in &names {
            assert!(
                RESERVED.contains(name),
                "{name} would reach a synth as a control"
            );
        }
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
        // and the table crosses as it is written
        let crossed: Value = serde_json::from_str(&keys_json()).unwrap();
        assert_eq!(crossed.as_array().unwrap().len(), KEYS.len());
        assert_eq!(crossed[0]["name"], "pitches");
        assert!(crossed.as_array().unwrap().iter().all(|key| {
            ["holds", "unit", "reads"]
                .iter()
                .all(|field| key[field].as_str().is_some_and(|text| !text.is_empty()))
        }));
    }

    #[test]
    fn a_written_pitch_sounds_its_letter_altered_in_its_octave() {
        let pitch = |step: &str, alter: f64, octave: i32| json!([{"step": step, "alter": alter, "octave": octave}]);
        assert_eq!(written_midinote(&pitch("c", 0.0, 4)), Some(60.0));
        assert_eq!(written_midinote(&pitch("e", -1.0, 4)), Some(63.0));
        assert_eq!(
            written_midinote(&pitch("B", 1.0, 3)),
            Some(60.0),
            "b sharp is that c"
        );
        assert_eq!(
            written_midinote(&json!([{"step": "a", "octave": 4}])),
            Some(69.0)
        );
        // a chord sounds its first; what is no list of pitches sounds nothing
        let chord = json!([{"step": "c", "octave": 4}, {"step": "g", "octave": 4}]);
        assert_eq!(written_midinote(&chord), Some(60.0));
        assert_eq!(written_midinote(&json!([])), None);
        assert_eq!(written_midinote(&json!(60)), None);
        assert_eq!(written_midinote(&json!([{"step": "h", "octave": 4}])), None);
    }
}
