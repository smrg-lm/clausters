//! The services one application composition supplies explicitly.
use super::{
    clipboard::Exchange,
    histories::Histories,
    operations::{Operation, Result},
    resources::Resources,
};

/// One ownership graph shared by editing, clipboard and pending operations.
#[derive(Debug)]
pub struct Services {
    /// Explicit shared and independent editing orders.
    pub histories: Histories,
    /// The only resource ownership graph in this composition.
    pub resources: Resources,
    /// Clipboard policy; borrows the graph for every operation.
    pub clipboard: Exchange,
}

impl Default for Services {
    fn default() -> Self {
        Self::new(Histories::new(), Resources::default())
    }
}

impl Services {
    /// Compose injected services without discovering them through global state.
    pub fn new(histories: Histories, mut resources: Resources) -> Self {
        let clipboard = Exchange::new(resources.new_owner());
        Self {
            histories,
            resources,
            clipboard,
        }
    }
}

/// The common lifecycle implemented by domain-specific applications.
pub trait Application: Sized {
    /// Typed application command. JSON is decoded before this method is called.
    type Command;
    /// The unpublished state delta carried by an operation.
    type Commit;
    /// Corrections and follow-up effects returned after commit or abort.
    type Outcome;
    /// Prepare effects and a delta without changing document contents/history.
    fn prepare(
        &self,
        command: Self::Command,
        services: &Services,
    ) -> std::result::Result<Operation<Self::Commit>, String>;
    /// Apply only the prepared object's delta; preserve focus and other objects.
    fn commit(&mut self, delta: Self::Commit, services: &mut Services)
    -> Completion<Self::Outcome>;
    /// Report a refusal/cancellation; no previous document state needs restoring.
    fn abort(
        &mut self,
        delta: Self::Commit,
        result: Result,
        completed_batches: usize,
        services: &mut Services,
    ) -> Completion<Self::Outcome>;
}

/// Application result consumed by the common operation coordinator.
#[derive(Debug)]
pub struct Completion<O> {
    /// Whether the application accepted its prepared delta.
    pub committed: bool,
    /// Domain corrections and follow-up effects.
    pub outcome: O,
    /// Effects required before pending resources can be released on refusal.
    pub recovery: Vec<clausters_editing::apply::Step>,
    /// Reserved buffer ids whose server effects have already been settled.
    pub returned: Vec<i64>,
}
