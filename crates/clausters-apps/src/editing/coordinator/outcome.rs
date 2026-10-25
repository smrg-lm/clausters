//! **An outcome of the editor's, carried out**: the tasks a turn's answer
//! comes to, from its file binding to its tail.

use super::run::*;
use super::*;

impl Session {
    /// **An outcome, carried out**, up to its file binding: what the clients'
    /// turn handling did first.
    pub(super) fn take(
        &mut self,
        id: u64,
        outcome: Value,
        lease: bool,
    ) -> Result<Vec<Task>, Failure> {
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
            tasks.push(Task::Playback(Op::CloseFile(
                action["file"].as_u64().unwrap_or(0),
            )));
        }
        if outcome.get("binding").is_some() {
            if truthy(&outcome["steps"]) {
                tasks.push(Task::Steps(outcome["steps"].clone()));
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
        outcome: Value,
        lease: bool,
        window: bool,
    ) -> Vec<Task> {
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
        // **The steps are the caller's to carry out**: the renders that make
        // the files the turn's list reads.
        let steps = outcome.get("steps").cloned().unwrap_or(json!([]));
        let copied = has(&outcome, "clipboard");
        if truthy(&steps) {
            tasks.push(Task::Steps(steps));
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
    pub(super) fn take_tail(
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
            tasks.push(Task::Playback(Op::pass(&outcome["pass"])));
        }
        if has(&outcome, "follow") {
            // A selection redrawn while this take loops: the loop is over the
            // new span, and the head goes into it once the sweep is let go.
            // Whether it loops, and whether it is this take, is the playback's
            // to say.
            let follow = &outcome["follow"];
            tasks.push(Task::Playback(Op::Follow {
                file: file_id(editing, member).map_or(0, |f| f as u64),
                from: follow["from"].as_u64().unwrap_or(0),
                to: follow["to"].as_u64().unwrap_or(0),
                place: follow["place"] == true,
            }));
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
