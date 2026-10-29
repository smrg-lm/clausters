//! **What each action starts with**: the tasks an action of the editor's
//! door is, and what the transport's answers lead to.

use super::run::*;
use super::*;
use crate::editing::Turned;
use crate::turn::{Event, Kind};

impl Session {
    pub(super) fn start(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        action: AudioAction,
        window: bool,
        ids: &mut IdSpaces,
    ) -> Answer {
        let id = self.next;
        self.next += 1;
        let mut run = Run {
            window,
            ..Default::default()
        };
        if action.transport() && file_id(editing, member).is_none() {
            return Answer::refused(id, Raise::Value, NO_FILE);
        }
        let tasks: Vec<Task> = match action {
            AudioAction::Open { empty } => {
                if self.playback.transport().is_none()
                    && let Err(error) = self.playback.alloc_transport(ids)
                {
                    return Answer::refused(id, Raise::Runtime, &error.to_string());
                }
                let mut tasks = Vec::new();
                if !empty {
                    tasks.push(Task::Operation {
                        command: Some(AudioCommand::Open),
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
            // or Escape): the caller stops the renders under way, each before
            // its next piece, and a batch that stops so deletes what it wrote.
            AudioAction::Event { args, .. }
                if args.get(3).and_then(Value::as_str) == Some("call_off") =>
            {
                vec![Task::Effect(Said::CallOff)]
            }
            AudioAction::Event { addr, args, lease } => {
                let event = Event { addr, args };
                let turned = editing
                    .prepare_audio_event(member, &event)
                    .or_else(|| editing.event(member, &event));
                match turned {
                    Some(turned) => self.turned(&mut run, turned, lease),
                    None => Vec::new(),
                }
            }
            AudioAction::Act { call } => {
                let turned = editing
                    .prepare_audio_action(member, &call)
                    .or_else(|| editing.act(member, &call));
                let Some(turned) = turned else {
                    run.tasks.push_back(Task::Reason(None));
                    self.runs.insert(id, run);
                    return self.drive(editing, member, id, ids);
                };
                let version = turned.version;
                let outcome = audio_outcome(turned.outcome);
                run.changed = outcome.changed;
                let reason = answer_reason(&outcome);
                let mut tasks = vec![
                    effect("version", json!({"version": version})),
                    Task::Take {
                        outcome,
                        lease: false,
                    },
                ];
                tasks.extend(release(turned.freed));
                tasks.push(Task::Reason(reason));
                tasks
            }
            AudioAction::Verb { call } => {
                let outcome = match editing.audio_verb(member, &call) {
                    Ok(outcome) => Box::new(outcome),
                    Err(error) if error == NO_FILE => {
                        return Answer::refused(id, Raise::Value, NO_FILE);
                    }
                    Err(error) => {
                        return Answer::refused(id, Raise::Value, &error);
                    }
                };
                run.changed = outcome.changed;
                let reason = answer_reason(&outcome);
                vec![
                    Task::Take {
                        outcome,
                        lease: false,
                    },
                    Task::Reason(reason),
                ]
            }
            AudioAction::Save { path, format } => vec![
                Task::Operation {
                    command: Some(AudioCommand::Save { path, format }),
                    prepared: None,
                    repairing: false,
                },
                Task::TakeCompleted,
                Task::Destination,
            ],
            AudioAction::Operation {
                command,
                prepared,
                repairing,
            } => vec![Task::Operation {
                command,
                prepared,
                repairing,
            }],
            AudioAction::Retry => {
                let pending = editing
                    .audio_operation(OperationCall::Control(Control::PendingRepairs { member }));
                pending
                    .protocol
                    .repairs
                    .unwrap_or_default()
                    .into_iter()
                    .map(|repair| {
                        let prepared =
                            editing.audio_operation(OperationCall::Control(Control::RetryRepair {
                                ticket: repair.ticket,
                            }));
                        Task::Operation {
                            command: None,
                            prepared: Some(Box::new(prepared)),
                            repairing: true,
                        }
                    })
                    .collect()
            }
            AudioAction::OpenFile { request } => match audio::open_paths(&request) {
                Ok(paths) => vec![Task::Read(super::paths(&paths))],
                Err(error) => return Answer::refused(id, Raise::Value, &error),
            },
            AudioAction::NewFile { shape } => {
                vec![Task::Read(VecDeque::from([run::Wanted::Shape(shape)]))]
            }
            AudioAction::Sound => vec![Task::Sound],
            AudioAction::SyncSound { file } => vec![Task::SyncSound(file)],
            AudioAction::Close => vec![Task::Close { closed: false }],
            AudioAction::Closed => vec![Task::Close { closed: true }],
            AudioAction::Playing => vec![Task::Rolling(Rolled::Playing {
                was: self.playback.rolling(),
            })],
            AudioAction::Play { at } => self.script_play(editing, member, at),
            AudioAction::Pause => {
                let mut tasks = Vec::new();
                if self.focused(editing, member) {
                    tasks.push(Task::Playback(Op::Pause));
                    self.paused = true;
                    tasks.push(Task::ShowTransport);
                }
                tasks
            }
            AudioAction::Stop => {
                self.paused = false;
                if self.focused(editing, member) {
                    self.script_stop(editing, member)
                } else {
                    vec![Task::Rolling(Rolled::Stop)]
                }
            }
            AudioAction::Locate { at } => self.script_locate(editing, member, at),
            AudioAction::Span { span, show } => {
                let band = editing
                    .audio_mut(member)
                    .map(|editor| editor.set_span(span))
                    .unwrap_or_default();
                let mut tasks = Vec::new();
                if show {
                    tasks.push(effect(
                        "band",
                        json!({"sel_start": band.0, "sel_len": band.1}),
                    ));
                }
                tasks.extend(self.follow(editing, member));
                tasks
            }
            AudioAction::Looping { on } => {
                if let Some(editor) = editing.audio_mut(member) {
                    editor.set_looping(on);
                }
                let mut tasks = self.follow(editing, member);
                tasks.push(effect("looping", json!({"on": on})));
                tasks
            }
        };
        run.tasks.extend(tasks);
        self.runs.insert(id, run);
        self.drive(editing, member, id, ids)
    }

    /// What a message to the member came to, as the event handling did.
    pub(super) fn turned(&mut self, run: &mut Run, turned: Turned, lease: bool) -> Vec<Task> {
        let Turned {
            outcome,
            stepped,
            freed,
            version,
            ..
        } = turned;
        let outcome = audio_outcome(outcome);
        let mut tasks = vec![effect("version", json!({"version": version}))];
        if outcome.turn == Kind::Closed {
            tasks.push(Task::Close { closed: true });
            tasks.push(Task::Answer(json!(false)));
            return tasks;
        }
        if outcome.turn == Kind::Step {
            if let Some(stepped) = stepped {
                tasks.push(Task::Stepped(Box::new(stepped)));
            }
            tasks.push(effect("echo", json!({"answer": outcome.answer})));
            tasks.extend(release(freed));
            tasks.push(Task::Sound);
            return tasks;
        }
        run.changed = outcome.changed;
        tasks.push(Task::Take { outcome, lease });
        tasks.extend(release(freed));
        tasks
    }

    pub(super) fn focused(&self, editing: &Editing, member: MemberId) -> bool {
        let file = file_id(editing, member);
        file.is_some() && self.playback.focus() == file.map(|f| f as u64)
    }

    pub(super) fn script_play(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        at: Option<f64>,
    ) -> Vec<Task> {
        if at.is_none() && self.paused && self.focused(editing, member) {
            self.paused = false;
            return vec![Task::Playback(Op::Resume), Task::ShowTransport];
        }
        self.paused = false;
        let mut tasks = Vec::new();
        if let Some(at) = at {
            tasks.extend(self.script_locate(editing, member, at));
        }
        let play = space(editing, member);
        tasks.push(Task::Playback(Op::play(
            file_id(editing, member),
            play.as_ref(),
        )));
        tasks.push(Task::ShowTransport);
        tasks
    }

    pub(super) fn script_stop(&mut self, editing: &mut Editing, member: MemberId) -> Vec<Task> {
        let play = space(editing, member);
        vec![
            Task::Playback(Op::stop(file_id(editing, member), play.as_ref())),
            Task::ShowTransport,
        ]
    }

    pub(super) fn script_locate(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        at: f64,
    ) -> Vec<Task> {
        let frame = editing
            .audio_mut(member)
            .map_or(0, |editor| editor.set_cursor(at));
        vec![
            effect("cursor", json!({"at": at})),
            Task::Rolling(Rolled::Locate(frame)),
        ]
    }

    /// A pass in progress ends as the span and the switch now say.
    pub(super) fn follow(&mut self, editing: &mut Editing, member: MemberId) -> Vec<Task> {
        if !self.focused(editing, member) {
            return Vec::new();
        }
        let pass = space(editing, member).map(|play| play.pass);
        vec![Task::Playback(Op::Pass(pass))]
    }

    /// **The file in focus sounds on the engine**: its list handed to the
    /// engine, and the playback's readers set to the buffers the engine
    /// answers, one per channel.
    pub(super) fn sync_held(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        file: &PlaybackFile,
        ids: &mut IdSpaces,
    ) -> Flow {
        let rate = self.rate;
        let wanted = file.id as u64;
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
        let buffers = match engine.take(member, &take, ids) {
            Ok(buffers) => buffers,
            Err(e) => return Flow::Fail(Failure::Own(Raise::Runtime, e)),
        };
        let frames = file.frames;
        let take_rate = file.rate;
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
    pub(super) fn rolled(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        rolled: Rolled,
        rolling: bool,
    ) -> Vec<Task> {
        let file = file_id(editing, member);
        match rolled {
            Rolled::View { frame } => vec![
                Task::Playback(Op::View {
                    file: file.map_or(0, |f| f as u64),
                    frame,
                }),
                // the meter follows the file now viewed, before the window is drawn
                Task::TellMeters,
            ],
            Rolled::Space(play) => vec![
                // whichever file plays stops, and the one shown comes into focus
                if rolling {
                    Task::Playback(Op::stop(file, Some(&play)))
                } else {
                    Task::Playback(Op::play(file, Some(&play)))
                },
                Task::ShowTransport,
            ],
            // **A placed cursor moves the music where it plays**: the file in
            // focus rolling goes on from the cursor, its pass ending as the
            // switch says from there; stopped, the transport stands on it.
            Rolled::Cue(frame) if rolling && self.focused(editing, member) => {
                let mut tasks = vec![Task::Playback(Op::Locate(frame))];
                if let Some(play) = space(editing, member) {
                    tasks.push(Task::Playback(Op::Pass(Some(play.pass))));
                }
                tasks.push(Task::ShowTransport);
                tasks
            }
            Rolled::Cue(frame) => vec![Task::Playback(Op::Cue(frame)), Task::ShowTransport],
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
            Rolled::Locate(frame) => vec![Task::Playback(
                if rolling && self.focused(editing, member) {
                    Op::Locate(frame)
                } else {
                    Op::Cue(frame)
                },
            )],
        }
    }
}

/// An audio member's outcome, out of the context's.
pub(super) fn audio_outcome(outcome: super::super::Outcome) -> Box<Outcome> {
    match outcome {
        super::super::Outcome::Audio(outcome) => outcome,
        _ => Box::default(),
    }
}

/// Why the turn did not do what it asked, as its answer says.
pub(super) fn answer_reason(outcome: &Outcome) -> Option<String> {
    use clausters_editing::conversation::Answer as Told;
    match &outcome.answer {
        Some(Told::Ack { reason, .. } | Told::Push { reason, .. }) => reason.clone(),
        _ => None,
    }
}
