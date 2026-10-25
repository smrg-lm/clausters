//! **The audio editor's coordinator**: every action of an audio member --
//! a gesture, a script's verb, a menu entry, the transport, a save -- enters
//! here and leaves as one ordered list of effects for its caller to carry
//! out.
//!
//! The caller is the application that holds the editor's engine
//! (`clausters_app_engine::audio::AudioApp`, on the GUI host's application
//! thread, natively and in a page). The clients are the editor's handler and
//! carry nothing out: their verbs reach the application through the host's
//! door, and it runs them here. Which playback verb follows a focus change,
//! when the meter is read, when the clipboard is published and the cut is
//! committed, when the takes a turn let go of are freed: this is the one
//! place that decides it.
//!
//! # Runs, effects and awaits
//!
//! An action starts a **run** (`do`). The run answers the **effects** it
//! has decided so far, in order, and stops on an **await** when what comes
//! next depends on something only its caller can do: carry steps out on the
//! engine, say whether the transport rolls, read a file's header, carry a
//! history step out, publish a clipboard document. The caller performs the
//! effects, then the await, and hands its result back (`run`, `result`),
//! which resumes the run where it stopped. A failed await unwinds the run as
//! an exception unwinds a call: through the cleanups it passed and into the
//! handler that catches it, or out to the caller, which raises what failed.
//!
//! Runs stack: a run waiting on its caller can see it start another (a
//! history step that restores a take), and the second runs to its end
//! before the first is resumed. A run waiting on a render is the exception:
//! the application hands the render to a thread of its own, so runs over
//! other files start and end meanwhile, and each run is resumed by its
//! number, in whichever order its render lands. The file it edits has
//! pending work until then, which refuses another edit of it.
//!
//! # What is the caller's
//!
//! The engine and the threads: the steps carried out on the engine it holds,
//! the renders run on its threads, the headers read through the core's file
//! layer, the ids it allocates its nodes and buffers from, and the window it
//! draws (the props an effect sets, the tree a `publish` draws again). A
//! take, a copy and a history entry cross as references -- the editor keeps
//! them and the JSON names them by number (`crate::audio::shared`) -- so
//! nothing here grows with a take's parts. Whether a file is dirty is never
//! the caller's: it reads it from the editor when asked. Everything this
//! module decides it decides through the editor's own doors, so its rules
//! stay where they are.
//!
//! # The vocabulary
//!
//! Effects, each `{"kind", ...}`: `version` (the context's version), `echo`
//! (an answer to the window), `set` (props on a widget), `publish` (the
//! window drawn again), `band` and `looping` (the selection band and the loop switch a script set),
//! `bind` (the file in focus, or none), `layers`, `cursor` (the
//! position a script placed), `locate` (the position a hand placed),
//! `changed` (the data changed in this gesture), `delete` (files to delete:
//! a closed file's renders), `release` (in each `freed` entry's `files`,
//! renders nothing names any more, to delete, as a turn hands them back),
//! `operationEcho`, `lease` (the event's clipboard lease), `closeWindow` and
//! `closedWindow`.
//!
//! Awaits, each `{"kind", ...}`: `steps` (`via` `join`, `plain` or
//! `service`), `shape` (a `path`, whose header the caller reads, or a new
//! file's `shape`), `batch` (an operation's batch on the member's engine),
//! `stepped`, `clipboard`. A result is `{}` on success, `{"failed": reason}`
//! on failure, and carries what the await asked for: the file's facts
//! (`frames`, `channels`, `rate`, `path`, `untitled`, `codec`,
//! `sample_format`), `changed`.

use std::collections::{BTreeMap, VecDeque};

use clausters_core::ids::IdSpaces;
use clausters_editing::apply::Medium;
use clausters_editing::audio_playback::AudioEditorPlayback;
use serde_json::{Value, json};

use super::{Editing, Member, MemberId};

mod actions;
mod driver;
mod outcome;
mod run;
mod step;
#[cfg(test)]
mod tests;

pub use driver::{Header, Raised, Ran, Render, Services};
use run::{Raise, Run, Task};

/// Why a verb that acts on a file is refused by an editor with none open.
const NO_FILE: &str = "no file is open";

/// **One audio member's coordinator**: its playback, what a script's pause
/// left, and the runs in progress. The span the space bar plays and the loop
/// switch are the file in focus's (its selection and its switch), read from
/// the editor and never kept here.
#[derive(Debug)]
pub(super) struct Session {
    playback: AudioEditorPlayback<Box<dyn Medium + Send>>,
    /// The engine's rate.
    rate: f64,
    /// Whether a script paused the pass, which a play without a position
    /// resumes.
    paused: bool,
    runs: BTreeMap<u64, Run>,
    next: u64,
}

impl Session {
    /// A session over the engine the context holds, which says its rate;
    /// `None` without one.
    fn new(editing: &mut Editing) -> Option<Session> {
        let engine = editing.engine.as_mut()?;
        Some(Session {
            rate: engine.rate(),
            playback: AudioEditorPlayback::in_medium(engine.medium()),
            paused: false,
            runs: BTreeMap::new(),
            next: 1,
        })
    }
}

/// **The takes the engine reads, brought up to date**: every file the
/// engine reads whose list changed since is handed to it again, and its
/// readers told its new length.
fn retake(
    editing: &mut Editing,
    member: MemberId,
    playback: &mut AudioEditorPlayback<Box<dyn Medium + Send>>,
    rate: f64,
    ids: &mut IdSpaces,
) {
    let Some(engine) = editing.engine.as_ref() else {
        return;
    };
    let rate = if rate > 0.0 { rate } else { engine.rate() };
    // only the files the engine reads, at a revision it does not
    let takes = match editing.member(member) {
        Some(Member::Audio(editor)) => editor.held_takes(rate, |file, revision| {
            engine.revision(file).is_some_and(|r| r != revision)
        }),
        _ => return,
    };
    for take in takes {
        let Some(engine) = editing.engine.as_mut() else {
            return;
        };
        // A list the engine cannot read is the engine's to say (it plays the
        // parts it can); the readers keep the length they had.
        if let Ok(buffers) = engine.take(&take, ids) {
            let frames = take_frames(editing, member, take.file);
            let take_rate = take_rate(editing, member, take.file);
            // the readers' span set again, or made again at another width
            let _ = playback.sync_streams(take.file, buffers, frames, take_rate, rate, ids);
        }
    }
}

/// The length of `file` at its own rate, as the playback counts it.
fn take_frames(editing: &Editing, member: MemberId, file: u64) -> u64 {
    match editing.member(member) {
        Some(Member::Audio(editor)) => editor.file_length(file).unwrap_or(0),
        _ => 0,
    }
}

/// The rate of `file`.
fn take_rate(editing: &Editing, member: MemberId, file: u64) -> f64 {
    match editing.member(member) {
        Some(Member::Audio(editor)) => editor.file_rate(file).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// **The coordinator's own door**, which its driver ([`Editing::audio_act`])
/// speaks: `do` starts a run, `run` with `result` resumes one, `query`
/// answers what a script reads of the transport.
fn call(editing: &mut Editing, request: &Value, ids: &mut IdSpaces) -> Value {
    let Some(member) = request["member"]
        .as_u64()
        .and_then(|m| MemberId::try_from(m).ok())
        .filter(|m| matches!(editing.member(*m), Some(Member::Audio(_))))
    else {
        return json!({"error": "the member is not an audio application"});
    };
    if let Some(action) = request.get("do") {
        // a verb before the open finds a session to run in
        let Some(mut session) = editing
            .sessions
            .remove(&member)
            .or_else(|| Session::new(editing))
        else {
            return json!({"error": "the audio editor has no engine"});
        };
        editing.note_playing(member, session.playback.playing());
        let answer = session.start(editing, member, action, ids);
        editing.note_playing(member, session.playback.playing());
        editing.sessions.insert(member, session);
        return answer;
    }
    if let Some(run) = request["run"].as_u64() {
        let Some(mut session) = editing.sessions.remove(&member) else {
            return json!({"error": "the audio editor has no session"});
        };
        editing.note_playing(member, session.playback.playing());
        let answer = session.resume(editing, member, run, &request["result"], ids);
        editing.note_playing(member, session.playback.playing());
        editing.sessions.insert(member, session);
        return answer;
    }
    let binding = member_call(editing, member, json!({"verb": "binding"}));
    match (editing.sessions.get(&member), request["query"].as_str()) {
        (Some(session), Some("transport")) => json!({
            "transport": session.playback.transport(),
            "span": binding["span"],
            "looping": binding["looping"] == true,
            "paused": session.paused,
            "focused": session.focused(editing, member),
            "state": session.state(),
        }),
        // **What the editor's engine plays**: the level its meter reads on
        // each channel of the file shown (none while that file is still),
        // and the samples played as silence for want of a frame read in
        // time since the engine opened.
        (Some(session), Some("output")) => {
            let engine = editing.engine.as_ref();
            let levels: Vec<f32> = match session.playback.meters() {
                Some((bus, channels)) if bus >= 0 => (0..channels)
                    .map(|c| engine.map_or(0.0, |e| e.control(bus as usize + c)))
                    .collect(),
                Some((_, channels)) => vec![0.0; channels],
                None => Vec::new(),
            };
            json!({"levels": levels, "starved": engine.map_or(0, |e| e.starved())})
        }
        _ => json!({"error": "the audio editor has no session"}),
    }
}

/// **What the space bar plays** in the file in focus, under its own loop
/// switch: where a pass starts, how it ends and where a stop goes back to;
/// `null` with no file open.
fn space(editing: &Editing, member: MemberId) -> Value {
    match editing.member(member) {
        Some(Member::Audio(editor)) if editor.has_file() => json!(editor.space()),
        _ => Value::Null,
    }
}

/// The focused file's identity, or `None` with no file open.
fn file_id(editing: &Editing, member: MemberId) -> Option<i64> {
    match editing.member(member) {
        Some(Member::Audio(editor)) if editor.has_file() => Some(editor.focused_file()),
        _ => None,
    }
}

/// **The files a batch's renders write**: each render's output and the peaks
/// beside it, as the editor names them.
fn rendered_files(steps: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for render in steps
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.get("render"))
    {
        if let Some(path) = render["path"].as_str().filter(|p| !p.is_empty()) {
            out.push(path.to_string());
            out.push(crate::audio::editor::peaks_beside(path));
        }
    }
    out
}

/// One verb of the member's own door, through the context's.
fn member_call(editing: &mut Editing, member: MemberId, call: Value) -> Value {
    door(
        editing,
        json!({"verb": "member", "member": member, "call": call}),
    )
}

/// One verb of the context's door, read as a value.
fn door(editing: &mut Editing, request: Value) -> Value {
    super::door::call_value(editing, request)
}

fn tagged(kind: &str, fields: Value) -> Value {
    let mut value = json!({"kind": kind});
    if let (Some(map), Value::Object(more)) = (value.as_object_mut(), fields) {
        map.extend(more);
    }
    value
}

fn effect(kind: &str, fields: Value) -> Task {
    Task::Effect(tagged(kind, fields))
}

fn has(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(|v| !v.is_null())
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}

/// A release of what nothing reaches, when there is anything in it.
fn release(freed: &Value) -> Option<Task> {
    truthy(freed).then(|| effect("release", json!({"freed": freed.clone()})))
}

fn finished(id: u64, raise: Raise, message: &str) -> Value {
    json!({"run": id, "done": true, "effects": [],
        "error": {"type": raise.name(), "message": message}})
}

/// The paths of an open request, each one file to read.
fn paths(paths: &Value) -> VecDeque<Value> {
    paths
        .as_array()
        .into_iter()
        .flatten()
        .map(|path| json!({"path": path}))
        .collect()
}
