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
//!   writes `pitches`. An edit to either moves the other
//!   (`super::render::set_key`).
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
                midinote, spelled by `spelling`; moving what the event sounds spells them \
                again, and writing them moves what it sounds",
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
        reads: "a preference, read where the event has no `pitches` and where a moved \
                sounding pitch spells them again",
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
    midinote_of(pitches.as_array()?.first()?)
}

/// **A MIDI number spelled**, sharps or flats for the black keys: the index of
/// its letter from C, its alteration and its scientific octave. The octave is
/// the one the letter is written in -- `cb4` sounds in octave 3 -- so it is
/// derived from the natural the spelling sits on.
#[must_use]
pub fn spell(midi: i32, flats: bool) -> (usize, i32, i32) {
    // (step index, alter) per pitch class, one table per accidental world.
    const SHARP: [(usize, i32); 12] = [
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
        (2, 0),
        (3, 0),
        (3, 1),
        (4, 0),
        (4, 1),
        (5, 0),
        (5, 1),
        (6, 0),
    ];
    const FLAT: [(usize, i32); 12] = [
        (0, 0),
        (1, -1),
        (1, 0),
        (2, -1),
        (2, 0),
        (3, 0),
        (4, -1),
        (4, 0),
        (5, -1),
        (5, 0),
        (6, -1),
        (6, 0),
    ];
    let table = if flats { &FLAT } else { &SHARP };
    let (step, alter) = table[midi.rem_euclid(12) as usize];
    (step, alter, (midi - alter).div_euclid(12) - 1)
}

/// The written pitch of one entry of `pitches`, as a MIDI number.
fn midinote_of(pitch: &Value) -> Option<f64> {
    let step = step_semitones(pitch.get("step")?.as_str()?)?;
    let octave = pitch.get("octave")?.as_f64()?;
    let alter = pitch.get("alter").and_then(Value::as_f64).unwrap_or(0.0);
    Some((octave + 1.0) * 12.0 + step + alter)
}

/// **`pitches` moved by `semitones`**, as a written note moves when what it
/// sounds is moved: by whole octaves each pitch keeps its letter and its sign,
/// and otherwise it is spelled again -- sharps, or flats when `flats` -- and a
/// sign the writer forced is not carried to a note it was not written for. A
/// value that is not a list of pitches comes back as it is.
#[must_use]
pub fn transposed(pitches: &Value, semitones: i32, flats: bool) -> Value {
    let Some(list) = pitches.as_array() else {
        return pitches.clone();
    };
    let moved = list.iter().map(|pitch| {
        if semitones % 12 == 0 {
            let mut pitch = pitch.clone();
            if let Some(octave) = pitch.get("octave").and_then(Value::as_i64) {
                pitch["octave"] = (octave + i64::from(semitones / 12)).into();
            }
            return pitch;
        }
        let Some(midi) = midinote_of(pitch) else {
            return pitch.clone();
        };
        let (step, alter, octave) = spell(midi.round() as i32 + semitones, flats);
        let step = ["c", "d", "e", "f", "g", "a", "b"][step];
        serde_json::json!({
            "step": step,
            "alter": alter,
            "octave": octave,
        })
    });
    Value::Array(moved.collect())
}

/// **The keys a signature names**, by their tonic: C, then the sharp keys,
/// then the flat ones, as far as seven signs.
pub const TONICS: [&str; 15] = [
    "C", "G", "D", "A", "E", "B", "F#", "C#", "F", "Bb", "Eb", "Ab", "Db", "Gb", "Cb",
];

/// The key signature of a tonic name, in fifths: sharps positive, `0` for a
/// name that is none.
#[must_use]
pub fn fifths_of(key: &str) -> i32 {
    match TONICS.iter().position(|k| *k == key) {
        Some(at) if at <= 7 => at as i32,
        Some(at) => 7 - at as i32,
        None => 0,
    }
}

/// The tonic name of a signature of `fifths`, held to seven signs.
#[must_use]
pub fn key_of(fifths: i32) -> &'static str {
    if fifths >= 0 {
        TONICS[fifths.min(7) as usize]
    } else {
        TONICS[(7 - fifths.max(-7)) as usize]
    }
}

const LETTERS: [&str; 7] = ["c", "d", "e", "f", "g", "a", "b"];

/// **A MIDI number spelled in a key** of `fifths`, as [`spell`] answers: of
/// the names it could have, the one nearest the tonic on the line of fifths
/// -- from four fifths under it to seven over, which writes a pitch of the
/// scale as the scale writes it, the flat third, sixth and seventh, and the
/// sharp first and fourth of a major key.
#[must_use]
pub fn spell_in(midi: i32, fifths: i32) -> (usize, i32, i32) {
    let low = fifths - 4;
    let place = (midi * 7 - low).rem_euclid(12) + low;
    // the line of fifths, F C G D A E B at -1..5, as indices from C
    let step = [3, 0, 4, 1, 5, 2, 6][(place + 1).rem_euclid(7) as usize];
    let alter = (place + 1).div_euclid(7);
    (step, alter, (midi - alter).div_euclid(12) - 1)
}

/// The sign a key of `fifths` writes on the letter `step` (an index from C).
fn signed(step: usize, fifths: i32) -> i32 {
    // the order sharps are added in, F C G D A E B, as indices from C; flats
    // go the other way
    const ORDER: [usize; 7] = [3, 0, 4, 1, 5, 2, 6];
    let rank = ORDER.iter().position(|s| *s == step).unwrap_or(0) as i32;
    if fifths > rank {
        1 + (fifths - rank - 1) / 7
    } else if fifths < rank - 6 {
        -(1 + (rank - 6 - fifths - 1) / 7)
    } else {
        0
    }
}

fn pitch_json(step: usize, alter: i32, octave: i32) -> Value {
    serde_json::json!({"step": LETTERS[step], "alter": alter, "octave": octave})
}

/// **`pitches` moved by `semitones`, spelled by a key** of `fifths`: as
/// [`transposed`], but a pitch spelled again is written as the key writes it
/// ([`spell_in`]) rather than by an accidental world alone.
#[must_use]
pub fn transposed_in(pitches: &Value, semitones: i32, fifths: i32) -> Value {
    if semitones % 12 == 0 {
        return transposed(pitches, semitones, false);
    }
    let Some(list) = pitches.as_array() else {
        return pitches.clone();
    };
    let moved = list.iter().map(|pitch| match midinote_of(pitch) {
        Some(midi) => {
            let (step, alter, octave) = spell_in(midi.round() as i32 + semitones, fifths);
            pitch_json(step, alter, octave)
        }
        None => pitch.clone(),
    });
    Value::Array(moved.collect())
}

/// **`pitches` moved by `steps` of a key** of `fifths`: each to the letter
/// `steps` away, with the sign the key writes on it, and a pitch the key
/// does not hold keeps how far it was from the key's own (a raised fourth
/// moved a step is a raised fifth). Negative steps go down.
#[must_use]
pub fn stepped_in(pitches: &Value, steps: i32, fifths: i32) -> Value {
    let Some(list) = pitches.as_array() else {
        return pitches.clone();
    };
    let moved = list.iter().map(|pitch| {
        let letter = pitch
            .get("step")
            .and_then(Value::as_str)
            .and_then(|s| LETTERS.iter().position(|l| l.eq_ignore_ascii_case(s)));
        let octave = pitch.get("octave").and_then(Value::as_i64);
        let (Some(letter), Some(octave)) = (letter, octave) else {
            return pitch.clone();
        };
        let alter = pitch
            .get("alter")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .round() as i32;
        let off = alter - signed(letter, fifths);
        let to = letter as i32 + steps;
        let step = to.rem_euclid(7) as usize;
        let octave = octave as i32 + to.div_euclid(7);
        pitch_json(step, signed(step, fifths) + off, octave)
    });
    Value::Array(moved.collect())
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

    #[test]
    fn a_pitch_is_spelled_as_its_key_writes_it() {
        let name = |midi: i32, key: &str| {
            let (step, alter, octave) = spell_in(midi, fifths_of(key));
            (LETTERS[step], alter, octave)
        };
        assert_eq!(name(70, "F"), ("b", -1, 4), "B flat in F");
        assert_eq!(name(66, "D"), ("f", 1, 4), "F sharp in D");
        assert_eq!(name(63, "C"), ("e", -1, 4), "the flat third");
        assert_eq!(name(66, "C"), ("f", 1, 4), "the sharp fourth");
        assert_eq!(
            name(71, "Gb"),
            ("c", -1, 5),
            "C flat in G flat, an octave up"
        );
        assert_eq!(fifths_of("Eb"), -3);
        assert_eq!(key_of(-3), "Eb");
        assert_eq!(key_of(9), "C#");
    }

    #[test]
    fn a_pitch_moved_in_a_key_is_written_as_the_key_writes_it() {
        let c = json!([{"step": "c", "alter": 0, "octave": 4}]);
        // a whole tone up in B flat is D; a semitone up in F is D flat
        assert_eq!(
            transposed_in(&c, 2, -2),
            json!([{"step": "d", "alter": 0, "octave": 4}])
        );
        assert_eq!(
            transposed_in(&c, 1, -1),
            json!([{"step": "d", "alter": -1, "octave": 4}])
        );
        // a step of D major from E is F sharp, and from B is C sharp above
        let e = json!([{"step": "e", "alter": 0, "octave": 4}]);
        assert_eq!(
            stepped_in(&e, 1, 2),
            json!([{"step": "f", "alter": 1, "octave": 4}])
        );
        let b = json!([{"step": "b", "alter": 0, "octave": 4}]);
        assert_eq!(
            stepped_in(&b, 1, 2),
            json!([{"step": "c", "alter": 1, "octave": 5}])
        );
        // a raised fourth of C moved a step is a raised fifth; down wraps the octave
        let fs = json!([{"step": "f", "alter": 1, "octave": 4}]);
        assert_eq!(
            stepped_in(&fs, 1, 0),
            json!([{"step": "g", "alter": 1, "octave": 4}])
        );
        assert_eq!(
            stepped_in(&c, -1, -1),
            json!([{"step": "b", "alter": -1, "octave": 3}])
        );
    }
}
