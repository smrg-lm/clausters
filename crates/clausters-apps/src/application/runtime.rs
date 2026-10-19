//! Shared pending-work protocol for any application implementing `Application`.
//! Each application retains its typed deltas while borrowing the composition's
//! single ownership graph. Clients execute batches and report results only.
use super::{
    operations::{Batch, Finished, Operations, Prepared, Result as IoResult, Ticket},
    resources::{Resources, ServerInstance},
    services::{Application, Completion, Services},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Control messages common to all applications, decoded at the JSON boundary.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Control {
    /// A dispatched batch settled.
    Complete {
        ticket: Ticket,
        batch: usize,
        result: Reply,
    },
    /// Stop issuing work after any in-flight batch settles.
    Cancel { ticket: Ticket },
    /// All repair effects settled successfully.
    Repaired { ticket: Ticket },
    /// Read the existing repair without consuming it.
    RetryRepair { ticket: Ticket },
    /// Keep the repair and its lock after an unsuccessful attempt.
    RepairFailed { ticket: Ticket, reason: String },
}

/// Executor report; application commit policy is separate from this result.
#[derive(Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "camelCase")]
pub enum Reply {
    /// Every wait succeeded.
    Success,
    /// A command or wait failed.
    Failed(String),
    /// Outstanding work drained after cancellation.
    Cancelled,
}

/// A failed operation retains both its resource lease and original answer.
#[derive(Debug)]
pub struct Repair<O> {
    /// Operation being repaired.
    pub ticket: Ticket,
    /// Endpoint-addressed repair steps.
    pub batches: Vec<Batch>,
    /// The original refusal, including subsequent repair failures.
    pub reason: String,
    /// Domain answer published after successful repair.
    pub outcome: O,
    /// Original gesture answered after recovery, or on a repair failure.
    pub receipt: Option<(u32, i64)>,
    /// Unused ids returned only after the recovery effects settle.
    pub returned: Vec<i64>,
}
impl<O> Repair<O> {
    /// Serialize existing steps only at the executor boundary.
    pub fn response(&self, resources: &Resources, version: i64) -> Value {
        let recoveries = self
            .batches
            .iter()
            .filter(|batch| resources.valid(batch.server))
            .map(|batch| {
                json!({"member":batch.member,"server":batch.server,
                "steps":clausters_editing::apply::steps_json(&batch.steps)})
            })
            .collect::<Vec<_>>();
        json!({"complete":true,"committed":false,"repair":self.ticket,
            "recoveries":recoveries,"reason":self.reason,"version":version})
    }
}

/// The composition handles domain completion and collection at explicit points.
pub enum Dispatch<C, O> {
    /// I/O has settled; ask the original application to commit or abort.
    Finish(Finished<C>),
    /// Recovery settled; collection and domain refusal may now be published.
    Repaired(Repair<O>),
    /// A protocol response requiring no domain policy.
    Response(Value),
}

/// One application's typed work registry; it owns no resource graph.
#[derive(Debug)]
pub struct Runtime<C, O> {
    /// Typed operation identity, dependency and access rules.
    pub operations: Operations<C>,
    repairs: BTreeMap<u64, Repair<O>>,
    walks: BTreeMap<u64, PendingWalk<C>>,
}
impl<C, O> Default for Runtime<C, O> {
    fn default() -> Self {
        Self {
            operations: Operations::default(),
            repairs: BTreeMap::new(),
            walks: BTreeMap::new(),
        }
    }
}
impl<C, O> Runtime<C, O> {
    /// Compose typed history plans without copying histories or spilled data.
    pub fn prepare_walk(
        &mut self,
        resources: &mut Resources,
        scope: super::histories::ScopeId,
        direction: clausters_document::history::Direction,
        expected: clausters_document::history::Walked,
        mut plans: Vec<(u32, super::operations::Operation<C>)>,
    ) -> Result<Ticket, &'static str> {
        if plans.is_empty() {
            return Err("the history operation has no participants");
        }
        let (_, mut operation) = plans.remove(0);
        let mut others = Vec::new();
        for (member, plan) in plans {
            operation.additional_targets.push(plan.target);
            operation.additional_targets.extend(plan.additional_targets);
            operation.batches.extend(plan.batches);
            operation.allocations.extend(plan.allocations);
            operation.reads.extend(plan.reads);
            others.push((member, plan.commit));
        }
        let ticket = self.operations.prepare(resources, operation)?;
        self.walks.insert(
            ticket.id,
            PendingWalk {
                scope,
                direction,
                expected,
                others,
                receipt: None,
            },
        );
        Ok(ticket)
    }

    /// Carry the originating gesture receipt through asynchronous history I/O.
    pub fn acknowledge_walk(&mut self, ticket: Ticket, member: u32, seq: i64) {
        if let Some(walk) = self.walks.get_mut(&ticket.id) {
            walk.receipt = Some((member, seq));
        }
    }

    /// Consume the history context captured by this operation, if any.
    pub fn take_walk(&mut self, ticket: Ticket) -> Option<PendingWalk<C>> {
        self.walks.remove(&ticket.id)
    }

    /// Apply the same finalization policy to every application type.
    pub fn finish<A: Application<Commit = C, Outcome = O>>(
        &mut self,
        app: &mut A,
        services: &mut Services,
        finished: Finished<C>,
    ) -> Completion<O> {
        self.operations.finish(app, services, finished)
    }

    /// Confirm the context version and acknowledgement using every editor's
    /// existing turn contract. Domain bindings do not duplicate this policy.
    pub fn finish_turn<A: Application<Commit = C, Outcome = O>>(
        &mut self,
        app: &mut A,
        services: &mut Services,
        finished: Finished<C>,
        version: &mut i64,
    ) -> Completion<O>
    where
        O: crate::turn::Turned,
    {
        let mut completion = self.finish(app, services, finished);
        if completion.committed && completion.outcome.changed() {
            *version += 1;
        }
        completion.outcome.confirm(*version);
        completion
    }

    /// Store an explicit recovery plan, retaining the operation's pending root.
    pub fn repair(&mut self, repair: Repair<O>, resources: &Resources, version: i64) -> Value {
        let response = repair.response(resources, version);
        self.repairs.insert(repair.ticket.id, repair);
        response
    }

    /// Hand out a batch once, or finish an effect-free operation.
    pub fn next(&mut self, ticket: Ticket) -> Dispatch<C, O> {
        if let Some((batch, work)) = self.operations.next(ticket) {
            Dispatch::Response(
                serde_json::to_value(Prepared {
                    member: work.member,
                    ticket,
                    batch,
                    server: work.server,
                    steps: work.steps,
                })
                .expect("prepared steps are serializable"),
            )
        } else if let Some(finished) = self.operations.finish_ready(ticket) {
            Dispatch::Finish(finished)
        } else {
            Dispatch::Response(json!({"accepted":false}))
        }
    }

    /// Execute common protocol rules without knowing any application's command
    /// vocabulary, document fields, server allocator or renderer.
    pub fn control(
        &mut self,
        resources: &mut Resources,
        control: Control,
        version: i64,
    ) -> Dispatch<C, O> {
        match control {
            Control::Complete {
                ticket,
                batch,
                result,
            } => {
                let result = match result {
                    Reply::Success => IoResult::Success,
                    Reply::Failed(reason) => IoResult::Failed(reason),
                    Reply::Cancelled => IoResult::Cancelled,
                };
                match self.operations.complete(resources, ticket, batch, result) {
                    Some(finished) => Dispatch::Finish(finished),
                    None => self.next(ticket),
                }
            }
            Control::Cancel { ticket } => {
                if !self.operations.cancel(ticket) {
                    return Dispatch::Response(json!({"accepted":false}));
                }
                match self.operations.finish_ready(ticket) {
                    Some(finished) => Dispatch::Finish(finished),
                    None => Dispatch::Response(json!({"accepted":true,"draining":true})),
                }
            }
            Control::Repaired { ticket } => {
                if !self
                    .repairs
                    .get(&ticket.id)
                    .is_some_and(|repair| repair.ticket == ticket)
                    || !self.operations.finish_abort(resources, ticket)
                {
                    return Dispatch::Response(json!({"accepted":false}));
                }
                Dispatch::Repaired(self.repairs.remove(&ticket.id).unwrap())
            }
            Control::RetryRepair { ticket } => Dispatch::Response(
                self.repairs
                    .get(&ticket.id)
                    .filter(|repair| repair.ticket == ticket)
                    .map(|repair| repair.response(resources, version))
                    .unwrap_or_else(|| json!({"accepted":false})),
            ),
            Control::RepairFailed { ticket, reason } => {
                let Some(repair) = self
                    .repairs
                    .get_mut(&ticket.id)
                    .filter(|repair| repair.ticket == ticket)
                else {
                    return Dispatch::Response(json!({"accepted":false}));
                };
                repair.reason = format!("{}; recovery is pending: {}", repair.reason, reason);
                let echo = repair.receipt.map(|(member, seq)| json!({"member":member,
                    "answer":clausters_editing::conversation::answer(seq, version, Some(repair.reason.clone()), Vec::new())}));
                Dispatch::Response(
                    json!({"complete":true,"committed":false,"reason":repair.reason,"echo":echo,"version":version}),
                )
            }
        }
    }

    /// Lost connections retire operations, repairs and locks together.
    pub fn invalidate(&mut self, resources: &mut Resources, server: ServerInstance) -> Vec<u64> {
        let retired = self.operations.invalidate(resources, server);
        for id in &retired {
            self.repairs.remove(id);
            self.walks.remove(id);
        }
        retired
    }
}

/// History coordination metadata independent of a domain's commit type.
#[derive(Debug)]
pub struct PendingWalk<C> {
    /// Explicit undo context captured at preparation.
    pub scope: super::histories::ScopeId,
    /// Original direction.
    pub direction: clausters_document::history::Direction,
    /// Expected entry; confirmation refuses if the order changed meanwhile.
    pub expected: clausters_document::history::Walked,
    /// Deltas for additional participants.
    pub others: Vec<(u32, C)>,
    /// Host gesture answered only after confirmation.
    pub receipt: Option<(u32, i64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{
        operations::{Access, Operation, Target},
        resources::{ResourceId, Source},
    };

    struct App<C> {
        accept: bool,
        committed: Option<C>,
    }
    impl<C> Application for App<C> {
        type Command = Operation<C>;
        type Commit = C;
        type Outcome = &'static str;
        fn prepare(&self, command: Operation<C>, _: &Services) -> Result<Operation<C>, String> {
            Ok(command)
        }
        fn commit(&mut self, delta: C, _: &mut Services) -> Completion<Self::Outcome> {
            if self.accept {
                self.committed = Some(delta);
            }
            Completion {
                committed: self.accept,
                outcome: "answer",
                recovery: Vec::new(),
                returned: Vec::new(),
            }
        }
        fn abort(
            &mut self,
            _: C,
            _: IoResult,
            _: usize,
            _: &mut Services,
        ) -> Completion<Self::Outcome> {
            Completion {
                committed: false,
                outcome: "aborted",
                recovery: Vec::new(),
                returned: Vec::new(),
            }
        }
    }
    fn operation<C>(object: i64, delta: C) -> Operation<C> {
        let server = ServerInstance {
            connection: 1,
            generation: 1,
        };
        Operation {
            target: Target {
                member: object as u32,
                object,
                server,
            },
            additional_targets: Vec::new(),
            access: Access::Write,
            batches: vec![Batch {
                member: object as u32,
                server,
                steps: Vec::new(),
            }],
            reads: Vec::new(),
            allocations: vec![Source {
                id: ResourceId {
                    server,
                    buffer: object,
                },
                member: object as u32,
                bytes: 400,
                owned: true,
                stored: None,
            }],
            commit: delta,
        }
    }
    fn settle<C, O>(
        runtime: &mut Runtime<C, O>,
        resources: &mut Resources,
        ticket: Ticket,
    ) -> Finished<C> {
        let Dispatch::Response(_) = runtime.next(ticket) else {
            panic!("expected a batch");
        };
        let Dispatch::Finish(finished) = runtime.control(
            resources,
            Control::Complete {
                ticket,
                batch: 0,
                result: Reply::Success,
            },
            1,
        ) else {
            panic!("expected completion");
        };
        finished
    }

    #[test]
    fn distinct_application_types_share_services_and_rejected_commit_repairs_once() {
        let mut services = Services::default();
        let mut first = App {
            accept: false,
            committed: None::<u32>,
        };
        let mut second = App {
            accept: true,
            committed: None::<String>,
        };
        let mut first_runtime = Runtime::default();
        let mut second_runtime = Runtime::default();
        let plan = first.prepare(operation(10, 7), &services).unwrap();
        let ticket = first_runtime
            .operations
            .prepare(&mut services.resources, plan)
            .unwrap();
        let other = second
            .prepare(operation(20, "notes".to_string()), &services)
            .unwrap();
        let other_ticket = second_runtime
            .operations
            .prepare(&mut services.resources, other)
            .unwrap();
        let finished = settle(&mut second_runtime, &mut services.resources, other_ticket);
        assert!(
            second_runtime
                .finish(&mut second, &mut services, finished)
                .committed
        );
        assert_eq!(second.committed.as_deref(), Some("notes"));
        let finished = settle(&mut first_runtime, &mut services.resources, ticket);
        let completion = first_runtime.finish(&mut first, &mut services, finished);
        assert!(!completion.committed);
        assert!(first_runtime.operations.busy(ticket.target));
        // Only the successful app's unreferenced scratch is collectible.
        assert_eq!(
            services
                .resources
                .collect()
                .iter()
                .map(|s| s.id.buffer)
                .collect::<Vec<_>>(),
            [20]
        );
        first_runtime.repair(
            Repair {
                ticket,
                batches: Vec::new(),
                reason: "revision changed".into(),
                outcome: completion.outcome,
                receipt: None,
                returned: Vec::new(),
            },
            &services.resources,
            1,
        );
        assert!(matches!(
            first_runtime.control(&mut services.resources, Control::Repaired { ticket }, 1),
            Dispatch::Repaired(_)
        ));
        assert!(!first_runtime.operations.busy(ticket.target));
        assert_eq!(services.resources.collect()[0].id.buffer, 10);
        assert!(
            matches!(first_runtime.control(&mut services.resources, Control::Repaired { ticket }, 1), Dispatch::Response(v) if v["accepted"] == false)
        );
        assert!(services.resources.collect().is_empty());
    }

    #[test]
    fn invalidation_retires_failed_repairs_and_late_replies_without_freeing_on_replacement() {
        let mut services = Services::default();
        let mut runtime = Runtime::default();
        let mut app = App {
            accept: false,
            committed: None::<u32>,
        };
        let ticket = runtime
            .operations
            .prepare(&mut services.resources, operation(10, 1))
            .unwrap();
        let finished = settle(&mut runtime, &mut services.resources, ticket);
        let completion = runtime.finish(&mut app, &mut services, finished);
        runtime.repair(
            Repair {
                ticket,
                batches: Vec::new(),
                reason: "refused".into(),
                outcome: completion.outcome,
                receipt: None,
                returned: Vec::new(),
            },
            &services.resources,
            1,
        );
        runtime.control(
            &mut services.resources,
            Control::RepairFailed {
                ticket,
                reason: "offline".into(),
            },
            1,
        );
        assert!(runtime.operations.busy(ticket.target));
        assert_eq!(
            runtime.invalidate(&mut services.resources, ticket.target.server),
            [ticket.id]
        );
        services.resources.invalidate(ticket.target.server);
        assert!(!runtime.operations.busy(ticket.target));
        assert!(services.resources.collect().is_empty());
        assert!(
            matches!(runtime.control(&mut services.resources, Control::RetryRepair { ticket }, 1), Dispatch::Response(v) if v["accepted"] == false)
        );
        assert!(
            matches!(runtime.control(&mut services.resources, Control::Complete { ticket, batch:0, result:Reply::Success }, 1), Dispatch::Response(v) if v["accepted"] == false)
        );
        let mut next = operation(10, 2);
        next.target.server.generation += 1;
        next.batches[0].server = next.target.server;
        next.allocations[0].id.server = next.target.server;
        assert!(
            runtime
                .operations
                .prepare(&mut services.resources, next)
                .is_ok()
        );
    }
}
