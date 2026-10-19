//! Confirmation and recovery of a multi-object history operation.
use super::{
    operations::{Batch, Finished, Result as IoResult, Target, Ticket},
    resources::ServerInstance,
    runtime::PendingWalk,
    services::Completion,
};
use crate::turn::Turned;
use clausters_editing::conversation::Answer;

/// The composition supplies history application and typed participant routing.
/// The transaction algorithm has no application-class or renderer dependency.
pub trait WalkHost<C> {
    /// The composition's committed step response.
    type Stepped;
    /// A participant's domain outcome.
    type Outcome: Turned;
    /// Validate the captured order and apply the entry atomically.
    fn confirm_walk(&mut self, walk: &PendingWalk<C>) -> Result<Self::Stepped, String>;
    /// Abort only the prepared participant's delta.
    fn abort_walk(
        &mut self,
        member: u32,
        delta: C,
        result: IoResult,
        completed: usize,
    ) -> Completion<Self::Outcome>;
    /// The endpoint belonging to that participant.
    fn walk_server(&self, member: u32) -> ServerInstance;
}

/// A history refusal and all the participants' recovery work.
pub struct Refused {
    /// Reason shared by the aborted participants.
    pub reason: String,
    /// Explicitly addressed effects, including endpoints that may be invalidated.
    pub recoveries: Vec<Batch>,
    /// Answers to publish only after recovery.
    pub corrections: Vec<(u32, Answer)>,
}

/// One decision over the complete history entry, independent of scheduling.
pub struct Decision<S> {
    /// Operation whose pending ownership is still held by the runtime.
    pub ticket: Ticket,
    /// Other locked objects retained on refusal.
    pub additional_targets: Vec<Target>,
    /// Original gesture, if the history walk came from a host event.
    pub receipt: Option<(u32, i64)>,
    /// Successful step, or the recovery required before releasing ownership.
    pub result: Result<S, Refused>,
}

/// Confirm once, or abort every prepared participant. No snapshot of the
/// composition, history or spilled payloads is made on either path.
pub fn finish<C, H: WalkHost<C>>(
    host: &mut H,
    finished: Finished<C>,
    walk: PendingWalk<C>,
) -> Decision<H::Stepped> {
    let result = match &finished.result {
        IoResult::Success => host.confirm_walk(&walk),
        IoResult::Failed(reason) => Err(reason.clone()),
        IoResult::Cancelled => Err("the history operation was cancelled".into()),
    };
    let result = result.map_err(|reason| {
        let deltas = std::iter::once((finished.ticket.target.member, finished.operation.commit))
            .chain(walk.others);
        let mut recoveries = Vec::new();
        let mut corrections = Vec::new();
        for (member, delta) in deltas {
            let mut completion = host.abort_walk(
                member,
                delta,
                IoResult::Failed(reason.clone()),
                finished.completed_batches,
            );
            if let Some(answer) = completion.outcome.take_answer() {
                corrections.push((member, answer));
            }
            recoveries.push(Batch {
                member,
                server: host.walk_server(member),
                steps: completion.recovery,
            });
        }
        Refused {
            reason,
            recoveries,
            corrections,
        }
    });
    Decision {
        ticket: finished.ticket,
        additional_targets: finished.operation.additional_targets,
        receipt: walk.receipt,
        result,
    }
}
