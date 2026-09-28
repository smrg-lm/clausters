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
//! take does: on the server's transport, by its position. The editor's
//! structure is a group that follows [`NOTES_EDITOR_TRANSPORT`], a group
//! inside it that the transport governs, and an **event lane** on that
//! transport whose notes are made in the governed group (`/lane_new`). The
//! lane takes the governed group's id as its own: one lane per playback, alive
//! as long as the group is. A play writes the sequence as the lane's data
//! ([`data`]), sets the transport's end mark at the sequence's end going back
//! to where the pass began, locates and rolls; a pause, a resume, a stop and a
//! loop are then the transport's, with nothing planned here.
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
//! as the lane's JSON, each note rendered by the core
//! (`clausters_core::event::render::synth`), so a note's def, its controls and
//! how it is released are the core's reading of its keys.

use clausters_core::event::render::{self, Arg, Type};
use clausters_core::ids::{IdError, IdSpaces};
use clausters_core::osc::OscType;
use clausters_core::tempomap::TempoMap;
use clausters_document::EventSequence;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::apply::{Applier, Endpoint, Step};
use crate::instance::Op;

/// The transport the notes editor plays on: its own, so playing a sequence
/// never moves a multitrack (0), an audio editor (1) or the host's monitor (2).
pub const NOTES_EDITOR_TRANSPORT: i32 = 3;

const EDITOR: &str = "notes";
const GOVERNED: &str = "notes/transport";

/// **One event placed on a transport's axis**: where it starts and, for a
/// note, where it is released, in seconds of that axis, and its keys.
///
/// What a lane's data is written from, so it is written once for every axis an
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
}

/// **A sequence placed on its own axis**: every event at the second its tempo
/// map puts it (one beat a second when it states none), a note released at
/// the second its sustain ends.
pub fn placed(sequence: &EventSequence) -> Vec<Placed> {
    let map = map(sequence);
    sequence
        .events
        .iter()
        .map(|event| {
            let keys = event.keys();
            let sustain = render::sustain_of(&keys).max(0.0);
            Placed {
                start: map.secs_at(event.at.0),
                end: map.secs_at(event.at.0 + sustain),
                keys,
            }
        })
        .collect()
}

/// **Placed events as an event lane's data** (`/lane_set`), at `rate` samples
/// a second: `{"notes": [[start, end, def, {controls}, "gate"|"free"]],
/// "messages": [[position, address, args...]]}`. A note is what the core
/// renders its keys to -- the def, `freq`, `amp` and every other numeric key,
/// released by `gate 0` when its def is gated and by a free otherwise; an OSC
/// event is its message. A rest sounds nothing, and a MIDI event has no OSC
/// spelling.
pub fn data(placed: &[Placed], rate: f64) -> Value {
    let sample = |secs: f64| (secs.max(0.0) * rate).round() as u64;
    let mut notes = Vec::new();
    let mut messages = Vec::new();
    for event in placed {
        match Type::of(&event.keys) {
            Type::Note => {
                let Some(synth) = render::synth(&event.keys, 0) else {
                    continue;
                };
                // `/synth_new def id addAction target name value ...`
                let def = match synth.start.get(1) {
                    Some(Arg::Str(def)) => def.clone(),
                    _ => continue,
                };
                let mut controls = Map::new();
                for pair in synth.start.get(5..).unwrap_or_default().chunks(2) {
                    if let [Arg::Str(name), value] = pair {
                        let value = match value {
                            Arg::Float(f) => f64::from(*f),
                            Arg::Int(i) => f64::from(*i),
                            Arg::Str(_) => continue,
                        };
                        controls.insert(name.clone(), json!(value));
                    }
                }
                let release = match synth.release.first() {
                    Some(Arg::Str(addr)) if addr == "/node_set" => "gate",
                    _ => "free",
                };
                notes.push(json!([
                    sample(event.start),
                    sample(event.end.max(event.start)),
                    def,
                    controls,
                    release
                ]));
            }
            Type::Osc => {
                let addr = event
                    .keys
                    .get("addr")
                    .and_then(Value::as_str)
                    .unwrap_or("/");
                let mut message = vec![json!(sample(event.start)), json!(addr)];
                if let Some(args) = event.keys.get("args").and_then(Value::as_array) {
                    message.extend(args.iter().cloned());
                }
                messages.push(Value::Array(message));
            }
            Type::Rest | Type::Midi => {}
        }
    }
    json!({"notes": notes, "messages": messages})
}

fn map(sequence: &EventSequence) -> TempoMap {
    sequence
        .tempo_map
        .clone()
        .unwrap_or_else(|| TempoMap::new(1.0))
}

/// **The notes editor, as it is playing.**
#[derive(Clone, Debug)]
pub struct NotesPlayback {
    applier: Applier,
    transport: i32,
    rolling: bool,
    /// The lane, once it is made: the governed group's id.
    lane: Option<i32>,
    /// The sample a pass goes back to: where the last play began.
    back: i64,
}

impl NotesPlayback {
    /// A playback that has made nothing yet, on transport `transport`.
    pub fn new(transport: i32) -> Self {
        Self {
            applier: Applier::new(Endpoint::default()),
            transport,
            rolling: false,
            lane: None,
            back: 0,
        }
    }

    /// The transport it plays on.
    pub fn transport(&self) -> i32 {
        self.transport
    }

    /// Whether the transport was last told to roll.
    pub fn rolling(&self) -> bool {
        self.rolling
    }

    /// Says whether the transport rolls, when the caller learned it from the
    /// engine -- a pass that ended on its mark.
    pub fn set_rolling(&mut self, rolling: bool) {
        self.rolling = rolling;
    }

    fn command(&self, addr: &str, args: Vec<OscType>) -> Vec<Step> {
        crate::apply::transport_command(self.transport, addr, args)
    }

    /// The editor's structure and its lane, the first time: the group that
    /// follows the transport, the governed group inside it, and the lane whose
    /// notes are made there.
    fn structure(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        if self.lane.is_some() {
            return Ok(Vec::new());
        }
        let mut steps = self.applier.apply(
            vec![
                Op::Follow {
                    handle: EDITOR.into(),
                    transport: self.transport,
                },
                Op::Governed {
                    handle: GOVERNED.into(),
                    parent: EDITOR.into(),
                    transport: self.transport,
                },
            ],
            ids,
        )?;
        let Some(group) = self.applier.node(GOVERNED) else {
            return Ok(steps);
        };
        steps.extend(self.command("/lane_new", vec![OscType::Int(group), OscType::Int(group)]));
        self.lane = Some(group);
        Ok(steps)
    }

    /// The lane's data, from `sequence` at `rate`.
    fn lane_set(&self, sequence: &EventSequence, rate: f64) -> Vec<Step> {
        let Some(lane) = self.lane else {
            return Vec::new();
        };
        let data = data(&placed(sequence), rate).to_string();
        vec![
            crate::apply::send("/lane_set", vec![OscType::Int(lane), OscType::String(data)]),
            Step::AwaitDone {
                command: "/lane_set".into(),
                index: None,
            },
        ]
    }

    /// The end mark at the sequence's end, going back to `back` (a sample).
    fn end_mark(&self, sequence: &EventSequence, back: i64, rate: f64) -> Vec<Step> {
        let end = (map(sequence).secs_at(sequence.duration()) * rate).round() as i64;
        self.command(
            "/transport_end",
            vec![OscType::Long(end.max(back)), OscType::Long(back)],
        )
    }

    /// **Plays `sequence` from beat `from`**: the lane takes the sequence, the
    /// transport's loop is cleared and its end mark set at the sequence's end
    /// (going back to `from`), and the transport is located at `from` and
    /// rolled.
    pub fn play(
        &mut self,
        sequence: &EventSequence,
        from: f64,
        rate: f64,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        let mut steps = self.structure(ids)?;
        steps.extend(self.lane_set(sequence, rate));
        let from = (map(sequence).secs_at(from.max(0.0)) * rate).round() as i64;
        self.back = from;
        steps.extend(self.command("/transport_loop", vec![]));
        steps.extend(self.end_mark(sequence, from, rate));
        steps.extend(self.command("/transport_locateSample", vec![OscType::Long(from)]));
        self.rolling = true;
        steps.extend(self.command("/transport_play", vec![]));
        Ok(steps)
    }

    /// **The sequence changed**: the lane takes it again, and the end mark
    /// follows its new end, still going back to where the last play began.
    /// Nothing before the first play: there is no lane yet.
    pub fn update(&mut self, sequence: &EventSequence, rate: f64) -> Vec<Step> {
        if self.lane.is_none() {
            return Vec::new();
        }
        let mut steps = self.lane_set(sequence, rate);
        steps.extend(self.end_mark(sequence, self.back, rate));
        steps
    }

    /// Rolls the transport again from where it stands.
    pub fn resume(&mut self) -> Vec<Step> {
        self.rolling = true;
        self.command("/transport_play", vec![])
    }

    /// Pauses where it stands: the notes freeze with the governed group, and a
    /// resume carries them on.
    pub fn pause(&mut self) -> Vec<Step> {
        self.rolling = false;
        self.command("/transport_stop", vec![])
    }

    /// **Stops and goes back to beat `back`**: the transport stops and is
    /// located there, which releases what the lane was sounding.
    pub fn stop(&mut self, sequence: &EventSequence, back: f64, rate: f64) -> Vec<Step> {
        let mut steps = self.pause();
        let sample = (map(sequence).secs_at(back.max(0.0)) * rate).round() as i64;
        steps.extend(self.command("/transport_locateSample", vec![OscType::Long(sample)]));
        steps
    }

    /// **The position cursor moved to beat `at`**: a stopped transport is
    /// located there, so the play cursor goes with it; a rolling pass is left
    /// alone, since the cursor is where the reader is and not a seek.
    pub fn cue(&mut self, sequence: &EventSequence, at: f64, rate: f64) -> Vec<Step> {
        if self.rolling {
            return Vec::new();
        }
        let sample = (map(sequence).secs_at(at.max(0.0)) * rate).round() as i64;
        self.command("/transport_locateSample", vec![OscType::Long(sample)])
    }

    /// Frees what the playback made: the lane (its notes released), the
    /// transport's end mark and the groups.
    pub fn close(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let Some(lane) = self.lane.take() else {
            return Ok(Vec::new());
        };
        let mut steps = self.pause();
        steps.push(crate::apply::send("/lane_free", vec![OscType::Int(lane)]));
        steps.push(Step::AwaitDone {
            command: "/lane_free".into(),
            index: None,
        });
        steps.extend(self.command("/transport_end", vec![]));
        steps.extend(self.applier.apply(
            vec![Op::Free {
                handle: EDITOR.into(),
                forget: vec![GOVERNED.into()],
            }],
            ids,
        )?);
        Ok(steps)
    }
}

/// **The playback's verbs as JSON**, one door for every binding. The sequence
/// played is handed in; a request `{"verb": ...}` answers `{"steps": [...]}`, a
/// query's own object, or `{"error": ...}`.
///
/// - `play` -- `from` (a beat), `rate`
/// - `update` -- `rate`
/// - `resume`, `pause`, `stop` (`back`, a beat; `rate`), `close`
/// - `cue` -- `at` (a beat), `rate`
/// - `setRolling` -- `rolling`
/// - `state` -- `{"transport", "rolling"}`
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
        "play" => answer(playback.play(sequence, number("from", 0.0), rate, ids)),
        "update" => answer(Ok(playback.update(sequence, rate))),
        "resume" => answer(Ok(playback.resume())),
        "pause" => answer(Ok(playback.pause())),
        "stop" => answer(Ok(playback.stop(sequence, number("back", 0.0), rate))),
        "cue" => answer(Ok(playback.cue(sequence, number("at", 0.0), rate))),
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
        "state" => {
            json!({"transport": playback.transport(), "rolling": playback.rolling()}).to_string()
        }
        other => json!({"error": format!("no notes playback verb {other:?}")}).to_string(),
    }
}

#[cfg(test)]
mod tests;
