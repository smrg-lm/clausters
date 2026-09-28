//! **How the notes editor is played**: a sequence's events planned onto a
//! transport of its own, and planned again when the sequence is edited while it
//! sounds.
//!
//! Like the audio editor's ([`crate::audio_playback`]) it sends nothing: every
//! verb answers [`Step`]s, and the caller sends them. What it plays is the
//! event sequence the caller hands every verb, so the notes editor and its
//! playback read the one sequence the script's handle names.
//!
//! # A plan on the transport's clock
//!
//! The editor's structure is a group that follows [`NOTES_EDITOR_TRANSPORT`]
//! and a group inside it that the transport governs: a pause freezes the notes
//! with the queue, and a resume carries both on. A play clears that transport's
//! queue, locates it, and writes every event from the start beat as a bundle on
//! the transport's **clock** (`/sched_atTransport`): a note as its `/synth_new`
//! in the governed group and its release, both rendered by the core
//! (`clausters_core::event::render::synth`); a raw OSC event as its message. The
//! clock does not jump, so the caller hands in where it stands (the
//! `transportSample` of a `/transport_query`) and every bundle is stamped from
//! there, `latency` ahead so nothing arrives late.
//!
//! # An edit is heard while it plays
//!
//! [`NotesPlayback::replan`] clears the queue and writes the plan again from
//! where the transport stands, beginning `latency` before it so an onset
//! stamped and not yet sounded goes back on its sample. **What is sounding
//! keeps its release**: the playback keeps each note it planned, and a replan
//! sends back the release of every note already started, on its own sample.
//! The transport's end mark follows the sequence's length, so a pass stops
//! where the last note does.

use clausters_core::event::render::{self, Arg, Type};
use clausters_core::ids::{IdError, IdSpaces, Space};
use clausters_core::osc::{self, OscMessage, OscPacket, OscTime, OscType};
use clausters_core::tempomap::TempoMap;
use clausters_document::EventSequence;
use serde_json::{Value, json};

use crate::apply::{Applier, Endpoint, Step};
use crate::instance::Op;

/// The transport the notes editor plays on: its own, so playing a sequence
/// never moves a multitrack (0), an audio editor (1) or the host's monitor (2).
pub const NOTES_EDITOR_TRANSPORT: i32 = 3;

/// How far ahead of the transport a plan is stamped, in seconds, when the
/// caller says nothing: the round trip a bundle has to make.
pub const LATENCY: f64 = 0.1;

const EDITOR: &str = "notes";
const GOVERNED: &str = "notes/transport";

/// One note the playback planned: its node, and the samples of the transport's
/// clock it starts and is released on.
#[derive(Clone, Debug, PartialEq)]
struct Planned {
    start: i64,
    end: i64,
    release: OscMessage,
}

/// **The notes editor, as it is playing.**
#[derive(Clone, Debug)]
pub struct NotesPlayback {
    applier: Applier,
    transport: i32,
    rolling: bool,
    planned: Vec<Planned>,
}

/// Where a plan is written from: the transport's clock and the beat that
/// stands on it, the engine's rate and the latency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct At {
    /// The transport's clock (`transportSample`) when the caller asked.
    pub clock: i64,
    /// The engine's sample rate.
    pub rate: f64,
    /// How far ahead a bundle is stamped, in seconds.
    pub latency: f64,
}

impl NotesPlayback {
    /// A playback that has made nothing yet, on transport `transport`.
    pub fn new(transport: i32) -> Self {
        Self {
            applier: Applier::new(Endpoint::default()),
            transport,
            rolling: false,
            planned: Vec::new(),
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

    fn map(sequence: &EventSequence) -> TempoMap {
        sequence
            .tempo_map
            .clone()
            .unwrap_or_else(|| TempoMap::new(1.0))
    }

    fn command(&self, addr: &str, args: Vec<OscType>) -> Vec<Step> {
        crate::apply::transport_command(self.transport, addr, args)
    }

    /// The editor's structure, the first time: the group that follows the
    /// transport and the governed group inside it.
    fn structure(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        if self.applier.node(GOVERNED).is_some() {
            return Ok(Vec::new());
        }
        self.applier.apply(
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
        )
    }

    /// Every sounding note freed and the transport's queue cleared: what a play
    /// and a stop start from.
    fn silence(&mut self) -> Vec<Step> {
        self.planned.clear();
        let mut steps = vec![crate::apply::send(
            "/sched_clear",
            vec![
                OscType::String("transport".into()),
                OscType::Int(self.transport),
            ],
        )];
        if let Some(group) = self.applier.node(GOVERNED) {
            steps.push(crate::apply::send(
                "/group_freeAll",
                vec![OscType::Int(group)],
            ));
        }
        steps
    }

    /// **Plays `sequence` from beat `from`**: whatever sounded is freed, the
    /// queue cleared, the transport located at `from` with its end mark at the
    /// sequence's end (going back to `from`), every event from there planned,
    /// and the transport rolled.
    pub fn play(
        &mut self,
        sequence: &EventSequence,
        from: f64,
        at: At,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        let mut steps = self.structure(ids)?;
        steps.extend(self.silence());
        let map = Self::map(sequence);
        let sample = |beat: f64| (map.secs_at(beat) * at.rate).round() as i64;
        steps.extend(self.command("/transport_loop", vec![]));
        steps.extend(self.command(
            "/transport_end",
            vec![
                OscType::Long(sample(sequence.duration().max(from))),
                OscType::Long(sample(from)),
            ],
        ));
        steps.extend(self.command("/transport_locateSample", vec![OscType::Long(sample(from))]));
        steps.extend(self.plan(sequence, from, from, at, ids)?);
        self.rolling = true;
        steps.extend(self.command("/transport_play", vec![]));
        Ok(steps)
    }

    /// **The sequence changed while it plays** (or while it is paused with a
    /// plan): the queue is cleared and the plan written again from `position`,
    /// the transport's position sample, which stands on `at.clock`. The
    /// releases of the notes already sounding are sent back on their own
    /// samples, and the end mark follows the sequence's new length.
    pub fn replan(
        &mut self,
        sequence: &EventSequence,
        position: i64,
        at: At,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        let map = Self::map(sequence);
        let secs = position as f64 / at.rate;
        let beat = map.beats_at(secs.max(0.0));
        let since = map.beats_at((secs - at.latency).max(0.0));
        let sounding: Vec<Planned> = self
            .planned
            .iter()
            .filter(|p| p.start <= at.clock && p.end > at.clock)
            .cloned()
            .collect();
        let mut steps = vec![crate::apply::send(
            "/sched_clear",
            vec![
                OscType::String("transport".into()),
                OscType::Int(self.transport),
            ],
        )];
        self.planned = sounding;
        for p in &self.planned {
            steps.push(self.stamped(p.end, vec![p.release.clone()]));
        }
        let end = (map.secs_at(sequence.duration().max(beat)) * at.rate).round() as i64;
        steps.extend(self.command(
            "/transport_end",
            vec![OscType::Long(end), OscType::Long(position)],
        ));
        steps.extend(self.plan(sequence, beat, since, at, ids)?);
        Ok(steps)
    }

    /// Writes every event from beat `since` onto the transport's clock, the
    /// beat `from` standing on `at.clock`.
    fn plan(
        &mut self,
        sequence: &EventSequence,
        from: f64,
        since: f64,
        at: At,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        let map = Self::map(sequence);
        let base = map.secs_at(from);
        let clock = |beat: f64| {
            at.clock + ((map.secs_at(beat) - base + at.latency) * at.rate).round() as i64
        };
        let group = self.applier.node(GOVERNED).unwrap_or(0);
        let mut steps = Vec::new();
        for event in sequence.events.iter().filter(|e| e.at.0 >= since) {
            let mut keys = event.keys();
            match Type::of(&keys) {
                Type::Note => {
                    keys.insert("target".into(), json!(group));
                    keys.insert("add_action".into(), json!(1));
                    let node = ids.alloc(Space::Nodes, 1)? as i32;
                    let Some(synth) = render::synth(&keys, node) else {
                        continue;
                    };
                    let (start, end) = (
                        clock(event.at.0),
                        clock(event.at.0 + synth.sustain.max(0.0)),
                    );
                    let release = message(&synth.release);
                    steps.push(self.stamped(start, vec![message(&synth.start)]));
                    steps.push(self.stamped(end, vec![release.clone()]));
                    self.planned.push(Planned {
                        start,
                        end,
                        release,
                    });
                }
                Type::Osc => {
                    let addr = keys.get("addr").and_then(Value::as_str).unwrap_or("/");
                    let args = keys
                        .get("args")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(osc_arg).collect())
                        .unwrap_or_default();
                    steps.push(self.stamped(clock(event.at.0), vec![osc::message(addr, args)]));
                }
                // A rest sounds nothing, and a MIDI message has no OSC spelling.
                Type::Rest | Type::Midi => {}
            }
        }
        Ok(steps)
    }

    /// `messages` as one bundle on the transport's clock at `sample`.
    fn stamped(&self, sample: i64, messages: Vec<OscMessage>) -> Step {
        let bundle = osc::bundle(OscTime::from((0, 1)), messages);
        let bytes = osc::encode(&OscPacket::Bundle(bundle)).unwrap_or_default();
        crate::apply::send(
            "/sched_atTransport",
            vec![
                OscType::Int(self.transport),
                OscType::Long(sample),
                OscType::Blob(bytes),
            ],
        )
    }

    /// Rolls the transport again from where it stands.
    pub fn resume(&mut self) -> Vec<Step> {
        self.rolling = true;
        self.command("/transport_play", vec![])
    }

    /// Pauses where it stands: the notes freeze with the queue, and a resume
    /// carries both on.
    pub fn pause(&mut self) -> Vec<Step> {
        self.rolling = false;
        self.command("/transport_stop", vec![])
    }

    /// **Stops and goes back to beat `back`**: the notes are freed, the queue
    /// cleared, and the transport located there.
    pub fn stop(&mut self, sequence: &EventSequence, back: f64, rate: f64) -> Vec<Step> {
        let mut steps = self.pause();
        steps.extend(self.silence());
        let sample = (Self::map(sequence).secs_at(back) * rate).round() as i64;
        steps.extend(self.command("/transport_locateSample", vec![OscType::Long(sample)]));
        steps
    }

    /// Frees what the playback made, and the transport's marks.
    pub fn close(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        if self.applier.node(GOVERNED).is_none() {
            return Ok(Vec::new());
        }
        let mut steps = self.pause();
        steps.extend(self.silence());
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

/// A rendered message's arguments as the OSC types they are.
fn message(args: &[Arg]) -> OscMessage {
    let mut args = args.iter();
    let addr = match args.next() {
        Some(Arg::Str(addr)) => addr.clone(),
        _ => "/".into(),
    };
    osc::message(
        addr,
        args.map(|a| match a {
            Arg::Str(s) => OscType::String(s.clone()),
            Arg::Int(i) => OscType::Int(*i),
            Arg::Float(f) => OscType::Float(*f),
        })
        .collect(),
    )
}

/// An OSC event's argument as the wire types it: an integer, a float, a
/// string; anything else as its text.
fn osc_arg(value: &Value) -> OscType {
    match value {
        Value::Number(n) if n.is_i64() => OscType::Int(n.as_i64().unwrap_or(0) as i32),
        Value::Number(n) => OscType::Float(n.as_f64().unwrap_or(0.0) as f32),
        Value::String(s) => OscType::String(s.clone()),
        Value::Bool(b) => OscType::Int(i32::from(*b)),
        other => OscType::String(other.to_string()),
    }
}

/// **The playback's verbs as JSON**, one door for every binding. The sequence
/// played is handed in; a request `{"verb": ...}` answers `{"steps": [...]}`, a
/// query's own object, or `{"error": ...}`.
///
/// - `play` -- `from` (a beat), `clock`, `rate`, `latency`
/// - `replan` -- `position` (the transport's position sample), `clock`,
///   `rate`, `latency`
/// - `resume`, `pause`, `stop` (`back`, a beat; `rate`), `close`
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
    let at = At {
        clock: request.get("clock").and_then(Value::as_i64).unwrap_or(0),
        rate: number("rate", 48_000.0),
        latency: number("latency", LATENCY),
    };
    let answer = |steps: Result<Vec<Step>, IdError>| crate::playback::answer_json(steps);
    match request.get("verb").and_then(Value::as_str).unwrap_or("") {
        "play" => answer(playback.play(sequence, number("from", 0.0), at, ids)),
        "replan" => answer(playback.replan(
            sequence,
            request.get("position").and_then(Value::as_i64).unwrap_or(0),
            at,
            ids,
        )),
        "resume" => answer(Ok(playback.resume())),
        "pause" => answer(Ok(playback.pause())),
        "stop" => answer(Ok(playback.stop(sequence, number("back", 0.0), at.rate))),
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
