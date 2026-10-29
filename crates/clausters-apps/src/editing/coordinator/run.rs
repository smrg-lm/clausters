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

/// **What the application carries out of a run**, in order with the
/// window's effects: one of the window's, or the application's own.
#[derive(Clone, Debug)]
pub(crate) enum Said {
    /// An effect of the window's, `{"kind", ...}`.
    Window(Value),
    /// Takes nothing reaches any more: their renders deleted.
    Release(Vec<Freed>),
    /// Files to delete: a closed file's renders, a failed batch's scratch.
    Delete(Vec<String>),
    /// The member's renders under way called off.
    CallOff,
    /// The event's clipboard lease let go.
    Lease,
}

/// **A file a run reads in**: one at a path, its header read, or a new
/// one of a shape.
#[derive(Clone, Debug)]
pub(super) enum Wanted {
    Path(String),
    Shape(Shape),
}

/// **What a run waits on**, for its driver to do in place or hand back.
#[derive(Clone, Debug)]
pub(super) enum Await {
    /// Steps to carry out: a playback's, a turn's, or an operation's batch.
    Steps(Vec<Step>),
    /// A file to add, its header read or its shape said.
    Shape(Wanted),
    /// The files at these paths, as they are on disk.
    Stat(Vec<String>),
    /// A history step the context took, to carry out.
    Stepped(Box<super::super::Stepped>),
    /// A copy's clipboard document, to publish unless `ready`.
    Clipboard { document: String, ready: bool },
}

/// **What an await came to**, as its driver hands it back.
#[derive(Clone, Debug)]
pub(super) enum Came {
    /// Done, with nothing to read.
    Done,
    /// A file's facts, as the editor reads a file it opens.
    File(Value),
    /// The files looked at on disk, each path to its stamp.
    Stamps(Value),
    /// A history step carried out: whether anything changed.
    Stepped(bool),
}

/// **How far a run went**: the effects it said, and its end or what it
/// waits on.
#[derive(Debug)]
pub(super) struct Answer {
    pub(super) run: u64,
    pub(super) effects: Vec<Said>,
    pub(super) end: End,
}

/// A run's end, or what it waits on.
#[derive(Debug)]
pub(super) enum End {
    /// It ended with its result, and the operation it completed last.
    Done(Value, Option<Box<Operated>>),
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
    /// One file read, to add.
    Read,
    /// The open files looked at on disk, the window having come back: the
    /// stamps go to the editor, which answers the window's `seq`.
    Stat { seq: i64 },
    /// An operation's batch carried out: complete it. The files its renders
    /// write go if it fails, where they are the editor's scratch.
    Batch {
        ticket: crate::application::operations::Ticket,
        batch: usize,
        repairing: bool,
        member: MemberId,
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
    Space(Play),
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
    Take { outcome: Box<Outcome>, lease: bool },
    /// A file of the list the window named, focused or closed, its outcome
    /// carried out.
    VerbTake(FileAction),
    /// The binding of a turn that names one, past its steps.
    Bound {
        outcome: Box<Outcome>,
        close_file: bool,
        lease: bool,
    },
    /// The rest of an outcome, past its file binding.
    TakeRest { outcome: Box<Outcome>, lease: bool },
    /// The tail of an outcome, past its steps and its clipboard.
    TakeTail(Box<Outcome>),
    /// The completed operation's outcome, carried out.
    TakeCompleted,
    /// A save's result: where it wrote.
    Destination,
    /// The run's result: whether the take changed, and why a verb refused.
    Reason(Option<String>),
    /// An operation, from its command or as prepared.
    Operation {
        command: Option<AudioCommand>,
        prepared: Option<Box<Operated>>,
        repairing: bool,
    },
    /// An operation's answer, looked at.
    OperationAnswer {
        answer: Box<Operated>,
        repairing: bool,
    },
    /// What an operation came to, reflected.
    OperationDone {
        answer: Box<Operated>,
        repairing: bool,
    },
    /// The clipboard document a copy left, published.
    PublishClipboard { document: String, ready: bool },
    /// A cut committed once its copy is on the clipboard.
    Cut(Box<Outcome>),
    /// Files read and added, one at a time.
    Read(VecDeque<Wanted>),
    /// The files a window coming back reads, looked at on disk.
    Stat(CheckDisk),
    /// A read file added to the editor.
    AddFile(Value),
    /// Make what sounds be the take as it now is.
    Sound,
    /// One file's playback brought in step.
    SyncSound(PlaybackFile),
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
    /// Steps to carry out.
    Steps(Vec<Step>),
    /// A history step to carry out.
    Stepped(Box<super::super::Stepped>),
    /// The window closed (`closed`: by the host, not asked).
    Close { closed: bool },
    /// A cleanup that runs whether or not the run fails.
    Finally(Cleanup),
    /// A handler for a failure of what precedes it.
    Catch(Handler),
    /// An effect, said in order.
    Effect(Said),
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
    /// told why, with the copy's answer.
    Clipboard {
        answer: Option<clausters_editing::conversation::Answer>,
    },
}

/// **One action in progress.**
#[derive(Debug, Default)]
pub(super) struct Run {
    pub(super) tasks: VecDeque<Task>,
    pub(super) waiting: Option<Resume>,
    /// Whether the take changed, as the outcome being carried out says.
    pub(super) changed: bool,
    /// What the last operation completed with.
    pub(super) completed: Option<Box<Operated>>,
    /// Whether the window is open on a host.
    pub(super) window: bool,
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
        result: Result<Came, String>,
        ids: &mut IdSpaces,
    ) -> Answer {
        let Some(waiting) = self.runs.get_mut(&id).and_then(|run| run.waiting.take()) else {
            return Answer::refused(id, Raise::Runtime, "no run is waiting under that number");
        };
        let mut effects = Vec::new();
        match (waiting, result) {
            // a batch that failed is the operation's to hear, not raised
            (
                Resume::Batch {
                    ticket,
                    batch,
                    repairing,
                    member: owner,
                    rendered,
                },
                result,
            ) => {
                // **A failed batch leaves nothing on disk.** Its renders up to
                // the failure wrote files no list will name -- the pieces of a
                // reverse of a day before the disk filled -- and they would sit
                // in the working folder until the editor closed.
                if result.is_err() && !rendered.is_empty() {
                    let scratch = editing
                        .audio(owner)
                        .map(|editor| editor.scratch(&rendered))
                        .unwrap_or_default();
                    if !scratch.is_empty() {
                        effects.push(Said::Delete(scratch));
                    }
                }
                let result = match result {
                    Ok(_) => Reply::Success,
                    Err(reason) => Reply::Failed(reason),
                };
                let answer = editing.audio_operation(OperationCall::Control(Control::Complete {
                    ticket,
                    batch,
                    result,
                }));
                self.push(
                    id,
                    vec![Task::OperationAnswer {
                        answer: Box::new(answer),
                        repairing,
                    }],
                );
            }
            // A look that failed finds nothing changed: the window's event
            // is answered all the same.
            (Resume::Stat { seq }, result) => {
                let stamps = match result {
                    Ok(Came::Stamps(stamps)) => stamps,
                    _ => json!({}),
                };
                let version = editing.version();
                let answer = editing
                    .audio_mut(member)
                    .map(|editor| editor.disk_answer(&stamps, seq, version).answer);
                self.push(
                    id,
                    vec![effect("echo", json!({"answer": answer.flatten()}))],
                );
            }
            (_, Err(reason)) => {
                return self.unwind(editing, id, Failure::Client(reason), effects);
            }
            (Resume::Steps | Resume::Clipboard, Ok(_)) => {}
            (Resume::Read, Ok(came)) => {
                if let Came::File(file) = came {
                    self.push(id, vec![Task::AddFile(file)]);
                }
            }
            (Resume::Stepped, Ok(came)) => {
                if let Some(run) = self.runs.get_mut(&id) {
                    run.result = json!(matches!(came, Came::Stepped(true)));
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
                    end: End::Done(run.result, run.completed),
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
        mut effects: Vec<Said>,
    ) -> Answer {
        let run = self.runs.remove(&id).unwrap_or_default();
        for task in run.tasks {
            match task {
                Task::Finally(Cleanup::Lease) => effects.push(Said::Lease),
                Task::Catch(Handler::Clipboard { answer }) => {
                    use clausters_editing::conversation::Answer as Told;
                    let version = editing.version();
                    let why = Some(failure.reason().to_string());
                    let answer = match answer {
                        Some(Told::Ack { seq, .. }) => Told::Ack {
                            seq,
                            doc_version: version,
                            reason: why,
                        },
                        Some(Told::Push {
                            seq, corrections, ..
                        }) => Told::Push {
                            seq,
                            doc_version: version,
                            reason: why,
                            corrections,
                        },
                        _ => Told::Ack {
                            seq: 0,
                            doc_version: version,
                            reason: why,
                        },
                    };
                    effects.push(Said::Window(tagged("echo", json!({"answer": answer}))));
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
    /// `file` from where it starts; one that did not read with no file open.
    pub(super) fn play(file: Option<i64>, play: Option<&Play>) -> Op {
        Op::Play {
            file: file.map_or(0, |f| f as u64),
            start: play.map_or(0, |p| p.start),
            pass: play.map(|p| p.pass),
        }
    }

    /// A stop going back to where `play` says.
    pub(super) fn stop(file: Option<i64>, play: Option<&Play>) -> Op {
        Op::Stop {
            file: file.map(|f| f as u64),
            back: play.map_or(0, |p| p.back),
        }
    }
}
