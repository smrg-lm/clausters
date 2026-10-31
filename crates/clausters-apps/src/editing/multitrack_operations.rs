//! The multitrack's typed runtime, driven through the application's caller.
use super::*;
use crate::application::{
    operations::{Prepared, Target},
    runtime::{Control, Dispatch, Reply},
};
use crate::multitrack::operations::{Command, Commit, Request};

pub(super) struct SaveTo {
    pub path: Option<String>,
    pub then: Option<crate::multitrack::operations::After>,
    pub save_as: bool,
}

impl Editing {
    pub(super) fn measure_multitrack(
        &mut self,
        member: MemberId,
        sources: Vec<(SourceId, String, clausters_editing::apply::Step)>,
        ran: &mut Ran,
    ) -> Result<(), String> {
        let seat = self
            .seats
            .get(member as usize)
            .ok_or("the member is absent")?;
        let Member::Multitrack(editor) = &seat.member else {
            return Err("the member is not a multitrack".into());
        };
        let request = Request {
            target: Target {
                member,
                object: editor.session_binding(),
                server: seat.server,
            },
            command: Command::Measure(sources),
        };
        let ticket =
            self.multitrack_runtime
                .prepare(editor.as_ref(), &mut self.services, request)?;
        let dispatch = self.multitrack_runtime.next(ticket);
        let Dispatch::Response(protocol) = dispatch else {
            unreachable!("source measurements wait on their batch");
        };
        self.wait_multitrack_operation(protocol.batch.expect("a measurement batch"), ran);
        Ok(())
    }

    pub(super) fn multitrack_landed(
        &mut self,
        member: MemberId,
        run: u64,
        result: Result<(), String>,
        caller: &mut dyn Caller,
        ids: &mut clausters_core::ids::IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        let Some(work) = self.multitrack_waits.get(&run) else {
            return ran;
        };
        if work.member != member {
            return ran;
        }
        let work = self.multitrack_waits.remove(&run).unwrap();
        let dispatch = self.multitrack_runtime.control(
            &mut self.services.resources,
            Control::Complete {
                ticket: work.ticket,
                batch: work.batch,
                result: match result {
                    Ok(()) => Reply::Success,
                    Err(reason) => Reply::Failed(reason),
                },
            },
            self.version,
        );
        self.drive_multitrack_operation(dispatch, caller, ids, &mut ran);
        ran
    }

    pub(super) fn drive_multitrack_operation(
        &mut self,
        dispatch: Dispatch<Commit, multitrack::Outcome>,
        caller: &mut dyn Caller,
        ids: &mut clausters_core::ids::IdSpaces,
        ran: &mut Ran,
    ) {
        let settled = super::settle_operation(
            &mut self.multitrack_runtime,
            &mut self.services,
            &mut self.version,
            &mut self.seats,
            |member| match member {
                Member::Multitrack(editor) => editor.as_mut(),
                _ => unreachable!("the operation belongs to a multitrack member"),
            },
            dispatch,
        );
        if let Some(work) = settled.protocol.batch {
            self.wait_multitrack_operation(work, ran);
            return;
        }
        if let Some(saved) = settled.outcome.and_then(|outcome| outcome.saved) {
            let member = settled
                .recovery
                .as_ref()
                .map(|batch| batch.member)
                .expect("a committed endpoint");
            let mut moved = false;
            let then = saved.then.clone();
            if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
                editor.set_checkpoint(Some(saved.session.clone()));
                let file = editor.file_mut();
                file.session = Some(saved.session);
                file.path = Some(saved.path.clone());
                if saved.save_as {
                    file.save_to = None;
                }
                moved = editor.unsaved_contents();
            }
            ran.end = Some(Ok(json!({"destination":saved.path})));
            // **what waited for the save is done only over what it
            // wrote**: an edit made while it was being written is work
            // the save does not hold, and the window keeps it
            match then {
                Some(_) if moved => ran.effects.push(json!({"kind":"say",
                        "reason":"the session changed while it was saved: it stays open"})),
                Some(crate::multitrack::operations::After::Close) => {
                    if let Some(mut tracks) = self.tracks.remove(&member) {
                        self.close_tracks(member, &mut tracks, ids);
                    }
                    ran.effects.push(self.closing(member, "closeWindow"));
                }
                Some(crate::multitrack::operations::After::File(change)) => {
                    if let Some(mut tracks) = self
                        .tracks
                        .remove(&member)
                        .or_else(|| super::multitrack_runs::Tracks::new(self))
                    {
                        if let Err(why) =
                            self.change_file(member, &mut tracks, change, caller, ids, ran)
                        {
                            ran.effects.push(json!({"kind":"say", "reason":why}));
                        }
                        self.tracks.insert(member, tracks);
                    }
                }
                None => {}
            }
        }
        if let Some(error) = settled.protocol.error {
            ran.end = Some(Err(Raised {
                kind: "command".into(),
                message: error,
            }));
        }
        if let Some(reason) = settled.protocol.reason {
            ran.end = Some(Err(Raised {
                kind: "command".into(),
                message: reason.clone(),
            }));
            ran.effects.push(json!({"kind":"say", "reason":reason}));
        }
        if settled.protocol.committed == Some(true)
            && !ran
                .effects
                .iter()
                .any(|effect| effect["kind"] == "closeWindow")
        {
            ran.effects.push(json!({"kind":"resync"}));
        }
        let freed = self.release();
        for freed in freed {
            caller.delete(&freed.files);
        }
    }

    pub(super) fn wait_multitrack_operation(&mut self, work: Prepared, ran: &mut Ran) {
        // A new dispatch gets a new run id, including a later batch of the
        // same operation. A duplicate landing cannot consume that batch.
        let run = self.services.resources.new_owner();
        ran.render = Some(Render {
            run,
            steps: work.steps.clone(),
        });
        self.multitrack_waits.insert(run, work);
    }
}

#[cfg(test)]
mod tests;
