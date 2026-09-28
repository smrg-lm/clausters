//! A sequence of events: what a notes editor edits, and what a timeline becomes
//! once it is rendered.
//!
//! # What an event is here
//!
//! An event is its **keys** -- a note's `midinote`, `sustain` and `amp`, a MIDI
//! message's `midicmd`, an OSC message's `addr` -- and what those keys mean is
//! `clausters_core::event`'s: the pitch and level families, their coherence,
//! the `type` that says what the event is and how it renders for a
//! destination. This module holds events and places them; it reads their keys
//! only through the core, when an edit writes one key of a family
//! ([`EventsIntent::Set`]) and when it measures how long a note sounds.
//!
//! # Why an event has an id
//!
//! A sequence is edited event by event -- a note moved, one key of it changed,
//! one removed -- and an edit that names its event by position hands the next
//! note the data of the one removed before it. So an event has an identity of
//! its own, minted here the first time the sequence holds it and kept across
//! every edit, the way a multitrack's region keeps its own. An event that
//! arrives with none ([`EventsIntent::SetEvents`] from a caller that only has a
//! list) takes the id of the unchanged event it matches, else a new one, so a
//! resend is still not an edit.
//!
//! # What else it holds
//!
//! - **Beats.** An event's `at` is in beats, the sequence's own logical axis,
//!   and the **tempo map** that turns them into seconds travels with it as data
//!   -- what a `.mid` carries, and what keeps a rendered timeline's time from
//!   being lost.
//! - **Curve lanes**: a CC, bend or pressure curve over the whole sequence,
//!   each a [`multitrack::Automation`](crate::multitrack::Automation) whose
//!   points are on the sequence's beats.
//! - **A note's own expression**: curves hung on one event, their points in
//!   beats from the note's start, as a region's automation is measured from the
//!   region's -- which is where MPE's per-note bend and pressure will live.
//!
//! A sequence is also a source a session can hold
//! ([`Location::Events`](crate::session::Location::Events)), so a multitrack's
//! region can be a window onto one.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use clausters_core::event::render;
use clausters_core::tempomap::TempoMap;

use crate::Opaque;
use crate::history::{Applied, Editable};
use crate::multitrack::{Automation, Extra};
use crate::timebase::Beat;

/// The domain name an event sequence is registered under.
pub const EVENTS: &str = "events";

/// One event: its identity, where it sits, and its keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Its identity in the sequence. `0` is "not yet held": the sequence mints
    /// one when the event arrives.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub id: u64,
    /// Where it sits, in the sequence's beats.
    pub at: Beat,
    /// Its keys, as the event spells them.
    #[serde(default, skip_serializing_if = "Opaque::is_empty")]
    pub data: Opaque,
    /// Curves over this event alone, their points in beats from its start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expression: Vec<Automation>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Event {
    /// An event at `at` with these keys and no identity yet.
    pub fn new(at: f64, data: Value) -> Self {
        Self {
            id: 0,
            at: Beat(at),
            data: Opaque(data),
            expression: Vec::new(),
        }
    }

    /// The event's keys, or an empty map when its data is not one.
    pub fn keys(&self) -> Map<String, Value> {
        self.data.0.as_object().cloned().unwrap_or_default()
    }

    /// How long it sounds, in beats (the core's reading of its keys).
    pub fn sustain(&self) -> f64 {
        render::sustain_of(&self.keys())
    }

    /// Whether two events say the same thing in the same place, identity aside.
    fn same(&self, other: &Self) -> bool {
        self.at == other.at && self.data == other.data && self.expression == other.expression
    }
}

/// A sequence of events in beats, with the tempo map that times them and the
/// curves beside them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Written")]
pub struct EventSequence {
    /// The events, in the order of their `at` (stable among equals).
    pub events: Vec<Event>,
    /// The beat-to-second map, when the sequence states one; without it a beat
    /// is whatever the player's clock makes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo_map: Option<TempoMap>,
    /// Curves over the whole sequence -- CC, bend, pressure -- on its beats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<Automation>,
    /// The last id minted. Kept, so an id is never handed out twice, not even
    /// to an event that comes back after its first holder was removed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_id: u64,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Extra,
}

/// What a sequence is read from: the object it writes, or a bare list of
/// events, which is how a timeline's events were written before a sequence
/// had anything beside them.
#[derive(Deserialize)]
#[serde(untagged)]
enum Written {
    List(Vec<Event>),
    Whole {
        #[serde(default)]
        events: Vec<Event>,
        #[serde(default)]
        tempo_map: Option<TempoMap>,
        #[serde(default)]
        lanes: Vec<Automation>,
        #[serde(default)]
        next_id: u64,
        #[serde(flatten, default)]
        extra: Extra,
    },
}

impl From<Written> for EventSequence {
    fn from(written: Written) -> Self {
        let mut sequence = match written {
            Written::List(events) => Self {
                events,
                ..Self::default()
            },
            Written::Whole {
                events,
                tempo_map,
                lanes,
                next_id,
                extra,
            } => Self {
                events,
                tempo_map,
                lanes,
                next_id,
                extra,
            },
        };
        sequence.hold();
        sequence
    }
}

/// An edit to a sequence, naming its event by id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "lowercase")]
pub enum EventsIntent {
    /// What the sequence's events are now, whole. An event with an id keeps
    /// it; one without takes the id of an unchanged event it matches, else a
    /// new one. The tempo map and the lanes are left as they are.
    SetEvents {
        /// The events, in any order.
        events: Vec<Event>,
    },
    /// One event more. Its id is minted when it has none, and the answer says
    /// which.
    Add {
        /// The event.
        event: Event,
    },
    /// One event fewer.
    Remove {
        /// Which.
        id: u64,
    },
    /// An event to another beat.
    Move {
        /// Which.
        id: u64,
        /// Where it goes.
        at: Beat,
    },
    /// One key of an event, written with its family's coherence: a moved
    /// `midinote` moves the `freq` and the `degree` the event holds.
    Set {
        /// Which event.
        id: u64,
        /// The key.
        key: String,
        /// Its value.
        value: Value,
    },
    /// An event's keys, whole.
    Keys {
        /// Which event.
        id: u64,
        /// What its keys are now.
        data: Opaque,
    },
    /// The tempo map, or none.
    Tempo {
        /// The map.
        #[serde(default)]
        tempo_map: Option<TempoMap>,
    },
    /// The sequence as it was: what every edit's inverse is.
    Restore {
        /// All of it.
        sequence: Box<EventSequence>,
    },
}

impl EventSequence {
    /// A sequence over these events, each given an id.
    pub fn new(events: Vec<Event>) -> Self {
        let mut sequence = Self {
            events,
            ..Self::default()
        };
        sequence.hold();
        sequence
    }

    /// Gives every event that has no id one, and keeps the events in beat
    /// order.
    fn hold(&mut self) {
        let highest = self.events.iter().map(|e| e.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(highest);
        for event in &mut self.events {
            if event.id == 0 {
                self.next_id += 1;
                event.id = self.next_id;
            }
        }
        self.sort();
    }

    fn sort(&mut self) {
        self.events.sort_by(|a, b| a.at.0.total_cmp(&b.at.0));
    }

    fn mint(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The event with this id.
    pub fn get(&self, id: u64) -> Option<&Event> {
        self.events.iter().find(|e| e.id == id)
    }

    fn index(&self, id: u64) -> Option<usize> {
        self.events.iter().position(|e| e.id == id)
    }

    /// Where the last event stops sounding, in beats; 0 for none.
    pub fn duration(&self) -> f64 {
        self.events
            .iter()
            .map(|e| e.at.0 + e.sustain().max(0.0))
            .fold(0.0, f64::max)
    }

    /// The edit that puts the sequence back as it is now.
    pub fn state(&self) -> EventsIntent {
        EventsIntent::Restore {
            sequence: Box::new(self.clone()),
        }
    }

    /// The events `incoming` states, each with an id: its own, the id of an
    /// unchanged event it matches, or a new one.
    fn identify(&mut self, incoming: Vec<Event>) -> Vec<Event> {
        let mut taken = vec![false; self.events.len()];
        for event in &incoming {
            if let Some(i) = self.index(event.id).filter(|_| event.id != 0) {
                taken[i] = true;
            }
        }
        let mut out = Vec::with_capacity(incoming.len());
        for mut event in incoming {
            if event.id == 0 {
                let matched = self
                    .events
                    .iter()
                    .enumerate()
                    .find(|(i, held)| !taken[*i] && held.same(&event))
                    .map(|(i, held)| (i, held.id));
                event.id = match matched {
                    Some((i, id)) => {
                        taken[i] = true;
                        id
                    }
                    None => self.mint(),
                };
            }
            out.push(event);
        }
        out
    }

    /// Applies one edit, and says what changed. Also what [`Editable::apply`]
    /// answers through.
    pub fn edit(&mut self, intent: EventsIntent) -> Result<Change, String> {
        let before = self.clone();
        let mut added = None;
        match intent {
            EventsIntent::SetEvents { events } => {
                self.events = self.identify(events);
                self.sort();
            }
            EventsIntent::Add { mut event } => {
                if event.id == 0 || self.index(event.id).is_some() {
                    event.id = self.mint();
                } else {
                    self.next_id = self.next_id.max(event.id);
                }
                added = Some(event.id);
                // After every event at the same beat, so an add at a shared
                // onset keeps the order it was made in.
                let at = self.events.partition_point(|e| e.at.0 <= event.at.0);
                self.events.insert(at, event);
            }
            EventsIntent::Remove { id } => {
                let i = self.index(id).ok_or_else(|| no_event(id))?;
                self.events.remove(i);
            }
            EventsIntent::Move { id, at } => {
                let i = self.index(id).ok_or_else(|| no_event(id))?;
                let mut event = self.events.remove(i);
                event.at = at;
                let at = self.events.partition_point(|e| e.at.0 <= event.at.0);
                self.events.insert(at, event);
            }
            EventsIntent::Set { id, key, value } => {
                let i = self.index(id).ok_or_else(|| no_event(id))?;
                let mut keys = self.events[i].keys();
                render::set_key(&mut keys, &key, value);
                self.events[i].data = Opaque(Value::Object(keys));
            }
            EventsIntent::Keys { id, data } => {
                let i = self.index(id).ok_or_else(|| no_event(id))?;
                self.events[i].data = data;
            }
            EventsIntent::Tempo { tempo_map } => self.tempo_map = tempo_map,
            EventsIntent::Restore { sequence } => {
                // Everything as it was but the counter, which only climbs: an
                // id a later edit could still name is never handed out again.
                let next_id = self.next_id.max(sequence.next_id);
                *self = *sequence;
                self.next_id = next_id;
            }
        }
        Ok(Change {
            applied: *self != before,
            added,
        })
    }
}

fn no_event(id: u64) -> String {
    format!("the sequence holds no event {id}")
}

/// What an edit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// Whether the sequence changed.
    pub applied: bool,
    /// The id an [`EventsIntent::Add`] gave its event.
    pub added: Option<u64>,
}

/// A sequence's edit as a history carries it.
pub fn payload(intent: &EventsIntent) -> Opaque {
    Opaque(serde_json::to_value(intent).unwrap_or(Value::Null))
}

impl Editable for EventSequence {
    fn apply(&mut self, payload: &Opaque) -> Applied {
        let Ok(intent) = serde_json::from_value::<EventsIntent>(payload.0.clone()) else {
            return Applied::refused(
                crate::events::payload(&self.state()),
                "not an edit written in this sequence's vocabulary",
            );
        };
        match self.edit(intent) {
            Ok(change) => Applied {
                effective: crate::events::payload(&self.state()),
                applied: change.applied,
                reason: None,
                stale: false,
            },
            Err(reason) => Applied::refused(crate::events::payload(&self.state()), reason),
        }
    }

    fn current(&self, _payload: &Opaque) -> Option<Opaque> {
        Some(crate::events::payload(&self.state()))
    }

    fn coalesce_key(&self, payload: &Opaque) -> Option<String> {
        coalesce_key(payload)
    }
}

/// What makes two edits to a sequence *the same thing done the same way*: the
/// same verb on the same event -- a note dragged across the grid is one undo
/// when the caller says the hand did not stop. A whole-list edit names no
/// event, so any two coalesce, as they always have.
pub fn coalesce_key(payload: &Opaque) -> Option<String> {
    let intent = serde_json::from_value::<EventsIntent>(payload.0.clone()).ok()?;
    Some(match intent {
        EventsIntent::Move { id, .. } => format!("{EVENTS}:move:{id}"),
        EventsIntent::Set { id, key, .. } => format!("{EVENTS}:set:{id}:{key}"),
        EventsIntent::Keys { id, .. } => format!("{EVENTS}:keys:{id}"),
        EventsIntent::Tempo { .. } => format!("{EVENTS}:tempo"),
        EventsIntent::SetEvents { .. } | EventsIntent::Restore { .. } => EVENTS.to_string(),
        EventsIntent::Add { .. } | EventsIntent::Remove { .. } => return None,
    })
}

/// **One verb of a sequence, as JSON** -- the door a client's handle speaks
/// through, the same over the C ABI and wasm. `request` is `{"verb": ...}`:
///
/// - `"state"`: the sequence, whole.
/// - `"len"`: `{"len": n}`.
/// - `"duration"`: `{"duration": beats}`, where the last event stops sounding.
/// - `"event"` with `id`: the event, or `null`.
/// - `"apply"` with `intent`: the edit applied, answering `{"applied",
///   "current"}` -- `current` the payload that puts it back, read before the
///   edit -- plus `"id"` for an add, or `{"error"}` when refused.
///
/// A request that does not read answers `{"error": ...}`.
pub fn call_json(sequence: &mut EventSequence, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return json!({"error": "the request is not JSON"}).to_string();
    };
    let answer = match request.get("verb").and_then(Value::as_str) {
        Some("state") => serde_json::to_value(&*sequence).unwrap_or(Value::Null),
        Some("len") => json!({"len": sequence.events.len()}),
        Some("duration") => json!({"duration": sequence.duration()}),
        Some("event") => {
            let id = request.get("id").and_then(Value::as_u64).unwrap_or(0);
            serde_json::to_value(sequence.get(id)).unwrap_or(Value::Null)
        }
        Some("apply") => {
            let intent = request.get("intent").cloned().unwrap_or(Value::Null);
            match serde_json::from_value::<EventsIntent>(intent) {
                Ok(intent) => {
                    let current = payload(&sequence.state());
                    match sequence.edit(intent) {
                        Ok(change) => {
                            let mut answer =
                                json!({"applied": change.applied, "current": current.0});
                            if let Some(id) = change.added {
                                answer["id"] = json!(id);
                            }
                            answer
                        }
                        Err(error) => json!({ "error": error }),
                    }
                }
                Err(error) => json!({"error": format!("not an edit of a sequence: {error}")}),
            }
        }
        other => json!({"error": format!("no sequence verb {other:?}")}),
    };
    answer.to_string()
}

#[cfg(test)]
mod tests;
