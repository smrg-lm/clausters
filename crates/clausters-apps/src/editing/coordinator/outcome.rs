//! **An outcome of the editor's, carried out**: the tasks a turn's answer
//! comes to, from its file binding to its tail.

use super::run::*;
use super::*;
use crate::turn::Kind;

impl Session {
    /// **An outcome, carried out**, up to its file binding: what the clients'
    /// turn handling did first.
    pub(super) fn take(
        &mut self,
        id: u64,
        mut outcome: Box<Outcome>,
        lease: bool,
    ) -> Result<Vec<Task>, Failure> {
        if let Some(prepared) = outcome.operation.take() {
            let mut tasks = vec![
                Task::Operation {
                    command: None,
                    prepared: Some(Box::new(Operated::prepared(prepared))),
                    repairing: false,
                },
                Task::TakeCompleted,
            ];
            if lease {
                tasks.push(Task::Finally(Cleanup::Lease));
            }
            return Ok(tasks);
        }
        if outcome.turn == Kind::Nothing {
            if let Some(run) = self.runs.get_mut(&id) {
                run.changed = false;
                run.result = json!(false);
            }
            return Ok(if lease {
                vec![Task::Effect(Said::Lease)]
            } else {
                Vec::new()
            });
        }
        let action = outcome.file_action;
        if let Some(action) = action.filter(|_| outcome.binding.is_none()) {
            return Ok(vec![
                effect("echo", json!({"answer": outcome.answer})),
                Task::VerbTake(action),
            ]);
        }
        let close_file = action.is_some_and(|a| a.verb == FileVerb::CloseFile);
        let mut tasks = Vec::new();
        if let Some(action) = action.filter(|_| close_file) {
            tasks.push(Task::Playback(Op::CloseFile(action.file as u64)));
        }
        if outcome.binding.is_some() {
            let steps = std::mem::take(&mut outcome.planned_steps);
            if !steps.is_empty() {
                tasks.push(Task::Steps(steps));
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
    pub(super) fn take_rest(
        &mut self,
        id: u64,
        mut outcome: Box<Outcome>,
        lease: bool,
        window: bool,
    ) -> Vec<Task> {
        let mut tasks = Vec::new();
        if let Some(layers) = &outcome.layers {
            tasks.push(effect("layers", json!({"layers": layers})));
        }
        // **A view placed in a lane of its own** (the stacked views, or back
        // to one): the window is drawn again and the host keeps what is its own.
        if outcome.restructure && window {
            tasks.push(effect("publish", json!({})));
            // the def carries no preview: each view is handed it again
            tasks.push(Task::Sound);
        }
        if let Some(run) = self.runs.get_mut(&id) {
            run.changed = outcome.changed;
        }
        // **The steps are the caller's to carry out**: the renders that make
        // the files the turn's list reads.
        let steps = std::mem::take(&mut outcome.planned_steps);
        if !steps.is_empty() {
            tasks.push(Task::Steps(steps));
        }
        if let Some(document) = outcome.clipboard.clone() {
            tasks.push(Task::PublishClipboard {
                document,
                ready: outcome.clipboard_ready,
            });
            if outcome.clipboard_cut.is_some() {
                tasks.push(Task::Cut(outcome.clone()));
            }
            tasks.push(Task::Catch(Handler::Clipboard {
                answer: outcome.answer.clone(),
            }));
        }
        if lease {
            tasks.push(Task::Finally(Cleanup::Lease));
        }
        tasks.push(Task::TakeTail(outcome));
        tasks
    }

    /// The tail of an outcome: files, playback, the transport, the answer.
    pub(super) fn take_tail(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        mut outcome: Box<Outcome>,
        changed: bool,
    ) -> Vec<Task> {
        let mut tasks = Vec::new();
        if outcome.new_file.is_some() || outcome.open_file.is_some() {
            tasks.push(effect("echo", json!({"answer": outcome.answer})));
            outcome.answer = None;
        }
        if let Some(shape) = outcome.new_file {
            tasks.push(Task::Read(VecDeque::from([Wanted::Shape(shape)])));
        }
        if let Some(open) = &outcome.open_file {
            tasks.push(Task::Read(paths(&open.paths)));
        }
        if let Some(check) = outcome.check_disk.take() {
            tasks.push(Task::Stat(check));
        }
        if let Some(update) = outcome.playback_update {
            tasks.push(Task::SyncSound(update.file));
            if update.publish {
                tasks.push(Task::Sound);
            }
        }
        if changed {
            tasks.push(effect("changed", json!({})));
            if !outcome.prepared_complete {
                tasks.push(Task::Sound);
            }
        }
        if let Some(play) = outcome.play {
            // The loop switch rides beside the space bar and `L`: a pass that
            // loops is the switch on.
            self.paused = false;
            tasks.push(Task::Rolling(Rolled::Space(play)));
        }
        if let Some(pass) = outcome.pass {
            // `L`: the pass in progress ends as the switch now says; a stopped
            // playback reads it on its next play.
            tasks.push(Task::Playback(Op::Pass(Some(pass))));
        }
        if let Some(follow) = outcome.follow {
            // A selection redrawn while this take loops: the loop is over the
            // new span, and the head goes into it once the sweep is let go.
            // Whether it loops, and whether it is this take, is the playback's
            // to say.
            tasks.push(Task::Playback(Op::Follow {
                file: file_id(editing, member).map_or(0, |f| f as u64),
                from: follow.from,
                to: follow.to,
                place: follow.place,
            }));
        }
        if let Some(cue) = outcome.cue {
            tasks.push(Task::Rolling(Rolled::Cue(cue)));
        }
        if let Some(at) = outcome.locate {
            tasks.push(effect("locate", json!({"at": at})));
        }
        if outcome.clipboard_cut.is_none() {
            tasks.push(effect("echo", json!({"answer": outcome.answer})));
        }
        if outcome.focus_changed {
            tasks.push(Task::ShowTransport);
        }
        if outcome.close {
            tasks.push(Task::Close { closed: false });
        }
        tasks.push(Task::Answer(json!(changed)));
        tasks
    }
}
