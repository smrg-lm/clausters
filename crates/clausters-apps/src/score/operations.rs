//! **A symbolic score's file**, over the shared application runtime: what a
//! window writes, and the window letting its document go.
//!
//! A save is prepared from one revision of the score, stored, and confirmed
//! only then: the file it names and what it holds become the score's when the
//! store succeeded and the document is still the one it was prepared from. What
//! waits for it (a close, Close file) is done only over what it wrote.
use clausters_editing::apply::Step;

use super::editor::{Outcome, ScoreEditor};
use crate::application::{
    histories::Object,
    operations::{Access, Batch, Operation, Result as IoResult, Target},
    services::{Application, Completion, Services},
};

/// **What a save is asked before**, done once it is confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// The window closes.
    Close,
    /// The window lets its document go and stays.
    Unbind,
}

/// A prepared revision of the score, retained through storage confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The document written.
    pub mei: String,
    /// Where: the score's file once the store is confirmed.
    pub path: String,
    /// What waits for the confirmation.
    pub then: Option<After>,
}

/// Typed work on a window's document.
#[derive(Debug)]
pub enum Command {
    /// Store a prepared revision and confirm it.
    Save(Box<Saved>),
    /// Let the document go, the window staying with none.
    Unbind,
}

/// Work on a document and the window it is asked of.
#[derive(Debug)]
pub struct Request {
    /// The window's document.
    pub target: Target,
    /// What to do with it.
    pub command: Command,
}

/// A delta on a document, unpublished until its store settles.
#[derive(Debug)]
pub struct Commit {
    target: Target,
    generation: u64,
    change: Command,
}

impl Application for ScoreEditor {
    type Command = Request;
    type Commit = Commit;
    type Outcome = Outcome;

    fn prepare(&self, request: Request, _services: &Services) -> Result<Operation<Commit>, String> {
        if request.target.object != self.binding() {
            return Err("the score changed before preparation".into());
        }
        let mut batches = Vec::new();
        let access = match &request.command {
            Command::Unbind => Access::Write,
            // what a store carries is a copy of one revision: the hand may
            // go on editing while it is written
            Command::Save(saved) => {
                batches.push(Batch {
                    member: request.target.member,
                    server: request.target.server,
                    steps: vec![Step::Store {
                        path: saved.path.clone(),
                        contents: saved.mei.clone(),
                    }],
                });
                Access::Snapshot(0)
            }
        };
        Ok(Operation {
            target: request.target,
            additional_targets: Vec::new(),
            access,
            batches,
            allocations: Vec::new(),
            reads: Vec::new(),
            commit: Commit {
                target: request.target,
                generation: self.generation(),
                change: request.command,
            },
        })
    }

    fn commit(&mut self, delta: Commit, services: &mut Services) -> Completion<Outcome> {
        if self.binding() != delta.target.object || self.generation() != delta.generation {
            return self.abort(
                delta,
                IoResult::Failed("another document is in the window".into()),
                0,
                services,
            );
        }
        let mut outcome = Outcome::default();
        match delta.change {
            Command::Save(saved) => {
                self.confirm_saved(&saved);
                outcome.saved = Some(saved);
            }
            Command::Unbind => {
                outcome.left = Some(self.let_go());
                // a document let go of takes its order with it: nothing walks
                // what nobody holds
                services.histories.close_order(Object {
                    member: delta.target.member,
                    id: delta.target.object,
                });
            }
        }
        Completion {
            committed: true,
            outcome,
            recovery: Vec::new(),
            returned: Vec::new(),
        }
    }

    fn abort(
        &mut self,
        _delta: Commit,
        _result: IoResult,
        _completed_batches: usize,
        _services: &mut Services,
    ) -> Completion<Outcome> {
        // the document was never touched: a store that failed leaves the
        // score and its file as they were
        Completion {
            committed: false,
            outcome: Outcome::default(),
            recovery: Vec::new(),
            returned: Vec::new(),
        }
    }
}
