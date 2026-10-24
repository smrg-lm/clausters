//! **The audio editor's coordinator**: every action of an audio member --
//! a gesture, a script's verb, a menu entry, the transport, a save -- enters
//! here and leaves as one ordered list of effects for the client to carry out.
//!
//! A client used to assemble this itself: which playback verb follows a
//! focus change, when the meter is read, when the clipboard is published and
//! the cut is committed, when the takes a turn let go of are freed. Two
//! clients assembling it is two policies; this is the one.
//!
//! # Runs, effects and awaits
//!
//! An action starts a **run** (`do`). The run answers the **effects** it
//! has decided so far, in order, and stops on an **await** when what comes
//! next depends on something only the client can do: carry steps out on the
//! server, say whether the transport rolls, read a file into a buffer, carry
//! a history step out, publish a clipboard document. The client performs the
//! effects, then the await, and hands its result back (`run`, `result`),
//! which resumes the run where it stopped. A failed await unwinds the run as
//! an exception unwinds a call: through the cleanups it passed and into the
//! handler that catches it, or out to the client, which raises what failed.
//!
//! Runs stack: a run waiting on the client can see the client start another
//! (a release that stores a take, a history step that restores one), and the
//! second runs to its end before the first is resumed. A run waiting on a
//! render is the exception: a client carries the render out beside its other
//! work, so runs over other files start and end meanwhile, and each run is
//! resumed by its number, in whichever order its render lands. The file it
//! edits has pending work until then, which refuses another edit of it.
//!
//! # What is the client's
//!
//! Sockets and waiting, reading a `SoundFile`, the buffer numbers it
//! allocates, and the language objects a projection effect names (the take's
//! `Buffer`, the cursor, the layers). Whether a file is dirty is never one
//! of them: a client reads it from the editor when asked. Everything this module
//! decides it decides through the doors the clients called before, so the
//! editor's own rules stay where they were.
//!
//! # The vocabulary
//!
//! Effects, each `{"kind", ...}`: `version` (the context's version), `echo`
//! (an answer to the window), `set` (props on a widget), `publish` (the
//! window drawn again), `band` and `looping` (the selection band and the loop switch a script set),
//! `bind` (the file in focus, or none), `layers`, `cursor` (the
//! position a script placed), `locate` (the position a hand placed),
//! `changed` (the data changed in this gesture), `free` (buffers to free),
//! `delete` (files to delete: a closed file's renders), `release` (freed
//! buffers and, in each `freed` entry's `files`, renders nothing names any
//! more, to delete, as a turn hands them back), `returned`
//! (buffer numbers an operation gives back), `operationEcho`, `bufferFree`
//! (a copy's buffer nobody holds), `lease` (the event's clipboard lease),
//! `closeWindow` and `closedWindow`.
//!
//! Awaits, each `{"kind", ...}`: `steps` (`via` `join`, `plain` or
//! `service`), `rolling`, `shape` (a `path`, whose header the client reads,
//! or a new file's `shape`), `batch` (an operation's batch on a member's
//! server), `stepped`, `clipboard`. A result is `{}` on success,
//! `{"failed": reason}` on failure, and carries what the await asked for:
//! `rolling`, the file's facts (`frames`, `channels`, `rate`, `path`,
//! `untitled`, `codec`, `sample_format` and the `display` buffer its join is
//! stitched into), `changed`.

use std::collections::{BTreeMap, VecDeque};

use clausters_core::ids::IdSpaces;
use clausters_editing::apply::{Applier, Endpoint, Medium};
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
    /// The engine's rate, handed over at open; 0 when it would not say.
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
    /// Whether the transport rolls.
    Rolling(Rolled),
    /// One file read into buffers, to add.
    Read,
    /// The open files looked at on disk, the window having come back: the
    /// stamps go to the editor, which answers the window's `seq`.
    Stat { seq: Value },
    /// An operation's batch carried out: complete it.
    Batch {
        ticket: Value,
        batch: Value,
        repairing: bool,
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
    /// The playback asked whether the transport rolls.
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
    /// told why, and a buffer nobody holds is freed.
    Clipboard { outcome: Value, published: bool },
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
    /// The endpoint and the engine's rate, as the open hands them over: a
    /// playback that has not taken its transport yet is made again for them.
    fn reopen(&mut self, editing: &mut Editing, chunk: usize, rate: f64) {
        if self.playback.transport().is_none() {
            self.playback = playback_in(editing, chunk);
        }
        // an engine the application holds says its own rate
        self.rate = editing.engine.as_ref().map_or(rate, |engine| engine.rate());
    }

    fn new(editing: &mut Editing, chunk: usize, rate: f64) -> Session {
        Session {
            playback: playback_in(editing, chunk),
            rate,
            paused: false,
            runs: BTreeMap::new(),
            next: 1,
        }
    }
}

/// **A playback in the medium the context plays on**: an engine the
/// application holds, or an endpoint's steps for a client to carry out.
fn playback_in(editing: &mut Editing, chunk: usize) -> AudioEditorPlayback<Box<dyn Medium + Send>> {
    let medium: Box<dyn Medium + Send> = match editing.engine.as_mut() {
        Some(engine) => engine.medium(),
        None => Box::new(Applier::new(Endpoint {
            chunk: chunk.max(1),
        })),
    };
    AudioEditorPlayback::in_medium(medium)
}

/// **The takes an engine the application holds reads, brought up to date**:
/// every file the engine reads whose list changed since is handed to it
/// again, and its readers told its new length. Nothing over a server.
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
    let takes = match editing.member(member) {
        Some(Member::Audio(editor)) => editor.held_takes(rate),
        _ => return,
    };
    for take in takes {
        let Some(engine) = editing.engine.as_mut() else {
            return;
        };
        if engine
            .revision(take.file)
            .is_none_or(|r| r == take.revision)
        {
            continue;
        }
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
        // a verb before the open (a page sets the layers while its server is
        // still asked for its rate) finds a session to run in
        let mut session = editing
            .sessions
            .remove(&member)
            .unwrap_or_else(|| Session::new(editing, 1, 0.0));
        if action["verb"] == "open" {
            session.reopen(
                editing,
                action["chunk"].as_u64().unwrap_or(1) as usize,
                action["rate"].as_f64().unwrap_or(0.0),
            );
        }
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
                if let Err(error) = self.playback.alloc_transport(ids) {
                    return finished(id, Raise::Runtime, &error.to_string());
                }
                // An engine the application holds reads the lists itself.
                if editing.engine.is_some() {
                    member_call(editing, member, json!({"verb": "sync", "held": true}));
                }
                // The join the editor plays is stitched at the engine's rate.
                if self.rate > 0.0 {
                    member_call(
                        editing,
                        member,
                        json!({"verb": "sync", "engine_rate": self.rate}),
                    );
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
                },
                failed,
            ) => {
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
            (Resume::Clipboard, None) => {
                // the copy is on the clipboard: a failure from here frees nothing
                if let Some(run) = self.runs.get_mut(&id)
                    && let Some(Task::Catch(Handler::Clipboard { published, .. })) = run
                        .tasks
                        .iter_mut()
                        .find(|t| matches!(t, Task::Catch(Handler::Clipboard { .. })))
                {
                    *published = true;
                }
            }
            (Resume::Rolling(rolled), None) => {
                let rolling = result["rolling"] == true;
                self.playback.set_rolling(rolling);
                let tasks = self.rolled(editing, member, rolled, rolling);
                self.push(id, tasks);
            }
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

    /// What a rolling answer leads to.
    /// **The file in focus sounds on an engine the application holds**: its
    /// list handed to the engine, and the playback's readers set to the
    /// buffers the engine answers, one per channel. `None` over a server.
    fn sync_held(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        file: &Value,
        ids: &mut IdSpaces,
    ) -> Option<Flow> {
        let rate = editing.engine.as_ref().map(|engine| {
            if self.rate > 0.0 {
                self.rate
            } else {
                engine.rate()
            }
        })?;
        let wanted = file["id"].as_u64()?;
        let take = match editing.member(member) {
            Some(Member::Audio(editor)) => editor
                .held_takes(rate)
                .into_iter()
                .find(|take| take.file == wanted)?,
            _ => return None,
        };
        let engine = editing.engine.as_mut()?;
        let buffers = match engine.take(&take, ids) {
            Ok(buffers) => buffers,
            Err(e) => return Some(Flow::Fail(Failure::Own(Raise::Runtime, e))),
        };
        let frames = file["frames"].as_u64().unwrap_or(0);
        let take_rate = file["rate"].as_f64().unwrap_or(0.0);
        if let Err(e) = self
            .playback
            .sync_streams(wanted, buffers, frames, take_rate, rate, ids)
        {
            return Some(Flow::Fail(Failure::Own(Raise::Runtime, e.to_string())));
        }
        self.push(id, vec![Task::RetainPlayback]);
        Some(Flow::Next)
    }

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
                Task::Catch(Handler::Clipboard { outcome, published }) => {
                    if has(&outcome, "clipboard") {
                        let mut answer = outcome["answer"].clone();
                        if answer.is_object() {
                            answer["reason"] = json!(failure.reason());
                            answer["docVersion"] = json!(editing.version());
                        }
                        effects.push(tagged("echo", json!({"answer": answer})));
                    }
                    if !published && has(&outcome, "clipboardBuffer") {
                        effects.push(tagged(
                            "bufferFree",
                            json!({"buffer": outcome["clipboardBuffer"]}),
                        ));
                    }
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
                if truthy(&outcome["freedBuffers"]) {
                    tasks.push(effect("free", json!({"buffers": outcome["freedBuffers"]})));
                }
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
                    json!({"document": outcome["clipboard"], "buffer": outcome["clipboardBuffer"],
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
                    if has(&answer, "returned") {
                        tasks.push(effect("returned", json!({"returned": answer["returned"]})));
                    }
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
                if has(&answer, "returned") {
                    effects.push(tagged("returned", json!({"returned": answer["returned"]})));
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
                if let Some(flow) = self.sync_held(editing, member, id, &file, ids) {
                    return flow;
                }
                let args = json!({"file": file["id"], "buffer": file["display"],
                    "channels": file["channels"], "frames": file["frames"],
                    "takeRate": file["rate"], "rate": self.rate});
                self.push(
                    id,
                    vec![Task::Playback { verb: "sync", args }, Task::RetainPlayback],
                );
                Flow::Next
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
            // an engine the application holds says whether it rolls
            Task::Rolling(rolled) if editing.engine.is_some() => {
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
            Task::Rolling(rolled) => Flow::Await(
                tagged("rolling", json!({"transport": self.playback.transport()})),
                Resume::Rolling(rolled),
            ),
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
        if copied || has(&outcome, "clipboardBuffer") {
            tasks.push(Task::Catch(Handler::Clipboard {
                outcome: outcome.clone(),
                published: false,
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
    use super::*;
    use clausters_core::ids::{IdShare, ServerShape};

    /// **A client that does what it is asked**: every await succeeds, the
    /// transport rolls as `rolling` says, a file read is a two-channel take
    /// in fresh buffers, and every effect and await is written down.
    struct Client {
        ids: IdSpaces,
        rolling: bool,
        next: i64,
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
                next: 100,
                fail: None,
                trace: Vec::new(),
                effects: Vec::new(),
                stamps: json!({}),
            }
        }

        fn call(&mut self, editing: &mut Editing, request: Value) -> Value {
            serde_json::from_str(&call_json_ids(editing, &request.to_string(), &mut self.ids))
                .unwrap()
        }

        fn buffer(&mut self) -> i64 {
            self.next += 1;
            self.next
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
                "rolling" => json!({"rolling": self.rolling}),
                "stat" => json!({"stamps": self.stamps}),
                "shape" => {
                    let display = self.buffer();
                    json!({"frames": 200, "channels": 2, "rate": 48000.0,
                        "display": display, "path": wait["path"], "untitled": false})
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
            json!({"verb": "openAudio", "key": "audio:empty", "empty": true, "display": 9})
        } else {
            json!({"verb": "openAudio", "key": "take:3", "path":"/in/take3.wav","workspace":"/work", "frames": 100,
                "display": 9})
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
                "workspace": "/work", "frames": 100, "display": 9,
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
    fn opening_copies_the_take_and_then_sounds_it() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        opened(&mut client, &mut editing, false);
        let trace = &client.trace;
        // the join is an operation's batch, and the sound follows it
        assert!(
            position(trace, "await:batch") < position(trace, "await:steps"),
            "{trace:?}"
        );
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
        let rolling = position(trace, "await:rolling");
        let publish = position(trace, "publish");
        assert!(position(trace, "await:shape") < bind, "{trace:?}");
        assert!(bind < rolling && rolling < publish, "{trace:?}");
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
        let rolling = position(&trace, "await:rolling");
        assert!(
            trace[rolling..].contains(&"await:steps".to_string()),
            "{trace:?}"
        );
        assert!(
            trace[rolling..].contains(&"set".to_string()),
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

    /// **Completions out of order, each to its own file**: a save of A is
    /// out while B comes into focus and is cut; B's batch answers first and
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
        let action = cut(2, &editing);

        let started = client.begin(&mut editing, member, action);
        let cut_b = until(&mut client, &mut editing, member, started, "batch", 1);
        let cut_b = client.finish(&mut editing, member, cut_b);
        assert!(cut_b["error"].is_null(), "{cut_b}");
        client.effects.clear();
        let saved = client.finish(&mut editing, member, write_a);
        assert!(saved["error"].is_null(), "{saved}");
        let shown = binding(&mut editing, member);
        assert_eq!(shown["id"], b, "the window still shows B");
        assert_eq!(shown["dirty"], true, "B's cut is B's");
        assert_eq!(shown["frames"], 180);
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
        let action = cut(1, &editing);

        let started = client.begin(&mut editing, member, action);
        let batch = until(&mut client, &mut editing, member, started, "batch", 1);
        let done = client.answer(&mut editing, member, &batch, json!({}));
        let done = client.finish(&mut editing, member, done);
        assert!(done["error"].is_null(), "{done}");
        let version = editing.version();
        let again = client.answer(&mut editing, member, &batch, json!({}));
        assert!(again["error"].is_string(), "{again}");
        assert_eq!(editing.version(), version);
        assert_eq!(binding(&mut editing, member)["frames"], 80);
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

    /// **A copy that fails before it reaches the clipboard** removes nothing:
    /// the cut's samples stay, and the window is told why.
    #[test]
    fn a_cut_whose_copy_fails_leaves_the_take_whole() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        let action = cut(1, &editing);

        let started = client.begin(&mut editing, member, action);
        let copy = until(&mut client, &mut editing, member, started, "batch", 1);
        let failed = client.answer(
            &mut editing,
            member,
            &copy,
            json!({"failed": "copy refused"}),
        );
        let failed = client.finish(&mut editing, member, failed);
        assert_eq!(failed["done"], true, "{failed}");
        assert_eq!(
            binding(&mut editing, member)["frames"],
            100,
            "nothing was removed"
        );
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

    #[test]
    fn the_answer_that_reloads_the_join_follows_the_steps_that_make_it() {
        let mut editing = Editing::new();
        let mut client = Client::new();
        let member = opened(&mut client, &mut editing, false);
        client.trace.clear();
        let version = editing.version();
        client.run(
            &mut editing,
            member,
            json!({"verb": "event", "addr": "/gui_event",
                "args": [12, 1, version, "cut", 10.0, 20.0], "window": true}),
        );
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
            client.run(
                editing,
                member,
                json!({"verb": "event", "addr": "/gui_event",
                    "args": [12, seq, version, "cut", 10.0, 20.0], "window": true}),
            );
            client.trace.iter().any(|k| k == "await:batch")
        };
        client.rolling = true;
        client.run(
            &mut editing,
            member,
            json!({"verb": "play", "window": true}),
        );
        let version = editing.version();
        assert!(!cut(&mut client, &mut editing, 1), "nothing is carried out");
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
    fn a_closed_file_s_takes_are_released_after_its_join_stops_reading_them() {
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
        // the playback lets the file go before its buffers are freed
        assert!(
            position(trace, "await:steps") < position(trace, "free"),
            "{trace:?}"
        );
    }
}
