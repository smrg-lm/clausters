//! Typed, two-phase application work. No snapshots, renderer or I/O executor.
//!
//! Preparation produces a delta and explicitly declares effects and ownership.
//! Contents and history change only when the application commits that delta.

use super::resources::{ResourceId, Resources, Root, RootKind, ServerInstance, Source};
use clausters_editing::apply::Step;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Stable object and endpoint, independent of the current view binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Editing member.
    pub member: u32,
    /// Application object identity.
    pub object: i64,
    /// Connection incarnation.
    pub server: ServerInstance,
}

/// A batch's result; cancellation is acknowledged after outstanding I/O settles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Result {
    /// All waits in the batch completed successfully.
    Success,
    /// I/O refused or timed out. No document delta is committed.
    Failed(String),
    /// The executor has stopped issuing work and drained outstanding replies.
    Cancelled,
}

/// One ordered group of existing server steps, with no inferred allocations.
#[derive(Clone, Debug)]
pub struct Batch {
    /// Member whose executor owns this endpoint.
    pub member: u32,
    /// The endpoint on which these steps run.
    pub server: ServerInstance,
    /// The existing send/wait vocabulary, interpreted by an adapter.
    pub steps: Vec<Step>,
}

/// How long an operation needs exclusive access to its object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Content or storage changes remain exclusive until confirmation.
    Write,
    /// Once this many batches have landed, an immutable snapshot allows
    /// later content edits: `Snapshot(0)` reads only what never changes, so
    /// an edit may start at once.
    Snapshot(usize),
}

/// A prepared operation: its unpublished delta and explicitly owned resources.
#[derive(Debug)]
pub struct Operation<C> {
    /// Original document and endpoint.
    pub target: Target,
    /// Other objects participating in the same atomic confirmation.
    pub additional_targets: Vec<Target>,
    /// Access policy stated by the application, independent of scheduling.
    pub access: Access,
    /// Ordered dependencies: a batch starts only after its predecessor succeeds.
    pub batches: Vec<Batch>,
    /// New resources reserved by preparation, never discovered from OSC strings.
    pub allocations: Vec<Source>,
    /// Existing resources that must survive this operation, including spilled ones.
    pub reads: Vec<ResourceId>,
    /// The application-specific change, applied only after all batches succeed.
    pub commit: C,
}

/// An operation's identity and original target. Executors must echo both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// Composition-scoped monotonic resource owner id.
    pub id: u64,
    /// Original object and server incarnation.
    pub target: Target,
}

#[derive(Debug)]
struct Pending<C> {
    operation: Operation<C>,
    next: usize,
    waiting: bool,
    cancelled: bool,
}

/// Operation identity, dependencies and pending ownership over injected resources.
#[derive(Debug)]
pub struct Operations<C> {
    pending: BTreeMap<u64, Pending<C>>,
    recovering: BTreeMap<u64, (Target, Vec<Target>)>,
}

impl<C> Default for Operations<C> {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            recovering: BTreeMap::new(),
        }
    }
}

/// The next batch handed to an executor at the binding boundary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Prepared {
    /// Member whose executor owns this batch.
    pub member: u32,
    /// Stable operation identity and original target.
    pub ticket: Ticket,
    /// Ordered batch number.
    pub batch: usize,
    /// Endpoint receiving this batch.
    pub server: ServerInstance,
    /// Existing Step encoding, produced only for the boundary.
    #[serde(serialize_with = "serialize_steps")]
    pub steps: Vec<Step>,
}
fn serialize_steps<S: serde::Serializer>(
    steps: &[Step],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    clausters_editing::apply::steps_json(steps).serialize(serializer)
}

/// The owner consumes a finished plan, commits or aborts, then releases its lease.
#[derive(Debug)]
pub struct Finished<C> {
    /// The consumed identity.
    pub ticket: Ticket,
    /// The prepared delta and effects; never a copy of the composition.
    pub operation: Operation<C>,
    /// Whether to commit, abort or discard cancelled work.
    pub result: Result,
    /// Batches whose waits completed before this terminal result.
    pub completed_batches: usize,
}

impl<C> Operations<C> {
    /// Finalize through the application's policy. Successful I/O alone does
    /// not release ownership: a refused delta enters the same repair lifecycle
    /// as an I/O failure, until the executor acknowledges recovery.
    pub fn finish<A: super::services::Application<Commit = C>>(
        &mut self,
        app: &mut A,
        services: &mut super::services::Services,
        finished: Finished<C>,
    ) -> super::services::Completion<A::Outcome> {
        let ticket = finished.ticket;
        let others = finished.operation.additional_targets;
        let completion = if finished.result == Result::Success {
            app.commit(finished.operation.commit, services)
        } else {
            app.abort(
                finished.operation.commit,
                finished.result,
                finished.completed_batches,
                services,
            )
        };
        if completion.committed {
            Self::release(&mut services.resources, ticket);
        } else {
            self.begin_abort(ticket, others);
        }
        completion
    }

    /// Retire work belonging to a lost server, including failed repairs.
    /// No effects may be sent to its replacement incarnation.
    pub fn invalidate(&mut self, resources: &mut Resources, server: ServerInstance) -> Vec<u64> {
        let mut retired = Vec::new();
        self.pending.retain(|id, pending| {
            let affected = pending.operation.target.server == server
                || pending
                    .operation
                    .additional_targets
                    .iter()
                    .any(|target| target.server == server)
                || pending
                    .operation
                    .batches
                    .iter()
                    .any(|batch| batch.server == server);
            if affected {
                Self::release(
                    resources,
                    Ticket {
                        id: *id,
                        target: pending.operation.target,
                    },
                );
                retired.push(*id);
            }
            !affected
        });
        self.recovering.retain(|id, (target, others)| {
            let affected =
                target.server == server || others.iter().any(|target| target.server == server);
            if affected {
                Self::release(
                    resources,
                    Ticket {
                        id: *id,
                        target: *target,
                    },
                );
                retired.push(*id);
            }
            !affected
        });
        retired
    }

    /// Whether this exact object has pending work. Other objects remain usable.
    pub fn busy(&self, target: Target) -> bool {
        self.pending.values().any(|p| {
            p.operation.target == target || p.operation.additional_targets.contains(&target)
        }) || self
            .recovering
            .values()
            .any(|(primary, others)| *primary == target || others.contains(&target))
    }

    /// Validate declarations and retain reads and allocations atomically.
    pub fn prepare(
        &mut self,
        resources: &mut Resources,
        operation: Operation<C>,
    ) -> std::result::Result<Ticket, &'static str> {
        if std::iter::once(&operation.target)
            .chain(&operation.additional_targets)
            .any(|target| {
                self.recovering
                    .values()
                    .any(|(primary, others)| primary == target || others.contains(target))
            })
        {
            return Err("the object is awaiting recovery");
        }
        if self.pending.values().any(|pending| {
            std::iter::once(&operation.target)
                .chain(&operation.additional_targets)
                .any(|target| {
                    pending.operation.target == *target
                        || pending.operation.additional_targets.contains(target)
                })
                && match (operation.access, pending.operation.access) {
                    (Access::Snapshot(_), _) | (_, Access::Write) => true,
                    (Access::Write, Access::Snapshot(batches)) => pending.next < batches,
                }
        }) {
            return Err("the object has pending work");
        }
        if !resources.valid(operation.target.server)
            || operation
                .batches
                .iter()
                .any(|batch| !resources.valid(batch.server))
        {
            return Err("the operation references a previous server instance");
        }
        let allocated: BTreeSet<_> = operation
            .allocations
            .iter()
            .map(|source| source.id)
            .collect();
        if allocated.len() != operation.allocations.len()
            || operation.allocations.iter().any(|source| {
                !source.owned
                    || !resources.valid(source.id.server)
                    || !resources.available(source.id)
            })
            || operation.reads.iter().any(|id| {
                !resources.valid(id.server)
                    || (resources.source(*id).is_none() && !allocated.contains(id))
            })
        {
            return Err("invalid operation resource declarations");
        }
        let ticket = Ticket {
            id: resources.new_owner(),
            target: operation.target,
        };
        for source in &operation.allocations {
            resources.register(source.clone());
        }
        let retained = resources.retain(
            Self::root(ticket),
            operation.reads.iter().copied().chain(allocated),
        );
        debug_assert!(retained);
        self.pending.insert(
            ticket.id,
            Pending {
                operation,
                next: 0,
                waiting: false,
                cancelled: false,
            },
        );
        Ok(ticket)
    }

    /// Read the unpublished plan without cloning or consuming its delta.
    pub fn operation(&self, ticket: Ticket) -> Option<&Operation<C>> {
        let pending = self.pending.get(&ticket.id)?;
        (pending.operation.target == ticket.target).then_some(&pending.operation)
    }

    /// Take the next batch once; a duplicate dispatch cannot issue its effects.
    pub fn next(&mut self, ticket: Ticket) -> Option<(usize, Batch)> {
        let pending = self.pending.get_mut(&ticket.id)?;
        if pending.operation.target != ticket.target || pending.waiting || pending.cancelled {
            return None;
        }
        let batch = pending.operation.batches.get(pending.next)?.clone();
        pending.waiting = true;
        Some((pending.next, batch))
    }

    /// Request cancellation. The in-flight batch must still settle before cleanup.
    pub fn cancel(&mut self, ticket: Ticket) -> bool {
        let Some(pending) = self.pending.get_mut(&ticket.id) else {
            return false;
        };
        if pending.operation.target != ticket.target {
            return false;
        }
        pending.cancelled = true;
        true
    }

    /// Consume only the matching batch result. Late, reordered and duplicate
    /// replies cannot commit or abort another operation.
    pub fn complete(
        &mut self,
        resources: &Resources,
        ticket: Ticket,
        batch: usize,
        mut result: Result,
    ) -> Option<Finished<C>> {
        let pending = self.pending.get_mut(&ticket.id)?;
        if pending.operation.target != ticket.target || !pending.waiting || pending.next != batch {
            return None;
        }
        if !resources.valid(ticket.target.server)
            || !resources.valid(pending.operation.batches[batch].server)
        {
            result = Result::Failed("the operation's server instance was lost".into());
        }
        if pending.cancelled {
            result = Result::Cancelled;
        }
        if result == Result::Success && pending.next + 1 < pending.operation.batches.len() {
            pending.next += 1;
            pending.waiting = false;
            return None;
        }
        let pending = self.pending.remove(&ticket.id)?;
        if result != Result::Success {
            self.recovering.insert(
                ticket.id,
                (ticket.target, pending.operation.additional_targets.clone()),
            );
        }
        Some(Finished {
            ticket,
            operation: pending.operation,
            completed_batches: if result == Result::Success {
                pending.next + 1
            } else {
                pending.next
            },
            result,
        })
    }

    /// Finish an operation with no effects, or a cancellation with no in-flight
    /// work. This does not release ownership before the application handles it.
    pub fn finish_ready(&mut self, ticket: Ticket) -> Option<Finished<C>> {
        let pending = self.pending.get(&ticket.id)?;
        if pending.operation.target != ticket.target
            || pending.waiting
            || (!pending.cancelled && pending.next < pending.operation.batches.len())
        {
            return None;
        }
        let pending = self.pending.remove(&ticket.id)?;
        let result = if pending.cancelled {
            Result::Cancelled
        } else {
            Result::Success
        };
        if result != Result::Success {
            self.recovering.insert(
                ticket.id,
                (ticket.target, pending.operation.additional_targets.clone()),
            );
        }
        Some(Finished {
            ticket,
            operation: pending.operation,
            result,
            completed_batches: pending.next,
        })
    }

    /// Reserve recovery after an application-level commit refusal.
    pub fn begin_abort(&mut self, ticket: Ticket, others: Vec<Target>) {
        self.recovering.insert(ticket.id, (ticket.target, others));
    }

    /// End recovery once its effects have settled. Failed cleanup keeps both
    /// ownership and the object lock, so a retry cannot race another edit.
    pub fn finish_abort(&mut self, resources: &mut Resources, ticket: Ticket) -> bool {
        if !self
            .recovering
            .get(&ticket.id)
            .is_some_and(|(target, _)| *target == ticket.target)
        {
            return false;
        }
        self.recovering.remove(&ticket.id);
        Self::release(resources, ticket);
        true
    }

    /// Drop the pending reason after commit has retained contents/history, or
    /// abort has decided its cleanup. Collection is still the composition's.
    pub fn release(resources: &mut Resources, ticket: Ticket) {
        resources.drop_root(Self::root(ticket));
    }

    fn root(ticket: Ticket) -> Root {
        Root {
            kind: RootKind::Pending,
            owner: ticket.id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(object: i64) -> Target {
        Target {
            member: 2,
            object,
            server: ServerInstance {
                connection: 3,
                generation: 1,
            },
        }
    }
    fn operation(object: i64, buffer: i64) -> Operation<&'static str> {
        Operation {
            target: target(object),
            additional_targets: Vec::new(),
            access: Access::Write,
            batches: vec![
                Batch {
                    member: target(object).member,
                    server: target(object).server,
                    steps: Vec::new()
                };
                2
            ],
            allocations: vec![Source {
                id: ResourceId {
                    server: target(object).server,
                    buffer,
                },
                member: 2,
                bytes: 400,
                owned: true,
                stored: None,
            }],
            reads: Vec::new(),
            commit: "delta",
        }
    }

    #[test]
    fn batches_are_ordered_and_results_address_one_original_object() {
        let mut resources = Resources::default();
        let mut operations = Operations::default();
        let ticket = operations
            .prepare(&mut resources, operation(7, 20))
            .unwrap();
        assert!(
            operations.prepare(&mut resources, operation(8, 21)).is_ok(),
            "another object is not blocked"
        );
        assert!(
            operations
                .prepare(&mut resources, operation(7, 22))
                .is_err()
        );
        assert!(
            resources.collect().is_empty(),
            "pending allocations are rooted before I/O"
        );
        assert_eq!(operations.next(ticket).unwrap().0, 0);
        assert!(operations.next(ticket).is_none());
        assert!(
            operations
                .complete(
                    &resources,
                    Ticket {
                        target: target(8),
                        ..ticket
                    },
                    0,
                    Result::Success
                )
                .is_none()
        );
        assert!(
            operations
                .complete(&resources, ticket, 1, Result::Success)
                .is_none()
        );
        assert!(
            operations
                .complete(&resources, ticket, 0, Result::Success)
                .is_none()
        );
        assert_eq!(operations.next(ticket).unwrap().0, 1);
        assert!(
            operations
                .complete(&resources, ticket, 0, Result::Success)
                .is_none()
        );
        let finished = operations
            .complete(&resources, ticket, 1, Result::Success)
            .unwrap();
        assert_eq!(finished.operation.commit, "delta");
        assert!(
            operations
                .complete(&resources, ticket, 1, Result::Success)
                .is_none()
        );
        assert!(
            resources.collect().is_empty(),
            "commit retains its own roots before pending ends"
        );
        Operations::<&str>::release(&mut resources, ticket);
        assert_eq!(resources.collect()[0].id.buffer, 20);
    }

    #[test]
    fn cancellation_drains_inflight_work_and_never_commits_on_a_replacement() {
        let mut resources = Resources::default();
        let mut operations = Operations::default();
        let ticket = operations
            .prepare(&mut resources, operation(7, 20))
            .unwrap();
        operations.next(ticket).unwrap();
        assert!(operations.cancel(ticket));
        assert!(operations.finish_ready(ticket).is_none());
        assert!(resources.collect().is_empty());
        let cancelled = operations
            .complete(&resources, ticket, 0, Result::Success)
            .unwrap();
        assert_eq!(cancelled.result, Result::Cancelled);
        assert!(operations.busy(ticket.target));
        assert!(operations.finish_abort(&mut resources, ticket));
        assert_eq!(resources.collect().len(), 1);
        let lost = operations
            .prepare(&mut resources, operation(7, 21))
            .unwrap();
        operations.next(lost).unwrap();
        resources.invalidate(lost.target.server);
        let finished = operations
            .complete(&resources, lost, 0, Result::Success)
            .unwrap();
        assert!(matches!(finished.result, Result::Failed(_)));
        assert!(operations.finish_abort(&mut resources, lost));
        assert!(
            resources.collect().is_empty(),
            "never free a lost id on a new server"
        );
    }
}
