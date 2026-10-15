//! Event buses: notes and channel messages, routed as audio is.
//!
//! The server has three kinds of bus. An audio bus carries a block of
//! samples, a control bus one value, and an **event bus** the notes and
//! channel messages written to it during the block, each at its sample. A
//! node reads the bus its `notes` control names, as it reads the audio bus
//! its `in` names: a plugin forwards what it reads to its note port, in the
//! port's dialect, and a `Voices` node gives each note a voice. What writes a
//! bus is never a node -- the system MIDI input, an event lane and a client's
//! `/bus_event` -- so a reader always finds a slice's events written before
//! the slice runs.
//!
//! An event is normalized: MIDI 2.0 resolution, values in their natural
//! units, sixteen channels. Notes carry an identity (`note_id`, or `-1` for a
//! note named by its channel and key, as MIDI names one) and every event
//! carries its **source**, the writer it came from, so what one writer
//! started only that writer ends: a lane's notes released when the lane is
//! cleared, a transport's when it stops, while the keyboard's go on.
//!
//! This module is the event's shape, its sources and the JSON a client writes
//! it in; the bus itself is the engine's.

use serde_json::{Map, Value, json};

/// Channels a bus carries, as MIDI's.
pub const CHANNELS: i16 = 16;

/// What an event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Kind {
    /// A note starts: `channel`, `key`, `note_id`, `value` its velocity in
    /// `0..=1`. With `joins` it continues the note of that id instead -- a
    /// legato line, which a reader plays on the voice it already holds.
    #[default]
    NoteOn = 1,
    /// A note ends: by `note_id`, or by `channel` and `key` (the oldest such
    /// note of the source); `value` its release velocity.
    NoteOff = 2,
    /// Notes stop at once, without their release: every field that is not
    /// `-1` must match.
    Choke = 3,
    /// A note's expression (`index` an [`Expression`]): one note's, by
    /// `note_id` or `key`, or the channel's when both are `-1`.
    Expression = 4,
    /// A channel's controller `index` (0..=127), `value` in `0..=1`.
    Controller = 5,
    /// A channel's pitch bend, `value` in `-1..=1`.
    Bend = 6,
    /// A channel's pressure, `value` in `0..=1`.
    Pressure = 7,
    /// A channel's program change, `index` the program.
    Program = 8,
    /// One of the reader's own controls, `index` resolved by the server from
    /// a name: a note's (`note_id`), or the node's when it is `-1`. Only an
    /// event addressed to one node carries it (`/node_event`), since a
    /// control's index means something only to the node it belongs to.
    Control = 9,
}

/// A note's expression, numbered as the native note format numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Expression {
    /// Gain, `0..=4`, 1 unity.
    Volume = 0,
    /// Pan, `0..=1`, 0.5 the center.
    Pan = 1,
    /// Tuning, in semitones from the key.
    Tuning = 2,
    /// Vibrato depth, `0..=1`.
    Vibrato = 3,
    /// Expression, `0..=1`.
    Expression = 4,
    /// Brightness -- MPE's timbre, MIDI's controller 74 -- `0..=1`.
    Brightness = 5,
    /// Pressure, `0..=1`.
    Pressure = 6,
}

impl Expression {
    /// The names a client writes them by, in their numbering.
    pub const NAMES: [&'static str; 7] = [
        "volume",
        "pan",
        "tuning",
        "vibrato",
        "expression",
        "brightness",
        "pressure",
    ];

    /// The expression a name or a number names.
    #[must_use]
    pub fn from_u32(n: u32) -> Option<Self> {
        Some(match n {
            0 => Self::Volume,
            1 => Self::Pan,
            2 => Self::Tuning,
            3 => Self::Vibrato,
            4 => Self::Expression,
            5 => Self::Brightness,
            6 => Self::Pressure,
            _ => return None,
        })
    }
}

/// One event, as a bus holds it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BusEvent {
    /// The writer it came from ([`midi_source`], [`lane_source`],
    /// [`client_source`]).
    pub source: u128,
    /// Its sample within the block; the engine sets it when it is written.
    pub offset: u32,
    pub kind: Kind,
    /// `0..16`, or `-1` for every channel.
    pub channel: i16,
    /// `0..=127`, or `-1` for any key.
    pub key: i16,
    /// The writer's identity for the note, or `-1` for none.
    pub note_id: i32,
    /// The note a `NoteOn` continues, or `-1` when it starts one.
    pub joins: i32,
    /// The controller, expression, program or control, by its kind.
    pub index: u32,
    pub value: f64,
}

impl BusEvent {
    /// An event of `kind` from `source` with every address a wildcard.
    #[must_use]
    pub fn new(kind: Kind, source: u128) -> Self {
        Self {
            source,
            kind,
            channel: -1,
            key: -1,
            note_id: -1,
            joins: -1,
            ..Self::default()
        }
    }

    /// Whether `self`, a note held by its reader, is one `event` names: from
    /// its source, and every address `event` gives matching.
    #[must_use]
    pub fn named_by(&self, event: &BusEvent) -> bool {
        self.source == event.source
            && (event.channel == -1 || event.channel == self.channel)
            && (event.key == -1 || event.key == self.key)
            && (event.note_id == -1 || event.note_id == self.note_id)
    }
}

// ---- sources ----

/// The system MIDI input's sources, by channel; a zone's are `256 + master`.
const MIDI: u128 = 1 << 95;
/// An event lane's sources: lane, its data's generation and its transport.
const LANE: u128 = 1 << 96;
/// A client's sources, by the number it writes.
const CLIENT: u128 = 1 << 97;

/// The system MIDI input's source for `channel` (a zone's master is
/// `256 + master`).
#[must_use]
pub fn midi_source(channel: u16) -> u128 {
    MIDI | u128::from(channel)
}

/// An event lane's source: the lane, the generation of its data and the
/// transport it plays on, so a transport's stop releases its lanes' notes
/// and a lane's new data releases only that lane's.
#[must_use]
pub fn lane_source(lane: i32, generation: u32, transport: usize) -> u128 {
    LANE | ((transport as u128) << 64) | ((lane as u32 as u128) << 32) | u128::from(generation)
}

/// A client's source, by the number it writes (0 unless it says).
#[must_use]
pub fn client_source(n: u32) -> u128 {
    CLIENT | u128::from(n)
}

/// The lane a source is, when it is a lane's.
#[must_use]
pub fn lane_of(source: u128) -> Option<i32> {
    (source >> 96 == 1).then_some(((source >> 32) & 0xffff_ffff) as u32 as i32)
}

/// The transport a lane's source plays on.
#[must_use]
pub fn transport_of(source: u128) -> Option<usize> {
    (source >> 96 == 1).then_some(((source >> 64) & 0xffff_ffff) as usize)
}

// ---- JSON ----

/// One event as a client writes it, with a [`Kind::Control`]'s control
/// still by name: the server resolves it against the node it is sent to.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub event: BusEvent,
    pub control: Option<String>,
}

fn field_i64(map: &Map<String, Value>, key: &str, default: i64) -> Result<i64, String> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && n.fract() == 0.0)
            .map(|n| n as i64)
            .ok_or_else(|| format!("{key} must be an integer")),
    }
}

fn field_f64(map: &Map<String, Value>, key: &str, default: f64) -> Result<f64, String> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("{key} must be a finite number")),
    }
}

/// One event from its JSON object:
///
/// ```text
/// {"kind": "note_on", "channel": 0, "key": 60, "note_id": 7, "velocity": 0.8}
/// {"kind": "note_on", "key": 62, "note_id": 8, "joins": 7}
/// {"kind": "note_off", "note_id": 7}
/// {"kind": "choke", "channel": 0}
/// {"kind": "expression", "expression": "tuning", "note_id": 7, "value": 0.5}
/// {"kind": "controller", "channel": 0, "controller": 74, "value": 0.5}
/// {"kind": "bend", "channel": 0, "value": -0.25}
/// {"kind": "pressure", "channel": 0, "value": 0.4}
/// {"kind": "program", "channel": 0, "program": 3}
/// {"kind": "control", "note_id": 7, "control": "cutoff", "value": 900}
/// ```
///
/// Every address defaults to `-1`, a note-on's channel to 0, a velocity to
/// 1 on and 0 off, and `source` (a client's number) to 0.
///
/// # Errors
/// An unknown kind or expression, or a field out of its range.
pub fn read(value: &Value) -> Result<Written, String> {
    let map = value.as_object().ok_or("an event is an object")?;
    let kind = match map.get("kind").and_then(Value::as_str) {
        Some("note_on") => Kind::NoteOn,
        Some("note_off") => Kind::NoteOff,
        Some("choke") => Kind::Choke,
        Some("expression") => Kind::Expression,
        Some("controller") => Kind::Controller,
        Some("bend") => Kind::Bend,
        Some("pressure") => Kind::Pressure,
        Some("program") => Kind::Program,
        Some("control") => Kind::Control,
        Some(other) => return Err(format!("unknown event kind {other:?}")),
        None => return Err("an event names its kind".into()),
    };
    let source = u32::try_from(field_i64(map, "source", 0)?)
        .map_err(|_| "source is a client's number, 0 or more")?;
    let mut event = BusEvent::new(kind, client_source(source));
    let channel_default = if kind == Kind::NoteOn { 0 } else { -1 };
    event.channel = i16::try_from(field_i64(map, "channel", channel_default)?)
        .ok()
        .filter(|c| (-1..CHANNELS).contains(c))
        .ok_or("channel is 0-15, or -1 for every channel")?;
    event.key = i16::try_from(field_i64(map, "key", -1)?)
        .ok()
        .filter(|k| (-1..=127).contains(k))
        .ok_or("key is 0-127, or -1 for any")?;
    event.note_id = i32::try_from(field_i64(map, "note_id", -1)?)
        .ok()
        .filter(|n| *n >= -1)
        .ok_or("note_id is 0 or more, or -1 for none")?;
    event.joins = i32::try_from(field_i64(map, "joins", -1)?)
        .ok()
        .filter(|n| *n >= -1)
        .ok_or("joins is a note_id, or -1")?;
    let unit = |v: f64, what: &str| {
        if (0.0..=1.0).contains(&v) {
            Ok(v)
        } else {
            Err(format!("{what} is 0-1"))
        }
    };
    let mut control = None;
    match kind {
        Kind::NoteOn => {
            if event.key < 0 || event.channel < 0 {
                return Err("a note-on names its key and channel".into());
            }
            if event.joins >= 0 && event.note_id < 0 {
                return Err("a note that joins another names its own note_id".into());
            }
            event.value = unit(field_f64(map, "velocity", 1.0)?, "velocity")?;
        }
        Kind::NoteOff => event.value = unit(field_f64(map, "velocity", 0.0)?, "velocity")?,
        Kind::Choke => {}
        Kind::Expression => {
            let named = map.get("expression").ok_or("an expression names which")?;
            let which = match named {
                Value::String(name) => Expression::NAMES
                    .iter()
                    .position(|n| n == name)
                    .map(|i| i as u32),
                other => other.as_u64().and_then(|n| u32::try_from(n).ok()),
            }
            .and_then(Expression::from_u32)
            .ok_or_else(|| format!("unknown expression {named}"))?;
            event.index = which as u32;
            event.value = field_f64(map, "value", 0.0)?;
        }
        Kind::Controller => {
            event.index = u32::try_from(field_i64(map, "controller", -1)?)
                .ok()
                .filter(|c| *c < 128)
                .ok_or("controller is 0-127")?;
            event.value = unit(field_f64(map, "value", 0.0)?, "a controller's value")?;
        }
        Kind::Bend => {
            let v = field_f64(map, "value", 0.0)?;
            if !(-1.0..=1.0).contains(&v) {
                return Err("a bend is -1 to 1".into());
            }
            event.value = v;
        }
        Kind::Pressure => event.value = unit(field_f64(map, "value", 0.0)?, "pressure")?,
        Kind::Program => {
            event.index = u32::try_from(field_i64(map, "program", -1)?)
                .ok()
                .filter(|p| *p < 128)
                .ok_or("program is 0-127")?;
        }
        Kind::Control => {
            control = Some(
                map.get("control")
                    .and_then(Value::as_str)
                    .ok_or("a control event names its control")?
                    .to_string(),
            );
            event.value = field_f64(map, "value", 0.0)?;
        }
    }
    Ok(Written { event, control })
}

/// The events a `/bus_event` or `/node_event` carries: one object, or a list.
///
/// # Errors
/// Text that is not JSON, or an event [`read`] refuses.
pub fn read_all(json: &[u8]) -> Result<Vec<Written>, String> {
    let value: Value = serde_json::from_slice(json).map_err(|e| format!("invalid JSON: {e}"))?;
    match &value {
        Value::Array(list) => list.iter().map(read).collect(),
        one => Ok(vec![read(one)?]),
    }
}

/// An event as [`read`] reads it (its source as a client's number, which
/// only a client's is).
#[must_use]
pub fn write(written: &Written) -> Value {
    let e = &written.event;
    let mut map = Map::new();
    let name = match e.kind {
        Kind::NoteOn => "note_on",
        Kind::NoteOff => "note_off",
        Kind::Choke => "choke",
        Kind::Expression => "expression",
        Kind::Controller => "controller",
        Kind::Bend => "bend",
        Kind::Pressure => "pressure",
        Kind::Program => "program",
        Kind::Control => "control",
    };
    map.insert("kind".into(), json!(name));
    if e.source & CLIENT != 0 && e.source as u32 != 0 {
        map.insert("source".into(), json!(e.source as u32));
    }
    for (key, v) in [
        ("channel", i64::from(e.channel)),
        ("key", i64::from(e.key)),
        ("note_id", i64::from(e.note_id)),
        ("joins", i64::from(e.joins)),
    ] {
        if v != -1 {
            map.insert(key.into(), json!(v));
        }
    }
    match e.kind {
        Kind::NoteOn | Kind::NoteOff => {
            map.insert("velocity".into(), json!(e.value));
        }
        Kind::Choke => {}
        Kind::Expression => {
            map.insert(
                "expression".into(),
                json!(Expression::NAMES[e.index as usize]),
            );
            map.insert("value".into(), json!(e.value));
        }
        Kind::Controller => {
            map.insert("controller".into(), json!(e.index));
            map.insert("value".into(), json!(e.value));
        }
        Kind::Program => {
            map.insert("program".into(), json!(e.index));
        }
        Kind::Bend | Kind::Pressure => {
            map.insert("value".into(), json!(e.value));
        }
        Kind::Control => {
            map.insert(
                "control".into(),
                json!(written.control.clone().unwrap_or_default()),
            );
            map.insert("value".into(), json!(e.value));
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_reads_back_what_it_writes() {
        let text = r#"[
            {"kind": "note_on", "channel": 2, "key": 60, "note_id": 7, "velocity": 0.5},
            {"kind": "note_on", "channel": 0, "key": 62, "note_id": 8, "joins": 7, "velocity": 1.0},
            {"kind": "note_off", "note_id": 7, "velocity": 0.25},
            {"kind": "choke", "channel": 3},
            {"kind": "expression", "expression": "tuning", "note_id": 8, "value": -0.5},
            {"kind": "controller", "channel": 0, "controller": 74, "value": 0.75},
            {"kind": "bend", "channel": 1, "value": -1.0},
            {"kind": "pressure", "channel": 1, "value": 0.5},
            {"kind": "program", "channel": 0, "program": 3},
            {"kind": "control", "note_id": 8, "control": "cutoff", "value": 900.0, "source": 4}
        ]"#;
        let events = read_all(text.as_bytes()).unwrap();
        assert_eq!(events.len(), 10);
        let again: Vec<Value> = events.iter().map(write).collect();
        assert_eq!(
            read_all(Value::from(again).to_string().as_bytes()).unwrap(),
            events
        );
        assert_eq!(events[1].event.joins, 7);
        assert_eq!(events[4].event.index, Expression::Tuning as u32);
        assert_eq!(events[9].control.as_deref(), Some("cutoff"));
        assert_eq!(events[9].event.source, client_source(4));
    }

    #[test]
    fn an_event_out_of_its_range_is_refused() {
        for bad in [
            r#"{"kind": "note_on", "key": 128}"#,
            r#"{"kind": "note_on", "key": 60, "velocity": 2}"#,
            r#"{"kind": "note_on", "key": 60, "joins": 3}"#,
            r#"{"kind": "controller", "controller": 200}"#,
            r#"{"kind": "bend", "value": 3}"#,
            r#"{"kind": "expression", "expression": "wobble"}"#,
            r#"{"kind": "control", "value": 1}"#,
            r#"{"kind": "unknown"}"#,
            r#"{"kind": "choke", "channel": 16}"#,
        ] {
            assert!(read_all(bad.as_bytes()).is_err(), "{bad} was read");
        }
    }

    #[test]
    fn a_held_note_is_named_by_its_source_and_every_address_given() {
        let held = BusEvent {
            channel: 1,
            key: 60,
            note_id: 9,
            ..BusEvent::new(Kind::NoteOn, midi_source(1))
        };
        let mut off = BusEvent::new(Kind::NoteOff, midi_source(1));
        assert!(held.named_by(&off));
        off.key = 60;
        assert!(held.named_by(&off));
        off.key = 61;
        assert!(!held.named_by(&off));
        off.key = -1;
        off.source = midi_source(2);
        assert!(!held.named_by(&off));
    }

    #[test]
    fn a_lane_source_says_its_lane_and_transport() {
        let source = lane_source(-3, 12, 2);
        assert_eq!(lane_of(source), Some(-3));
        assert_eq!(transport_of(source), Some(2));
        assert_eq!(lane_of(midi_source(3)), None);
        assert_eq!(lane_of(client_source(3)), None);
    }
}
