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
//! - **Its automation**: a CC, bend or pressure curve over the whole sequence,
//!   each a [`multitrack::Automation`](crate::multitrack::Automation) whose
//!   points are on the sequence's beats -- a channel's function, as MIDI's are,
//!   on the one channel its target names (`channel`) or on every one.
//! - **A note's own automation**: curves hung on one event, their points in
//!   beats from the note's start, as a region's automation is measured from the
//!   region's -- which is where MPE's per-note bend and pressure live. Unlike
//!   a region's, a note's curve may run past its end: a note's end is its
//!   release, not its silence.
//!
//! A sequence is also a source a session can hold
//! ([`Location::Events`](crate::session::Location::Events)), so a multitrack's
//! region can be a window onto one.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use clausters_core::event::render;
use clausters_core::tempomap::TempoMap;

use crate::NodeId;
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
    /// Read under `expression` too, its name before it was the multitrack's.
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "expression")]
    pub automation: Vec<Automation>,
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
            automation: Vec::new(),
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
        self.at == other.at && self.data == other.data && self.automation == other.automation
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
    pub automation: Vec<Automation>,
    /// **Which MIDI specification it is written for**, or none: a sequence
    /// for the server, where every curve is legal. See [`MidiSpec`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiSpec>,
    /// **What a score says that is not any note's**, when the sequence was
    /// rendered from one: the meter and its changes, the key, the clefs, the
    /// barlines and breaks, the page and its text -- and what has two ends, a
    /// slur or a hairpin, which names them by event id. An event says what
    /// its own note is on a page with the notation keys
    /// (`clausters_core::event::notation`); this is the rest, kept with the
    /// sequence so a score rendered into one loses nothing on the way.
    ///
    /// It is held as it was written and not read here: the score's model is
    /// the notation layer's (`clausters_core::notation`, a feature this crate
    /// does not ask for), and nothing a sequence does -- playing, editing an
    /// event, drawing a roll -- depends on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notation: Option<Value>,
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
        /// Read under `lanes` too, its name before it was the multitrack's.
        #[serde(default, alias = "lanes")]
        automation: Vec<Automation>,
        #[serde(default)]
        midi: Option<MidiSpec>,
        #[serde(default)]
        notation: Option<Value>,
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
                automation,
                midi,
                notation,
                next_id,
                extra,
            } => Self {
                events,
                tempo_map,
                automation,
                midi,
                notation,
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
    /// new one. The tempo map and the automation are left as they are.
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
    /// A curve over the whole sequence (a CC, a bend, a pressure, a control),
    /// whole: it replaces the sequence's curve of its id, or is one curve
    /// more -- its id minted when it has none, and the answer says which.
    Automation {
        /// The curve, its points on the sequence's beats.
        automation: Automation,
    },
    /// One of the sequence's curves fewer.
    RemoveAutomation {
        /// Which.
        curve: NodeId,
    },
    /// A curve over one event, whole: it replaces that event's curve of its
    /// id, or is one more -- its id minted when it has none.
    EventAutomation {
        /// Which event.
        id: u64,
        /// The curve, its points in beats from the event's start.
        automation: Automation,
    },
    /// One of an event's curves fewer.
    RemoveEventAutomation {
        /// Which event.
        id: u64,
        /// Which of its curves.
        curve: NodeId,
    },
    /// The MIDI specification the sequence is written for, or none. Refused
    /// when a curve it holds has no spelling in that spec.
    Midi {
        /// The spec.
        #[serde(default)]
        midi: Option<MidiSpec>,
    },
    /// A curve of the sequence given to the notes it reaches: each note takes
    /// the stretch of the curve its span covers as a curve of its own, and
    /// the sequence's goes. See `scopes`.
    AutomationToEvents {
        /// Which curve.
        curve: NodeId,
    },
    /// The notes' curves over `target` gathered into one curve of the
    /// sequence -- of the notes on `channel`, or of every note -- the answer
    /// saying which. Refused where two notes that sound at once have
    /// different curves.
    EventsToAutomation {
        /// What the curves drive, as a note's curve names it.
        target: Opaque,
        /// The notes' channel, or none for every note.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<i64>,
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
        // Events and curves draw on one counter, so no id is ever two things.
        let highest = self
            .events
            .iter()
            .map(|e| e.id)
            .chain(self.automation.iter().map(|a| a.id.0))
            .chain(
                self.events
                    .iter()
                    .flat_map(|e| e.automation.iter().map(|a| a.id.0)),
            )
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(highest);
        for event in &mut self.events {
            if event.id == 0 {
                self.next_id += 1;
                event.id = self.next_id;
            }
        }
        // A curve read with no id (a file's stream) takes one the same way.
        let curves = self
            .automation
            .iter_mut()
            .chain(self.events.iter_mut().flat_map(|e| e.automation.iter_mut()));
        for curve in curves {
            if curve.id.0 == 0 {
                self.next_id += 1;
                curve.id = NodeId(self.next_id);
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

    /// The sequence as the MIDI a file holds, at `ppq` ticks per beat: every
    /// event's messages at their ticks (`render::midi`, on channel 0 unless an
    /// event says otherwise), its automation as its channels' messages and
    /// its notes' as theirs -- as its [`MidiSpec`] says them (see
    /// `midi`), MIDI 1.0 when it names none -- and the tempo as Set Tempo
    /// marks. An OSC event has no MIDI spelling and is left out, as is a curve
    /// the spec cannot say. A tempo ramp is written as the step at its
    /// breakpoint -- a file's tempo only steps.
    pub fn to_midi(&self, ppq: u16) -> Midi {
        let tick = |beat: f64| (beat * f64::from(ppq)).round().max(0.0) as u32;
        let events: Vec<(u32, Vec<u8>)> = midi::write(self)
            .into_iter()
            .map(|(at, bytes)| (tick(at), bytes))
            .collect();
        let tempo = self
            .tempo_map
            .as_ref()
            .map(|map| {
                map.breakpoints()
                    .iter()
                    .map(|b| (tick(b.beats), (1e6 / b.tempo).round() as u32))
                    .collect()
            })
            .unwrap_or_default();
        (events, tempo)
    }

    /// The sequence a MIDI file holds, at `ppq` ticks per beat: its notes
    /// paired into events (`render::from_midi_messages`), its streams as
    /// curves -- a channel's as the sequence's automation, poly pressure and
    /// an MPE zone's member channels as the notes' (see `midi`) -- its other
    /// messages as events, the spec it is written for (MPE when it declares a
    /// zone, else MIDI 1.0), and its tempo marks as the tempo map -- or the
    /// format's own
    /// 120 quarter notes a minute when it states none.
    ///
    /// # Errors
    /// A tempo mark the tempo map refuses.
    pub fn from_midi(
        ppq: u16,
        events: &[(u32, Vec<u8>)],
        tempo: &[(u32, u32)],
    ) -> Result<Self, String> {
        let beat = |tick: u32| f64::from(tick) / f64::from(ppq.max(1));
        let messages: Vec<(f64, Vec<u8>)> =
            events.iter().map(|(t, b)| (beat(*t), b.clone())).collect();
        let points: Vec<clausters_core::tempomap::Breakpoint> = if tempo.is_empty() {
            vec![(0, 500_000)]
        } else {
            tempo.to_vec()
        }
        .iter()
        .map(|&(t, micros)| clausters_core::tempomap::Breakpoint {
            beats: beat(t),
            tempo: 1e6 / f64::from(micros.max(1)),
            curve: clausters_core::tempomap::Curve::Step,
        })
        .collect();
        let tempo_map =
            TempoMap::from_breakpoints(&points).map_err(|e| format!("the file's tempo: {e:?}"))?;
        let (events, automation, spec) = midi::read(&messages);
        let mut sequence = Self {
            events,
            automation,
            midi: Some(spec),
            tempo_map: Some(tempo_map),
            ..Self::default()
        };
        sequence.hold();
        Ok(sequence)
    }

    /// **The sequence as the packets a MIDI 2.0 clip of it holds**, at `ppq`
    /// ticks per beat: its notes at 16-bit velocity, its automation as 32-bit
    /// channel messages and its notes' as per-note messages (see
    /// `midi`), and its tempo as Set Tempo messages -- each packet its UMP
    /// words, at its tick.
    pub fn to_ump(&self, ppq: u16) -> Vec<(u32, Vec<u32>)> {
        let tick = |beat: f64| (beat * f64::from(ppq)).round().max(0.0) as u32;
        let mut packets: Vec<(u32, Vec<u32>)> = self
            .tempo_map
            .as_ref()
            .map(|map| {
                map.breakpoints()
                    .iter()
                    .map(|b| {
                        (
                            tick(b.beats),
                            midi::set_tempo((1e6 / b.tempo).round() as u32),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        packets.extend(
            midi::write_ump(self)
                .into_iter()
                .map(|(at, words)| (tick(at), words)),
        );
        // Stable: a tempo at a tick goes before what plays on it.
        packets.sort_by_key(|(t, _)| *t);
        packets
    }

    /// **The sequence a MIDI 2.0 clip holds**, at `ppq` ticks per beat: its
    /// notes, its channels' messages as its automation and its per-note
    /// messages as the notes' (see `midi`), its Set Tempo messages as the tempo
    /// map -- 120 quarter notes a minute when it has none -- and MIDI 2.0 as
    /// its spec.
    ///
    /// # Errors
    /// A tempo the tempo map refuses.
    pub fn from_ump(ppq: u16, packets: &[(u32, Vec<u32>)]) -> Result<Self, String> {
        let beat = |tick: u32| f64::from(tick) / f64::from(ppq.max(1));
        let mut tempo: Vec<clausters_core::tempomap::Breakpoint> = packets
            .iter()
            .filter_map(|(t, words)| {
                let micros = midi::tempo_of(words)?;
                Some(clausters_core::tempomap::Breakpoint {
                    beats: beat(*t),
                    tempo: 1e6 / f64::from(micros.max(1)),
                    curve: clausters_core::tempomap::Curve::Step,
                })
            })
            .collect();
        if tempo.is_empty() {
            tempo.push(clausters_core::tempomap::Breakpoint {
                beats: 0.0,
                tempo: 2.0,
                curve: clausters_core::tempomap::Curve::Step,
            });
        }
        let tempo_map =
            TempoMap::from_breakpoints(&tempo).map_err(|e| format!("the clip's tempo: {e:?}"))?;
        let in_beats: Vec<(f64, Vec<u32>)> = packets
            .iter()
            .map(|(t, words)| (beat(*t), words.clone()))
            .collect();
        let (events, automation) = midi::read_ump(&in_beats);
        let mut sequence = Self {
            events,
            automation,
            midi: Some(MidiSpec::Midi2),
            tempo_map: Some(tempo_map),
            ..Self::default()
        };
        sequence.hold();
        Ok(sequence)
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
            EventsIntent::Midi { midi } => {
                if let Some(spec) = midi
                    && let Some(why) = self.unsayable(spec)
                {
                    return Err(why);
                }
                self.midi = midi;
            }
            EventsIntent::Automation { mut automation } => {
                if let Some(spec) = self.midi
                    && !spec.says_channel(&automation.target.0)
                {
                    return Err(spec.refusal("a curve of the sequence", &automation.target.0));
                }
                if automation.id.0 == 0 {
                    automation.id = NodeId(self.mint());
                } else {
                    self.next_id = self.next_id.max(automation.id.0);
                }
                added = Some(automation.id.0);
                match self.automation.iter_mut().find(|a| a.id == automation.id) {
                    Some(held) => *held = automation,
                    None => self.automation.push(automation),
                }
            }
            EventsIntent::RemoveAutomation { curve } => {
                let i = self
                    .automation
                    .iter()
                    .position(|a| a.id == curve)
                    .ok_or_else(|| format!("the sequence holds no curve {}", curve.0))?;
                self.automation.remove(i);
            }
            EventsIntent::EventAutomation { id, mut automation } => {
                let i = self.index(id).ok_or_else(|| no_event(id))?;
                if let Some(spec) = self.midi
                    && !spec.says_note(&automation.target.0)
                {
                    return Err(spec.refusal("a note's curve", &automation.target.0));
                }
                if automation.id.0 == 0 {
                    automation.id = NodeId(self.mint());
                } else {
                    self.next_id = self.next_id.max(automation.id.0);
                }
                added = Some(automation.id.0);
                let curves = &mut self.events[i].automation;
                match curves.iter_mut().find(|a| a.id == automation.id) {
                    Some(held) => *held = automation,
                    None => curves.push(automation),
                }
            }
            EventsIntent::RemoveEventAutomation { id, curve } => {
                let i = self.index(id).ok_or_else(|| no_event(id))?;
                let curves = &mut self.events[i].automation;
                let j = curves
                    .iter()
                    .position(|a| a.id == curve)
                    .ok_or_else(|| format!("event {id} holds no curve {}", curve.0))?;
                curves.remove(j);
            }
            EventsIntent::AutomationToEvents { curve } => {
                let mut after = self.clone();
                after.automation_to_events(curve)?;
                *self = after;
            }
            EventsIntent::EventsToAutomation { target, channel } => {
                let mut after = self.clone();
                added = Some(after.events_to_automation(&target.0, channel)?);
                *self = after;
            }
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

/// A sequence as a MIDI file holds it: the messages at their ticks, and the
/// tempo marks as `(tick, microseconds per quarter note)`.
pub type Midi = (Vec<(u32, Vec<u8>)>, Vec<(u32, u32)>);

/// **Which MIDI specification a sequence is written for**: what its curves
/// can say, per note and per channel, and so what a file of it and a MIDI
/// destination hear.
///
/// A control acts on a channel or on one note, and the three specs differ in
/// the second: **MIDI 1.0** has one per-note message, poly pressure; **MPE**
/// spends a member channel on each note so its channel's bend, pressure and
/// CC 74 (timbre) are the note's; **MIDI 2.0** says per-note pitch bend, poly
/// pressure and per-note controllers natively. Per channel all three say a
/// CC, the bend, channel pressure and timbre (CC 74) -- and none says a bare
/// `control`, which is a def's name and has no MIDI spelling. A sequence with
/// no spec is one for the server, where any curve is legal. (MIDI 1.0 and 2.0
/// are protocols and MPE a specification over them, so the name is the word
/// that covers the three.)
///
/// Written `"1.0"`, `"2.0"`, or `{"mpe": {"upper": false, "members": 15}}` --
/// the zone: lower (master channel 1) or upper (16), and its member count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MidiSpec {
    /// MIDI 1.0: one channel's messages, poly pressure the only per-note one.
    #[serde(rename = "1.0")]
    Midi1,
    /// MPE, over MIDI 1.0: a zone whose member channels are one note each.
    #[serde(rename = "mpe")]
    Mpe {
        /// The upper zone (master channel 16) rather than the lower (1).
        #[serde(default)]
        upper: bool,
        /// How many member channels the zone has.
        #[serde(default = "fifteen")]
        members: u8,
    },
    /// MIDI 2.0: per-note messages of its own.
    #[serde(rename = "2.0")]
    Midi2,
}

fn fifteen() -> u8 {
    15
}

/// What a curve's target drives, as MIDI can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveKind {
    /// A controller by number (`{"cc": n}`).
    Cc(u8),
    /// The pitch bend (`{"bend": ...}`).
    Bend,
    /// Pressure: channel pressure over a channel, poly pressure over a note.
    Pressure,
    /// MPE's third dimension, CC 74 (`{"timbre": ...}`).
    Timbre,
    /// A def's control and nothing MIDI names (`{"control": name}` alone).
    Control,
}

impl CurveKind {
    /// What `target` drives. A `cc` is named first, since a curve over one
    /// that also names the control it reaches is still that CC to MIDI.
    pub fn of(target: &Value) -> Self {
        let has = |key: &str| target.get(key).is_some();
        if let Some(cc) = target.get("cc").and_then(Value::as_f64) {
            CurveKind::Cc(cc.clamp(0.0, 127.0) as u8)
        } else if has("bend") {
            CurveKind::Bend
        } else if has("pressure") {
            CurveKind::Pressure
        } else if has("timbre") {
            CurveKind::Timbre
        } else {
            CurveKind::Control
        }
    }
}

impl MidiSpec {
    /// Whether a curve over a channel with this target can be said.
    pub fn says_channel(&self, target: &Value) -> bool {
        CurveKind::of(target) != CurveKind::Control
    }

    /// Whether a curve over one note with this target can be said.
    pub fn says_note(&self, target: &Value) -> bool {
        match (self, CurveKind::of(target)) {
            (_, CurveKind::Control) => false,
            (MidiSpec::Midi1, kind) => kind == CurveKind::Pressure,
            (MidiSpec::Mpe { .. }, kind) => !matches!(kind, CurveKind::Cc(_)),
            (MidiSpec::Midi2, _) => true,
        }
    }

    /// How the spec is named to a reader: `MIDI 1.0`, `MPE`, `MIDI 2.0`.
    pub fn label(&self) -> &'static str {
        match self {
            MidiSpec::Midi1 => "MIDI 1.0",
            MidiSpec::Mpe { .. } => "MPE",
            MidiSpec::Midi2 => "MIDI 2.0",
        }
    }

    /// Why `what` over `target` cannot be said in this spec.
    fn refusal(&self, what: &str, target: &Value) -> String {
        let kind = match CurveKind::of(target) {
            CurveKind::Cc(n) => format!("CC {n}"),
            CurveKind::Bend => "bend".into(),
            CurveKind::Pressure => "pressure".into(),
            CurveKind::Timbre => "timbre".into(),
            CurveKind::Control => "a control with no MIDI spelling".into(),
        };
        format!("{what} over {kind} has no {} spelling", self.label())
    }
}

impl EventSequence {
    /// The first curve the sequence holds that `spec` cannot say, as why.
    fn unsayable(&self, spec: MidiSpec) -> Option<String> {
        if let Some(curve) = self
            .automation
            .iter()
            .find(|l| !spec.says_channel(&l.target.0))
        {
            return Some(spec.refusal("a curve of the sequence", &curve.target.0));
        }
        self.events.iter().find_map(|event| {
            event
                .automation
                .iter()
                .find(|c| !spec.says_note(&c.target.0))
                .map(|c| {
                    format!(
                        "{} (event {})",
                        spec.refusal("a note's curve", &c.target.0),
                        event.id
                    )
                })
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
    /// The id an [`EventsIntent::Add`] gave its event, or a
    /// [`EventsIntent::Automation`] or [`EventsIntent::EventAutomation`] its curve.
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
        EventsIntent::Automation { automation } => {
            format!("{EVENTS}:automation:{}", automation.id.0)
        }
        EventsIntent::EventAutomation { id, automation } => {
            format!("{EVENTS}:event:{id}:automation:{}", automation.id.0)
        }
        EventsIntent::SetEvents { .. } | EventsIntent::Restore { .. } => EVENTS.to_string(),
        EventsIntent::Add { .. }
        | EventsIntent::Remove { .. }
        | EventsIntent::RemoveAutomation { .. }
        | EventsIntent::RemoveEventAutomation { .. }
        | EventsIntent::Midi { .. }
        | EventsIntent::AutomationToEvents { .. }
        | EventsIntent::EventsToAutomation { .. } => return None,
    })
}

/// **One verb of a sequence, as JSON** -- the door a client's handle speaks
/// through, the same over the C ABI and wasm. `request` is `{"verb": ...}`:
///
/// - `"state"`: the sequence, whole.
/// - `"len"`: `{"len": n}`.
/// - `"duration"`: `{"duration": beats}`, where the last event stops sounding.
/// - `"midi"` with `ppq`: `{"events": [[tick, [bytes]]], "tempo": [[tick,
///   micros]]}` -- what a file writer takes ([`EventSequence::to_midi`]).
/// - `"loadmidi"` with `ppq`, `events` and `tempo` as a reader gives them: the
///   sequence becomes the one the file holds ([`EventSequence::from_midi`]),
///   answering `{"len": n}`.
/// - `"ump"` with `ppq`: `{"events": [[tick, [words]]]}` -- the packets a
///   MIDI 2.0 clip writer takes ([`EventSequence::to_ump`]).
/// - `"loadump"` with `ppq` and `events` as a clip reader gives them: the
///   sequence becomes the one the clip holds ([`EventSequence::from_ump`]),
///   answering `{"len": n}`.
/// - `"event"` with `id`: the event, or `null`.
/// - `"ids"`: `{"ids": [id]}`, the events' ids in beat order -- of those at
///   exactly `at`, when it is given, or of those in the half-open window
///   `[from, to)`, either end left open when it is not.
/// - `"automation"`: `{"automation": [curve]}`, the sequence's curves -- or,
///   with `id`, that event's, and `null` when it holds no such event.
/// - `"apply"` with `intent`: the edit applied, answering `{"applied",
///   "current"}` -- `current` the payload that puts it back, read before the
///   edit -- plus `"id"` for an add, or `{"error"}` when refused. With
///   `"inverse": false` it leaves `current` out: a caller that records
///   nothing does not pay for a copy of the whole sequence per edit.
///
/// A request that does not read answers `{"error": ...}`.
///
/// [`mutates`] says which requests change the sequence, for a door that has to
/// keep a sizing pass from changing anything.
pub fn call_json(sequence: &mut EventSequence, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return json!({"error": "the request is not JSON"}).to_string();
    };
    let answer = match request.get("verb").and_then(Value::as_str) {
        Some("state") => serde_json::to_value(&*sequence).unwrap_or(Value::Null),
        Some("len") => json!({"len": sequence.events.len()}),
        Some("duration") => json!({"duration": sequence.duration()}),
        Some("midi") => {
            let ppq = request.get("ppq").and_then(Value::as_u64).unwrap_or(480) as u16;
            let (events, tempo) = sequence.to_midi(ppq);
            json!({"events": events.iter().map(|(t, b)| json!([t, b])).collect::<Vec<_>>(), "tempo": tempo})
        }
        Some("loadmidi") => {
            let ppq = request.get("ppq").and_then(Value::as_u64).unwrap_or(480) as u16;
            let pairs = |key: &str| -> Vec<Value> {
                request
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            };
            let events: Vec<(u32, Vec<u8>)> = pairs("events")
                .iter()
                .filter_map(|e| {
                    let tick = e.get(0)?.as_u64()? as u32;
                    let bytes = e
                        .get(1)?
                        .as_array()?
                        .iter()
                        .filter_map(Value::as_u64)
                        .map(|b| b as u8)
                        .collect();
                    Some((tick, bytes))
                })
                .collect();
            let tempo: Vec<(u32, u32)> = pairs("tempo")
                .iter()
                .filter_map(|m| Some((m.get(0)?.as_u64()? as u32, m.get(1)?.as_u64()? as u32)))
                .collect();
            match EventSequence::from_midi(ppq, &events, &tempo) {
                Ok(read) => {
                    *sequence = read;
                    json!({"len": sequence.events.len()})
                }
                Err(error) => json!({ "error": error }),
            }
        }
        Some("ump") => {
            let ppq = request.get("ppq").and_then(Value::as_u64).unwrap_or(480) as u16;
            let packets = sequence.to_ump(ppq);
            json!({"events": packets.iter().map(|(t, w)| json!([t, w])).collect::<Vec<_>>()})
        }
        Some("loadump") => {
            let ppq = request.get("ppq").and_then(Value::as_u64).unwrap_or(480) as u16;
            let packets: Vec<(u32, Vec<u32>)> = request
                .get("events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    let tick = e.get(0)?.as_u64()? as u32;
                    let words = e
                        .get(1)?
                        .as_array()?
                        .iter()
                        .filter_map(Value::as_u64)
                        .map(|w| w as u32)
                        .collect();
                    Some((tick, words))
                })
                .collect();
            match EventSequence::from_ump(ppq, &packets) {
                Ok(read) => {
                    *sequence = read;
                    json!({"len": sequence.events.len()})
                }
                Err(error) => json!({ "error": error }),
            }
        }
        Some("event") => {
            let id = request.get("id").and_then(Value::as_u64).unwrap_or(0);
            serde_json::to_value(sequence.get(id)).unwrap_or(Value::Null)
        }
        Some("ids") => {
            let bound = |key: &str| request.get(key).and_then(Value::as_f64);
            let ids: Vec<u64> = match bound("at") {
                Some(at) => sequence
                    .events
                    .iter()
                    .filter(|e| e.at.0 == at)
                    .map(|e| e.id)
                    .collect(),
                None => {
                    let (from, to) = (bound("from"), bound("to"));
                    sequence
                        .events
                        .iter()
                        .filter(|e| from.is_none_or(|f| e.at.0 >= f))
                        .filter(|e| to.is_none_or(|t| e.at.0 < t))
                        .map(|e| e.id)
                        .collect()
                }
            };
            json!({ "ids": ids })
        }
        Some("automation") => match request.get("id").and_then(Value::as_u64) {
            Some(id) => sequence
                .get(id)
                .map_or(Value::Null, |e| json!({ "automation": e.automation })),
            None => json!({ "automation": sequence.automation }),
        },
        Some("apply") => {
            let intent = request.get("intent").cloned().unwrap_or(Value::Null);
            let inverse = request
                .get("inverse")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            match serde_json::from_value::<EventsIntent>(intent) {
                Ok(intent) => {
                    let current = inverse.then(|| payload(&sequence.state()));
                    match sequence.edit(intent) {
                        Ok(change) => {
                            let mut answer = json!({ "applied": change.applied });
                            if let Some(current) = current {
                                answer["current"] = current.0;
                            }
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

/// Whether `request` is a verb that changes the sequence (`apply`,
/// `loadmidi`, `loadump`). Every other verb reads.
pub fn mutates(request: &str) -> bool {
    serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|r| r.get("verb").and_then(Value::as_str).map(str::to_owned))
        .is_some_and(|verb| matches!(verb.as_str(), "apply" | "loadmidi" | "loadump"))
}

mod midi;
mod scopes;
#[cfg(feature = "notation")]
pub mod score;

#[cfg(test)]
mod tests;
