//! The multitrack's typed runtime, driven through the application's caller.
use super::*;
use crate::application::{
    operations::{Prepared, Target},
    runtime::{Control, Dispatch, Reply, Settled},
};
use crate::multitrack::operations::{Command, Commit, Request};

pub(super) struct SaveTo {
    pub path: Option<String>,
    pub close: bool,
    pub save_as: bool,
}

impl Editing {
    pub(super) fn retry_multitrack(
        &mut self,
        member: MemberId,
        caller: &mut dyn Caller,
        ids: &mut clausters_core::ids::IdSpaces,
        ran: &mut Ran,
    ) {
        let Dispatch::Response(protocol) = self.multitrack_runtime.control(
            &mut self.services.resources,
            Control::PendingRepairs { member },
            self.version,
        ) else {
            unreachable!("repair discovery has no domain transition")
        };
        for repair in protocol.repairs.unwrap_or_default() {
            let dispatch = self.multitrack_runtime.control(
                &mut self.services.resources,
                Control::RetryRepair {
                    ticket: repair.ticket,
                },
                self.version,
            );
            let mut retried = Ran::default();
            self.drive_multitrack_operation(dispatch, caller, ids, &mut retried);
            if !self
                .multitrack_runtime
                .operations
                .busy(repair.ticket.target)
            {
                retried.effects.retain(|effect| effect["kind"] != "say");
                retried.end = Some(Ok(Value::Null));
            }
            ran.effects.extend(retried.effects);
            if ran.end.as_ref().is_none_or(Result::is_ok) {
                ran.end = retried.end;
            }
        }
    }

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
        mut dispatch: Dispatch<Commit, multitrack::Outcome>,
        caller: &mut dyn Caller,
        ids: &mut clausters_core::ids::IdSpaces,
        ran: &mut Ran,
    ) {
        loop {
            let settled = match dispatch {
                Dispatch::Finish(finished) => {
                    let member = finished.ticket.target.member;
                    let Member::Multitrack(editor) = &mut self.seats[member as usize].member else {
                        unreachable!("the operation belongs to a multitrack member");
                    };
                    self.multitrack_runtime.settle_turn(
                        editor.as_mut(),
                        &mut self.services,
                        finished,
                        &mut self.version,
                    )
                }
                Dispatch::Repaired(repair) => crate::application::runtime::Runtime::<
                    Commit,
                    multitrack::Outcome,
                >::repaired_turn(repair, self.version),
                Dispatch::Response(protocol) => Settled {
                    protocol,
                    outcome: None,
                    recovery: None,
                    returned: None,
                },
            };
            if let Some(work) = settled.protocol.batch {
                self.wait_multitrack_operation(work, ran);
                return;
            }
            if let Some(ticket) = settled.protocol.repair {
                let result = settled
                    .protocol
                    .recoveries
                    .unwrap_or_default()
                    .iter()
                    .try_for_each(|batch| caller.carry(&batch.steps));
                dispatch = self.multitrack_runtime.control(
                    &mut self.services.resources,
                    match result {
                        Ok(()) => Control::Repaired { ticket },
                        Err(reason) => Control::RepairFailed { ticket, reason },
                    },
                    self.version,
                );
                continue;
            }
            if let Some(saved) = settled.outcome.and_then(|outcome| outcome.saved) {
                let member = settled
                    .recovery
                    .as_ref()
                    .map(|batch| batch.member)
                    .expect("a committed endpoint");
                if let Some(tracks) = self.tracks.get_mut(&member) {
                    tracks.saved = Some(saved.session.clone());
                    tracks.session = Some((saved.session, saved.path.clone()));
                    if saved.save_as {
                        tracks.save_to = None;
                    }
                }
                if saved.close {
                    if let Some(mut tracks) = self.tracks.remove(&member) {
                        self.close_tracks(member, &mut tracks, ids);
                    }
                    ran.effects.push(self.closing(member, "closeWindow"));
                }
                ran.end = Some(Ok(json!({"destination":saved.path})));
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
            return;
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
