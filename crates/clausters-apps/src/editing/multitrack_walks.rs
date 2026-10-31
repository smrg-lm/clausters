//! Multitrack history plans use the same confirmation algorithm as audio.
use super::*;
use crate::application::{
    operations::{Finished, Result as IoResult, Target},
    runtime::{Control, PendingWalk},
    services::{Application, Completion},
    walks::WalkHost,
};
use crate::multitrack::operations::{Command, Commit, Request};

impl Editing {
    pub(super) fn prepare_multitrack_walk(&mut self, direction: Direction) -> Option<Stepped> {
        let expected = self.services.histories.peek_walk(direction)?;
        let scope = self.services.histories.selected();
        let mut plans = Vec::new();
        for (structure, _) in &expected.legs {
            for (id, seat) in self.seats.iter_mut().enumerate() {
                let object = self.services.histories.object_for(id as u32, *structure);
                if seat.structure != *structure && object.is_none() {
                    continue;
                }
                // Audio participation is prepared by its existing runtime.
                if matches!(seat.member, Member::Audio(_)) {
                    return None;
                }
                if let Member::Multitrack(editor) = &mut seat.member {
                    let request = Request {
                        target: Target {
                            member: id as u32,
                            object: object.map_or(editor.session_binding(), |object| object.id),
                            server: seat.server,
                        },
                        command: Command::Restore,
                    };
                    let prepared = editor
                        .over_session(request.target.object, |editor| {
                            editor.prepare(request, &self.services)
                        })
                        .unwrap_or_else(|| Err("the session is no longer retained".into()));
                    match prepared {
                        Ok(plan) => plans.push((id as u32, plan)),
                        Err(reason) => return Some(self.refused_multitrack_walk(reason)),
                    }
                }
            }
        }
        if plans.is_empty() {
            return None;
        }
        let ticket = match self.multitrack_runtime.prepare_walk(
            &mut self.services.resources,
            scope,
            direction,
            expected,
            plans,
        ) {
            Ok(ticket) => ticket,
            Err(reason) => return Some(self.refused_multitrack_walk(reason.into())),
        };
        let finished = self
            .multitrack_runtime
            .operations
            .finish_ready(ticket)
            .expect("a multitrack walk has no I/O");
        let walk = self
            .multitrack_runtime
            .take_walk(ticket)
            .expect("a prepared walk");
        Some(self.settle_multitrack_walk(finished, walk))
    }

    pub(super) fn settle_multitrack_walk(
        &mut self,
        finished: Finished<Commit>,
        walk: PendingWalk<Commit>,
    ) -> Stepped {
        let decision = crate::application::walks::finish(self, finished, walk);
        match decision.result {
            Ok(mut stepped) => {
                self.multitrack_runtime.finish_walk(
                    &mut self.services.resources,
                    decision.ticket,
                    decision.receipt,
                    stepped.reason.clone(),
                    self.version,
                );
                stepped.freed.extend(self.release());
                stepped
            }
            Err(refused) => {
                let reason = refused.reason.clone();
                debug_assert!(
                    refused
                        .recoveries
                        .iter()
                        .all(|batch| batch.steps.is_empty())
                );
                self.multitrack_runtime.refuse_walk(
                    decision.ticket,
                    decision.additional_targets,
                    decision.receipt,
                    refused,
                    &self.services.resources,
                    self.version,
                );
                // These plans have no external effects to recover. The common
                // runtime releases them without an executor or another edit.
                self.multitrack_runtime.control(
                    &mut self.services.resources,
                    Control::Repaired {
                        ticket: decision.ticket,
                    },
                    self.version,
                );
                self.refused_multitrack_walk(reason)
            }
        }
    }

    fn refused_multitrack_walk(&self, reason: String) -> Stepped {
        Stepped {
            stepped: false,
            operation: None,
            reason: Some(reason),
            effects: Vec::new(),
            corrections: Vec::new(),
            freed: Vec::new(),
            version: self.version,
        }
    }
}

impl WalkHost<Commit> for Editing {
    type Stepped = Stepped;
    type Outcome = multitrack::Outcome;
    fn confirm_walk(&mut self, walk: &PendingWalk<Commit>) -> Result<Stepped, String> {
        self.services.histories.select(walk.scope);
        if self.services.histories.peek_walk(walk.direction).as_ref() != Some(&walk.expected) {
            return Err("the editing order changed while the step was pending".into());
        }
        let stepped = self.step_now(walk.direction);
        if stepped.stepped {
            Ok(stepped)
        } else {
            Err(stepped
                .reason
                .unwrap_or_else(|| "the history step was refused".into()))
        }
    }
    fn abort_walk(
        &mut self,
        member: u32,
        delta: Commit,
        result: IoResult,
        completed: usize,
    ) -> Completion<multitrack::Outcome> {
        let Member::Multitrack(editor) = &mut self.seats[member as usize].member else {
            unreachable!()
        };
        editor.abort(delta, result, completed, &mut self.services)
    }
    fn walk_server(&self, member: u32) -> ServerInstance {
        self.seats[member as usize].server
    }
}
