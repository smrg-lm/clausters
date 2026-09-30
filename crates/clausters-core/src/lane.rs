//! An event lane's data: what `/lane_set` carries.
//!
//! A lane holds what one transport plays as data -- notes, server commands
//! and MIDI messages -- every position in samples of that transport. A client
//! writes it from a sequence placed on the transport's axis and the server
//! reads it, so the shape is one type here and both sides go through it: a
//! field that moves on one side moves on the other.
//!
//! On the wire it is JSON, each entry an array so a long lane stays compact:
//!
//! ```text
//! {"notes":    [[start, end, voice, {"control": value, ...}, "gate"|"free"], ...],
//!  "messages": [[position, "/address", args...], ...],
//!  "midi":     [[position, byte, byte, ...], ...]}
//! ```
//!
//! Every list may be absent, and an empty one is not written. A note's `voice`
//! is a def's name, played as a synth, or `{"graph": id, "slot": name}`, one
//! more of a slot of a running graph instance, its controls the slot's ports.
//! A graph is how a note carries more than its def: the curves that shape it,
//! read beside it.

use serde_json::{Map, Value, json};

use crate::event::render::Arg;

/// How a lane's note is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// `/node_set <id> gate 0`: the def's envelope releases it.
    Gate,
    /// `/node_free <id>`.
    Free,
}

/// What a lane's note is made as.
#[derive(Debug, Clone, PartialEq)]
pub enum LaneVoice {
    /// A synth of this def (`/synth_new`), its controls the def's.
    Def(String),
    /// One more of `slot` in the graph instance `graph` (`/graph_addSlot`),
    /// its controls the slot's ports.
    Slot { graph: i32, slot: String },
}

/// A note: a voice with `controls`, started at `start` and released at `end`.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneNote {
    pub start: u64,
    pub end: u64,
    pub voice: LaneVoice,
    /// The controls it starts with, by name.
    pub controls: Vec<(String, f64)>,
    pub release: Release,
}

/// A command the server takes in a timed bundle, run at `position` as written.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneMessage {
    pub position: u64,
    pub addr: String,
    pub args: Vec<Arg>,
}

/// A MIDI message, played at `position` as though it had reached the server's
/// MIDI input then.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneMidi {
    pub position: u64,
    pub bytes: Vec<u8>,
}

/// A lane's data whole.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LaneData {
    pub notes: Vec<LaneNote>,
    pub messages: Vec<LaneMessage>,
    pub midi: Vec<LaneMidi>,
}

impl LaneData {
    /// The data as `/lane_set` carries it.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        if !self.notes.is_empty() {
            let notes = self.notes.iter().map(|note| {
                let controls: Map<String, Value> = note
                    .controls
                    .iter()
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect();
                let release = match note.release {
                    Release::Gate => "gate",
                    Release::Free => "free",
                };
                let voice = match &note.voice {
                    LaneVoice::Def(def) => json!(def),
                    LaneVoice::Slot { graph, slot } => json!({"graph": graph, "slot": slot}),
                };
                json!([note.start, note.end, voice, controls, release])
            });
            out.insert("notes".into(), Value::Array(notes.collect()));
        }
        if !self.messages.is_empty() {
            let messages = self.messages.iter().map(|message| {
                let mut entry = vec![json!(message.position), json!(message.addr)];
                entry.extend(message.args.iter().map(|arg| match arg {
                    Arg::Str(s) => json!(s),
                    Arg::Int(i) => json!(i),
                    Arg::Float(f) => json!(f64::from(*f)),
                }));
                Value::Array(entry)
            });
            out.insert("messages".into(), Value::Array(messages.collect()));
        }
        if !self.midi.is_empty() {
            let midi = self.midi.iter().map(|midi| {
                let mut entry = vec![json!(midi.position)];
                entry.extend(midi.bytes.iter().map(|b| json!(b)));
                Value::Array(entry)
            });
            out.insert("midi".into(), Value::Array(midi.collect()));
        }
        Value::Object(out)
    }

    /// Reads the data `/lane_set` carries.
    ///
    /// # Errors
    /// Text that is not JSON, or an entry that is not the shape above.
    pub fn from_json(json: &[u8]) -> Result<Self, String> {
        let value: Value =
            serde_json::from_slice(json).map_err(|e| format!("invalid JSON: {e}"))?;
        let list = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        };
        let mut data = Self::default();
        for note in list("notes") {
            let fields = note.as_array().ok_or("a note is an array")?;
            let [start, end, voice, controls, tail @ ..] = fields.as_slice() else {
                return Err("a note is [start, end, voice, controls, release]".into());
            };
            let voice = match voice {
                Value::String(def) => LaneVoice::Def(def.clone()),
                Value::Object(slot) => LaneVoice::Slot {
                    graph: slot
                        .get("graph")
                        .and_then(Value::as_i64)
                        .and_then(|g| i32::try_from(g).ok())
                        .ok_or("a note's graph is a node id")?,
                    slot: slot
                        .get("slot")
                        .and_then(Value::as_str)
                        .ok_or("a note's slot is a name")?
                        .to_string(),
                },
                _ => return Err("a note's voice is a def's name or a graph's slot".into()),
            };
            let controls = controls
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(name, value)| value.as_f64().map(|v| (name.clone(), v)))
                .collect();
            let release = match tail.first().and_then(Value::as_str) {
                None | Some("gate") => Release::Gate,
                Some("free") => Release::Free,
                Some(other) => {
                    return Err(format!("a note is released by gate or free, not {other}"));
                }
            };
            data.notes.push(LaneNote {
                start: sample(start, "start")?,
                end: sample(end, "end")?,
                voice,
                controls,
                release,
            });
        }
        for message in list("messages") {
            let fields = message.as_array().ok_or("a message is an array")?;
            let [position, addr, args @ ..] = fields.as_slice() else {
                return Err("a message is [position, address, args...]".into());
            };
            let addr = addr.as_str().ok_or("a message's address is a string")?;
            let args = args
                .iter()
                .map(|a| match a {
                    Value::Number(n) if n.is_i64() => Arg::Int(n.as_i64().unwrap_or(0) as i32),
                    Value::Number(n) => Arg::Float(n.as_f64().unwrap_or(0.0) as f32),
                    Value::String(s) => Arg::Str(s.clone()),
                    Value::Bool(b) => Arg::Int(i32::from(*b)),
                    other => Arg::Str(other.to_string()),
                })
                .collect();
            data.messages.push(LaneMessage {
                position: sample(position, "a message's position")?,
                addr: addr.to_string(),
                args,
            });
        }
        for midi in list("midi") {
            let fields = midi.as_array().ok_or("a MIDI message is an array")?;
            let [position, bytes @ ..] = fields.as_slice() else {
                return Err("a MIDI message is [position, bytes...]".into());
            };
            let bytes = bytes
                .iter()
                .map(|b| {
                    b.as_u64()
                        .and_then(|b| u8::try_from(b).ok())
                        .ok_or_else(|| "a MIDI byte is 0-255".to_string())
                })
                .collect::<Result<Vec<u8>, String>>()?;
            data.midi.push(LaneMidi {
                position: sample(position, "a MIDI message's position")?,
                bytes,
            });
        }
        Ok(data)
    }
}

/// A position: a finite number >= 0, rounded to a sample.
fn sample(v: &Value, what: &str) -> Result<u64, String> {
    v.as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n.round() as u64)
        .ok_or_else(|| format!("{what} must be a sample >= 0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_reads_back_what_it_writes() {
        let data = LaneData {
            notes: vec![
                LaneNote {
                    start: 10,
                    end: 20,
                    voice: LaneVoice::Def("default".into()),
                    controls: vec![("amp".into(), 0.25), ("freq".into(), 440.0)],
                    release: Release::Free,
                },
                LaneNote {
                    start: 12,
                    end: 30,
                    voice: LaneVoice::Slot {
                        graph: 1000,
                        slot: "note.0".into(),
                    },
                    controls: vec![("freq".into(), 330.0)],
                    release: Release::Gate,
                },
            ],
            messages: vec![LaneMessage {
                position: 5,
                addr: "/node_set".into(),
                args: vec![Arg::Int(3), Arg::Str("gate".into()), Arg::Float(1.0)],
            }],
            midi: vec![LaneMidi {
                position: 7,
                bytes: vec![0x90, 60, 100],
            }],
        };
        let text = data.to_json().to_string();
        assert_eq!(LaneData::from_json(text.as_bytes()), Ok(data));
    }

    #[test]
    fn an_empty_list_is_not_written_and_an_absent_one_is_empty() {
        assert_eq!(LaneData::default().to_json(), json!({}));
        assert_eq!(LaneData::from_json(b"{}"), Ok(LaneData::default()));
    }

    #[test]
    fn a_byte_out_of_range_is_refused() {
        assert!(LaneData::from_json(br#"{"midi": [[0, 300]]}"#).is_err());
        assert!(LaneData::from_json(br#"{"midi": [[-1, 144, 60, 1]]}"#).is_err());
    }
}
