//! **A run of an action**: the tasks it is made of, what it waits on and
//! how a failure unwinds it -- pushed, driven to its next await or its end,
//! resumed with its caller's result.

use super::*;
use clausters_editing::audio_playback::Pass;

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
    Await(Await, Resume),
    Fail(Failure),
}

/// **What a run waits on**, for its driver to do in place or hand back.
#[derive(Clone, Debug)]
pub(super) enum Await {
    /// Steps to carry out, in the JSON a runner walks: a playback's, a
    /// turn's, or an operation's batch.
    Steps(Value),
    /// A file to add: `{"path"}`, whose header is read, or a new file's
    /// `{"shape"}`.
    Shape(Value),
    /// The files at these paths, as they are on disk.
    Stat(Value),
    /// A history step the context took, to carry out.
    Stepped(Value),
    /// A copy's clipboard document, to publish unless `ready`.
    Clipboard { document: Value, ready: bool },
}

/// **How far a run went**: the effects it said, and its end or what it
/// waits on.
#[derive(Debug)]
pub(super) struct Answer {
    pub(super) run: u64,
    pub(super) effects: Vec<Value>,
    pub(super) end: End,
}

/// A run's end, or what it waits on.
#[derive(Debug)]
pub(super) enum End {
    /// It ended with its result.
    Done(Value),
    /// It ended on a failure, unwound.
    Failed(Failure),
    /// It waits.
    Waits(Await),
}

impl Answer {
    /// A run that ended at once on a failure of the coordinator's.
    pub(super) fn refused(run: u64, raise: Raise, message: &str) -> Answer {
        Answer {
            run,
            effects: Vec::new(),
            end: End::Failed(Failure::Own(raise, message.into())),
        }
    }
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
    Playback(Op),
    /// The member's playback root retained.
    RetainPlayback,
    /// The member's playback root dropped.
    DropPlayback,
    /// Steps to carry out, in the JSON a runner walks.
    Steps(Value),
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
        result: Result<Value, String>,
        ids: &mut IdSpaces,
    ) -> Answer {
        let Some(waiting) = self.runs.get_mut(&id).and_then(|run| run.waiting.take()) else {
            return Answer::refused(id, Raise::Runtime, "no run is waiting under that number");
        };
        let (result, failed) = match &result {
            Ok(result) => (result.clone(), None),
            Err(reason) => (Value::Null, Some(reason.as_str())),
        };
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
                return self.unwind(editing, id, Failure::Client(reason.to_string()), effects);
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
            effects.append(&mut answer.effects);
            answer.effects = effects;
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
    ) -> Answer {
        let mut effects = Vec::new();
        loop {
            let Some(task) = self.runs.get_mut(&id).and_then(|run| run.tasks.pop_front()) else {
                retake(editing, member, &mut self.playback, self.rate, ids);
                let run = self.runs.remove(&id).unwrap_or_default();
                return Answer {
                    run: id,
                    effects,
                    end: End::Done(run.result),
                };
            };
            match self.step(editing, member, id, task, &mut effects, ids) {
                Flow::Next => {}
                Flow::Await(wait, resume) => {
                    retake(editing, member, &mut self.playback, self.rate, ids);
                    if let Some(run) = self.runs.get_mut(&id) {
                        run.waiting = Some(resume);
                    }
                    return Answer {
                        run: id,
                        effects,
                        end: End::Waits(wait),
                    };
                }
                Flow::Fail(failure) => return self.unwind(editing, id, failure, effects),
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
        mut effects: Vec<Value>,
    ) -> Answer {
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
        Answer {
            run: id,
            effects,
            end: End::Failed(failure),
        }
    }
}

/// **A verb of the member's playback**, as a run asks it.
#[derive(Clone, Debug)]
pub(super) enum Op {
    /// The window shows `file`, at `frame`: it comes into focus.
    View {
        file: u64,
        frame: u64,
    },
    /// `file` plays from `start` for `pass`; `None` for a pass that did not
    /// read.
    Play {
        file: u64,
        start: u64,
        pass: Option<Pass>,
    },
    /// The pass in progress ends as this says.
    Pass(Option<Pass>),
    /// A loop in progress over `file` repeats over `from..to`, the head
    /// placed into it when `place` says.
    Follow {
        file: u64,
        from: u64,
        to: u64,
        place: bool,
    },
    Resume,
    Pause,
    /// What plays stops, going back to `back`, or only when it is `file`.
    Stop {
        file: Option<u64>,
        back: u64,
    },
    /// The playing head moves to the frame.
    Locate(u64),
    /// The stopped transport stands on the frame.
    Cue(u64),
    /// The playback goes, with its nodes.
    Close,
    /// `file` is closed: its readers go.
    CloseFile(u64),
}

impl Op {
    /// What the space bar asks, `play` as the editor reads it: a play of
    /// `file` from where it starts.
    pub(super) fn play(file: Option<i64>, play: &Value) -> Op {
        Op::Play {
            file: file.map_or(0, |f| f as u64),
            start: play["start"].as_u64().unwrap_or(0),
            pass: pass_of(&play["pass"]),
        }
    }

    /// A stop going back to where `play` says.
    pub(super) fn stop(file: Option<i64>, play: &Value) -> Op {
        Op::Stop {
            file: file.map(|f| f as u64),
            back: play["back"].as_u64().unwrap_or(0),
        }
    }

    /// The pass `pass` says.
    pub(super) fn pass(pass: &Value) -> Op {
        Op::Pass(pass_of(pass))
    }
}

fn pass_of(pass: &Value) -> Option<Pass> {
    serde_json::from_value(pass.clone()).ok()
}
