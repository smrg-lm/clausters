//! Rendering an event for the destination it plays on.
//!
//! One event, several destinations: a synth on the server (a `/synth_new` and
//! its release), a MIDI port (the messages its `type` names), or another
//! application over OSC. What an event *is* is its `type` key --
//! `note` (the default), `rest`, `midi` (with `midicmd`) or `osc` -- and each
//! render reads the keys that type owns.
//!
//! Here the event is its map of keys, as the document stores it and as a
//! client hands it across as JSON, written with the reference client's spelling
//! (`add_action`, `has_gate`). A client whose idiom spells a key otherwise
//! translates it at its door.

use serde_json::{Map, Value};

use super::{Level, split_degree};
use super::{LevelKey, MAJOR, Pitch, PitchKey, Spelling, delta, sustain, velocity_of_amp};

/// The keys that drive timing, structure, pitch, level, notation and the other
/// destinations, and are never sent to a synth as controls. Every other number
/// an event holds is.
pub const RESERVED: &[&str] = &[
    "type",
    "instrument",
    "dur",
    "legato",
    "stretch",
    "sustain",
    "delta",
    "add_action",
    "target",
    "group",
    "server",
    "has_gate",
    "midinote",
    "degree",
    "alter",
    "octave",
    "root",
    "scale",
    "node",
    "velocity",
    "db",
    // What the note says on a page.
    "articulations",
    "dynamic",
    "ornament",
    "grace",
    "stem",
    "spelling",
    "accidental",
    "tie",
    // What a MIDI or an OSC event is.
    "channel",
    "midicmd",
    "cc",
    "program",
    "bytes",
    "addr",
    "args",
];

/// What an event is, from its `type` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    /// A sounding note: a synth, or a note-on and its note-off. The default.
    Note,
    /// Silence that still takes its `delta`.
    Rest,
    /// A MIDI message, which `midicmd` names.
    Midi,
    /// A raw OSC message, `addr` and `args`.
    Osc,
}

impl Type {
    /// The type an event's keys name; an unknown or absent `type` is a note.
    #[must_use]
    pub fn of(keys: &Map<String, Value>) -> Self {
        match keys.get("type").and_then(Value::as_str) {
            Some("rest") => Self::Rest,
            Some("midi") => Self::Midi,
            Some("osc") => Self::Osc,
            _ => Self::Note,
        }
    }
}

fn number(keys: &Map<String, Value>, key: &str) -> Option<f64> {
    keys.get(key).and_then(Value::as_f64)
}

fn number_or(keys: &Map<String, Value>, key: &str, default: f64) -> f64 {
    number(keys, key).unwrap_or(default)
}

/// The scale an event's degree indexes: its `scale` key, else the major.
#[must_use]
pub fn scale_of(keys: &Map<String, Value>) -> Vec<f32> {
    match keys.get("scale").and_then(Value::as_array) {
        Some(steps) => steps
            .iter()
            .filter_map(Value::as_f64)
            .map(|v| v as f32)
            .collect(),
        None => MAJOR.to_vec(),
    }
}

/// The pitch keys an event's map holds.
#[must_use]
pub fn pitch_of(keys: &Map<String, Value>) -> Pitch {
    let mut pitch = Pitch {
        freq: number(keys, "freq"),
        midinote: number(keys, "midinote"),
        degree: number(keys, "degree"),
        alter: number(keys, "alter"),
        octave: number(keys, "octave"),
        root: number(keys, "root"),
    };
    // A degree written in SuperCollider's fraction reads as the two keys.
    if let Some(degree) = pitch.degree {
        let (whole, written) = split_degree(degree);
        if written != 0.0 {
            pitch.degree = Some(whole);
            pitch.alter = Some(pitch.alter.unwrap_or(0.0) + written);
        }
    }
    pitch
}

/// The level keys an event's map holds.
#[must_use]
pub fn level_of(keys: &Map<String, Value>) -> Level {
    Level {
        amp: number(keys, "amp"),
        velocity: number(keys, "velocity"),
        db: number(keys, "db"),
    }
}

/// The `spelling` an event states.
#[must_use]
pub fn spelling_of(keys: &Map<String, Value>) -> Spelling {
    Spelling::from_key(keys.get("spelling").and_then(Value::as_str))
}

/// Writes `key` into an event's map with its family's coherence: the pitch and
/// level keys the event holds are rewritten to agree, and any other key is
/// written as it is. What an editor calls when a hand moves one key of a note.
pub fn set_key(keys: &mut Map<String, Value>, key: &str, value: Value) {
    let Some(pitch_key) = PitchKey::from_name(key) else {
        if let (Some(level_key), Some(v)) = (LevelKey::from_name(key), value.as_f64()) {
            let mut level = level_of(keys);
            level.set(level_key, v);
            write(
                keys,
                [
                    ("amp", level.amp),
                    ("velocity", level.velocity),
                    ("db", level.db),
                ],
            );
        }
        keys.insert(key.into(), value);
        return;
    };
    let v = value.as_f64();
    if pitch_key == PitchKey::Scale {
        // The scale is a list: written first, so the degree reads through it.
        keys.insert(key.into(), value.clone());
    } else if v.is_none() {
        keys.insert(key.into(), value);
        return;
    }
    let scale = scale_of(keys);
    let mut pitch = pitch_of(keys);
    pitch.set(pitch_key, v.unwrap_or(0.0), &scale, spelling_of(keys));
    write(
        keys,
        [
            ("freq", pitch.freq),
            ("midinote", pitch.midinote),
            ("degree", pitch.degree),
            ("alter", pitch.alter),
            ("octave", pitch.octave),
            ("root", pitch.root),
        ],
    );
    // The key written keeps the value it was handed (`64` stays `64`), unless
    // it was a fractional degree, which the two keys now spell.
    let split = pitch_key == PitchKey::Degree && v.is_some_and(|d| split_degree(d).1 != 0.0);
    if pitch_key != PitchKey::Scale && !split {
        keys.insert(key.into(), value);
    }
}

fn write<const N: usize>(keys: &mut Map<String, Value>, named: [(&str, Option<f64>); N]) {
    for (name, held) in named {
        if let Some(v) = held {
            keys.insert(name.into(), v.into());
        }
    }
}

/// Beats until the next event.
#[must_use]
pub fn delta_of(keys: &Map<String, Value>) -> f64 {
    delta(
        number_or(keys, "dur", 1.0),
        number_or(keys, "stretch", 1.0),
        number(keys, "delta"),
    )
}

/// Beats the event sounds.
#[must_use]
pub fn sustain_of(keys: &Map<String, Value>) -> f64 {
    sustain(
        number_or(keys, "dur", 1.0),
        number_or(keys, "legato", 0.8),
        number_or(keys, "stretch", 1.0),
        number(keys, "sustain"),
    )
}

/// One argument of a rendered OSC message, typed as the wire carries it.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Str(String),
    Int(i32),
    Float(f32),
}

impl Arg {
    /// The argument as a door carries it: `[tag, value]`, the tag `"s"`, `"i"`
    /// or `"f"`, so a float with an integral value stays a float.
    #[must_use]
    pub fn tagged(&self) -> Value {
        match self {
            Self::Str(s) => Value::from(vec![Value::from("s"), Value::from(s.as_str())]),
            Self::Int(i) => Value::from(vec![Value::from("i"), Value::from(*i)]),
            Self::Float(f) => Value::from(vec![Value::from("f"), Value::from(f64::from(*f))]),
        }
    }
}

/// A note rendered for a synth: the message that starts it, the one that ends
/// it, and when the second is due after the first, in beats.
#[derive(Debug, Clone, PartialEq)]
pub struct Synth {
    /// `/synth_new instrument node add_action target freq f amp a ...`.
    pub start: Vec<Arg>,
    /// `/node_set node gate 0` when the note releases by its gate, else
    /// `/node_free node`.
    pub release: Vec<Arg>,
    /// Beats from the start to the release.
    pub sustain: f64,
}

impl Synth {
    /// The render as a door carries it: `{"start": [..], "release": [..],
    /// "sustain": beats}`, each message a list of tagged arguments with the
    /// address first.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let tagged = |args: &[Arg]| Value::from(args.iter().map(Arg::tagged).collect::<Vec<_>>());
        serde_json::json!({
            "start": tagged(&self.start),
            "release": tagged(&self.release),
            "sustain": self.sustain,
        })
    }
}

/// Whether an event releases by closing its gate: it says so (`has_gate`), or
/// it plays the built-in `"default"` instrument, whose envelope frees itself
/// on release. Everything else is freed.
#[must_use]
pub fn releases_by_gate(keys: &Map<String, Value>) -> bool {
    let gated = match keys.get("has_gate") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|v| v != 0.0),
        _ => false,
    };
    gated || instrument(keys) == "default"
}

fn instrument(keys: &Map<String, Value>) -> &str {
    keys.get("instrument")
        .and_then(Value::as_str)
        .unwrap_or("default")
}

/// A note event as the messages that start and end its synth, on node `node`;
/// `None` for anything that is not a note. The controls are `freq` and `amp`
/// (the ones a synth takes by convention), `out` when stated, then every other
/// number the event holds and does not reserve ([`RESERVED`]), by name.
#[must_use]
pub fn synth(keys: &Map<String, Value>, node: i32) -> Option<Synth> {
    if Type::of(keys) != Type::Note {
        return None;
    }
    let scale = scale_of(keys);
    let mut start = vec![
        Arg::Str("/synth_new".into()),
        Arg::Str(instrument(keys).into()),
        Arg::Int(node),
        Arg::Int(number_or(keys, "add_action", 1.0) as i32),
        Arg::Int(number_or(keys, "target", 0.0) as i32),
        Arg::Str("freq".into()),
        Arg::Float(pitch_of(keys).freq(&scale) as f32),
        Arg::Str("amp".into()),
        Arg::Float(level_of(keys).amp() as f32),
    ];
    if let Some(out) = number(keys, "out") {
        start.push(Arg::Str("out".into()));
        start.push(Arg::Float(out as f32));
    }
    // By name, so the order is the same whichever map the keys came in.
    let mut controls: Vec<(&String, f64)> = keys
        .iter()
        .filter(|(k, v)| {
            !RESERVED.contains(&k.as_str())
                && !matches!(k.as_str(), "freq" | "amp" | "out")
                && v.is_number()
        })
        .filter_map(|(k, v)| v.as_f64().map(|n| (k, n)))
        .collect();
    controls.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in controls {
        start.push(Arg::Str(name.clone()));
        start.push(Arg::Float(value as f32));
    }
    let release = if releases_by_gate(keys) {
        vec![
            Arg::Str("/node_set".into()),
            Arg::Int(node),
            Arg::Str("gate".into()),
            Arg::Float(0.0),
        ]
    } else {
        vec![Arg::Str("/node_free".into()), Arg::Int(node)]
    };
    Some(Synth {
        start,
        release,
        sustain: sustain_of(keys),
    })
}

/// A MIDI message an event renders to, `at` beats after the event.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiMessage {
    pub at: f64,
    pub bytes: Vec<u8>,
}

fn data7(value: f64) -> u8 {
    value.round().clamp(0.0, 127.0) as u8
}

/// The velocity a MIDI message of this event carries: an explicit `velocity`
/// as it is (0..127, a note-on at 0 being the note-off it is on the wire), else
/// its amplitude's.
fn raw_velocity(keys: &Map<String, Value>) -> u8 {
    match number(keys, "velocity") {
        Some(v) => data7(v),
        None => velocity_of_amp(level_of(keys).amp()) as u8,
    }
}

fn note_number(keys: &Map<String, Value>) -> u8 {
    data7(pitch_of(keys).midinote(&scale_of(keys)))
}

/// An event as the MIDI messages it plays: a note as its note-on and, `sustain`
/// beats later, its note-off; a `midi` event as the one message its `midicmd`
/// names (`note_on`, `note_off`, `cc`, `program`, `bend`, `aftertouch`,
/// `polytouch`, or `raw` with its `bytes`); a rest as nothing. `channel` is the
/// destination's own, used when the event states none. An OSC event is refused:
/// it has no MIDI spelling.
///
/// # Errors
/// An OSC event, or a `midicmd` this does not know.
pub fn midi(keys: &Map<String, Value>, channel: u8) -> Result<Vec<MidiMessage>, String> {
    let ch = number(keys, "channel").map_or(channel, |c| c as u8) & 0x0F;
    let now = |bytes: Vec<u8>| Ok(vec![MidiMessage { at: 0.0, bytes }]);
    match Type::of(keys) {
        Type::Rest => Ok(Vec::new()),
        Type::Osc => Err("an osc event has no MIDI spelling: play it on an OSC destination".into()),
        Type::Note => {
            let note = note_number(keys);
            // A note-on at velocity 0 would be its own note-off.
            let velocity = level_of(keys).velocity() as u8;
            Ok(vec![
                MidiMessage {
                    at: 0.0,
                    bytes: vec![0x90 | ch, note, velocity],
                },
                MidiMessage {
                    at: sustain_of(keys),
                    bytes: vec![0x80 | ch, note, 0],
                },
            ])
        }
        Type::Midi => {
            let value = number_or(keys, "value", 0.0);
            match keys.get("midicmd").and_then(Value::as_str).unwrap_or("") {
                "note_on" => now(vec![0x90 | ch, note_number(keys), raw_velocity(keys)]),
                "note_off" => now(vec![0x80 | ch, note_number(keys), raw_velocity(keys)]),
                "cc" => now(vec![
                    0xB0 | ch,
                    data7(number_or(keys, "cc", 0.0)),
                    data7(value),
                ]),
                "program" => now(vec![0xC0 | ch, data7(number_or(keys, "program", 0.0))]),
                "aftertouch" => now(vec![0xD0 | ch, data7(value)]),
                "polytouch" => now(vec![0xA0 | ch, note_number(keys), data7(value)]),
                "bend" => {
                    let wide = (value.round() + 8192.0).clamp(0.0, 16383.0) as u16;
                    now(vec![0xE0 | ch, (wide & 0x7F) as u8, (wide >> 7) as u8])
                }
                "raw" => now(keys
                    .get("bytes")
                    .and_then(Value::as_array)
                    .map(|b| {
                        b.iter()
                            .filter_map(Value::as_f64)
                            .map(|v| v as u8)
                            .collect()
                    })
                    .unwrap_or_default()),
                other => Err(format!(
                    "no midicmd {other:?}: it is one of note_on, note_off, cc, program, bend, aftertouch, polytouch or raw"
                )),
            }
        }
    }
}

/// A MIDI message's bytes as the `midi` event that plays them back: the
/// channel-voice messages by name (`note_on`, `note_off`, `cc`, `program`,
/// `bend`, `aftertouch`, `polytouch`), and anything else -- a system message,
/// a running-status fragment -- as `raw` with its bytes. [`midi`] renders the
/// event back to the same bytes.
#[must_use]
pub fn from_midi(bytes: &[u8]) -> Map<String, Value> {
    let mut keys = Map::new();
    keys.insert("type".into(), "midi".into());
    let raw = |mut keys: Map<String, Value>| {
        keys.insert("midicmd".into(), "raw".into());
        keys.insert(
            "bytes".into(),
            Value::from(bytes.iter().map(|&b| u64::from(b)).collect::<Vec<_>>()),
        );
        keys
    };
    let Some(&status) = bytes.first() else {
        return raw(keys);
    };
    let (kind, ch) = (status & 0xF0, status & 0x0F);
    let data = |i: usize| bytes.get(i).map(|&b| f64::from(b & 0x7F));
    let (cmd, fields): (&str, Vec<(&str, Option<f64>)>) = match (kind, bytes.len()) {
        (0x90, 3) => (
            "note_on",
            vec![("midinote", data(1)), ("velocity", data(2))],
        ),
        (0x80, 3) => (
            "note_off",
            vec![("midinote", data(1)), ("velocity", data(2))],
        ),
        (0xB0, 3) => ("cc", vec![("cc", data(1)), ("value", data(2))]),
        (0xC0, 2) => ("program", vec![("program", data(1))]),
        (0xD0, 2) => ("aftertouch", vec![("value", data(1))]),
        (0xA0, 3) => ("polytouch", vec![("midinote", data(1)), ("value", data(2))]),
        (0xE0, 3) => {
            let wide = data(1).unwrap_or(0.0) + data(2).unwrap_or(0.0) * 128.0;
            ("bend", vec![("value", Some(wide - 8192.0))])
        }
        _ => return raw(keys),
    };
    keys.insert("midicmd".into(), cmd.into());
    keys.insert("channel".into(), f64::from(ch).into());
    for (name, value) in fields {
        if let Some(v) = value {
            keys.insert(name.into(), v.into());
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_note_renders_its_synth_and_release() {
        let e = keys(
            json!({"instrument": "sine", "degree": 0, "amp": 0.3, "cutoff": 800,
                            "dur": 1.0, "legato": 0.5, "tie": true, "velocity": 90}),
        );
        let s = synth(&e, 1000).unwrap();
        assert_eq!(s.start[0], Arg::Str("/synth_new".into()));
        assert_eq!(s.start[2], Arg::Int(1000));
        assert_eq!(
            s.start[5..],
            [
                Arg::Str("freq".into()),
                Arg::Float(261.625_58),
                Arg::Str("amp".into()),
                Arg::Float(0.3),
                Arg::Str("cutoff".into()),
                Arg::Float(800.0),
            ]
        );
        assert_eq!(
            s.release,
            vec![Arg::Str("/node_free".into()), Arg::Int(1000)]
        );
        assert_eq!(s.sustain, 0.5);
        let gated = synth(&keys(json!({"instrument": "default"})), 1).unwrap();
        assert_eq!(gated.release[0], Arg::Str("/node_set".into()));
        assert!(synth(&keys(json!({"type": "rest"})), 1).is_none());
    }

    #[test]
    fn midi_bytes_round_trip_through_an_event() {
        for bytes in [
            vec![0x91, 60, 100],
            vec![0x91, 60, 0],
            vec![0x82, 64, 10],
            vec![0xB0, 7, 99],
            vec![0xC3, 12],
            vec![0xD0, 55],
            vec![0xA5, 60, 33],
            vec![0xE0, 0x00, 0x40],
            vec![0xE1, 0x7F, 0x7F],
            vec![0xF0, 0x7E, 0x7F, 0xF7],
        ] {
            let event = from_midi(&bytes);
            let back = midi(&event, 0).unwrap();
            assert_eq!(back.len(), 1);
            assert_eq!(back[0].bytes, bytes, "{event:?}");
        }
        assert_eq!(from_midi(&[0xE0, 0x00, 0x40])["value"], 0.0);
    }

    #[test]
    fn a_note_on_a_midi_destination_is_its_on_and_off() {
        let e = keys(json!({"midinote": 64, "amp": 0.5, "dur": 2.0, "legato": 0.5}));
        let msgs = midi(&e, 3).unwrap();
        assert_eq!(msgs[0].bytes, vec![0x93, 64, 64]);
        assert_eq!(
            msgs[1],
            MidiMessage {
                at: 1.0,
                bytes: vec![0x83, 64, 0]
            }
        );
        let on_channel = keys(json!({"midinote": 64, "channel": 9}));
        assert_eq!(midi(&on_channel, 3).unwrap()[0].bytes[0], 0x99);
        assert!(midi(&keys(json!({"type": "osc", "addr": "/x"})), 0).is_err());
        assert!(midi(&keys(json!({"type": "rest"})), 0).unwrap().is_empty());
    }

    #[test]
    fn set_key_keeps_a_map_coherent() {
        let mut e = keys(json!({"freq": 440.0, "midinote": 69, "amp": 0.1}));
        set_key(&mut e, "midinote", json!(72));
        assert!((e["freq"].as_f64().unwrap() - 523.251_130_601_197_3).abs() < 1e-9);
        set_key(&mut e, "velocity", json!(127));
        assert_eq!(e["amp"], 1.0);
        set_key(&mut e, "label", json!("x"));
        assert_eq!(e["label"], "x");
        assert_eq!(e["midinote"], json!(72), "the key written keeps its value");

        let mut by_degree = keys(json!({"degree": 2, "midinote": 64}));
        set_key(&mut by_degree, "scale", json!([0, 2, 3, 5, 7, 8, 10]));
        assert_eq!(by_degree["midinote"], 63.0, "a new scale moves the note");
    }
}
