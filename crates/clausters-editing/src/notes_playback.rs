//! **How the notes editor is played**: a sequence's events as the data of an
//! event lane on a transport of its own, which the server plays by its
//! position.
//!
//! Like the audio editor's ([`crate::audio_playback`]) it sends nothing: every
//! verb answers [`Step`]s, and the caller sends them. What it plays is the
//! event sequence the caller hands every verb, so the notes editor and its
//! playback read the one sequence the script's handle names.
//!
//! # The transport plays it
//!
//! A roll holds concrete data, as a clip of audio does, so it plays the way a
//! take does: on a transport of the server, by its position. **Each playback
//! takes a transport of its own** from the caller's id spaces
//! (`Space::Transports`, above the one addressed by number), so two
//! sequences sound together and a roll's play cursor is its own sequence's;
//! [`NotesPlayback::close`] gives it back. The editor's
//! structure is a group that follows that transport, a group
//! inside it that the transport governs, and an **event lane** on that
//! transport whose notes are made in the group that follows (`/lane_new`) --
//! not in the governed one, since a note is a voice and not a reader: a stop
//! releases it and its release rings out, where one frozen would sound again,
//! mid-release, on the next play. The lane takes the governed group's id as
//! its own: one lane per playback, alive as long as the group is. A play writes the sequence as the lane's data
//! ([`data`]), locates and rolls; a pause, a resume, a stop and a loop are
//! then the transport's, with nothing planned here. **Where a pass ends** is
//! [`End`], as for a multitrack on its transport: open by default, the
//! transport rolling on past the last note; or at the last note's end, or at
//! an end marker, going back to the position cursor.
//!
//! **An edit is heard while it plays** because [`NotesPlayback::update`] sends
//! the lane its new data: the server feeds it again from where the position
//! is, and what sounds keeps its release. No clock is asked for and nothing is
//! re-stamped.
//!
//! # Placed events and lane data
//!
//! What a lane holds is events **placed** on a transport's axis ([`Placed`]),
//! in seconds: [`placed`] places a sequence through its own tempo map, and a
//! multitrack places each sequence its boxes read at the box's place on the
//! timeline (`crate::multitrack::placed_notes`). [`data`] writes placed events
//! as the lane's data (`clausters_core::event_lane::EventLaneData`), each event rendered
//! by the core -- a note by `clausters_core::event::render::synth`, so its def,
//! its controls and how it is released are the core's reading of its keys; a
//! MIDI event by `render::midi`, as the message the server plays through the
//! channel's binding; an OSC event as the message it names.

use clausters_core::event::render::{self, Arg, Type};
use clausters_core::event_lane::{
    EventLaneData, EventLaneDestination, EventLaneMessage, EventLaneMidi, EventLaneNote, Release,
};
use clausters_core::ids::{IdError, IdSpaces, Space};
use clausters_core::osc::OscType;
use clausters_core::tempomap::TempoMap;
use clausters_document::{EventSequence, Point, Second};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::apply::{Applier, Endpoint, Medium, Step};
use crate::instance::Op;
use crate::note_curves::{self, NoteCurves};
use crate::pass::{self, Pass, Passes, View, samples};
use crate::playback::End;

const EDITOR: &str = "notes";
const GOVERNED: &str = "notes/transport";
const CURVES: &str = "notes/curves";

/// **One event placed on a transport's axis**: where it starts and, for a
/// note, where it is released, in seconds of that axis, and its keys.
///
/// What an event lane's data is written from, so it is written once for every axis an
/// event is heard on: the notes editor places a sequence through its own tempo
/// map ([`placed`]), and a multitrack places each sequence its boxes read at
/// the box's place on the timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// Where it starts, in seconds of the axis.
    pub start: f64,
    /// Where a note is released, in seconds of the axis.
    pub end: f64,
    /// Its keys.
    pub keys: Map<String, Value>,
    /// Its identity, stable across edits: what its curves' tables are kept by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Which sequence it is of, as the sequence's curves name it: a curve over a
    /// channel reaches the notes of its own sequence alone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// The note's own curves, their points in seconds from its start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<PlacedCurve>,
}

/// **A curve placed on a transport's axis**: what it drives and its points in
/// seconds -- a note's from the note's start, a sequence's on the axis itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlacedCurve {
    /// Its identity, stable across edits.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Its sequence, as the sequence's notes name it ([`Placed::scope`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// What it drives, as the sequence's curve names it.
    pub target: Value,
    /// Its break-points, in seconds.
    pub points: Vec<Point<Second>>,
}

/// **Events and the curves over their channels**, placed on one axis.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Written")]
pub struct Placement {
    /// The events, in the order of their start.
    pub events: Vec<Placed>,
    /// The sequences' own curves: each over a channel of its sequence.
    pub curves: Vec<PlacedCurve>,
}

/// What a placement is read from: the object it writes, or a bare list of
/// events, which is how placed events were written before they had curves.
#[derive(Deserialize)]
#[serde(untagged)]
enum Written {
    List(Vec<Placed>),
    Whole {
        #[serde(default)]
        events: Vec<Placed>,
        /// Read under `lanes` too, its name before the sequence's curves
        /// were its automation.
        #[serde(default, alias = "lanes")]
        curves: Vec<PlacedCurve>,
    },
}

impl From<Written> for Placement {
    fn from(written: Written) -> Self {
        match written {
            Written::List(events) => Self {
                events,
                curves: Vec::new(),
            },
            Written::Whole { events, curves } => Self { events, curves },
        }
    }
}

/// **A sequence placed on its own axis**: every event at the second its tempo
/// map puts it (one beat a second when it states none), a note released at
/// the second its sustain ends, and each curve's points through the same map.
pub fn placed(sequence: &EventSequence) -> Placement {
    let map = map(sequence);
    let events = sequence
        .events
        .iter()
        .map(|event| {
            let keys = event.keys();
            let sustain = render::sustain_of(&keys).max(0.0);
            let start = map.secs_at(event.at.0);
            let curves = event
                .automation
                .iter()
                .filter(|curve| curve.enabled && !curve.points.is_empty())
                .map(|curve| PlacedCurve {
                    id: curve.id.0.to_string(),
                    scope: String::new(),
                    target: curve.target.0.clone(),
                    points: curve
                        .points
                        .iter()
                        .map(|p| Point {
                            at: Second(map.secs_at(event.at.0 + p.at.0) - start),
                            value: p.value,
                            data: p.data.clone(),
                        })
                        .collect(),
                })
                .collect();
            Placed {
                start,
                end: map.secs_at(event.at.0 + sustain),
                keys,
                id: event.id.to_string(),
                scope: String::new(),
                curves,
            }
        })
        .collect();
    let curves = sequence
        .automation
        .iter()
        .filter(|curve| curve.enabled && !curve.points.is_empty())
        .map(|curve| PlacedCurve {
            id: curve.id.0.to_string(),
            scope: String::new(),
            target: curve.target.0.clone(),
            points: curve
                .points
                .iter()
                .map(|p| Point {
                    at: Second(map.secs_at(p.at.0)),
                    value: p.value,
                    data: p.data.clone(),
                })
                .collect(),
        })
        .collect();
    Placement { events, curves }
}

/// **A note as its voice is started**: the def the core renders its keys to,
/// the controls it starts with, and how it is released.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    pub def: String,
    pub controls: Vec<(String, f64)>,
    pub release: Release,
}

/// A note's [`Voice`], or `None` for keys the core renders no synth from.
pub fn voice_of(keys: &Map<String, Value>) -> Option<Voice> {
    let synth = render::synth(keys, 0)?;
    // `/synth_new def id addAction target name value ...`
    let def = match synth.start.get(1) {
        Some(Arg::Str(def)) => def.clone(),
        _ => return None,
    };
    let mut controls = Vec::new();
    for pair in synth.start.get(5..).unwrap_or_default().chunks(2) {
        if let [Arg::Str(name), value] = pair {
            let value = match value {
                Arg::Float(f) => f64::from(*f),
                Arg::Int(i) => f64::from(*i),
                Arg::Str(_) => continue,
            };
            controls.push((name.clone(), value));
        }
    }
    let release = match synth.release.first() {
        Some(Arg::Str(addr)) if addr == "/node_set" => Release::Gate,
        _ => Release::Free,
    };
    Some(Voice {
        def,
        controls,
        release,
    })
}

/// **A note that plays in a graph**: one more of `slot` in the instance
/// `graph`, with the ports its curves' readers take beside its controls
/// (`crate::note_curves`).
#[derive(Clone, Debug, PartialEq)]
pub struct SlotNote {
    pub graph: i32,
    pub slot: String,
    pub ports: Vec<(String, f64)>,
}

/// **Placed events as an event lane's data** (`/lane_set`), at `rate` samples
/// a second (the shape is [`EventLaneData`]'s). A note is what the core renders its
/// keys to -- the def, `freq`, `amp` and every other numeric key, released by
/// `gate 0` when its def is gated and by a free otherwise -- as a synth, or as
/// the slot `slots` names for it at its index, when curves shape it; a note
/// whose instrument is a node is that node's note, its key, tuning, velocity,
/// channel and controls the core's reading of its keys. A note that `joins`
/// another (a voice's line, by the joined event's id) continues it on the
/// lane, so a line sounds the same from a pattern and from a sequence. A MIDI
/// event is the message the core spells it as, which the server plays as
/// though it had reached its MIDI input; an OSC event is a message the server
/// runs as written. A rest sounds nothing.
pub fn data(placed: &[Placed], rate: f64, slots: &[Option<SlotNote>]) -> Value {
    let sample = |secs: f64| (secs.max(0.0) * rate).round() as u64;
    let mut data = EventLaneData::default();
    // Each note's index in the lane, by its event's id, for the joins.
    let mut by_id: std::collections::HashMap<String, usize> = Default::default();
    let joined = |keys: &Map<String, Value>, by_id: &std::collections::HashMap<String, usize>| {
        keys.get("joins")
            .and_then(Value::as_i64)
            .and_then(|id| by_id.get(&id.to_string()).copied())
    };
    for (i, event) in placed.iter().enumerate() {
        match Type::of(&event.keys) {
            Type::Note if render::node_note_of(&event.keys).is_some() => {
                let Some(note) = render::node_note_of(&event.keys) else {
                    continue;
                };
                let mut controls = vec![
                    ("key".to_owned(), f64::from(note.key)),
                    ("velocity".to_owned(), note.velocity),
                    ("channel".to_owned(), f64::from(note.channel)),
                ];
                if note.tuning.abs() > 1e-9 {
                    controls.push(("tuning".to_owned(), note.tuning));
                }
                controls.extend(note.controls);
                let joins = joined(&event.keys, &by_id);
                if !event.id.is_empty() {
                    by_id.insert(event.id.clone(), data.notes.len());
                }
                data.notes.push(EventLaneNote {
                    start: sample(event.start),
                    end: sample(event.end.max(event.start)),
                    destination: EventLaneDestination::Node(note.node),
                    controls,
                    release: Release::Free,
                    joins,
                });
            }
            Type::Note => {
                let Some(Voice {
                    def,
                    mut controls,
                    release,
                }) = voice_of(&event.keys)
                else {
                    continue;
                };
                let destination = match slots.get(i).and_then(Option::as_ref) {
                    Some(slot) => {
                        controls.extend(slot.ports.iter().cloned());
                        EventLaneDestination::Slot {
                            graph: slot.graph,
                            slot: slot.slot.clone(),
                        }
                    }
                    None => EventLaneDestination::Def(def),
                };
                let joins = joined(&event.keys, &by_id);
                if !event.id.is_empty() {
                    by_id.insert(event.id.clone(), data.notes.len());
                }
                data.notes.push(EventLaneNote {
                    start: sample(event.start),
                    end: sample(event.end.max(event.start)),
                    destination,
                    controls,
                    release,
                    joins,
                });
            }
            Type::Midi => {
                // A `midi` event is one message, at its own start.
                for message in render::midi(&event.keys, 0).unwrap_or_default() {
                    data.midi.push(EventLaneMidi {
                        position: sample(event.start),
                        bytes: message.bytes,
                    });
                }
            }
            Type::Osc => {
                let Some(addr) = event.keys.get("addr").and_then(Value::as_str) else {
                    continue;
                };
                let args = event
                    .keys
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|arg| match arg {
                        Value::Number(n) if n.is_i64() => n.as_i64().map(|i| Arg::Int(i as i32)),
                        Value::Number(n) => n.as_f64().map(|f| Arg::Float(f as f32)),
                        Value::String(s) => Some(Arg::Str(s.clone())),
                        Value::Bool(b) => Some(Arg::Int(i32::from(*b))),
                        _ => None,
                    })
                    .collect();
                data.messages.push(EventLaneMessage {
                    position: sample(event.start),
                    addr: addr.to_string(),
                    args,
                });
            }
            Type::Rest => {}
        }
    }
    data.to_json()
}

fn map(sequence: &EventSequence) -> TempoMap {
    sequence
        .tempo_map
        .clone()
        .unwrap_or_else(|| TempoMap::new(1.0))
}

/// **The notes editor, as it is playing**, carrying its ops out in `M`: a
/// server's messages by default ([`Applier`]), or an engine an application
/// holds.
#[derive(Clone, Debug)]
pub struct NotesPlayback<M: Medium = Applier> {
    applier: M,
    /// The transport it plays on, once it has one.
    transport: Option<i32>,
    /// Whether the transport was taken from the id spaces, and so is given
    /// back by [`Self::close`]; one handed in is the caller's.
    owned: bool,
    /// **Whether the transport rolls and its pass loops**: the rules every
    /// editor plays by ([`crate::pass`]), in the transport's samples, which
    /// a beat is crossed to by the sequence's tempo.
    passes: Passes,
    /// The lane, once it is made: the governed group's id.
    lane: Option<i32>,
    /// **What the pass in progress was read off**, in samples: the range and
    /// the position cursor -- where a pass goes back to, which the last play,
    /// a cue or a stop put there.
    view: View,
    /// The pass in progress as last asked of the transport, so it is asked
    /// again only when it moves.
    pass: Option<Pass>,
    /// Where a pass ends ([`End`]): open unless asked.
    end: End,
    /// The graphs, readers and tables the sequence's curves play through.
    curves: NoteCurves,
}

impl NotesPlayback {
    /// A playback that has made nothing yet and holds no transport: it takes
    /// one of its own the first time it needs it ([`Self::alloc_transport`]).
    pub fn new() -> Self {
        NotesPlayback::in_medium(Applier::new(Endpoint::default()))
    }

    /// A playback on transport `transport`, which is the caller's to name and
    /// is not given back.
    pub fn on(transport: i32) -> Self {
        Self {
            transport: Some(transport),
            owned: false,
            ..Self::new()
        }
    }
}

impl<M: Medium> NotesPlayback<M> {
    /// A playback that carries its ops out in `medium`, has made nothing
    /// yet and holds no transport.
    pub fn in_medium(medium: M) -> NotesPlayback<M> {
        NotesPlayback {
            applier: medium,
            transport: None,
            owned: true,
            passes: Passes::default(),
            lane: None,
            view: View::default(),
            pass: None,
            end: End::Open,
            curves: NoteCurves::new(CURVES),
        }
    }

    /// **The transport it plays on, taken from `ids` the first time**: one no
    /// other playback of this client holds, nor any other client of the
    /// server. [`IdError::Exhausted`] when every one is taken -- the server
    /// has a fixed number of them (`--transports`).
    pub fn alloc_transport(&mut self, ids: &mut IdSpaces) -> Result<i32, IdError> {
        if let Some(transport) = self.transport {
            return Ok(transport);
        }
        let transport = ids.alloc(Space::Transports, 1)? as i32;
        self.transport = Some(transport);
        Ok(transport)
    }

    /// Where a pass ends.
    pub fn end(&self) -> End {
        self.end
    }

    /// **Where a pass ends** ([`End`]): open by default -- the transport rolls
    /// on past the last note, as a multitrack's does -- or where the last note
    /// ends, or at an end marker, a beat of the sequence; each going back to
    /// the position cursor. Sent only when the mark moves.
    pub fn set_end(&mut self, end: End, sequence: &EventSequence, rate: f64) -> Vec<Step> {
        self.end = end;
        self.remark(sequence, rate)
    }

    /// The transport it plays on, or `None` before it has taken one.
    pub fn transport(&self) -> Option<i32> {
        self.transport
    }

    /// Whether the transport was last told to roll.
    pub fn rolling(&self) -> bool {
        self.passes.rolling()
    }

    /// Says whether the transport rolls, when the caller learned it from the
    /// engine -- a pass that ended on its mark.
    pub fn set_rolling(&mut self, rolling: bool) {
        self.passes.set_rolling(rolling);
    }

    /// A command of its transport, as the op that carries it out in the
    /// medium; nothing before it has one, since there is nothing of this
    /// playback's made yet.
    fn command(&mut self, addr: &str, args: Vec<OscType>) -> Vec<Step> {
        match self
            .transport
            .and_then(|transport| crate::apply::transport_op(transport, addr, &args))
        {
            Some(op) => self.plain(vec![op]),
            None => Vec::new(),
        }
    }

    /// Ops that allocate nothing, carried out in the medium.
    fn plain(&mut self, ops: Vec<Op>) -> Vec<Step> {
        let mut none = IdSpaces::new(
            clausters_core::ids::ServerShape::DEFAULT,
            clausters_core::ids::IdShare::WHOLE,
        );
        self.applier.apply(ops, &mut none).unwrap_or_default()
    }

    /// The editor's structure and its lane, the first time: the group that
    /// follows the transport, the governed group inside it, and the lane --
    /// named by the governed group's id, its notes made in the group that
    /// follows, which a stop does not freeze.
    fn structure(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        if self.lane.is_some() {
            return Ok(Vec::new());
        }
        let transport = self.alloc_transport(ids)?;
        let mut steps = self.applier.apply(
            vec![
                Op::Follow {
                    handle: EDITOR.into(),
                    transport,
                },
                Op::Governed {
                    handle: GOVERNED.into(),
                    parent: EDITOR.into(),
                    transport,
                },
            ],
            ids,
        )?;
        let (Some(group), Some(follows)) = (self.applier.node(GOVERNED), self.applier.node(EDITOR))
        else {
            return Ok(steps);
        };
        steps.extend(self.plain(vec![Op::Lane {
            transport,
            lane: group,
            target: follows,
        }]));
        // The transport is the editor's, and whatever end it was left with is
        // not this playback's: a pass is open until it is asked otherwise.
        steps.extend(self.command("/transport_end", vec![]));
        self.pass = None;
        self.lane = Some(group);
        Ok(steps)
    }

    /// **The lane's data, from `sequence` at `rate`**, after the graphs its
    /// curves play through: a channel's instance in the group that follows
    /// the transport, which a stop does not freeze, since its notes are
    /// voices.
    fn lane_set(
        &mut self,
        sequence: &EventSequence,
        rate: f64,
        pass: bool,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        let Some(lane) = self.lane else {
            return Ok(Vec::new());
        };
        let placement = placed(sequence);
        let plan = note_curves::plan(&placement, rate).scoped(lane);
        let ops = self.curves.ops(&plan, EDITOR, pass);
        let mut steps = self.applier.apply(ops, ids)?;
        let slots = self.curves.slots(&plan, &self.applier);
        let data = data(&placement.events, rate, &slots).to_string();
        steps.extend(self.plain(vec![Op::LaneSet { lane, data }]));
        Ok(steps)
    }

    /// **Where a pass of `sequence` ends, in samples**, as the rule is handed
    /// it ([`pass::space`]): where [`End`] says for a pass that stops, and for
    /// a loop over no range every note. The notes end where the last does --
    /// its onset and its length, the latest of them -- and a note's release
    /// rings out past it, since a stop releases the notes and does not freeze
    /// them.
    fn end_of(&self, sequence: &EventSequence, rate: f64, looping: bool) -> Option<u64> {
        let contents = placed(sequence)
            .events
            .iter()
            .filter(|p| render::Type::of(&p.keys) == Type::Note)
            .map(|p| p.end)
            .fold(0.0, f64::max);
        let secs = match (looping, self.end) {
            (true, _) | (false, End::Contents) => (contents > 0.0).then_some(contents),
            (false, End::Open) => None,
            (false, End::At(beat)) => Some(map(sequence).secs_at(beat.max(0.0))),
        };
        secs.map(|secs| (secs * rate).round().max(0.0) as u64)
    }

    /// A beat of `sequence` as a frame of the transport, never before the first.
    fn frame(sequence: &EventSequence, beat: f64, rate: f64) -> u64 {
        Self::sample(sequence, beat, rate).max(0) as u64
    }

    /// **The pass in progress ends as the view now says**, asked of the
    /// transport only when it moved.
    fn repass(&mut self, sequence: &EventSequence, rate: f64, looping: bool) -> Vec<Step> {
        let end = self.end_of(sequence, rate, looping);
        let pass = pass::space(looping, self.view.selection, self.view.cursor, end).1;
        if self.pass == Some(pass) {
            return Vec::new();
        }
        let ops = self.passes.set_pass(self.transport, pass, &samples);
        if !ops.is_empty() {
            self.pass = Some(pass);
        }
        self.plain(ops)
    }

    /// **A pass over no range ends where the notes or the end now say**: read
    /// again while it plays, when the sequence or the end moved.
    fn remark(&mut self, sequence: &EventSequence, rate: f64) -> Vec<Step> {
        if self.view.selection.is_some() {
            return Vec::new();
        }
        self.repass(sequence, rate, self.passes.looping())
    }

    /// A beat of `sequence` as a sample of the transport.
    fn sample(sequence: &EventSequence, beat: f64, rate: f64) -> i64 {
        (map(sequence).secs_at(beat.max(0.0)) * rate).round() as i64
    }

    /// **Plays `sequence` from beat `from`**, to where [`End`] says and back
    /// to `from`: [`Self::play_pass`] over no range, not looping.
    pub fn play(
        &mut self,
        sequence: &EventSequence,
        from: f64,
        rate: f64,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        self.play_pass(sequence, from, None, false, rate, ids)
    }

    /// **The space bar's play: the audio editor's pass over a roll**
    /// ([`pass::space`]). From beat `from` -- the position cursor, and where a
    /// pass goes back to -- or from the start of `range` (`[start, end]` in
    /// beats) to its end; with the loop switch the transport loops the range,
    /// or with none every note; and with neither it ends where [`End`] says.
    /// The lane takes the sequence first.
    #[allow(clippy::too_many_arguments)] // one pass: where, how far, and its sequence
    pub fn play_pass(
        &mut self,
        sequence: &EventSequence,
        from: f64,
        range: Option<(f64, f64)>,
        looping: bool,
        rate: f64,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        let mut steps = self.structure(ids)?;
        steps.extend(self.lane_set(sequence, rate, true, ids)?);
        let at = |beat| Self::frame(sequence, beat, rate);
        self.view = View {
            selection: range.map(|(a, b)| (at(a), at(b.max(a)))),
            cursor: Some(at(from)),
        };
        let end = self.end_of(sequence, rate, looping);
        let (start, pass) = pass::space(looping, self.view.selection, self.view.cursor, end);
        self.pass = Some(pass);
        let ops = self.passes.play(self.transport, start, pass, &samples);
        steps.extend(self.plain(ops));
        Ok(steps)
    }

    /// **The loop switch changed while it plays**: the pass in progress now
    /// loops over `range` (`[start, end]` in beats), or over every note with
    /// none -- or, switched off, goes on to the range's end, or to where
    /// [`End`] says, and back to the position cursor. From where the transport
    /// stands: nothing is located and nothing restarts. Stopped, it answers
    /// nothing, since the next play reads the switch.
    pub fn set_loop(
        &mut self,
        sequence: &EventSequence,
        range: Option<(f64, f64)>,
        looping: bool,
        rate: f64,
    ) -> Vec<Step> {
        if !self.passes.rolling() || self.lane.is_none() {
            return Vec::new();
        }
        let at = |beat| Self::frame(sequence, beat, rate);
        self.view.selection = range.map(|(a, b)| (at(a), at(b.max(a))));
        self.repass(sequence, rate, looping)
    }

    /// **The position cursor was placed at beat `at`**: rolling, the music
    /// goes on from it, its pass ending as the loop switch says from there;
    /// stopped, the transport stands on it ([`Passes::place`]).
    pub fn place(&mut self, sequence: &EventSequence, at: f64, rate: f64) -> Vec<Step> {
        let frame = Self::frame(sequence, at, rate);
        self.view.cursor = Some(frame);
        let looping = self.passes.looping();
        let end = self.end_of(sequence, rate, looping);
        let pass = pass::space(looping, self.view.selection, self.view.cursor, end).1;
        if self.passes.rolling() {
            self.pass = Some(pass);
        }
        let ops = self.passes.place(self.transport, frame, pass, &samples);
        self.plain(ops)
    }

    /// **The sequence changed**: the lane takes it again, and a pass over no
    /// range follows its new end.
    /// Nothing before the first play: there is no lane yet.
    pub fn update(
        &mut self,
        sequence: &EventSequence,
        rate: f64,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        if self.lane.is_none() {
            return Ok(Vec::new());
        }
        let mut steps = self.lane_set(sequence, rate, false, ids)?;
        if self.passes.rolling() {
            steps.extend(self.remark(sequence, rate));
        }
        Ok(steps)
    }

    /// Rolls the transport again from where it stands.
    pub fn resume(&mut self) -> Vec<Step> {
        let ops = self.passes.resume(self.transport);
        self.plain(ops)
    }

    /// Pauses where it stands: the server releases what the lane sounds, and a
    /// resume plays on from there -- the notes that start after it.
    pub fn pause(&mut self) -> Vec<Step> {
        let ops = self.passes.pause(self.transport);
        self.plain(ops)
    }

    /// **Stops and goes back to beat `back`**: the transport stops and is
    /// located there, which releases what the lane was sounding.
    pub fn stop(&mut self, sequence: &EventSequence, back: f64, rate: f64) -> Vec<Step> {
        let frame = Self::frame(sequence, back, rate);
        self.view.cursor = Some(frame);
        let ops = self.passes.stop(self.transport, frame, &samples);
        self.plain(ops)
    }

    /// **The position cursor moved to beat `at` where nothing plays**: a
    /// stopped transport is located there, so the play cursor goes with it; a
    /// rolling pass is left alone ([`Passes::cue`]).
    pub fn cue(&mut self, sequence: &EventSequence, at: f64, rate: f64) -> Vec<Step> {
        if self.passes.rolling() {
            return Vec::new();
        }
        let frame = Self::frame(sequence, at, rate);
        self.view.cursor = Some(frame);
        let ops = self.passes.cue(self.transport, frame, &samples);
        self.plain(ops)
    }

    /// Frees what the playback made -- the lane (its notes released), the
    /// transport's end mark and the groups -- and gives back the transport it
    /// took, which another playback may then take.
    pub fn close(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let mut steps = Vec::new();
        if let Some(lane) = self.lane.take() {
            steps.extend(self.pause());
            steps.extend(self.plain(vec![Op::LaneFree { lane }]));
            steps.extend(self.command("/transport_loop", vec![]));
            steps.extend(self.command("/transport_end", vec![]));
            let mut ops = self.curves.teardown();
            ops.push(Op::Free {
                handle: EDITOR.into(),
                forget: vec![GOVERNED.into()],
            });
            steps.extend(self.applier.apply(ops, ids)?);
        }
        self.passes = Passes::default();
        self.pass = None;
        if self.owned
            && let Some(transport) = self.transport.take()
        {
            ids.release(Space::Transports, i64::from(transport), 1)?;
        }
        Ok(steps)
    }
}

impl Default for NotesPlayback {
    fn default() -> Self {
        Self::new()
    }
}

/// **The playback's verbs as JSON**, one door for every binding. The sequence
/// played is handed in; a request `{"verb": ...}` answers `{"steps": [...]}`, a
/// query's own object, or `{"error": ...}`.
///
/// - `play` -- `from` (a beat), `rate`, and the space bar's pass: `range`
///   (`[start, end]` in beats) and `loop` (a boolean)
/// - `update` -- `rate`
/// - `loop` -- `range` and `loop`, as for `play`, and `rate`: the loop switch
///   changed while it plays
/// - `resume`, `pause`, `stop` (`back`, a beat; `rate`), `close`
/// - `cue` -- `at` (a beat), `rate`
/// - `end` -- `end` (`null`, `"contents"` or a beat), `rate`: where a pass
///   ends ([`End`])
/// - `setRolling` -- `rolling`
/// - `open` -- takes the transport it plays on, when it has none, and answers
///   `{"transport"}`; refused when the server has none left
/// - `state` -- `{"transport", "rolling", "end"}`, `transport` `null` before
///   it has taken one
pub fn call_json(
    playback: &mut NotesPlayback,
    sequence: &EventSequence,
    request: &str,
    ids: &mut IdSpaces,
) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return json!({"error": "not a request"}).to_string();
    };
    let number =
        |key: &str, default: f64| request.get(key).and_then(Value::as_f64).unwrap_or(default);
    let rate = number("rate", 48_000.0);
    let answer = |steps: Result<Vec<Step>, IdError>| crate::playback::answer_json(steps);
    match request.get("verb").and_then(Value::as_str).unwrap_or("") {
        "play" => {
            let range = request
                .get("range")
                .and_then(Value::as_array)
                .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
            let looping = request
                .get("loop")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            answer(playback.play_pass(sequence, number("from", 0.0), range, looping, rate, ids))
        }
        "update" => answer(playback.update(sequence, rate, ids)),
        "loop" => {
            let range = request
                .get("range")
                .and_then(Value::as_array)
                .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
            let looping = request
                .get("loop")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            answer(Ok(playback.set_loop(sequence, range, looping, rate)))
        }
        "resume" => answer(Ok(playback.resume())),
        "pause" => answer(Ok(playback.pause())),
        "stop" => answer(Ok(playback.stop(sequence, number("back", 0.0), rate))),
        "cue" => answer(Ok(playback.cue(sequence, number("at", 0.0), rate))),
        "end" => match request.get("end").and_then(End::from_json) {
            Some(end) => answer(Ok(playback.set_end(end, sequence, rate))),
            None => json!({"error": "an end is null, \"contents\" or a beat"}).to_string(),
        },
        "close" => answer(playback.close(ids)),
        "setRolling" => {
            playback.set_rolling(
                request
                    .get("rolling")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            );
            answer(Ok(Vec::new()))
        }
        "open" => match playback.alloc_transport(ids) {
            Ok(transport) => json!({"transport": transport}).to_string(),
            Err(e) => json!({"error": e.to_string()}).to_string(),
        },
        "state" => json!({
            "transport": playback.transport(),
            "rolling": playback.rolling(),
            "end": playback.end().to_json(),
        })
        .to_string(),
        other => json!({"error": format!("no notes playback verb {other:?}")}).to_string(),
    }
}

#[cfg(test)]
mod tests;
