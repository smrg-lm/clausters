//! **A window whose document is a file**, over the shared application
//! runtime: what it writes, which document it holds, and the forms that ask
//! where -- one rule for every editor whose document a file holds whole, the
//! score editor's MEI and the roll's MIDI or notation file alike.
//!
//! Which document the window holds is no edit ([`crate::closing`]): a new one,
//! one read from a file or none each takes the place of the one there, whose
//! order goes with it.
//!
//! A save is prepared from one revision of the document, stored, and
//! confirmed only then: the file it names and what it holds become the
//! window's when the store succeeded and the document is still the one it was
//! prepared from. What waits for it (a close, Close file) is done only over
//! what it wrote.
//!
//! What is each editor's is said through [`FileWindow`]: how its document is
//! written to a file and read from one, and which of its forms asks where.
//! `application!` gives such an editor the runtime's [`Application`], and
//! the editing context carries the store out the same way for each
//! (`crate::editing`'s file runs).
//!
//! [`Application`]: crate::application::services::Application

use clausters_editing::apply::Step;
use clausters_editing::conversation::Correction;
use serde_json::Value;

use crate::application::{
    histories::Object,
    operations::{Access, Batch, Operation, Target},
    services::{Completion, Services},
};
use crate::closing::{FileChange, Leaving};
use crate::turn::{Converse, Turned};

/// A prepared revision of a document, retained through storage confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The file's bytes, written.
    pub contents: Vec<u8>,
    /// **The document as the window holds it**, at that revision: what it
    /// compares itself with to know it has changes the file does not hold.
    pub revision: Vec<u8>,
    /// Where: the window's file once the store is confirmed.
    pub path: String,
    /// What waits for the confirmation.
    pub then: Option<Leaving>,
}

/// Typed work on a window's document.
#[derive(Debug)]
pub enum Command {
    /// Store a prepared revision and confirm it.
    Save(Box<Saved>),
    /// Put this document in the window, as the window holds one, read and
    /// written by the caller.
    Replace {
        /// Its contents, as [`FileWindow::read_file`] answered them.
        contents: Vec<u8>,
        /// The file that holds them, or none for a new one.
        path: Option<String>,
    },
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
    change: Command,
}

/// **What a turn says of the window's file**, beside the rest of an editor's
/// outcome: the file to save, the document to change, and what the runtime
/// settled. Implemented by `outcome!` over the usual field names.
pub trait FileOutcome: Turned {
    /// The file the window's document is to be saved to.
    fn set_save(&mut self, path: String);
    /// Whether the turn names a file to save.
    fn saves(&self) -> bool;
    /// The window's document to change once nothing is lost.
    fn set_file(&mut self, change: FileChange);
    /// The revision a store confirmed.
    fn set_saved(&mut self, saved: Saved);
    /// What the window left its document as, when it let it go.
    fn set_left(&mut self, left: Option<Value>);
    /// The revision a store confirmed, taken.
    fn take_saved(&mut self) -> Option<Saved>;
    /// What the window left its document as, taken.
    fn take_left(&mut self) -> Option<Value>;
}

/// Implements [`FileOutcome`] for an outcome with the fields `save`, `file`,
/// `saved` and `left`.
macro_rules! outcome {
    ($outcome:ty) => {
        impl crate::file::FileOutcome for $outcome {
            fn set_save(&mut self, path: String) {
                self.save = Some(path);
            }
            fn saves(&self) -> bool {
                self.save.is_some()
            }
            fn set_file(&mut self, change: crate::closing::FileChange) {
                self.file = Some(change);
            }
            fn set_saved(&mut self, saved: crate::file::Saved) {
                self.saved = Some(Box::new(saved));
            }
            fn set_left(&mut self, left: Option<serde_json::Value>) {
                self.left = left;
            }
            fn take_saved(&mut self) -> Option<crate::file::Saved> {
                self.saved.take().map(|saved| *saved)
            }
            fn take_left(&mut self) -> Option<serde_json::Value> {
                self.left.take()
            }
        }
    };
}
pub(crate) use outcome;

/// **An editor whose document is a file.**
pub trait FileWindow: Converse<Outcome: FileOutcome> {
    /// The identity of the document in the window: one let go of takes its
    /// order with it, and a store prepared from one is not confirmed over
    /// another.
    fn binding(&self) -> i64;
    /// The file the document was read from or last saved to, if any.
    fn file_path(&self) -> Option<String>;
    /// Whether the window holds a document.
    fn has_document(&self) -> bool;
    /// **Why this window has no file of its own**, when it has none: a view
    /// over a document somebody else holds.
    fn no_file(&self) -> Option<&'static str> {
        None
    }
    /// The revision a store wrote is what the file holds, and the file is the
    /// window's from now on.
    fn confirm_saved(&mut self, saved: &Saved);
    /// **Another document in the window**, as [`FileWindow::read_file`]
    /// answered it, read from `path` or new. Answers what it left the one
    /// before as.
    fn replace_document(&mut self, contents: &[u8], path: Option<String>) -> Option<Value>;
    /// **The document let go of**, the window staying with none. Answers what
    /// it left it as.
    fn let_go(&mut self) -> Option<Value>;
    /// **The document as its file holds it**, at this revision: the file's
    /// bytes, and the document as the window compares itself with.
    ///
    /// # Errors
    /// When the document cannot be written in its format.
    fn write_file(&self) -> Result<(Vec<u8>, Vec<u8>), String>;
    /// **A file's bytes as the window holds a document**, what
    /// [`Command::Replace`] carries; the window is left as it was.
    ///
    /// # Errors
    /// When the bytes are not a document the window reads.
    fn read_file(&self, bytes: &[u8]) -> Result<Vec<u8>, String>;
    /// The document File > New makes, as the window holds one.
    ///
    /// # Errors
    /// When it cannot be made.
    fn blank(&self) -> Result<Vec<u8>, String>;
    /// What the close form was asked about, when it is up for something.
    fn leaving(&mut self) -> &mut Option<Leaving>;
    /// **The form that asks where the document is saved**, opened holding the
    /// file it has: the reason it cannot be, or the corrections that show it.
    fn ask_where(&mut self) -> (Option<String>, Vec<Correction>);
}

/// **Save**: the document goes to its file, which the turn's outcome names
/// for whoever drives the editor to write; one with no file yet is asked for
/// one.
pub fn save<W: FileWindow>(
    window: &mut W,
    out: &mut W::Outcome,
) -> (Option<String>, Vec<Correction>) {
    if let Some(why) = window.no_file() {
        return (Some(why.into()), Vec::new());
    }
    match window.file_path() {
        Some(path) => {
            out.set_save(path);
            (None, Vec::new())
        }
        None => window.ask_where(),
    }
}

/// **What the close form was asked about goes**: the window or its document,
/// once whatever the turn saves is written.
pub fn let_what_waits_go<W: FileWindow>(window: &mut W, out: &mut W::Outcome) {
    match window.leaving().take() {
        Some(Leaving::Window) => out.set_close(true),
        Some(Leaving::File(change)) => out.set_file(change),
        None => {}
    }
}

/// **The close form's Save**: the document saved, and what waited goes once
/// it is written; a document with no file is asked for one first.
pub fn save_then_go<W: FileWindow>(window: &mut W, out: &mut W::Outcome) -> Option<String> {
    let (reason, _) = save(window, out);
    if out.saves() {
        let_what_waits_go(window, out);
    }
    reason
}

/// **A path a file form gave**: the file to open in the window, by the rule
/// a change of document follows, or the file the document is saved to from
/// now on, and what waited for it.
pub fn path_given<W: FileWindow>(
    window: &mut W,
    open: bool,
    path: &str,
    out: &mut W::Outcome,
) -> Option<String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Some("a file is named by its path".into());
    }
    if open {
        return change_file(window, FileChange::Open(path), out).0;
    }
    out.set_save(path);
    let_what_waits_go(window, out);
    None
}

/// **A change of the window's document** (File > New, Open, Close file), by
/// the rule every application changes one by ([`crate::closing::change_file`]);
/// a window with no file of its own has none to change.
pub fn change_file<W: FileWindow>(
    window: &mut W,
    change: FileChange,
    out: &mut W::Outcome,
) -> (Option<String>, Vec<Correction>) {
    if let Some(why) = window.no_file() {
        return (Some(why.into()), Vec::new());
    }
    crate::closing::change_file(window, change, out)
}

/// The operation a request is, prepared over `window`.
///
/// # Errors
/// When another document is in the window.
pub fn prepare<W: FileWindow>(window: &W, request: Request) -> Result<Operation<Commit>, String> {
    if request.target.object != window.binding() {
        return Err("the document changed before preparation".into());
    }
    let mut batches = Vec::new();
    let access = match &request.command {
        Command::Replace { .. } | Command::Unbind => Access::Write,
        // what a store carries is a copy of one revision: the hand may go on
        // editing while it is written
        Command::Save(saved) => {
            batches.push(Batch {
                member: request.target.member,
                server: request.target.server,
                steps: vec![Step::Store {
                    path: saved.path.clone(),
                    contents: saved.contents.clone(),
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
            change: request.command,
        },
    })
}

/// The commit of a settled operation over `window`.
pub fn commit<W: FileWindow>(
    window: &mut W,
    delta: Commit,
    services: &mut Services,
) -> Completion<W::Outcome> {
    if window.binding() != delta.target.object {
        return aborted();
    }
    let mut outcome = W::Outcome::default();
    match delta.change {
        Command::Save(saved) => {
            window.confirm_saved(&saved);
            outcome.set_saved(*saved);
        }
        Command::Replace { contents, path } => {
            outcome.set_left(window.replace_document(&contents, path));
            close_order(services, &delta.target);
        }
        Command::Unbind => {
            outcome.set_left(window.let_go());
            close_order(services, &delta.target);
        }
    }
    Completion {
        committed: true,
        outcome,
        recovery: Vec::new(),
        returned: Vec::new(),
    }
}

/// **An operation that failed or lost its document**: the document was never
/// touched, so a store that failed leaves it and its file as they were.
pub fn aborted<O: Default>() -> Completion<O> {
    Completion {
        committed: false,
        outcome: O::default(),
        recovery: Vec::new(),
        returned: Vec::new(),
    }
}

/// Implements the runtime's `Application` for a [`FileWindow`].
macro_rules! application {
    ($window:ty) => {
        impl crate::application::services::Application for $window {
            type Command = crate::file::Request;
            type Commit = crate::file::Commit;
            type Outcome = <$window as crate::turn::Converse>::Outcome;

            fn prepare(
                &self,
                request: crate::file::Request,
                _services: &crate::application::services::Services,
            ) -> Result<crate::application::operations::Operation<crate::file::Commit>, String>
            {
                crate::file::prepare(self, request)
            }

            fn commit(
                &mut self,
                delta: crate::file::Commit,
                services: &mut crate::application::services::Services,
            ) -> crate::application::services::Completion<Self::Outcome> {
                crate::file::commit(self, delta, services)
            }

            fn abort(
                &mut self,
                _delta: crate::file::Commit,
                _result: crate::application::operations::Result,
                _completed_batches: usize,
                _services: &mut crate::application::services::Services,
            ) -> crate::application::services::Completion<Self::Outcome> {
                crate::file::aborted()
            }
        }
    };
}
pub(crate) use application;

/// A document let go of takes its order with it: nothing walks what nobody
/// holds, and nothing walks back across the change.
fn close_order(services: &mut Services, target: &Target) {
    services.histories.close_order(Object {
        member: target.member,
        id: target.object,
    });
}
