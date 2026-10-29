//! **One task of a run, carried out**: an effect said, a door of the
//! context or the editor called, a playback verb carried out, or an await
//! handed to the caller.

use super::actions as arms;
use super::run::*;
use super::*;

impl Session {
    pub(super) fn step(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        task: Task,
        effects: &mut Vec<Said>,
        ids: &mut IdSpaces,
    ) -> Flow {
        let window = self.runs.get(&id).is_some_and(|run| run.window);
        match task {
            Task::Effect(said) => {
                effects.push(said);
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
                    let destination = run
                        .completed
                        .as_ref()
                        .and_then(|completed| completed.destination.clone());
                    run.result = json!({"destination": destination});
                }
                Flow::Next
            }
            Task::Finally(Cleanup::Lease) => {
                effects.push(Said::Lease);
                Flow::Next
            }
            // reached without a failure: nothing to handle
            Task::Catch(_) => Flow::Next,
            Task::Steps(steps) => Flow::Await(Await::Steps(steps), Resume::Steps),
            Task::Stepped(stepped) => Flow::Await(Await::Stepped(stepped), Resume::Stepped),
            Task::Take { outcome, lease } => match self.take(id, outcome, lease) {
                Ok(tasks) => {
                    self.push(id, tasks);
                    Flow::Next
                }
                Err(failure) => Flow::Fail(failure),
            },
            Task::VerbTake(action) => {
                let at = audio::FileAt::File(action.file);
                match editing.audio_file_verb(member, action.verb, at) {
                    Ok(outcome) => {
                        self.push(
                            id,
                            vec![Task::Take {
                                outcome: Box::new(outcome),
                                lease: false,
                            }],
                        );
                        Flow::Next
                    }
                    Err(error) => Flow::Fail(Failure::Own(Raise::Value, error)),
                }
            }
            Task::Bound {
                mut outcome,
                close_file,
                lease,
            } => {
                let mut tasks = Vec::new();
                // a closed file's renders: its history went with it
                let deleted = std::mem::take(&mut outcome.delete_files);
                if !deleted.is_empty() {
                    tasks.push(Task::Effect(Said::Delete(deleted)));
                }
                if close_file {
                    tasks.push(Task::RetainPlayback);
                }
                if outcome.close {
                    tasks.push(Task::Close { closed: false });
                    tasks.push(Task::Answer(json!(false)));
                    self.push(id, tasks);
                    return Flow::Next;
                }
                let binding = outcome.binding.clone().unwrap_or(Value::Null);
                if outcome.focus_changed && binding.is_null() {
                    // the last file closed: the editor stays, with none
                    tasks.push(effect("bind", json!({"binding": null})));
                } else if outcome.focus_changed {
                    // the file in focus brings back the span and the loop
                    // switch it left, which are its own
                    tasks.push(effect("bind", json!({"binding": binding})));
                    tasks.push(Task::Sound);
                    // a pass in progress goes on: the window only looks elsewhere
                    tasks.push(Task::Rolling(Rolled::View {
                        frame: binding["cursor"].as_u64().unwrap_or(0),
                    }));
                }
                tasks.push(Task::TakeRest { outcome, lease });
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
                let outcome = self
                    .runs
                    .get(&id)
                    .and_then(|run| run.completed.as_ref())
                    .and_then(|completed| completed.outcome.clone())
                    .unwrap_or_default();
                if let Some(run) = self.runs.get_mut(&id) {
                    run.changed = outcome.changed;
                }
                self.push(
                    id,
                    vec![Task::Take {
                        outcome: Box::new(outcome),
                        lease: false,
                    }],
                );
                Flow::Next
            }
            Task::PublishClipboard { document, ready } => {
                Flow::Await(Await::Clipboard { document, ready }, Resume::Clipboard)
            }
            // a cut's second half, once its copy is on the clipboard
            Task::Cut(outcome) => {
                let Some(cut) = outcome.clipboard_cut.clone() else {
                    return Flow::Next;
                };
                let continued = editing.prepare_finish_cut(member, &cut);
                let version = continued.version;
                let mut next = arms::audio_outcome(continued.outcome);
                use clausters_editing::conversation::Answer as Told;
                if let Some(Told::Ack { seq, .. } | Told::Push { seq, .. }) = next.answer.as_mut() {
                    *seq = match outcome.answer.as_ref() {
                        Some(Told::Ack { seq, .. } | Told::Push { seq, .. }) => *seq,
                        _ => 0,
                    };
                }
                if let Some(run) = self.runs.get_mut(&id) {
                    run.changed = next.changed;
                }
                let mut tasks = vec![effect("version", json!({"version": version}))];
                tasks.push(Task::Take {
                    outcome: std::mem::take(&mut next),
                    lease: false,
                });
                tasks.extend(release(continued.freed));
                self.push(id, tasks);
                Flow::Next
            }
            Task::Stat(check) => {
                Flow::Await(Await::Stat(check.paths), Resume::Stat { seq: check.seq })
            }
            Task::Read(mut files) => {
                let Some(file) = files.pop_front() else {
                    return Flow::Next;
                };
                if !files.is_empty() {
                    self.push(id, vec![Task::Read(files)]);
                }
                Flow::Await(Await::Shape(file), Resume::Read)
            }
            Task::AddFile(file) => {
                let tasks = match editing.add_audio_file(member, &file) {
                    Ok(operated) => vec![
                        Task::Operation {
                            command: None,
                            prepared: Some(Box::new(operated)),
                            repairing: false,
                        },
                        Task::TakeCompleted,
                    ],
                    Err((error, freed)) => {
                        if !freed.is_empty() {
                            effects.push(Said::Release(freed));
                        }
                        return Flow::Fail(Failure::Own(Raise::Value, error));
                    }
                };
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
                    None => Box::new(editing.audio_operation(OperationCall::Prepare {
                        member,
                        command: command.unwrap_or(AudioCommand::Open),
                    })),
                };
                self.push(id, vec![Task::OperationAnswer { answer, repairing }]);
                Flow::Next
            }
            Task::OperationAnswer { answer, repairing } => {
                if !answer.protocol.complete {
                    if answer.protocol.accepted == Some(false) {
                        return Flow::Fail(Failure::Own(
                            Raise::Runtime,
                            "the operation is no longer available".into(),
                        ));
                    }
                    if let Some(error) = &answer.protocol.error {
                        return Flow::Fail(Failure::Own(Raise::Value, error.clone()));
                    }
                    let Some(prepared) = answer.protocol.batch else {
                        return Flow::Fail(Failure::Own(
                            Raise::Runtime,
                            "the operation handed out no batch".into(),
                        ));
                    };
                    let rendered = rendered_files(&prepared.steps);
                    return Flow::Await(
                        Await::Steps(prepared.steps),
                        Resume::Batch {
                            ticket: prepared.ticket,
                            batch: prepared.batch,
                            repairing,
                            member: prepared.member,
                            rendered,
                        },
                    );
                }
                // the audio editor's work leaves nothing to recover: a
                // refused operation is repaired at once
                if let Some(ticket) = answer.protocol.repair {
                    let answer = Box::new(
                        editing
                            .audio_operation(OperationCall::Control(Control::Repaired { ticket })),
                    );
                    self.push(id, vec![Task::OperationDone { answer, repairing }]);
                    return Flow::Next;
                }
                self.push(id, vec![Task::OperationDone { answer, repairing }]);
                Flow::Next
            }
            Task::OperationDone { answer, repairing } => {
                if let Some(version) = answer.protocol.version {
                    effects.push(Said::Window(tagged("version", json!({"version": version}))));
                }
                if let Some(echo) = &answer.protocol.echo {
                    effects.push(Said::Window(tagged("echo", json!({"answer": echo.answer}))));
                }
                if let Some(freed) = answer.freed.clone().filter(|f| !f.is_empty()) {
                    effects.push(Said::Release(freed));
                }
                let committed = answer.protocol.committed == Some(true);
                let repaired = repairing && answer.repaired;
                let refusal = (!committed && !repaired).then(|| {
                    let reason = answer
                        .protocol
                        .reason
                        .clone()
                        .or_else(|| answer.outcome.as_ref().and_then(arms::answer_reason))
                        .unwrap_or_else(|| "the operation was refused".into());
                    (answer.protocol.echo.is_none(), reason)
                });
                let told = answer
                    .outcome
                    .as_ref()
                    .map(|outcome| outcome.answer.clone());
                if let Some(run) = self.runs.get_mut(&id) {
                    run.result = json!({"completed": answer});
                    run.completed = Some(answer);
                }
                if let Some((unechoed, reason)) = refusal {
                    if unechoed {
                        effects.push(Said::Window(tagged(
                            "echo",
                            json!({"answer": told.flatten()}),
                        )));
                    }
                    return Flow::Fail(Failure::Own(Raise::Command, reason));
                }
                Flow::Next
            }
            Task::Sound => {
                let file = editing.audio(member).and_then(AudioEditor::playback_file);
                match file.filter(|file| file.active) {
                    Some(file) => {
                        self.push(id, vec![Task::SyncSound(file), Task::TellMeters]);
                        Flow::Next
                    }
                    None => Flow::Next,
                }
            }
            Task::SyncSound(file) => {
                if !file.active {
                    return Flow::Next;
                }
                self.sync_held(editing, member, id, &file, ids)
            }
            Task::TellMeters => {
                let meters = self.playback.meters();
                let corrections = editing
                    .audio_mut(member)
                    .map(|editor| editor.sync_meters(meters))
                    .unwrap_or_default();
                // the window's meter, drawn before the playback said where it
                // writes, reads there from now on
                if window {
                    effects.extend(corrections.into_iter().map(set));
                }
                Flow::Next
            }
            Task::ShowTransport => {
                if !window {
                    return Flow::Next;
                }
                let playing = self.playback.playing().map(|f| f as i64);
                let meters = self.playback.meters();
                let corrections = editing
                    .audio_mut(member)
                    .map(|editor| editor.playing(playing, Some(meters)))
                    .unwrap_or_default();
                effects.extend(corrections.into_iter().map(set));
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
            Task::Playback(op) => {
                let playback = &mut self.playback;
                let steps = match op {
                    Op::View { file, frame } => Ok(playback.view(file, frame)),
                    Op::Play {
                        file,
                        start,
                        pass: Some(pass),
                    } => Ok(playback.play(file, start, pass)),
                    Op::Play { pass: None, .. } => Err("a play needs a pass".to_string()),
                    Op::Pass(Some(pass)) => Ok(playback.set_pass(pass)),
                    Op::Pass(None) => Err("a pass is a loop or an end".to_string()),
                    Op::Follow {
                        file,
                        from,
                        to,
                        place,
                    } => Ok(playback.follow(file, from, to, place)),
                    Op::Resume => Ok(playback.resume()),
                    Op::Pause => Ok(playback.pause()),
                    Op::Stop {
                        file: Some(file),
                        back,
                    } => Ok(playback.stop_viewing(file, back)),
                    Op::Stop { file: None, back } => Ok(playback.stop(back)),
                    Op::Place {
                        frame,
                        pass: Some(pass),
                    } => Ok(playback.place(frame, pass)),
                    Op::Place { frame, pass: None } => Ok(playback.cue(frame)),
                    Op::Locate(frame) => Ok(playback.locate(frame)),
                    Op::Cue(frame) => Ok(playback.cue(frame)),
                    Op::Close => {
                        // the window's readings go with it, as a file's do
                        if let Some(engine) = editing.engine.as_mut() {
                            engine.close_member(member, ids);
                        }
                        playback.close(ids).map_err(|e| e.to_string())
                    }
                    Op::CloseFile(file) => {
                        if let Some(engine) = editing.engine.as_mut() {
                            engine.close(member, file, ids);
                        }
                        playback.close_file(file, ids).map_err(|e| e.to_string())
                    }
                };
                match steps {
                    Err(error) => Flow::Fail(Failure::Own(Raise::Runtime, error)),
                    Ok(steps) if steps.is_empty() => Flow::Next,
                    Ok(steps) => Flow::Await(Await::Steps(steps), Resume::Steps),
                }
            }
            Task::RetainPlayback => {
                let freed = editing.retain_audio_playback(member);
                self.push(id, release(freed).into_iter().collect());
                Flow::Next
            }
            Task::DropPlayback => {
                let freed = editing.drop_audio_playback(member);
                self.push(id, release(freed).into_iter().collect());
                Flow::Next
            }
            Task::Close { closed } => {
                if let Some(editor) = editing.audio_mut(member) {
                    editor.detach_view();
                }
                self.push(
                    id,
                    vec![
                        Task::Playback(Op::Close),
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

/// A correction of the window's, as the effect that sets it.
fn set(correction: clausters_editing::conversation::Correction) -> Said {
    Said::Window(tagged(
        "set",
        json!({"widget": correction.widget, "props": correction.props}),
    ))
}
