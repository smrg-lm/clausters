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
//! {"notes":    [[start, end, voice, {"control": value, ...}, "gate"|"free", joins], ...],
//!  "messages": [[position, "/address", args...], ...],
//!  "midi":     [[position, byte, byte, ...], ...],
//!  "ump":      [[position, word, word, ...], ...]}
//! ```
//!
//! Every list may be absent, and an empty one is not written. A note's
//! destination (`voice` above, its place on the wire) is a def's name, played
//! as a synth; `{"graph": id, "slot": name}`, one more of a slot of a running
//! graph instance, its controls the slot's ports -- a graph is how a note
//! carries more than its def: the curves that shape it, read beside it; or
//! `{"node": id}`, a note of a node that reads notes (a plugin's or a
//! `Voices` node), never a new one. `joins`, when written, is the index in
//! the list of the note this one continues: a legato line, which the
//! destination plays as one held voice.

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

/// Where a lane's note is played.
#[derive(Debug, Clone, PartialEq)]
pub enum EventLaneDestination {
    /// A note of a node that reads notes -- a plugin's or a `Voices` node --
    /// never a new node.
    Node(i32),
    /// A synth of this def (`/synth_new`), its controls the def's.
    Def(String),
    /// One more of `slot` in the graph instance `graph` (`/graph_addSlot`),
    /// its controls the slot's ports.
    Slot { graph: i32, slot: String },
}

/// A note: played at `destination` with `controls`, started at `start` and
/// released at `end`.
#[derive(Debug, Clone, PartialEq)]
pub struct EventLaneNote {
    pub start: u64,
    pub end: u64,
    pub destination: EventLaneDestination,
    /// The controls it starts with, by name.
    pub controls: Vec<(String, f64)>,
    pub release: Release,
    /// The note of the lane, by its index in the list, that this one
    /// continues: the line goes on rather than starting again.
    pub joins: Option<usize>,
}

/// A command the server takes in a timed bundle, run at `position` as written.
#[derive(Debug, Clone, PartialEq)]
pub struct EventLaneMessage {
    pub position: u64,
    pub addr: String,
    pub args: Vec<Arg>,
}

/// A MIDI message, played at `position` as though it had reached the server's
/// MIDI input then.
#[derive(Debug, Clone, PartialEq)]
pub struct EventLaneMidi {
    pub position: u64,
    pub bytes: Vec<u8>,
}

/// A MIDI 2.0 packet -- a Universal MIDI Packet's words -- played at
/// `position` as though it had reached the server's MIDI input then.
#[derive(Debug, Clone, PartialEq)]
pub struct EventLaneUmp {
    pub position: u64,
    pub words: Vec<u32>,
}

/// A lane's data whole.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventLaneData {
    pub notes: Vec<EventLaneNote>,
    pub messages: Vec<EventLaneMessage>,
    pub midi: Vec<EventLaneMidi>,
    pub ump: Vec<EventLaneUmp>,
}

impl EventLaneData {
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
                let destination = match &note.destination {
                    EventLaneDestination::Node(node) => json!({"node": node}),
                    EventLaneDestination::Def(def) => json!(def),
                    EventLaneDestination::Slot { graph, slot } => {
                        json!({"graph": graph, "slot": slot})
                    }
                };
                match note.joins {
                    Some(joins) => {
                        json!([note.start, note.end, destination, controls, release, joins])
                    }
                    None => json!([note.start, note.end, destination, controls, release]),
                }
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
        if !self.ump.is_empty() {
            let ump = self.ump.iter().map(|packet| {
                let mut entry = vec![json!(packet.position)];
                entry.extend(packet.words.iter().map(|w| json!(w)));
                Value::Array(entry)
            });
            out.insert("ump".into(), Value::Array(ump.collect()));
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
            let destination = match voice {
                Value::String(def) => EventLaneDestination::Def(def.clone()),
                Value::Object(node) if node.contains_key("node") => EventLaneDestination::Node(
                    node.get("node")
                        .and_then(Value::as_i64)
                        .and_then(|n| i32::try_from(n).ok())
                        .filter(|n| *n > 0)
                        .ok_or("a note on a node needs a positive node id")?,
                ),
                Value::Object(slot) => EventLaneDestination::Slot {
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
                _ => {
                    return Err("a note is played by a def's name, a graph's slot or a node".into());
                }
            };
            if matches!(destination, EventLaneDestination::Node(_)) {
                let values = controls
                    .as_object()
                    .ok_or("a node's note controls must be an object")?;
                // `key`, `velocity`, `channel` and `tuning` are the note's; any
                // other is one of the node's controls, set for this note.
                if values
                    .values()
                    .any(|v| v.as_f64().is_none_or(|v| !v.is_finite()))
                {
                    return Err("a node's note controls are numbers".into());
                }
                for (name, maximum) in [("key", 127.0), ("channel", 15.0)] {
                    if let Some(value) = values.get(name).and_then(Value::as_f64)
                        && (value.fract() != 0.0 || !(0.0..=maximum).contains(&value))
                    {
                        return Err(format!("invalid note {name}"));
                    }
                }
                if values
                    .get("velocity")
                    .and_then(Value::as_f64)
                    .is_some_and(|v| !(0.0..=1.0).contains(&v))
                {
                    return Err("invalid note velocity".into());
                }
            }
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
            let joins = match tail.get(1) {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .map(|j| j as usize)
                        .filter(|j| *j != data.notes.len())
                        .ok_or("a note joins another by its index")?,
                ),
            };
            data.notes.push(EventLaneNote {
                start: sample(start, "start")?,
                end: sample(end, "end")?,
                destination,
                controls,
                release,
                joins,
            });
        }
        if let Some(bad) = data
            .notes
            .iter()
            .filter_map(|n| n.joins)
            .find(|j| *j >= data.notes.len())
        {
            return Err(format!(
                "a note joins note {bad}, which the lane does not hold"
            ));
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
            data.messages.push(EventLaneMessage {
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
            data.midi.push(EventLaneMidi {
                position: sample(position, "a MIDI message's position")?,
                bytes,
            });
        }
        for packet in list("ump") {
            let fields = packet.as_array().ok_or("a MIDI 2.0 packet is an array")?;
            let [position, words @ ..] = fields.as_slice() else {
                return Err("a MIDI 2.0 packet is [position, words...]".into());
            };
            let words = words
                .iter()
                .map(|w| {
                    w.as_u64()
                        .and_then(|w| u32::try_from(w).ok())
                        .ok_or_else(|| "a packet's word is 32 bits".to_string())
                })
                .collect::<Result<Vec<u32>, String>>()?;
            data.ump.push(EventLaneUmp {
                position: sample(position, "a MIDI 2.0 packet's position")?,
                words,
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
        let data = EventLaneData {
            notes: vec![
                EventLaneNote {
                    start: 10,
                    end: 20,
                    destination: EventLaneDestination::Def("default".into()),
                    controls: vec![("amp".into(), 0.25), ("freq".into(), 440.0)],
                    release: Release::Free,
                    joins: None,
                },
                EventLaneNote {
                    start: 12,
                    end: 30,
                    destination: EventLaneDestination::Slot {
                        graph: 1000,
                        slot: "note.0".into(),
                    },
                    controls: vec![("freq".into(), 330.0)],
                    release: Release::Gate,
                    joins: Some(0),
                },
            ],
            messages: vec![EventLaneMessage {
                position: 5,
                addr: "/node_set".into(),
                args: vec![Arg::Int(3), Arg::Str("gate".into()), Arg::Float(1.0)],
            }],
            midi: vec![EventLaneMidi {
                position: 7,
                bytes: vec![0x90, 60, 100],
            }],
            ump: vec![EventLaneUmp {
                position: 9,
                words: vec![0x4090_3C00, 0xFFFF_0000],
            }],
        };
        let text = data.to_json().to_string();
        assert_eq!(EventLaneData::from_json(text.as_bytes()), Ok(data));
    }

    #[test]
    fn an_empty_list_is_not_written_and_an_absent_one_is_empty() {
        assert_eq!(EventLaneData::default().to_json(), json!({}));
        assert_eq!(
            EventLaneData::from_json(b"{}"),
            Ok(EventLaneData::default())
        );
    }

    #[test]
    fn a_byte_out_of_range_is_refused() {
        assert!(EventLaneData::from_json(br#"{"midi": [[0, 300]]}"#).is_err());
        assert!(EventLaneData::from_json(br#"{"midi": [[-1, 144, 60, 1]]}"#).is_err());
    }
}

#[cfg(test)]
mod node_tests {
    use super::*;
    #[test]
    fn a_note_on_a_node_and_a_join_round_trip() {
        let json = br#"{"notes":[[10,20,{"node":42},{"key":60,"velocity":0.5},"free"],
                                 [20,30,{"node":42},{"key":62,"velocity":0.5},"free",0]]}"#;
        let data = EventLaneData::from_json(json).unwrap();
        assert_eq!(data.notes[0].destination, EventLaneDestination::Node(42));
        assert_eq!(data.notes[1].joins, Some(0));
        assert_eq!(
            EventLaneData::from_json(&serde_json::to_vec(&data.to_json()).unwrap()).unwrap(),
            data
        );
    }

    #[test]
    fn a_join_outside_the_lane_or_onto_itself_is_refused() {
        assert!(EventLaneData::from_json(br#"{"notes":[[0,1,"x",{},"free",3]]}"#).is_err());
        assert!(EventLaneData::from_json(br#"{"notes":[[0,1,"x",{},"free",0]]}"#).is_err());
    }
}
