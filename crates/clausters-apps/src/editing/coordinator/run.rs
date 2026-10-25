//! **A run of an action**: the tasks it is made of, what it waits on and
//! how a failure unwinds it -- pushed, driven to its next await or its end,
//! resumed with its caller's result.

use super::*;

/// What a client raises when a run ends in an error of the coordinator's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Raise {
    /// A request the editor cannot take (`ValueError`, `RangeError`).
    Value,
    /// An operation the server or the editor refused (`CommandError`).
    Command,
    /// A state the editor is not in any more (`RuntimeError`, `Error`).
    Runtime,
}

impl Raise {
    pub(super) fn name(self) -> &'static str {
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
pub(super) enum Failure {
    Client(String),
    Own(Raise, String),
}

impl Failure {
    pub(super) fn reason(&self) -> &str {
        match self {
            Failure::Client(reason) | Failure::Own(_, reason) => reason,
        }
    }
}

/// How one step of a run went on.
pub(super) enum Flow {
    Next,
    Await(Value, Resume),
    Fail(Failure),
}

/// **What a resumed run does with the client's result.**
#[derive(Clone, Debug)]
pub(super) enum Resume {
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
pub(super) enum Rolled {
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
pub(super) enum Task {
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
pub(super) enum Cleanup {
    /// The event's clipboard lease is released.
    Lease,
}

#[derive(Clone, Debug)]
pub(super) enum Handler {
    /// A copy that failed before or while it was published: the window is
    /// told why.
    Clipboard { outcome: Value },
}

/// **One action in progress.**
#[derive(Debug, Default)]
pub(super) struct Run {
    pub(super) tasks: VecDeque<Task>,
    pub(super) waiting: Option<Resume>,
    /// Whether the take changed, as the outcome being carried out says.
    pub(super) changed: bool,
    /// What the last operation completed with.
    pub(super) completed: Value,
    /// Whether the window is open on a host.
    pub(super) window: bool,
    /// Why a verb refused, answered with the run's first effects.
    pub(super) reason: Option<Value>,
    pub(super) result: Value,
}

impl Session {
    pub(super) fn push(&mut self, id: u64, tasks: Vec<Task>) {
        if let Some(run) = self.runs.get_mut(&id) {
            for task in tasks.into_iter().rev() {
                run.tasks.push_front(task);
            }
        }
    }

    pub(super) fn resume(
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

    /// Carry the run on until it waits or ends.
    pub(super) fn drive(
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
    pub(super) fn unwind(
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
}
