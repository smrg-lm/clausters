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
use clausters_editing::audio_playback::{self, AudioEditorPlayback};
use serde_json::{Value, json};

use super::{Editing, Member, MemberId, call_json};

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

/// What a client raises when a run ends in an error of the coordinator's.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Raise {
    /// A request the editor cannot take (`ValueError`, `RangeError`).
    Value,
    /// An operation the server or the editor refused (`CommandError`).
    Command,
    /// A state the editor is not in any more (`RuntimeError`, `Error`).
    Runtime,
}

impl Raise {
    fn name(self) -> &'static str {
        match self {
            Raise::Value => "value",
            Raise::Command => "command",
            Raise::Runtime => "runtime",
        }
    }
}

/// **A failure unwinding a run**: one of the client's (an await that
/// failed, re-raised as it was) or one of the coordinator's.
#[derive(Clone, Debug)]
enum Failure {
    Client(String),
    Own(Raise, String),
}

impl Failure {
    fn reason(&self) -> &str {
        match self {
            Failure::Client(reason) | Failure::Own(_, reason) => reason,
        }
    }
}

/// How one step of a run went on.
enum Flow {
    Next,
    Await(Value, Resume),
    Fail(Failure),
}

/// **What a resumed run does with the client's result.**
#[derive(Clone, Debug)]
enum Resume {
    /// Steps carried out: nothing to read, a failure unwinds.
    Steps,
    /// One file read into buffers, to add.
    Read,
    /// The open files looked at on disk, the window having come back: the
    /// stamps go to the editor, which answers the window's `seq`.
    Stat { seq: Value },
    /// An operation's batch carried out: complete it. The files its renders
    /// write go if it fails, where they are the editor's scratch.
    Batch {
        ticket: Value,
        batch: Value,
        repairing: bool,
        member: Option<MemberId>,
        rendered: Vec<String>,
    },
    /// A repair's recovery batch carried out.
    Recovery {
        ticket: Value,
        rest: Vec<Value>,
        repairing: bool,
    },
    /// A history step carried out: whether it changed anything.
    Stepped,
    /// A clipboard document published.
    Clipboard,
}

/// What a rolling answer was asked for.
#[derive(Clone, Debug)]
enum Rolled {
    /// The focused file is viewed again at its cursor.
    View { frame: u64 },
    /// The space bar: stop a rolling pass or play one.
    Space(Value),
    /// The position cursor moved: the play cursor goes with it.
    Cue(u64),
    /// A script asked whether it plays.
    Playing { was: bool },
    /// A script's stop, when the file shown is not in focus.
    Stop,
    /// A script's locate.
    Locate(u64),
}

/// **One step of a run**: a part of what the clients' turn handling did, in
/// its order.
#[derive(Clone, Debug)]
enum Task {
    /// An outcome to carry out (`lease`: the event's clipboard lease rides
    /// on it).
    Take { outcome: Value, lease: bool },
    /// A member verb a file action names, its answer carried out.
    VerbTake(Value),
    /// The binding of a turn that names one, past its steps.
    Bound {
        outcome: Value,
        close_file: bool,
        lease: bool,
    },
    /// The rest of an outcome, past its file binding.
    TakeRest { outcome: Value, lease: bool },
    /// The tail of an outcome, past its steps and its clipboard.
    TakeTail(Value),
    /// The completed operation's outcome, carried out.
    TakeCompleted,
    /// A save's result: where it wrote.
    Destination,
    /// The run's result: whether the take changed, and why a verb refused.
    Reason(Value),
    /// An operation, from its command or as prepared.
    Operation {
        command: Option<Value>,
        prepared: Option<Value>,
        repairing: bool,
    },
    /// An operation's answer, looked at.
    OperationAnswer { answer: Value, repairing: bool },
    /// What an operation came to, reflected.
    OperationDone { answer: Value, repairing: bool },
    /// The clipboard document a copy left, published.
    PublishClipboard(Value),
    /// A cut committed once its copy is on the clipboard.
    Cut(Value),
    /// A cut's continuation, once the buffers are topped up.
    CutAct(Value),
    /// Files read and added, one at a time.
    Read(VecDeque<Value>),
    /// The files a window coming back reads, looked at on disk
    /// (`checkDisk`: `{paths, seq}`).
    Stat(Value),
    /// A read file added to the editor.
    AddFile(Value),
    /// Make what sounds be the take as it now is.
    Sound,
    /// One file's playback brought in step.
    SyncSound(Value),
    /// The window told where its level is read from.
    TellMeters,
    /// The window's play cursor and the list's play mark corrected.
    ShowTransport,
    /// The engine asked whether the transport rolls.
    Rolling(Rolled),
    /// A playback verb, its steps carried out.
    Playback { verb: &'static str, args: Value },
    /// The member's playback root retained.
    RetainPlayback,
    /// The member's playback root dropped.
    DropPlayback,
    /// Steps to carry out on the server.
    Steps { via: &'static str, steps: Value },
    /// A history step to carry out.
    Stepped(Value),
    /// The window closed (`closed`: by the host, not asked).
    Close { closed: bool },
    /// A cleanup that runs whether or not the run fails.
    Finally(Cleanup),
    /// A handler for a failure of what precedes it.
    Catch(Handler),
    /// An effect, emitted in order.
    Effect(Value),
    /// A result the run answers.
    Answer(Value),
}

#[derive(Clone, Debug)]
enum Cleanup {
    /// The event's clipboard lease is released.
    Lease,
}

#[derive(Clone, Debug)]
enum Handler {
    /// A copy that failed before or while it was published: the window is
    /// told why.
    Clipboard { outcome: Value },
}

/// **One action in progress.**
#[derive(Debug, Default)]
struct Run {
    tasks: VecDeque<Task>,
    waiting: Option<Resume>,
    /// Whether the take changed, as the outcome being carried out says.
    changed: bool,
    /// What the last operation completed with.
    completed: Value,
    /// Whether the window is open on a host.
    window: bool,
    /// Why a verb refused, answered with the run's first effects.
    reason: Option<Value>,
    result: Value,
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

/// **The `audio` door**: `do` starts a run, `run` with `result` resumes one,
/// `query` answers what a script reads of the transport.
pub(super) fn call(editing: &mut Editing, request: &Value, ids: &mut IdSpaces) -> Value {
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

/// One verb of the member's own door, as a client calls it.
fn member_call(editing: &mut Editing, member: MemberId, call: Value) -> Value {
    door(
        editing,
        json!({"verb": "member", "member": member, "call": call}),
    )
}

/// One verb of the context's door, as a client calls it.
fn door(editing: &mut Editing, request: Value) -> Value {
    serde_json::from_str(&call_json(editing, &request.to_string())).unwrap_or(Value::Null)
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

/// The verbs a script plays the transport with, which need a file open.
const TRANSPORT: &[&str] = &[
    "playing", "play", "pause", "stop", "locate", "span", "looping",
];

impl Session {
    fn start(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        action: &Value,
        ids: &mut IdSpaces,
    ) -> Value {
        let id = self.next;
        self.next += 1;
        let mut run = Run {
            window: action["window"] == true,
            ..Default::default()
        };
        let verb = action["verb"].as_str().unwrap_or_default();
        if TRANSPORT.contains(&verb) && file_id(editing, member).is_none() {
            return finished(id, Raise::Value, NO_FILE);
        }
        let tasks: Vec<Task> = match verb {
            "open" => {
                if self.playback.transport().is_none()
                    && let Err(error) = self.playback.alloc_transport(ids)
                {
                    return finished(id, Raise::Runtime, &error.to_string());
                }
                let mut tasks = Vec::new();
                if action["empty"] != true {
                    tasks.push(Task::Operation {
                        command: Some(json!({"kind": "open"})),
                        prepared: None,
                        repairing: false,
                    });
                }
                tasks.push(Task::Sound);
                tasks.push(Task::Answer(
                    json!({"transport": self.playback.transport()}),
                ));
                tasks
            }
            // **The window's work called off** (its progress panel's button,
            // or Escape): the client stops the renders under way, each before
            // its next piece, and a batch that stops so deletes what it wrote.
            "event" if action["args"][3] == "call_off" => {
                vec![effect("call_off", json!({}))]
            }
            "event" => {
                let turned = door(
                    editing,
                    json!({"verb": "event", "member": member,
                        "addr": action["addr"], "args": action["args"]}),
                );
                self.turned(&mut run, turned, action["lease"] == true)
            }
            "act" => {
                let turned = door(
                    editing,
                    json!({"verb": "act", "member": member, "call": action["call"]}),
                );
                let outcome = turned.get("outcome").cloned().unwrap_or(json!({}));
                run.changed = outcome["changed"] == true;
                let mut tasks = vec![
                    effect("version", json!({"version": turned["version"]})),
                    Task::Take {
                        outcome: outcome.clone(),
                        lease: false,
                    },
                ];
                tasks.extend(release(&turned["freed"]));
                run.reason = Some(outcome["answer"]["reason"].clone());
                tasks.push(Task::Reason(outcome["answer"]["reason"].clone()));
                tasks
            }
            "verb" => {
                let answer = member_call(editing, member, action["call"].clone());
                if answer["error"] == NO_FILE {
                    return finished(id, Raise::Value, NO_FILE);
                }
                run.changed = answer["changed"] == true;
                run.reason = Some(answer["answer"]["reason"].clone());
                vec![
                    Task::Take {
                        outcome: answer.clone(),
                        lease: false,
                    },
                    Task::Reason(answer["answer"]["reason"].clone()),
                ]
            }
            "save" => {
                let mut command = json!({"kind": "save"});
                if let (Some(map), Value::Object(request)) =
                    (command.as_object_mut(), &action["request"])
                {
                    map.extend(request.clone());
                }
                vec![
                    Task::Operation {
                        command: Some(command),
                        prepared: None,
                        repairing: false,
                    },
                    Task::TakeCompleted,
                    Task::Destination,
                ]
            }
            "operation" => vec![Task::Operation {
                command: action.get("command").filter(|c| !c.is_null()).cloned(),
                prepared: action.get("prepared").filter(|p| !p.is_null()).cloned(),
                repairing: action["repairing"] == true,
            }],
            "retry" => {
                let pending = door(
                    editing,
                    json!({"verb": "operation", "action": "pendingRepairs", "member": member}),
                );
                pending["repairs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|repair| {
                        let prepared = door(
                            editing,
                            json!({"verb": "operation", "action": "retryRepair",
                                "ticket": repair["ticket"]}),
                        );
                        Task::Operation {
                            command: None,
                            prepared: Some(prepared),
                            repairing: true,
                        }
                    })
                    .collect()
            }
            "openFile" => {
                let answer = member_call(
                    editing,
                    member,
                    json!({"verb": "open_file", "paths": action["paths"]}),
                );
                if let Some(error) = answer["error"].as_str() {
                    return finished(id, Raise::Value, error);
                }
                vec![Task::Read(paths(&answer["openFile"]["paths"]))]
            }
            "newFile" => vec![Task::Read(VecDeque::from([
                json!({"shape": action["shape"]}),
            ]))],
            "sound" => vec![Task::Sound],
            "syncSound" => vec![Task::SyncSound(action["file"].clone())],
            "close" => vec![Task::Close { closed: false }],
            "closed" => vec![Task::Close { closed: true }],
            "playing" => vec![Task::Rolling(Rolled::Playing {
                was: self.playback.rolling(),
            })],
            "play" => self.script_play(editing, member, action["at"].as_f64()),
            "pause" => {
                let mut tasks = Vec::new();
                if self.focused(editing, member) {
                    tasks.push(Task::Playback {
                        verb: "pause",
                        args: json!({}),
                    });
                    self.paused = true;
                    tasks.push(Task::ShowTransport);
                }
                tasks
            }
            "stop" => {
                self.paused = false;
                if self.focused(editing, member) {
                    self.script_stop(editing, member)
                } else {
                    vec![Task::Rolling(Rolled::Stop)]
                }
            }
            "locate" => self.script_locate(editing, member, action["at"].as_f64().unwrap_or(0.0)),
            "span" => {
                let span = action["span"]
                    .as_array()
                    .and_then(|s| Some((s.first()?.as_f64()?, s.get(1)?.as_f64()?)));
                let band = member_call(
                    editing,
                    member,
                    json!({"verb": "span", "span": span.map(|(a, b)| json!([a, b]))}),
                );
                let mut tasks = Vec::new();
                if action["show"] != false {
                    tasks.push(effect(
                        "band",
                        json!({"sel_start": band["sel_start"], "sel_len": band["sel_len"]}),
                    ));
                }
                tasks.extend(self.follow(editing, member));
                tasks
            }
            "looping" => {
                let on = action["on"] == true;
                member_call(editing, member, json!({"verb": "looping", "on": on}));
                let mut tasks = self.follow(editing, member);
                tasks.push(effect("looping", json!({"on": on})));
                tasks
            }
            other => return finished(id, Raise::Value, &format!("no audio verb {other:?}")),
        };
        run.tasks.extend(tasks);
        let changed = run.changed;
        let reason = run.reason.take();
        self.runs.insert(id, run);
        let mut answer = self.drive(editing, member, id, ids);
        // known before any await: what a client answers at once
        answer["changed"] = json!(changed);
        if let Some(reason) = reason {
            answer["reason"] = reason;
        }
        answer
    }

    /// What a message to the member came to, as the event handling did.
    fn turned(&mut self, run: &mut Run, turned: Value, lease: bool) -> Vec<Task> {
        let outcome = turned.get("outcome").cloned().unwrap_or(json!({}));
        let mut tasks = vec![effect("version", json!({"version": turned["version"]}))];
        if outcome["turn"] == "closed" {
            tasks.push(Task::Close { closed: true });
            tasks.push(Task::Answer(json!(false)));
            return tasks;
        }
        if outcome["turn"] == "step" {
            tasks.push(Task::Stepped(
                turned.get("stepped").cloned().unwrap_or(json!({})),
            ));
            tasks.push(effect("echo", json!({"answer": outcome["answer"]})));
            tasks.extend(release(&turned["freed"]));
            tasks.push(Task::Sound);
            return tasks;
        }
        run.changed = outcome["changed"] == true;
        tasks.push(Task::Take { outcome, lease });
        tasks.extend(release(&turned["freed"]));
        tasks
    }

    fn focused(&self, editing: &Editing, member: MemberId) -> bool {
        let file = file_id(editing, member);
        file.is_some() && self.playback.focus() == file.map(|f| f as u64)
    }

    fn script_play(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        at: Option<f64>,
    ) -> Vec<Task> {
        if at.is_none() && self.paused && self.focused(editing, member) {
            self.paused = false;
            return vec![
                Task::Playback {
                    verb: "resume",
                    args: json!({}),
                },
                Task::ShowTransport,
            ];
        }
        self.paused = false;
        let mut tasks = Vec::new();
        if let Some(at) = at {
            tasks.extend(self.script_locate(editing, member, at));
        }
        let play = member_call(editing, member, json!({"verb": "play"}));
        tasks.push(Task::Playback {
            verb: "play",
            args: json!({"file": file_id(editing, member), "start": play["start"], "pass": play["pass"]}),
        });
        tasks.push(Task::ShowTransport);
        tasks
    }

    fn script_stop(&mut self, editing: &mut Editing, member: MemberId) -> Vec<Task> {
        let play = member_call(editing, member, json!({"verb": "play"}));
        vec![
            Task::Playback {
                verb: "stop",
                args: json!({"back": play["back"], "file": file_id(editing, member)}),
            },
            Task::ShowTransport,
        ]
    }

    fn script_locate(&mut self, editing: &mut Editing, member: MemberId, at: f64) -> Vec<Task> {
        let frame = member_call(editing, member, json!({"verb": "locate", "at": at}))["cue"]
            .as_u64()
            .unwrap_or(0);
        vec![
            effect("cursor", json!({"at": at})),
            Task::Rolling(Rolled::Locate(frame)),
        ]
    }

    /// A pass in progress ends as the span and the switch now say.
    fn follow(&mut self, editing: &mut Editing, member: MemberId) -> Vec<Task> {
        if !self.focused(editing, member) {
            return Vec::new();
        }
        let play = member_call(editing, member, json!({"verb": "play"}));
        vec![Task::Playback {
            verb: "pass",
            args: json!({"pass": play["pass"]}),
        }]
    }

    fn push(&mut self, id: u64, tasks: Vec<Task>) {
        if let Some(run) = self.runs.get_mut(&id) {
            for task in tasks.into_iter().rev() {
                run.tasks.push_front(task);
            }
        }
    }

    fn resume(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        result: &Value,
        ids: &mut IdSpaces,
    ) -> Value {
        let Some(waiting) = self.runs.get_mut(&id).and_then(|run| run.waiting.take()) else {
            return json!({"error": "no run is waiting under that number"});
        };
        let failed = result.get("failed").and_then(Value::as_str);
        let mut effects = Vec::new();
        match (waiting, failed) {
            // a batch that failed is the operation's to hear, not raised
            (
                Resume::Batch {
                    ticket,
                    batch,
                    repairing,
                    member,
                    rendered,
                },
                failed,
            ) => {
                // **A failed batch leaves nothing on disk.** Its renders up to
                // the failure wrote files no list will name -- the pieces of a
                // reverse of a day before the disk filled -- and they would sit
                // in the working folder until the editor closed.
                if failed.is_some()
                    && let Some(member) = member.filter(|_| !rendered.is_empty())
                {
                    let scratch = member_call(
                        editing,
                        member,
                        json!({"verb": "scratch", "files": rendered}),
                    );
                    if truthy(&scratch["files"]) {
                        effects.push(json!({"kind": "delete", "files": scratch["files"]}));
                    }
                }
                let status = match failed {
                    Some(reason) => json!({"status": "failed", "reason": reason}),
                    None => json!({"status": "success"}),
                };
                let answer = door(
                    editing,
                    json!({"verb": "operation", "action": "complete", "ticket": ticket,
                        "batch": batch, "result": status}),
                );
                self.push(id, vec![Task::OperationAnswer { answer, repairing }]);
            }
            (
                Resume::Recovery {
                    ticket, repairing, ..
                },
                Some(reason),
            ) => {
                let answer = door(
                    editing,
                    json!({"verb": "operation", "action": "repairFailed",
                        "ticket": ticket, "reason": reason}),
                );
                self.push(id, vec![Task::OperationDone { answer, repairing }]);
            }
            (
                Resume::Recovery {
                    ticket,
                    rest,
                    repairing,
                },
                None,
            ) => {
                if let Some((next, more)) = rest.split_first() {
                    if let Some(run) = self.runs.get_mut(&id) {
                        run.waiting = Some(Resume::Recovery {
                            ticket,
                            rest: more.to_vec(),
                            repairing,
                        });
                    }
                    return json!({"run": id, "done": false, "effects": [],
                        "await": tagged("batch", json!({"member": next["member"],
                            "server": next["server"], "steps": next["steps"]}))});
                }
                let answer = door(
                    editing,
                    json!({"verb": "operation", "action": "repaired", "ticket": ticket}),
                );
                self.push(id, vec![Task::OperationDone { answer, repairing }]);
            }
            // A look that failed finds nothing changed: the window's event
            // is answered all the same.
            (Resume::Stat { seq }, failed) => {
                let stamps = if failed.is_some() {
                    json!({})
                } else {
                    result["stamps"].clone()
                };
                let answer = member_call(
                    editing,
                    member,
                    json!({"verb": "on_disk", "stamps": stamps, "seq": seq}),
                );
                self.push(
                    id,
                    vec![effect("echo", json!({"answer": answer["answer"]}))],
                );
            }
            (_, Some(reason)) => {
                if let Some(answer) = self.unwind(
                    editing,
                    id,
                    Failure::Client(reason.to_string()),
                    &mut effects,
                ) {
                    return answer;
                }
            }
            (Resume::Steps, None) => {}
            (Resume::Clipboard, None) => {}
            (Resume::Read, None) => self.push(id, vec![Task::AddFile(result.clone())]),
            (Resume::Stepped, None) => {
                if let Some(run) = self.runs.get_mut(&id) {
                    run.result = json!(result["changed"] == true);
                }
            }
        }
        let mut answer = self.drive(editing, member, id, ids);
        if !effects.is_empty() {
            effects.extend(answer["effects"].as_array().cloned().unwrap_or_default());
            answer["effects"] = json!(effects);
        }
        answer
    }

    /// **The file in focus sounds on the engine**: its list handed to the
    /// engine, and the playback's readers set to the buffers the engine
    /// answers, one per channel.
    fn sync_held(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        file: &Value,
        ids: &mut IdSpaces,
    ) -> Flow {
        let rate = self.rate;
        let Some(wanted) = file["id"].as_u64() else {
            return Flow::Next;
        };
        let take = match editing.member(member) {
            Some(Member::Audio(editor)) => editor
                .held_takes(rate, |file, _| file == wanted)
                .into_iter()
                .next(),
            _ => None,
        };
        let (Some(take), Some(engine)) = (take, editing.engine.as_mut()) else {
            return Flow::Next;
        };
        let buffers = match engine.take(&take, ids) {
            Ok(buffers) => buffers,
            Err(e) => return Flow::Fail(Failure::Own(Raise::Runtime, e)),
        };
        let frames = file["frames"].as_u64().unwrap_or(0);
        let take_rate = file["rate"].as_f64().unwrap_or(0.0);
        if let Err(e) = self
            .playback
            .sync_streams(wanted, buffers, frames, take_rate, rate, ids)
        {
            return Flow::Fail(Failure::Own(Raise::Runtime, e.to_string()));
        }
        self.push(id, vec![Task::RetainPlayback]);
        Flow::Next
    }

    /// What the engine's answer whether the transport rolls leads to.
    fn rolled(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        rolled: Rolled,
        rolling: bool,
    ) -> Vec<Task> {
        let file = file_id(editing, member);
        match rolled {
            Rolled::View { frame } => vec![
                Task::Playback {
                    verb: "view",
                    args: json!({"file": file, "frame": frame}),
                },
                // the meter follows the file now viewed, before the window is drawn
                Task::TellMeters,
            ],
            Rolled::Space(play) => vec![
                // whichever file plays stops, and the one shown comes into focus
                if rolling {
                    Task::Playback {
                        verb: "stop",
                        args: json!({"back": play["back"], "file": file}),
                    }
                } else {
                    Task::Playback {
                        verb: "play",
                        args: json!({"file": file, "start": play["start"], "pass": play["pass"]}),
                    }
                },
                Task::ShowTransport,
            ],
            // **A placed cursor moves the music where it plays**: the file in
            // focus rolling goes on from the cursor, its pass ending as the
            // switch says from there; stopped, the transport stands on it.
            Rolled::Cue(frame) if rolling && self.focused(editing, member) => {
                let play = member_call(editing, member, json!({"verb": "play"}));
                let mut tasks = vec![Task::Playback {
                    verb: "locate",
                    args: json!({"frame": frame}),
                }];
                if play["pass"].is_object() {
                    tasks.push(Task::Playback {
                        verb: "pass",
                        args: json!({"pass": play["pass"]}),
                    });
                }
                tasks.push(Task::ShowTransport);
                tasks
            }
            Rolled::Cue(frame) => vec![
                Task::Playback {
                    verb: "cue",
                    args: json!({"frame": frame}),
                },
                Task::ShowTransport,
            ],
            Rolled::Playing { was } => {
                let mut tasks = Vec::new();
                if was != rolling {
                    tasks.push(Task::ShowTransport);
                }
                tasks.push(Task::Answer(json!(
                    rolling && self.focused(editing, member)
                )));
                tasks
            }
            Rolled::Stop if rolling => self.script_stop(editing, member),
            Rolled::Stop => Vec::new(),
            Rolled::Locate(frame) => vec![Task::Playback {
                verb: if rolling && self.focused(editing, member) {
                    "locate"
                } else {
                    "cue"
                },
                args: json!({"frame": frame}),
            }],
        }
    }

    /// Carry the run on until it waits or ends.
    fn drive(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        ids: &mut IdSpaces,
    ) -> Value {
        let mut effects = Vec::new();
        loop {
            let Some(task) = self.runs.get_mut(&id).and_then(|run| run.tasks.pop_front()) else {
                retake(editing, member, &mut self.playback, self.rate, ids);
                let run = self.runs.remove(&id).unwrap_or_default();
                return json!({"run": id, "done": true, "effects": effects, "result": run.result});
            };
            match self.step(editing, member, id, task, &mut effects, ids) {
                Flow::Next => {}
                Flow::Await(request, resume) => {
                    retake(editing, member, &mut self.playback, self.rate, ids);
                    if let Some(run) = self.runs.get_mut(&id) {
                        run.waiting = Some(resume);
                    }
                    return json!({"run": id, "done": false, "effects": effects, "await": request});
                }
                Flow::Fail(failure) => {
                    if let Some(answer) = self.unwind(editing, id, failure, &mut effects) {
                        return answer;
                    }
                }
            }
        }
    }

    /// **A failure unwinds the run**: the cleanups it passes run and the
    /// handlers it passes see it; nothing in this vocabulary swallows one, so
    /// the run ends and the client raises it. Answers the ended run.
    fn unwind(
        &mut self,
        editing: &Editing,
        id: u64,
        failure: Failure,
        effects: &mut Vec<Value>,
    ) -> Option<Value> {
        let run = self.runs.remove(&id).unwrap_or_default();
        for task in run.tasks {
            match task {
                Task::Finally(Cleanup::Lease) => effects.push(tagged("lease", json!({}))),
                Task::Catch(Handler::Clipboard { outcome }) if has(&outcome, "clipboard") => {
                    let mut answer = outcome["answer"].clone();
                    if answer.is_object() {
                        answer["reason"] = json!(failure.reason());
                        answer["docVersion"] = json!(editing.version());
                    }
                    effects.push(tagged("echo", json!({"answer": answer})));
                }
                _ => {}
            }
        }
        let mut answer = json!({"run": id, "done": true, "effects": std::mem::take(effects)});
        match failure {
            Failure::Client(reason) => {
                answer["rethrow"] = json!(true);
                answer["reason"] = json!(reason);
            }
            Failure::Own(raise, message) => {
                answer["error"] = json!({"type": raise.name(), "message": message});
            }
        }
        Some(answer)
    }

    fn step(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        task: Task,
        effects: &mut Vec<Value>,
        ids: &mut IdSpaces,
    ) -> Flow {
        let window = self.runs.get(&id).is_some_and(|run| run.window);
        match task {
            Task::Effect(value) => {
                effects.push(value);
                Flow::Next
            }
            Task::Answer(value) => {
                if let Some(run) = self.runs.get_mut(&id) {
                    run.result = value;
                }
                Flow::Next
            }
            Task::Reason(reason) => {
                if let Some(run) = self.runs.get_mut(&id) {
                    run.result = json!({"changed": run.result == true, "reason": reason});
                }
                Flow::Next
            }
            Task::Destination => {
                if let Some(run) = self.runs.get_mut(&id) {
                    run.result = json!({"destination": run.completed["destination"]});
                }
                Flow::Next
            }
            Task::Finally(Cleanup::Lease) => {
                effects.push(tagged("lease", json!({})));
                Flow::Next
            }
            // reached without a failure: nothing to handle
            Task::Catch(_) => Flow::Next,
            Task::Steps { via, steps } => Flow::Await(
                tagged("steps", json!({"via": via, "steps": steps})),
                Resume::Steps,
            ),
            Task::Stepped(stepped) => Flow::Await(
                tagged("stepped", json!({"stepped": stepped})),
                Resume::Stepped,
            ),
            Task::Take { outcome, lease } => match self.take(id, outcome, lease) {
                Ok(tasks) => {
                    self.push(id, tasks);
                    Flow::Next
                }
                Err(failure) => Flow::Fail(failure),
            },
            Task::VerbTake(call) => {
                let answer = member_call(editing, member, call);
                if answer["error"] == NO_FILE {
                    return Flow::Fail(Failure::Own(Raise::Value, NO_FILE.into()));
                }
                self.push(
                    id,
                    vec![Task::Take {
                        outcome: answer,
                        lease: false,
                    }],
                );
                Flow::Next
            }
            Task::Bound {
                outcome,
                close_file,
                lease,
            } => {
                let mut tasks = Vec::new();
                // a closed file's renders: its history went with it
                if truthy(&outcome["deleteFiles"]) {
                    tasks.push(effect("delete", json!({"files": outcome["deleteFiles"]})));
                }
                if close_file {
                    tasks.push(Task::RetainPlayback);
                }
                if outcome["close"] == true {
                    tasks.push(Task::Close { closed: false });
                    tasks.push(Task::Answer(json!(false)));
                    self.push(id, tasks);
                    return Flow::Next;
                }
                if outcome["focusChanged"] == true && outcome["binding"].is_null() {
                    // the last file closed: the editor stays, with none
                    self.paused = false;
                    tasks.push(effect("bind", json!({"binding": null})));
                } else if outcome["focusChanged"] == true {
                    // the file in focus brings back the span and the loop
                    // switch it left, which are its own
                    self.paused = false;
                    tasks.push(effect("bind", json!({"binding": outcome["binding"]})));
                    tasks.push(Task::Sound);
                    // a pass in progress goes on: the window only looks elsewhere
                    tasks.push(Task::Rolling(Rolled::View {
                        frame: outcome["binding"]["cursor"].as_u64().unwrap_or(0),
                    }));
                }
                let mut rest = outcome;
                rest["steps"] = json!([]);
                tasks.push(Task::TakeRest {
                    outcome: rest,
                    lease,
                });
                self.push(id, tasks);
                Flow::Next
            }
            Task::TakeRest { outcome, lease } => {
                let tasks = self.take_rest(id, outcome, lease, window);
                self.push(id, tasks);
                Flow::Next
            }
            Task::TakeTail(outcome) => {
                let changed = self.runs.get(&id).is_some_and(|run| run.changed);
                let tasks = self.take_tail(editing, member, outcome, changed);
                self.push(id, tasks);
                Flow::Next
            }
            Task::TakeCompleted => {
                let completed = self
                    .runs
                    .get(&id)
                    .map(|run| run.completed.clone())
                    .unwrap_or_default();
                let outcome = completed.get("outcome").cloned().unwrap_or(json!({}));
                if let Some(run) = self.runs.get_mut(&id) {
                    run.changed = outcome["changed"] == true;
                }
                self.push(
                    id,
                    vec![Task::Take {
                        outcome,
                        lease: false,
                    }],
                );
                Flow::Next
            }
            Task::PublishClipboard(outcome) => Flow::Await(
                tagged(
                    "clipboard",
                    json!({"document": outcome["clipboard"],
                        "ready": outcome["clipboardReady"] == true}),
                ),
                Resume::Clipboard,
            ),
            Task::Cut(outcome) => {
                self.push(id, vec![Task::CutAct(outcome)]);
                Flow::Next
            }
            Task::CutAct(outcome) => {
                let continued = door(
                    editing,
                    json!({"verb": "act", "member": member, "call": outcome["clipboardCut"]}),
                );
                let mut next = continued.get("outcome").cloned().unwrap_or(json!({}));
                if next["answer"].is_object() {
                    next["answer"]["seq"] = match &outcome["answer"]["seq"] {
                        Value::Null => json!(0),
                        seq => seq.clone(),
                    };
                }
                if let Some(run) = self.runs.get_mut(&id) {
                    run.changed = next["changed"] == true;
                }
                let mut tasks = vec![effect("version", json!({"version": continued["version"]}))];
                tasks.push(Task::Take {
                    outcome: next,
                    lease: false,
                });
                tasks.extend(release(&continued["freed"]));
                self.push(id, tasks);
                Flow::Next
            }
            Task::Stat(check) => Flow::Await(
                tagged("stat", json!({"paths": check["paths"]})),
                Resume::Stat {
                    seq: check["seq"].clone(),
                },
            ),
            Task::Read(mut files) => {
                let Some(file) = files.pop_front() else {
                    return Flow::Next;
                };
                if !files.is_empty() {
                    self.push(id, vec![Task::Read(files)]);
                }
                let request = match file.get("shape") {
                    Some(shape) => json!({"shape": shape}),
                    None => json!({"path": file["path"]}),
                };
                Flow::Await(tagged("shape", request), Resume::Read)
            }
            Task::AddFile(file) => {
                let mut call = file;
                call["verb"] = json!("add_file");
                let answer = member_call(editing, member, call);
                let mut tasks = Vec::new();
                if has(&answer, "error") {
                    tasks.extend(release(&answer["freed"]));
                }
                tasks.push(Task::Take {
                    outcome: answer,
                    lease: false,
                });
                self.push(id, tasks);
                Flow::Next
            }
            Task::Operation {
                command,
                prepared,
                repairing,
            } => {
                let answer = match prepared {
                    Some(prepared) => prepared,
                    None => door(
                        editing,
                        json!({"verb": "operation", "action": "prepare", "member": member,
                            "command": command.unwrap_or(json!({}))}),
                    ),
                };
                self.push(id, vec![Task::OperationAnswer { answer, repairing }]);
                Flow::Next
            }
            Task::OperationAnswer { answer, repairing } => {
                if answer["complete"] != true {
                    if answer["accepted"] == false {
                        return Flow::Fail(Failure::Own(
                            Raise::Runtime,
                            "the operation is no longer available".into(),
                        ));
                    }
                    if let Some(error) = answer["error"].as_str() {
                        return Flow::Fail(Failure::Own(Raise::Value, error.into()));
                    }
                    return Flow::Await(
                        tagged(
                            "batch",
                            json!({"member": answer["member"], "server": answer["server"],
                                "steps": answer.get("steps").cloned().unwrap_or(json!([]))}),
                        ),
                        Resume::Batch {
                            ticket: answer["ticket"].clone(),
                            batch: answer["batch"].clone(),
                            repairing,
                            member: answer["member"]
                                .as_u64()
                                .and_then(|m| MemberId::try_from(m).ok()),
                            rendered: rendered_files(&answer["steps"]),
                        },
                    );
                }
                if has(&answer, "repair") {
                    let recoveries = answer["recoveries"].as_array().cloned().unwrap_or_default();
                    if let Some((first, rest)) = recoveries.split_first() {
                        return Flow::Await(
                            tagged(
                                "batch",
                                json!({"member": first["member"], "server": first["server"],
                                    "steps": first["steps"]}),
                            ),
                            Resume::Recovery {
                                ticket: answer["repair"].clone(),
                                rest: rest.to_vec(),
                                repairing,
                            },
                        );
                    }
                    let answer = door(
                        editing,
                        json!({"verb": "operation", "action": "repaired", "ticket": answer["repair"]}),
                    );
                    self.push(id, vec![Task::OperationDone { answer, repairing }]);
                    return Flow::Next;
                }
                self.push(id, vec![Task::OperationDone { answer, repairing }]);
                Flow::Next
            }
            Task::OperationDone { answer, repairing } => {
                if has(&answer, "version") {
                    effects.push(tagged("version", json!({"version": answer["version"]})));
                }
                if has(&answer, "echo") {
                    effects.push(tagged("operationEcho", json!({"echo": answer["echo"]})));
                }
                if truthy(&answer["freed"]) {
                    effects.push(tagged("release", json!({"freed": answer["freed"]})));
                }
                if let Some(run) = self.runs.get_mut(&id) {
                    run.completed = answer.clone();
                    run.result = json!({"completed": answer.clone()});
                }
                if answer["committed"] != true && !(repairing && answer["repaired"] == true) {
                    if !has(&answer, "echo") {
                        effects.push(tagged(
                            "echo",
                            json!({"answer": answer["outcome"]["answer"]}),
                        ));
                    }
                    let reason = answer["reason"]
                        .as_str()
                        .or_else(|| answer["outcome"]["answer"]["reason"].as_str())
                        .unwrap_or("the operation was refused")
                        .to_string();
                    return Flow::Fail(Failure::Own(Raise::Command, reason));
                }
                Flow::Next
            }
            Task::Sound => {
                let file = member_call(editing, member, json!({"verb": "playback"}));
                if file["active"] == false || !file.is_object() {
                    return Flow::Next;
                }
                let tasks = vec![Task::SyncSound(file), Task::TellMeters];
                self.push(id, tasks);
                Flow::Next
            }
            Task::SyncSound(file) => {
                if file["active"] == false {
                    return Flow::Next;
                }
                self.sync_held(editing, member, id, &file, ids)
            }
            Task::TellMeters => {
                let meters = self.meters();
                member_call(editing, member, json!({"verb": "sync", "meters": meters}));
                Flow::Next
            }
            Task::ShowTransport => {
                if !window {
                    return Flow::Next;
                }
                let state = self.state();
                let answer = member_call(
                    editing,
                    member,
                    json!({"verb": "playing", "file": state["playing"], "meters": state["meters"]}),
                );
                for correction in answer["corrections"].as_array().into_iter().flatten() {
                    effects.push(tagged(
                        "set",
                        json!({"widget": correction["widget"], "props": correction["props"]}),
                    ));
                }
                Flow::Next
            }
            // the engine says whether it rolls
            Task::Rolling(rolled) => {
                let rolling = self.playback.transport().is_some_and(|transport| {
                    editing
                        .engine
                        .as_ref()
                        .is_some_and(|engine| engine.rolling(transport))
                });
                self.playback.set_rolling(rolling);
                let tasks = self.rolled(editing, member, rolled, rolling);
                self.push(id, tasks);
                Flow::Next
            }
            Task::Playback { verb, args } => {
                if verb == "closeFile"
                    && let (Some(engine), Some(file)) =
                        (editing.engine.as_mut(), args["file"].as_u64())
                {
                    engine.close(file, ids);
                }
                let mut request = args;
                request["verb"] = json!(verb);
                let answer: Value = serde_json::from_str(&audio_playback::call_json(
                    &mut self.playback,
                    &request.to_string(),
                    ids,
                ))
                .unwrap_or(Value::Null);
                if let Some(error) = answer["error"].as_str() {
                    return Flow::Fail(Failure::Own(Raise::Runtime, error.into()));
                }
                match answer.get("steps") {
                    Some(steps) if truthy(steps) => Flow::Await(
                        tagged("steps", json!({"via": "service", "steps": steps})),
                        Resume::Steps,
                    ),
                    _ => Flow::Next,
                }
            }
            Task::RetainPlayback => {
                let result = door(
                    editing,
                    json!({"verb": "retainMember", "member": member, "kind": "playback"}),
                );
                self.push(id, release(&result["freed"]).into_iter().collect());
                Flow::Next
            }
            Task::DropPlayback => {
                let result = door(
                    editing,
                    json!({"verb": "dropRoot", "root": {"kind": "playback", "owner": member}}),
                );
                self.push(id, release(&result["freed"]).into_iter().collect());
                Flow::Next
            }
            Task::Close { closed } => {
                member_call(editing, member, json!({"verb": "detach_view"}));
                self.push(
                    id,
                    vec![
                        Task::Playback {
                            verb: "close",
                            args: json!({}),
                        },
                        Task::DropPlayback,
                        effect(
                            if closed {
                                "closedWindow"
                            } else {
                                "closeWindow"
                            },
                            json!({}),
                        ),
                    ],
                );
                Flow::Next
            }
        }
    }

    fn meters(&self) -> Value {
        self.playback.meters().map_or(
            Value::Null,
            |(bus, channels)| json!({"bus": bus, "channels": channels}),
        )
    }

    fn state(&self) -> Value {
        json!({"playing": self.playback.playing(), "meters": self.meters(),
            "rolling": self.playback.rolling(), "focus": self.playback.focus()})
    }

    /// **An outcome, carried out**, up to its file binding: what the clients'
    /// turn handling did first.
    fn take(&mut self, id: u64, outcome: Value, lease: bool) -> Result<Vec<Task>, Failure> {
        if has(&outcome, "operation") {
            let mut tasks = vec![
                Task::Operation {
                    command: None,
                    prepared: Some(outcome["operation"].clone()),
                    repairing: false,
                },
                Task::TakeCompleted,
            ];
            if lease {
                tasks.push(Task::Finally(Cleanup::Lease));
            }
            return Ok(tasks);
        }
        if let Some(error) = outcome["error"].as_str() {
            return Err(Failure::Own(Raise::Value, error.into()));
        }
        if matches!(outcome["turn"].as_str(), None | Some("nothing")) {
            if let Some(run) = self.runs.get_mut(&id) {
                run.changed = false;
                run.result = json!(false);
            }
            return Ok(if lease {
                vec![effect("lease", json!({}))]
            } else {
                Vec::new()
            });
        }
        let action = &outcome["fileAction"];
        if !action.is_null() && outcome.get("binding").is_none() {
            return Ok(vec![
                effect("echo", json!({"answer": outcome["answer"]})),
                Task::VerbTake(json!({"verb": action["verb"], "file": action["file"]})),
            ]);
        }
        let close_file = action["verb"] == "close_file";
        let mut tasks = Vec::new();
        if close_file {
            tasks.push(Task::Playback {
                verb: "closeFile",
                args: json!({"file": action["file"]}),
            });
        }
        if outcome.get("binding").is_some() {
            if truthy(&outcome["steps"]) {
                tasks.push(Task::Steps {
                    via: "join",
                    steps: outcome["steps"].clone(),
                });
            }
            tasks.push(Task::Bound {
                outcome,
                close_file,
                lease,
            });
        } else {
            tasks.push(Task::TakeRest { outcome, lease });
        }
        Ok(tasks)
    }

    /// The rest of an outcome past its binding: the layers, a redraw, the
    /// steps and the clipboard.
    fn take_rest(&mut self, id: u64, outcome: Value, lease: bool, window: bool) -> Vec<Task> {
        let mut tasks = Vec::new();
        if has(&outcome, "layers") {
            tasks.push(effect("layers", json!({"layers": outcome["layers"]})));
        }
        // **A view placed in a lane of its own** (the stacked views, or back
        // to one): the window is drawn again and the host keeps what is its own.
        if outcome["restructure"] == true && window {
            tasks.push(effect("publish", json!({})));
            // the def carries no preview: each view is handed it again
            tasks.push(Task::Sound);
        }
        if let Some(run) = self.runs.get_mut(&id) {
            run.changed = outcome["changed"] == true;
        }
        // **The steps are the client's to carry out**: a new take made where
        // the turn needed one and the join stitched over the list -- or, for a
        // save from the window, the file written.
        let steps = outcome.get("steps").cloned().unwrap_or(json!([]));
        let copied = has(&outcome, "clipboard");
        if truthy(&steps) {
            tasks.push(Task::Steps {
                via: if copied || lease { "plain" } else { "join" },
                steps,
            });
        }
        if copied {
            tasks.push(Task::PublishClipboard(outcome.clone()));
            if has(&outcome, "clipboardCut") {
                tasks.push(Task::Cut(outcome.clone()));
            }
        }
        if copied {
            tasks.push(Task::Catch(Handler::Clipboard {
                outcome: outcome.clone(),
            }));
        }
        if lease {
            tasks.push(Task::Finally(Cleanup::Lease));
        }
        tasks.push(Task::TakeTail(outcome));
        tasks
    }

    /// The tail of an outcome: files, playback, the transport, the answer.
    fn take_tail(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        mut outcome: Value,
        changed: bool,
    ) -> Vec<Task> {
        let mut tasks = Vec::new();
        if has(&outcome, "newFile") || has(&outcome, "openFile") {
            tasks.push(effect("echo", json!({"answer": outcome["answer"]})));
            outcome["answer"] = Value::Null;
        }
        if has(&outcome, "newFile") {
            tasks.push(Task::Read(VecDeque::from([
                json!({"shape": outcome["newFile"]}),
            ])));
        }
        if has(&outcome, "openFile") {
            tasks.push(Task::Read(paths(&outcome["openFile"]["paths"])));
        }
        if has(&outcome, "checkDisk") {
            tasks.push(Task::Stat(outcome["checkDisk"].clone()));
        }
        if has(&outcome, "playbackUpdate") {
            let update = &outcome["playbackUpdate"];
            tasks.push(Task::SyncSound(update["file"].clone()));
            if update["publish"] == true {
                tasks.push(Task::Sound);
            }
        }
        if changed {
            tasks.push(effect("changed", json!({})));
            if outcome["preparedComplete"] != true {
                tasks.push(Task::Sound);
            }
        }
        if has(&outcome, "play") {
            // The loop switch rides beside the space bar and `L`: a pass that
            // loops is the switch on.
            self.paused = false;
            tasks.push(Task::Rolling(Rolled::Space(outcome["play"].clone())));
        }
        if has(&outcome, "pass") {
            // `L`: the pass in progress ends as the switch now says; a stopped
            // playback reads it on its next play.
            tasks.push(Task::Playback {
                verb: "pass",
                args: json!({"pass": outcome["pass"]}),
            });
        }
        if has(&outcome, "follow") {
            // A selection redrawn while this take loops: the loop is over the
            // new span, and the head goes into it once the sweep is let go.
            // Whether it loops, and whether it is this take, is the playback's
            // to say.
            let mut args = outcome["follow"].clone();
            args["file"] = json!(file_id(editing, member));
            tasks.push(Task::Playback {
                verb: "follow",
                args,
            });
        }
        if let Some(cue) = outcome["cue"].as_u64() {
            tasks.push(Task::Rolling(Rolled::Cue(cue)));
        }
        if has(&outcome, "locate") {
            tasks.push(effect("locate", json!({"at": outcome["locate"]})));
        }
        if !has(&outcome, "clipboardCut") {
            tasks.push(effect("echo", json!({"answer": outcome["answer"]})));
        }
        if outcome["focusChanged"] == true {
            tasks.push(Task::ShowTransport);
        }
        if outcome["close"] == true {
            tasks.push(Task::Close { closed: false });
        }
        tasks.push(Task::Answer(json!(changed)));
        tasks
    }
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

#[cfg(test)]
mod tests {
    use super::super::call_json_ids;
    use super::super::held::{HeldEngine, HeldTake};
    use super::*;
    use clausters_core::ids::{IdShare, ServerShape, Space};
    use clausters_editing::apply::{Applier, Endpoint};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// What the engine below has been told, as a test reads it.
    #[derive(Debug, Default)]
    struct Told {
        rolling: bool,
        /// The revision it reads of each file.
        revisions: HashMap<u64, u64>,
        /// The files closed, in order.
        closed: Vec<u64>,
    }

    /// **An engine that plays nothing**: its playback's ops are steps a
    /// test reads, its transport rolls as the client says, and a take handed
    /// to it is one buffer per channel.
    #[derive(Debug)]
    struct Engine(Arc<Mutex<Told>>);

    impl HeldEngine for Engine {
        fn medium(&mut self) -> Box<dyn Medium + Send> {
            Box::new(Applier::new(Endpoint { chunk: 4096 }))
        }
        fn rate(&self) -> f64 {
            48_000.0
        }
        fn take(&mut self, take: &HeldTake, ids: &mut IdSpaces) -> Result<Vec<i32>, String> {
            self.0
                .lock()
                .unwrap()
                .revisions
                .insert(take.file, take.revision);
            (0..take.channels)
                .map(|_| {
                    ids.alloc(Space::Buffers, 1)
                        .map(|b| b as i32)
                        .map_err(|e| e.to_string())
                })
                .collect()
        }
        fn revision(&self, file: u64) -> Option<u64> {
            self.0.lock().unwrap().revisions.get(&file).copied()
        }
        fn close(&mut self, file: u64, _ids: &mut IdSpaces) {
            let mut told = self.0.lock().unwrap();
            told.revisions.remove(&file);
            told.closed.push(file);
        }
        fn rolling(&self, _transport: i32) -> bool {
            self.0.lock().unwrap().rolling
        }
        fn control(&self, _bus: usize) -> f32 {
            0.0
        }
        fn starved(&self) -> u64 {
            0
        }
    }

    /// **A client that does what it is asked**: every await succeeds, the
    /// engine's transport rolls as `rolling` says, a file read is a
    /// two-channel take, and every effect and await is written down.
    struct Client {
        ids: IdSpaces,
        rolling: bool,
        engine: Arc<Mutex<Told>>,
        fail: Option<&'static str>,
        trace: Vec<String>,
        effects: Vec<Value>,
        /// What a look on disk finds, by path.
        stamps: Value,
    }

    impl Client {
        fn new() -> Client {
            Client {
                ids: IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE),
                rolling: false,
                engine: Arc::default(),
                fail: None,
                trace: Vec::new(),
                effects: Vec::new(),
                stamps: json!({}),
            }
        }

        fn call(&mut self, editing: &mut Editing, request: Value) -> Value {
            if editing.engine.is_none() {
                editing.hold_engine(Box::new(Engine(Arc::clone(&self.engine))));
            }
            self.engine.lock().unwrap().rolling = self.rolling;
            serde_json::from_str(&call_json_ids(editing, &request.to_string(), &mut self.ids))
                .unwrap()
        }

        /// Run one action to its end, answering its awaits.
        fn run(&mut self, editing: &mut Editing, member: MemberId, action: Value) -> Value {
            let first = self.begin(editing, member, action);
            self.finish(editing, member, first)
        }

        /// **Start an action and stop at its first await**, which is handed
        /// back unanswered: a service that has not answered yet. The effects
        /// decided before it are carried out.
        fn begin(&mut self, editing: &mut Editing, member: MemberId, action: Value) -> Value {
            let answer = self.call(
                editing,
                json!({"verb": "audio", "member": member, "do": action}),
            );
            self.log(&answer);
            answer
        }

        /// Answer the await `pending` stopped at with `result`, and stop at
        /// the next one or at the end.
        fn answer(
            &mut self,
            editing: &mut Editing,
            member: MemberId,
            pending: &Value,
            result: Value,
        ) -> Value {
            let answer = self.call(
                editing,
                json!({"verb": "audio", "member": member, "run": pending["run"], "result": result}),
            );
            self.log(&answer);
            answer
        }

        /// Carry a run on from `answer` to its end, answering every await
        /// as the service would.
        fn finish(&mut self, editing: &mut Editing, member: MemberId, mut answer: Value) -> Value {
            while answer["done"] != true {
                let result = self.reply(editing, member, &answer["await"]);
                answer = self.answer(editing, member, &answer, result);
            }
            answer
        }

        fn log(&mut self, answer: &Value) {
            for effect in answer["effects"].as_array().cloned().unwrap_or_default() {
                self.trace
                    .push(effect["kind"].as_str().unwrap().to_string());
                self.effects.push(effect);
            }
            if answer["done"] != true {
                let kind = answer["await"]["kind"].as_str().unwrap_or_default();
                self.trace.push(format!("await:{kind}"));
            }
        }

        /// What the service answers an await with: a failure when `fail`
        /// names its kind, else what it asked for. A history step is carried
        /// out as a client carries it: its operation run to its end.
        fn reply(&mut self, editing: &mut Editing, member: MemberId, wait: &Value) -> Value {
            let kind = wait["kind"].as_str().unwrap_or_default();
            if self.fail == Some(kind) {
                self.fail = None;
                return json!({"failed": "refused"});
            }
            match kind {
                "stat" => json!({"stamps": self.stamps}),
                "shape" => {
                    json!({"frames": 200, "channels": 2, "rate": 48000.0,
                        "path": wait["path"], "untitled": false})
                }
                "stepped" => {
                    let prepared = wait["stepped"]["operation"].clone();
                    if prepared.is_object() {
                        self.run(
                            editing,
                            member,
                            json!({"verb": "operation", "prepared": prepared}),
                        );
                    }
                    json!({"changed": true})
                }
                _ => json!({}),
            }
        }
    }

    /// The file in focus as the editor binds it.
    fn binding(editing: &mut Editing, member: MemberId) -> Value {
        member_call(editing, member, json!({"verb": "binding"}))
    }

    /// A cut of 20 frames from frame 10 of the file in focus, from its picture.
    fn cut(seq: i64, editing: &Editing) -> Value {
        json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [12, seq, editing.version(), "cut", 10.0, 20.0]})
    }

    /// The same 20 frames reversed: an edit that renders, so its run waits
    /// on a batch.
    fn reverse(seq: i64, editing: &Editing) -> Value {
        json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [12, seq, editing.version(), "reverse", 10.0, 20.0]})
    }

    fn save(path: &str) -> Value {
        json!({"verb": "save", "request": {"path": path, "format": "float"}})
    }

    /// Carry `pending` on to the first await of `kind`, answering the
    /// others; that one is handed back unanswered.
    fn until(
        client: &mut Client,
        editing: &mut Editing,
        member: MemberId,
        mut pending: Value,
        kind: &str,
        nth: usize,
    ) -> Value {
        let mut seen = 0;
        loop {
            assert_ne!(
                pending["done"], true,
                "the run ended before its {kind}: {pending}"
            );
            if pending["await"]["kind"] == kind {
                seen += 1;
                if seen == nth {
                    return pending;
                }
            }
            let result = client.reply(editing, member, &pending["await"]);
            pending = client.answer(editing, member, &pending, result);
        }
    }

    fn opened(client: &mut Client, editing: &mut Editing, empty: bool) -> MemberId {
        let request = if empty {
            json!({"verb": "openAudio", "key": "audio:empty", "empty": true})
        } else {
            json!({"verb": "openAudio", "key": "take:3", "path":"/in/take3.wav","workspace":"/work", "frames": 100})
        };
        let member = client.call(editing, request)["member"].as_u64().unwrap() as MemberId;
        client.call(
            editing,
            json!({"verb": "member", "member": member, "call": {"verb": "sync", "window": 900}}),
        );
        client.call(
            editing,
            json!({"verb": "member", "member": member,
                "call": {"verb": "window", "widget": 12, "meter": 13, "filesWidget": 14}}),
        );
        let answer = client.run(
            editing,
            member,
            json!({"verb": "open", "chunk": 4096, "rate": 48000.0, "empty": empty, "window": true}),
        );
        assert!(answer["error"].is_null(), "{answer}");
        assert!(answer["result"]["transport"].is_i64(), "{answer}");
        member
    }

    fn position(trace: &[String], kind: &str) -> usize {
        trace
            .iter()
            .position(|k| k == kind)
            .unwrap_or_else(|| panic!("no {kind} in {trace:?}"))
    }

    /// **The window coming back looks at the files on disk**: the
    /// activation asks the client for their stamps, and a file that changed
    /// is said in the answer to the window's event.
    #[test]
    fn a_window_coming_back_is_told_a_file_changed_on_disk() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = client.call(
            &mut editing,
            json!({"verb": "openAudio", "key": "take:3", "path": "/in/take3.wav",
                "workspace": "/work", "frames": 100,
                "stamp": {"modified": 1.0, "size": 10}}),
        )["member"]
            .as_u64()
            .unwrap() as MemberId;
        client.call(
            &mut editing,
            json!({"verb": "member", "member": member, "call": {"verb": "sync", "window": 900}}),
        );
        client.call(
            &mut editing,
            json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": 12}}),
        );
        client.run(
            &mut editing,
            member,
            json!({"verb": "open", "chunk": 4096, "rate": 48000.0, "empty": false, "window": true}),
        );
        client.stamps = json!({"/in/take3.wav": {"modified": 2.0, "size": 10}});
        client.trace.clear();
        client.effects.clear();
        let version = editing.version();
        let back = client.run(
            &mut editing,
            member,
            json!({"verb": "event", "addr": "/gui_event", "window": true,
                "args": [900, 7, version, "activated", 1]}),
        );
        assert!(back["error"].is_null(), "{back}");
        assert!(
            client.trace.contains(&"await:stat".to_string()),
            "{:?}",
            client.trace
        );
        let said = client
            .effects
            .iter()
            .find(|e| e["kind"] == "echo")
            .map(|e| e["answer"].clone())
            .unwrap();
        assert_eq!(said["seq"], 7);
        assert_eq!(said["reason"], "take3.wav changed on disk");
    }

    #[test]
    fn opening_measures_the_take_and_hands_it_to_the_engine() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        opened(&mut client, &mut editing, false);
        let trace = &client.trace;
        // the peaks are an operation's batch, and the engine reads the take
        assert!(trace.contains(&"await:batch".to_string()), "{trace:?}");
        assert_eq!(client.engine.lock().unwrap().revisions.len(), 1);
        assert!(
            !trace.contains(&"dirty".to_string()),
            "dirty is read from the editor, never projected: {trace:?}"
        );
    }

    #[test]
    fn an_empty_editor_opens_with_nothing_to_sound() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        opened(&mut client, &mut editing, true);
        assert!(
            !client.trace.iter().any(|k| k.starts_with("await:")),
            "{:?}",
            client.trace
        );
        let refused = client.run(&mut editing, 0, json!({"verb": "play"}));
        assert_eq!(refused["error"]["message"], NO_FILE);
    }

    #[test]
    fn an_opened_file_comes_into_focus_and_its_meter_is_told_before_the_redraw() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, true);
        client.trace.clear();
        let answer = client.run(
            &mut editing,
            member,
            json!({"verb": "openFile", "paths": ["/takes/a.wav"], "window": true}),
        );
        assert!(answer["error"].is_null(), "{answer}");
        let trace = &client.trace;
        let bind = position(trace, "bind");
        let publish = position(trace, "publish");
        assert!(position(trace, "await:shape") < bind, "{trace:?}");
        assert!(bind < publish, "{trace:?}");
        // the meter is known when the window is drawn again
        let session = editing.sessions.get(&member).unwrap();
        assert!(session.playback.meters().is_some());
        // and nothing is set on a widget the window does not draw
        assert!(file_id(&editing, member).is_some());
    }

    #[test]
    fn the_space_bar_plays_or_stops_as_the_transport_rolls() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let play = |client: &mut Client, editing: &mut Editing| {
            client.trace.clear();
            client.run(
                editing,
                member,
                json!({"verb": "event", "addr": "/gui_event",
                    "args": [900, 1, 0, "play", 0], "window": true}),
            )
        };
        play(&mut client, &mut editing);
        let trace = client.trace.clone();
        assert!(trace.contains(&"await:steps".to_string()), "{trace:?}");
        assert!(
            trace.contains(&"set".to_string()),
            "the play cursor: {trace:?}"
        );
        assert_eq!(
            editing.sessions[&member].playback.playing(),
            editing.sessions[&member].playback.focus()
        );
        client.rolling = true;
        play(&mut client, &mut editing);
        assert_eq!(
            editing.sessions[&member].playback.playing(),
            None,
            "stopped"
        );
    }

    #[test]
    fn a_failed_await_ends_the_run_for_the_client_to_raise() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        client.fail = Some("steps");
        let answer = client.run(
            &mut editing,
            member,
            json!({"verb": "play", "window": true}),
        );
        assert_eq!(answer["done"], true);
        assert_eq!(answer["rethrow"], true, "{answer}");
        assert_eq!(answer["reason"], "refused");
    }

    #[test]
    fn closing_the_last_file_draws_the_empty_window_and_corrects_nothing_absent() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        client.trace.clear();
        client.effects.clear();
        let answer = client.run(
            &mut editing,
            member,
            json!({"verb": "verb", "call": {"verb": "close_file"}, "window": true}),
        );
        assert!(answer["error"].is_null(), "{answer}");
        let bind = client
            .effects
            .iter()
            .find(|e| e["kind"] == "bind")
            .expect("a bind");
        assert!(bind["binding"].is_null());
        assert!(client.trace.contains(&"publish".to_string()));
        assert!(
            !client
                .effects
                .iter()
                .any(|e| e["kind"] == "set" && e["widget"] == 13),
            "the meter is not drawn: {:?}",
            client.effects
        );
    }

    #[test]
    fn a_script_reads_the_span_and_the_loop_switch_it_set() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        client.run(
            &mut editing,
            member,
            json!({"verb": "span", "span": [0.0, 0.001]}),
        );
        client.run(&mut editing, member, json!({"verb": "looping", "on": true}));
        let state = client.call(
            &mut editing,
            json!({"verb": "audio", "member": member, "query": "transport"}),
        );
        assert_eq!(state["span"], json!([0.0, 0.001]));
        assert_eq!(state["looping"], true);
    }

    /// **The loop switch and the span are the file's**, one state a script,
    /// a hand and the window's menu all read: a script's switch is the menu's
    /// check, and a file brought back into focus brings back its own.
    #[test]
    fn a_script_s_loop_switch_is_the_menu_s_check_and_each_file_keeps_its_own() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let checked = |client: &mut Client, editing: &mut Editing| {
            client.call(editing, json!({"verb": "actions", "member": member}))["loop"]["checked"]
                .clone()
        };
        let transport = |client: &mut Client, editing: &mut Editing| {
            client.call(
                editing,
                json!({"verb": "audio", "member": member, "query": "transport"}),
            )
        };
        client.run(&mut editing, member, json!({"verb": "looping", "on": true}));
        assert_eq!(
            checked(&mut client, &mut editing),
            true,
            "the menu reads the switch"
        );
        client.run(
            &mut editing,
            member,
            json!({"verb": "span", "span": [0.0, 0.001]}),
        );
        client.run(
            &mut editing,
            member,
            json!({"verb": "openFile", "paths": ["/takes/b.wav"], "window": true}),
        );
        let other = transport(&mut client, &mut editing);
        assert_eq!(other["looping"], false, "the new file's own switch");
        assert!(other["span"].is_null(), "and its own span");
        assert_eq!(checked(&mut client, &mut editing), false);
        client.run(
            &mut editing,
            member,
            json!({"verb": "verb", "call": {"verb": "focus_file", "index": 0}, "window": true}),
        );
        let first = transport(&mut client, &mut editing);
        assert_eq!(first["looping"], true);
        assert_eq!(first["span"], json!([0.0, 0.001]));
        assert_eq!(checked(&mut client, &mut editing), true);
    }

    /// **An edit while the write is out**: the save releases the take once
    /// its snapshot is copied, so a cut lands while the file is written; the
    /// write completing afterwards cleans only the revision it wrote, and an
    /// undo back to that revision is clean.
    #[test]
    fn an_edit_during_a_save_stays_dirty_and_its_undo_is_clean() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let first = client.begin(&mut editing, member, save("/takes/a.wav"));
        let write = until(&mut client, &mut editing, member, first, "batch", 1);
        let action = cut(1, &editing);

        let edit = client.run(&mut editing, member, action);
        assert!(
            edit["error"].is_null(),
            "an edit is taken while the write is out: {edit}"
        );
        assert_eq!(edit["result"], true, "the cut changed the take: {edit}");
        let saved = client.finish(&mut editing, member, write);
        assert!(saved["error"].is_null(), "{saved}");
        assert_eq!(saved["result"]["destination"]["path"], "/takes/a.wav");
        let now = binding(&mut editing, member);
        assert_eq!(now["dirty"], true, "the cut is newer than what was written");
        assert_eq!(
            now["file"]["path"], "/takes/a.wav",
            "the target moved on success"
        );
        assert_eq!(now["frames"], 80);
        let version = editing.version();
        let undo = client.run(
            &mut editing,
            member,
            json!({"verb": "event", "addr": "/gui_event", "window": true,
                "args": [900, 2, version, "undo"]}),
        );
        assert!(undo["error"].is_null(), "{undo}");
        let back = binding(&mut editing, member);
        assert_eq!(back["frames"], 100);
        assert_eq!(back["dirty"], false, "the saved revision again");
    }

    /// **A write the server refuses** keeps the file dirty and its target
    /// where it was; the same save, retried, succeeds.
    #[test]
    fn a_failed_save_keeps_the_target_and_a_retry_writes_it() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let action = cut(1, &editing);

        client.run(&mut editing, member, action);
        let before = binding(&mut editing, member);
        let first = client.begin(&mut editing, member, save("/takes/refused.wav"));
        let write = until(&mut client, &mut editing, member, first, "batch", 1);
        let failed = client.answer(
            &mut editing,
            member,
            &write,
            json!({"failed": "quota exceeded"}),
        );
        let failed = client.finish(&mut editing, member, failed);
        assert_eq!(failed["done"], true);
        assert!(
            failed["error"].is_object() || failed["rethrow"] == true,
            "the client raises the refusal: {failed}"
        );
        let after = binding(&mut editing, member);
        assert_eq!(after["dirty"], true);
        assert_eq!(after["file"]["path"], before["file"]["path"]);
        let saved = client.run(&mut editing, member, save("/takes/refused.wav"));
        assert!(saved["error"].is_null(), "{saved}");
        let clean = binding(&mut editing, member);
        assert_eq!(clean["dirty"], false);
        assert_eq!(clean["file"]["path"], "/takes/refused.wav");
    }

    /// **The window's work called off**: its `"call_off"` is answered with
    /// the effect that has the client stop its renders, and nothing of the
    /// take moves.
    #[test]
    fn a_window_call_off_has_the_client_stop_its_renders() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let before = binding(&mut editing, member);
        client.effects.clear();
        let call_off = json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [900, 3, editing.version(), "call_off"]});
        let answer = client.run(&mut editing, member, call_off);
        assert!(answer["error"].is_null(), "{answer}");
        assert!(
            client.effects.iter().any(|e| e["kind"] == "call_off"),
            "{:?}",
            client.effects
        );
        assert_eq!(binding(&mut editing, member), before);
    }

    /// **A failed render leaves nothing in the working folder**: a reverse
    /// whose render the disk refuses deletes what it wrote there, its peaks
    /// with it, and a save that fails deletes nothing the user named.
    #[test]
    fn a_failed_render_deletes_its_scratch_and_nothing_else() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let reverse = json!({"verb": "event", "addr": "/gui_event", "window": true,
            "args": [12, 1, editing.version(), "reverse", 10.0, 20.0]});
        let first = client.begin(&mut editing, member, reverse);
        let render = until(&mut client, &mut editing, member, first, "batch", 1);
        let failed = client.answer(
            &mut editing,
            member,
            &render,
            json!({"failed": "Disk quota exceeded"}),
        );
        let deleted = |answer: &Value| -> Vec<String> {
            answer["effects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| e["kind"] == "delete")
                .flat_map(|e| e["files"].as_array().cloned().unwrap_or_default())
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect()
        };
        let files = deleted(&failed);
        assert!(!files.is_empty(), "{failed}");
        assert!(files.iter().all(|f| f.starts_with("/work/")), "{files:?}");
        assert!(files.iter().any(|f| f.ends_with(".w64")), "{files:?}");
        assert!(files.iter().any(|f| f.ends_with(".peaks")), "{files:?}");
        client.finish(&mut editing, member, failed);

        let first = client.begin(&mut editing, member, save("/takes/refused.wav"));
        let write = until(&mut client, &mut editing, member, first, "batch", 1);
        let failed = client.answer(
            &mut editing,
            member,
            &write,
            json!({"failed": "quota exceeded"}),
        );
        assert!(
            deleted(&failed).iter().all(|f| !f.starts_with("/takes/")),
            "{failed}"
        );
    }

    /// **Completions out of order, each to its own file**: a save of A is
    /// out while B comes into focus and is reversed; B's batch answers first and
    /// A's last. Each lands on the file it was started on, and the late one
    /// moves nothing in the window, which is showing B.
    #[test]
    fn completions_in_another_order_land_on_the_file_each_was_started_on() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let action = cut(1, &editing);

        client.run(&mut editing, member, action);
        let a = binding(&mut editing, member)["id"].clone();
        let first = client.begin(&mut editing, member, save("/takes/a.wav"));
        let write_a = until(&mut client, &mut editing, member, first, "batch", 1);
        client.run(
            &mut editing,
            member,
            json!({"verb": "openFile", "paths": ["/takes/b.wav"], "window": true}),
        );
        let b = binding(&mut editing, member)["id"].clone();
        assert_ne!(a, b);
        let action = reverse(2, &editing);

        let started = client.begin(&mut editing, member, action);
        let reversed = until(&mut client, &mut editing, member, started, "batch", 1);
        let reversed = client.finish(&mut editing, member, reversed);
        assert!(reversed["error"].is_null(), "{reversed}");
        client.effects.clear();
        let saved = client.finish(&mut editing, member, write_a);
        assert!(saved["error"].is_null(), "{saved}");
        let shown = binding(&mut editing, member);
        assert_eq!(shown["id"], b, "the window still shows B");
        assert_eq!(shown["dirty"], true, "B's edit is B's");
        assert_eq!(shown["frames"], 200);
        assert!(
            !client
                .effects
                .iter()
                .any(|e| e["kind"] == "bind" || e["kind"] == "publish"),
            "A's late save moves nothing in the window: {:?}",
            client.effects
        );
        client.run(
            &mut editing,
            member,
            json!({"verb": "verb", "call": {"verb": "focus_file", "index": 0}, "window": true}),
        );
        let a_now = binding(&mut editing, member);
        assert_eq!(a_now["id"], a);
        assert_eq!(a_now["dirty"], false, "A's save cleaned A");
        assert_eq!(a_now["file"]["path"], "/takes/a.wav");
        assert_eq!(a_now["frames"], 80);
    }

    /// **A result answered twice** is taken once: the second answer finds
    /// no run waiting and changes nothing.
    #[test]
    fn a_duplicate_completion_is_refused_and_changes_nothing() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let action = reverse(1, &editing);

        let started = client.begin(&mut editing, member, action);
        let batch = until(&mut client, &mut editing, member, started, "batch", 1);
        let done = client.answer(&mut editing, member, &batch, json!({}));
        let done = client.finish(&mut editing, member, done);
        assert!(done["error"].is_null(), "{done}");
        let version = editing.version();
        let again = client.answer(&mut editing, member, &batch, json!({}));
        assert!(again["error"].is_string(), "{again}");
        assert_eq!(editing.version(), version);
        let state = client.call(&mut editing, json!({"verb": "state", "member": member}));
        assert_eq!(state["undoLabel"], "reverse", "one entry: {state}");
    }

    /// **The window closed while a save is out**: the view goes and the
    /// playback with it; the write completing afterwards still saves the
    /// file the client holds, and does not draw or sound the closed window
    /// again.
    #[test]
    fn a_save_completing_after_its_window_closed_saves_and_sounds_nothing() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let action = cut(1, &editing);

        client.run(&mut editing, member, action);
        let first = client.begin(&mut editing, member, save("/takes/a.wav"));
        let write = until(&mut client, &mut editing, member, first, "batch", 1);
        let closed = client.run(
            &mut editing,
            member,
            json!({"verb": "close", "window": true}),
        );
        assert!(closed["error"].is_null(), "{closed}");
        client.trace.clear();
        client.effects.clear();
        let saved = client.finish(&mut editing, member, write);
        assert!(saved["error"].is_null(), "{saved}");
        assert_eq!(binding(&mut editing, member)["dirty"], false);
        assert!(
            !client
                .effects
                .iter()
                .any(|e| matches!(e["kind"].as_str(), Some("publish" | "set" | "bind"))),
            "nothing is drawn: {:?}",
            client.effects
        );
        assert!(
            !client.trace.iter().any(|k| k == "await:steps"),
            "nothing sounds: {:?}",
            client.trace
        );
    }

    /// **An edit whose render fails** changes nothing: the take stays as
    /// it was, clean, and nothing is recorded.
    #[test]
    fn an_edit_whose_render_fails_leaves_the_take_whole() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let action = reverse(1, &editing);

        let started = client.begin(&mut editing, member, action);
        let render = until(&mut client, &mut editing, member, started, "batch", 1);
        let failed = client.answer(
            &mut editing,
            member,
            &render,
            json!({"failed": "render refused"}),
        );
        let failed = client.finish(&mut editing, member, failed);
        assert_eq!(failed["done"], true, "{failed}");
        assert_eq!(binding(&mut editing, member)["frames"], 100);
        assert_eq!(binding(&mut editing, member)["dirty"], false);
        let undo = client.call(&mut editing, json!({"verb": "state", "member": member}));
        assert_eq!(undo["canUndo"], false, "and nothing was recorded");
    }

    fn last(trace: &[String], kind: &str) -> usize {
        trace
            .iter()
            .rposition(|k| k == kind)
            .unwrap_or_else(|| panic!("no {kind} in {trace:?}"))
    }

    /// **The answer that has the window read the list again** follows the
    /// renders of the files it names.
    #[test]
    fn the_answer_that_reloads_the_list_follows_the_renders_that_make_it() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        client.trace.clear();
        let action = reverse(1, &editing);
        client.run(&mut editing, member, action);
        let trace = &client.trace;
        assert!(
            last(trace, "await:batch") < last(trace, "echo"),
            "{trace:?}"
        );
    }

    /// **A file is not edited while it plays**: a cut and an undo over the
    /// playing file are refused, the menu reads them refused for it, and once
    /// it stops the cut goes through.
    #[test]
    fn a_file_is_not_edited_while_it_plays() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let cut = |client: &mut Client, editing: &mut Editing, seq: i64| {
            client.trace.clear();
            client.effects.clear();
            let version = editing.version();
            let answer = client.run(
                editing,
                member,
                json!({"verb": "event", "addr": "/gui_event",
                    "args": [12, seq, version, "cut", 10.0, 20.0], "window": true}),
            );
            answer["result"] == true
        };
        client.rolling = true;
        client.run(
            &mut editing,
            member,
            json!({"verb": "play", "window": true}),
        );
        let version = editing.version();
        assert!(!cut(&mut client, &mut editing, 1), "nothing is cut");
        assert_eq!(editing.version(), version, "and nothing changed");
        let said = client
            .effects
            .iter()
            .any(|e| e.to_string().contains(crate::audio::editor::PLAYING));
        assert!(said, "the window is told why: {:?}", client.effects);
        let states = editing.action_states(member);
        for verb in ["cut", "paste", "undo", "redo"] {
            assert_eq!(states[verb]["enabled"], false, "{verb}: {states}");
        }
        assert_ne!(
            states["copy"]["reason"],
            json!(crate::audio::editor::PLAYING),
            "a copy is no edit"
        );
        client.rolling = false;
        client.run(
            &mut editing,
            member,
            json!({"verb": "stop", "window": true}),
        );
        assert!(cut(&mut client, &mut editing, 2), "stopped, it is edited");
    }

    #[test]
    fn a_closed_file_stops_being_read_before_what_it_measured_is_deleted() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        client.run(
            &mut editing,
            member,
            json!({"verb": "openFile", "paths": ["/takes/b.wav"], "window": true}),
        );
        client.trace.clear();
        client.run(
            &mut editing,
            member,
            json!({"verb": "verb", "call": {"verb": "close_file"}, "window": true}),
        );
        let trace = &client.trace;
        // the engine lets the file go before its peaks are deleted
        assert_eq!(client.engine.lock().unwrap().closed.len(), 1);
        assert!(
            position(trace, "await:steps") < position(trace, "delete"),
            "{trace:?}"
        );
    }
}
