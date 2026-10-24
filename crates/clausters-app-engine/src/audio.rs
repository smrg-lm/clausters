//! **The audio editor, carried out by the application that holds its
//! engine**: every action of the editor's coordinator run to its end in the
//! application's process.
//!
//! The coordinator (`clausters_apps::editing`, the `audio` door) answers an
//! action as an ordered list of effects and, where what comes next depends on
//! it, one thing to wait on. A client used to carry those out: steps on its
//! server, a file's header read, the clipboard published, a history step
//! carried. [`AudioApp`] carries them out here, over an engine the application
//! holds, and hands back only what is the window's (props to set, the window
//! drawn again, an answer, the file bound), which the host draws.
//!
//! **A render does not hold the application.** A run that waits on a render
//! hands it to a thread of its own and stops; the next [`AudioApp::landed`]
//! resumes it with how the render went, so a render of another file starts
//! meanwhile and either lands first. A history step is carried to its end in
//! place, renders included, since what it undoes is waited for by the step
//! that asked.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use clausters_apps::editing::{Editing, MemberId, call_json_ids};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_editing::apply::{Step, steps_from_json};
use serde_json::{Value, json};

use crate::AppEngine;
use crate::held::HeldAudio;

/// **What an action raised**: its kind, as the coordinator names it
/// (`value`, `command`, `runtime`), and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Raised {
    /// `value` (a request the editor cannot take), `command` (an operation
    /// refused) or `runtime` (a state the editor is not in any more).
    pub kind: String,
    /// Why.
    pub message: String,
}

/// **An action carried as far as it goes**: the effects that are the
/// window's, in order, and its end -- `None` while it waits on a render.
#[derive(Debug)]
pub struct Ran {
    /// The member it is of.
    pub member: MemberId,
    /// What the caller named the action by, handed back with its end.
    pub token: u64,
    /// The window's effects, each `{"kind", ...}` as the coordinator says
    /// them: `echo`, `set`, `publish`, `band`, `looping`, `bind`, `layers`,
    /// `cursor`, `locate`, `changed`, `closeWindow`, `closedWindow`; and
    /// `resync` (every widget drawn again, after a history step), `clipboard`
    /// (`document`, the clipboard's) and `progress` (`path`, `done`, `total`;
    /// `done` null once a render has ended).
    pub effects: Vec<Value>,
    /// Its result, or what it raised; `None` while a render runs.
    pub end: Option<Result<Value, Raised>>,
}

/// What a render thread says back.
enum Note {
    /// How far a render has got.
    Progress {
        member: MemberId,
        /// What the render is for, in its verb's words.
        label: String,
        done: Option<u64>,
        total: u64,
    },
    /// A render ended: the run it was for, and how it went.
    Landed {
        member: MemberId,
        token: u64,
        run: Value,
        result: Value,
        lease: bool,
    },
}

/// **The audio editor's application half**: the engine it holds, the ids
/// its nodes, buses and buffers are numbered from, and the renders in
/// flight.
pub struct AudioApp {
    engine: Arc<Mutex<AppEngine>>,
    ids: IdSpaces,
    notes: (Sender<Note>, Receiver<Note>),
    /// Each member's renders under way, which [`AudioApp::call_off`] lets go.
    live: HashMap<MemberId, Vec<Arc<AtomicBool>>>,
}

impl std::fmt::Debug for AudioApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioApp").finish_non_exhaustive()
    }
}

impl AudioApp {
    /// The application half over `engine`.
    pub fn new(engine: Arc<Mutex<AppEngine>>) -> AudioApp {
        AudioApp {
            engine,
            ids: IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE),
            notes: channel(),
            live: HashMap::new(),
        }
    }

    /// **Calls off member `member`'s renders**: each stops before its next
    /// step, where its window closed in the middle of one -- a reverse of a
    /// day goes on writing its pieces otherwise, for a folder that is gone.
    pub fn call_off(&mut self, member: MemberId) {
        for live in self.live.remove(&member).unwrap_or_default() {
            live.store(false, Ordering::Release);
        }
    }

    /// The engine it holds.
    pub fn engine(&self) -> &Arc<Mutex<AppEngine>> {
        &self.engine
    }

    /// **Has `editing` play its audio members on this engine**; called once,
    /// before the first opens.
    pub fn hold(&self, editing: &mut Editing) {
        editing.hold_engine(Box::new(HeldAudio::new(Arc::clone(&self.engine))));
    }

    /// One verb of the context's door, with this engine's ids.
    pub fn call(&mut self, editing: &mut Editing, request: &Value) -> Value {
        serde_json::from_str(&call_json_ids(editing, &request.to_string(), &mut self.ids))
            .unwrap_or(Value::Null)
    }

    /// **Starts `action` on audio member `member`** and carries it as far as
    /// it goes; `token` comes back with its end. `lease` says the action
    /// holds the clipboard lease its event was prepared with.
    pub fn act(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        action: Value,
        token: u64,
        lease: bool,
    ) -> Ran {
        let answer = self.call(
            editing,
            &json!({"verb": "audio", "member": member, "do": action}),
        );
        let mut ran = Ran {
            member,
            token,
            effects: Vec::new(),
            end: None,
        };
        self.drive(editing, member, answer, lease, false, &mut ran);
        ran
    }

    /// **Takes back what the engine handed over** and resumes every run whose
    /// render landed, in the order they landed; a render's progress comes
    /// back as a `progress` effect of a [`Ran`] with no end.
    pub fn landed(&mut self, editing: &mut Editing) -> Vec<Ran> {
        self.engine.lock().unwrap_or_else(|e| e.into_inner()).pump();
        let mut out = Vec::new();
        while let Ok(note) = self.notes.1.try_recv() {
            match note {
                Note::Progress {
                    member,
                    label,
                    done,
                    total,
                } => out.push(Ran {
                    member,
                    token: 0,
                    effects: vec![json!({"kind": "progress", "label": label,
                        "done": done, "total": total})],
                    end: None,
                }),
                Note::Landed {
                    member,
                    token,
                    run,
                    result,
                    lease,
                } => {
                    let answer = self.call(
                        editing,
                        &json!({"verb": "audio", "member": member, "run": run,
                            "result": result}),
                    );
                    let mut ran = Ran {
                        member,
                        token,
                        effects: Vec::new(),
                        end: None,
                    };
                    self.drive(editing, member, answer, lease, false, &mut ran);
                    out.push(ran);
                }
            }
        }
        out
    }

    /// Carries a run on from `answer` until it ends or hands a render off.
    /// `inline` carries a render in place (a history step's).
    fn drive(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        mut answer: Value,
        lease: bool,
        inline: bool,
        ran: &mut Ran,
    ) {
        loop {
            if answer.get("run").is_none() {
                ran.end = Some(Err(Raised {
                    kind: "runtime".into(),
                    message: answer["error"]
                        .as_str()
                        .unwrap_or("the audio editor did not answer")
                        .into(),
                }));
                return;
            }
            for effect in answer["effects"].as_array().cloned().unwrap_or_default() {
                self.effect(editing, effect, lease, ran);
            }
            if answer["done"] == true {
                ran.end = Some(end_of(&answer));
                return;
            }
            let wait = answer["await"].clone();
            let kind = wait["kind"].as_str().unwrap_or_default();
            let result = match kind {
                "steps" | "batch" => {
                    let steps = steps_from_json(&wait["steps"]);
                    let renders = steps.iter().any(|s| matches!(s, Step::Render(_)));
                    if renders && !inline {
                        self.hand_off(member, ran.token, answer["run"].clone(), steps, lease);
                        return;
                    }
                    carried(crate::steps::carry(&steps, |_, _, _| {}))
                }
                "rolling" => {
                    let transport = wait["transport"].as_u64().unwrap_or(0) as usize;
                    let rolling = self
                        .engine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .rolling(transport);
                    json!({ "rolling": rolling })
                }
                "shape" => shape(&wait),
                "stat" => stat(&wait),
                "stepped" => self.stepped(editing, member, &wait["stepped"], ran),
                "clipboard" => self.clipboard(editing, member, &wait, ran),
                other => json!({"failed": format!("the audio editor cannot wait on {other:?}")}),
            };
            answer = self.call(
                editing,
                &json!({"verb": "audio", "member": member, "run": answer["run"],
                    "result": result}),
            );
        }
    }

    /// **A render carried out off the application's turn**: the steps on a
    /// thread of their own, their progress and their end said back.
    fn hand_off(
        &mut self,
        member: MemberId,
        token: u64,
        run: Value,
        steps: Vec<Step>,
        lease: bool,
    ) {
        let tx = self.notes.0.clone();
        let live = Arc::new(AtomicBool::new(true));
        let renders = self.live.entry(member).or_default();
        renders.retain(|r| r.load(Ordering::Acquire) && Arc::strong_count(r) > 1);
        renders.push(Arc::clone(&live));
        // the window is told at once that a render runs, not a tenth of a
        // second into it
        if let Some(Step::Render(job)) = steps.iter().find(|s| matches!(s, Step::Render(_))) {
            let _ = tx.send(Note::Progress {
                member,
                label: job.label.clone(),
                done: Some(0),
                total: 1,
            });
        }
        let spawned = std::thread::Builder::new()
            .name("audio-render".into())
            .spawn(move || {
                let mut last = std::time::Instant::now();
                let mut told = |job: &clausters_editing::apply::Render, done, total| {
                    // at most ten times a second, so a long render costs the
                    // window nothing
                    if last.elapsed().as_millis() >= 100 {
                        last = std::time::Instant::now();
                        let _ = tx.send(Note::Progress {
                            member,
                            label: job.label.clone(),
                            done: Some(done),
                            total,
                        });
                    }
                };
                let result = carried(crate::steps::carry_while(&steps, &live, &mut told));
                drop(live);
                let _ = tx.send(Note::Progress {
                    member,
                    label: String::new(),
                    done: None,
                    total: 0,
                });
                let _ = tx.send(Note::Landed {
                    member,
                    token,
                    run,
                    result,
                    lease,
                });
            });
        if let Err(e) = spawned {
            let _ = self.notes.0.send(Note::Landed {
                member,
                token,
                run: Value::Null,
                result: json!({"failed": format!("no thread for the render: {e}")}),
                lease,
            });
        }
    }

    /// One effect of a run: carried out here when it is the application's,
    /// handed to the window otherwise.
    fn effect(&mut self, editing: &mut Editing, effect: Value, lease: bool, ran: &mut Ran) {
        match effect["kind"].as_str().unwrap_or_default() {
            // the context keeps its own version, and an engine the application
            // holds has no buffers of a server's to free or give back
            "version" | "free" | "returned" | "bufferFree" => {}
            "release" => {
                for freed in effect["freed"].as_array().into_iter().flatten() {
                    delete(&freed["files"]);
                }
            }
            "delete" => delete(&effect["files"]),
            "call_off" => self.call_off(ran.member),
            "operationEcho" => ran.effects.push(json!({"kind": "echo",
                "answer": effect["echo"]["answer"]})),
            "lease" => {
                if lease {
                    self.call(
                        editing,
                        &json!({"verb": "exchange", "action": "release",
                            "lease": effect["lease"]}),
                    );
                }
            }
            _ => ran.effects.push(effect),
        }
    }

    /// **A step of the history carried out** -- one the context already took
    /// -- and whether anything moved: the operation it names run to its end,
    /// the files it changed sounded again, and every widget drawn again.
    fn stepped(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        stepped: &Value,
        ran: &mut Ran,
    ) -> Value {
        if stepped["stepped"] != true {
            return json!({"changed": false});
        }
        self.carry(editing, member, stepped, ran);
        for freed in stepped["freed"].as_array().into_iter().flatten() {
            delete(&freed["files"]);
        }
        ran.effects.push(json!({"kind": "resync"}));
        json!({"changed": true})
    }

    fn carry(&mut self, editing: &mut Editing, member: MemberId, stepped: &Value, ran: &mut Ran) {
        if let Some(operation) = stepped.get("operation").filter(|o| !o.is_null()) {
            let at = operation["member"].as_u64().unwrap_or(member as u64) as MemberId;
            let answer = self.call(
                editing,
                &json!({"verb": "audio", "member": at, "do":
                    {"verb": "operation", "prepared": operation}}),
            );
            let mut nested = Ran {
                member: at,
                token: 0,
                effects: Vec::new(),
                end: None,
            };
            self.drive(editing, at, answer, false, true, &mut nested);
            ran.effects.extend(nested.effects);
            if let Some(Ok(completed)) = nested.end {
                self.carry(editing, at, &completed["stepped"], ran);
            }
            return;
        }
        for effect in stepped["effects"].as_array().into_iter().flatten() {
            if effect["kind"] != "audio" {
                continue;
            }
            let at = effect["member"].as_u64().unwrap_or(member as u64) as MemberId;
            let steps = steps_from_json(&effect["steps"]);
            if let Err(e) = crate::steps::carry(&steps, |_, _, _| {}) {
                tracing::warn!("the audio editor's step: {e}");
            }
            if !effect["file"].is_null() {
                let answer = self.call(
                    editing,
                    &json!({"verb": "audio", "member": at, "do":
                        {"verb": "syncSound", "file": effect["file"]}}),
                );
                let mut nested = Ran {
                    member: at,
                    token: 0,
                    effects: Vec::new(),
                    end: None,
                };
                self.drive(editing, at, answer, false, true, &mut nested);
                ran.effects.extend(nested.effects);
            }
        }
    }

    /// **A copy published**: the clipboard names the spans of files it is
    /// of, and the window's clipboard holds the document.
    fn clipboard(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        wait: &Value,
        ran: &mut Ran,
    ) -> Value {
        let mut document = wait["document"].clone();
        if wait["ready"] != true {
            let published = self.call(
                editing,
                &json!({"verb": "exchange", "action": "publish", "member": member,
                    "document": document}),
            );
            if let Some(error) = published["error"].as_str() {
                return json!({ "failed": error });
            }
            document = published["document"].clone();
        }
        ran.effects
            .push(json!({"kind": "clipboard", "document": document}));
        json!({})
    }
}

/// How a run ended: its result, or what it raised.
fn end_of(answer: &Value) -> Result<Value, Raised> {
    if answer["rethrow"] == true {
        return Err(Raised {
            kind: "command".into(),
            message: answer["reason"].as_str().unwrap_or_default().into(),
        });
    }
    match answer.get("error").filter(|e| !e.is_null()) {
        Some(error) => Err(Raised {
            kind: error["type"].as_str().unwrap_or("runtime").into(),
            message: error["message"].as_str().unwrap_or_default().into(),
        }),
        None => Ok(answer.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// A carried batch as an await's result.
fn carried(outcome: Result<(), String>) -> Value {
    match outcome {
        Ok(()) => json!({}),
        Err(failed) => json!({ "failed": failed }),
    }
}

/// The containers a part of a list reads in place.
const IN_PLACE: &[&str] = &["wav", "w64", "rf64"];

/// **A file asked for**, by the window or a script: its header read (a new
/// file is the shape it is asked as). There is no display buffer: the engine
/// reads the list itself.
fn shape(wait: &Value) -> Value {
    if let Some(shape) = wait.get("shape").filter(|s| !s.is_null()) {
        return json!({"frames": shape["frames"], "channels": shape["channels"],
            "rate": shape["rate"], "display": 0, "path": null, "untitled": true,
            "codec": null, "sample_format": null});
    }
    let path = wait["path"].as_str().unwrap_or_default();
    match clausters::server::nrt::probe_audio(path) {
        Ok(info) if IN_PLACE.contains(&info.codec.as_str()) => json!({
            "frames": info.frames, "channels": info.channels,
            "rate": info.sample_rate, "display": 0, "path": path,
            "untitled": false, "codec": info.codec,
            "sample_format": info.sample_format,
        }),
        Ok(info) => json!({"failed": format!(
            "{path} is a {} file, which the audio editor does not read in place", info.codec)}),
        Err(e) => json!({ "failed": e }),
    }
}

/// **The files asked about, as they are on disk**: when each was last
/// modified, in milliseconds since the epoch, and its size, or `null` for
/// one that is not there.
fn stat(wait: &Value) -> Value {
    let mut stamps = serde_json::Map::new();
    for path in wait["paths"].as_array().into_iter().flatten() {
        let Some(path) = path.as_str() else {
            continue;
        };
        let stamp = std::fs::metadata(path).ok().map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
            json!({"modified": modified, "size": m.len()})
        });
        stamps.insert(path.into(), stamp.unwrap_or(Value::Null));
    }
    json!({ "stamps": stamps })
}

/// Deletes the files `files` names: renders nothing reaches any more.
fn delete(files: &Value) {
    for path in files.as_array().into_iter().flatten() {
        if let Some(path) = path.as_str()
            && let Err(e) = std::fs::remove_file(path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("the audio editor could not delete {path}: {e}");
        }
    }
}
