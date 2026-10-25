//! **One task of a run, carried out**: an effect said, a door of the
//! context or the editor called, a playback verb carried out, or an await
//! handed to the caller.

use super::run::*;
use super::*;

impl Session {
    pub(super) fn step(
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

    pub(super) fn meters(&self) -> Value {
        self.playback.meters().map_or(
            Value::Null,
            |(bus, channels)| json!({"bus": bus, "channels": channels}),
        )
    }

    pub(super) fn state(&self) -> Value {
        json!({"playing": self.playback.playing(), "meters": self.meters(),
            "rolling": self.playback.rolling(), "focus": self.playback.focus()})
    }
}
