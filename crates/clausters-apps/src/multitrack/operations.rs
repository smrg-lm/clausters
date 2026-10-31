//! Typed session work over the shared application runtime.
//!
//! Each delta names the session binding and the files it measured. A reply
//! cannot attach peaks to another session, even when source ids are reused.
use super::editor::{MultitrackEditor, Outcome};
use crate::application::{
    histories::Object,
    operations::{Access, Batch, Operation, Result as IoResult, Target},
    services::{Application, Completion, Services},
};
use clausters_document::{SourceId, parts::TakeFile, session::Session};
use clausters_editing::apply::Step;

/// Session work decoded by the application before preparation.
#[derive(Debug)]
pub struct Request {
    /// Original session and endpoint.
    pub target: Target,
    /// Domain work decoded before entering the runtime.
    pub command: Command,
}

/// **A session as its holder keeps it**: what a window hands the client
/// that opened it when it lets the session go (closed, or another session
/// in its place), and what a window opens from again. The checkpoint
/// belongs to the contents, apart from the path.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Snapshot {
    /// Current contents, including source locations and shared sequences.
    pub session: Session,
    /// Last confirmed destination, or none for an untitled session.
    pub path: Option<String>,
    /// The confirmed checkpoint, kept through changes not saved yet.
    pub saved: Option<Session>,
}

/// A prepared session snapshot, retained through storage confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    /// Contents written, before paths are made relative to the destination.
    pub session: Session,
    /// Destination confirmed only after storage succeeds.
    pub path: String,
    /// What waits for this save to be confirmed: the window's close, or a
    /// change of its session.
    pub then: Option<After>,
    /// A successful Save as replaces an explicit default destination.
    pub save_as: bool,
}

/// **What a save is asked before**, done once it is confirmed and only
/// when nothing moved since it was captured: the close form's Save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum After {
    /// The window closes.
    Close,
    /// The window's session changes (File > New, Open, Close file).
    File(super::editor::FileChange),
}

/// A session read and resolved by the host, ready for one binding transition.
#[derive(Debug)]
pub struct Opened {
    /// Session contents.
    pub session: Session,
    /// Files, joins and sequences resolved before publication.
    pub resolved: super::sources::Resolved,
}

/// Typed session work; the runtime knows none of these domain variants.
#[derive(Debug)]
pub enum Command {
    /// Start a new empty session with no path.
    New,
    /// Unbind the session without closing the window.
    Unbind,
    /// Publish a completely read session and a new source binding.
    Open(Box<Opened>),
    /// Prepare this member's participation in the common history walk.
    Restore,
    /// Each source, its output and the existing measure step.
    Measure(Vec<(SourceId, String, Step)>),
    /// Store an immutable document and confirm its captured checkpoint.
    Save(Box<Saved>),
}

#[derive(Debug)]
enum Change {
    New,
    Unbind,
    Open(Box<Opened>),
    Restore,
    Measure(Vec<(SourceId, TakeFile, String)>),
    Save(Box<Saved>),
}

/// A session delta, unpublished until its dependencies settle.
#[derive(Debug)]
pub struct Commit {
    target: Target,
    binding_generation: u64,
    change: Change,
}

impl Application for MultitrackEditor {
    type Command = Request;
    type Commit = Commit;
    type Outcome = Outcome;

    fn prepare(&self, request: Request, _services: &Services) -> Result<Operation<Commit>, String> {
        if request.target.object != self.session_binding() {
            return Err("the session binding changed before preparation".into());
        }
        let mut sources = Vec::new();
        let mut batches = Vec::new();
        let mut access = Access::Snapshot(0);
        let change = match request.command {
            Command::New => {
                access = Access::Write;
                Change::New
            }
            Command::Unbind => {
                access = Access::Write;
                Change::Unbind
            }
            Command::Open(opened) => {
                access = Access::Write;
                Change::Open(opened)
            }
            Command::Restore => {
                access = Access::Write;
                Change::Restore
            }
            Command::Measure(measured) => {
                for (id, peaks, step) in measured {
                    let file = self.take(id).ok_or("the measured source is absent")?;
                    sources.push((id, file.clone(), peaks));
                    batches.push(Batch {
                        member: request.target.member,
                        server: request.target.server,
                        steps: vec![step],
                    });
                }
                Change::Measure(sources)
            }
            Command::Save(saved) => {
                let folder = std::path::Path::new(&saved.path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let written = super::sources::written(saved.session.clone(), &folder);
                let contents = serde_json::to_string_pretty(&written).map_err(|e| e.to_string())?;
                batches.push(Batch {
                    member: request.target.member,
                    server: request.target.server,
                    steps: vec![Step::Store {
                        path: saved.path.clone(),
                        contents,
                    }],
                });
                Change::Save(saved)
            }
        };
        Ok(Operation {
            target: request.target,
            additional_targets: Vec::new(),
            // The source files are immutable. Navigation or a replacement
            // session may proceed while their summaries are being measured.
            access,
            batches,
            allocations: Vec::new(),
            reads: Vec::new(),
            commit: Commit {
                target: request.target,
                binding_generation: self.binding_generation(),
                change,
            },
        })
    }

    fn commit(&mut self, delta: Commit, services: &mut Services) -> Completion<Outcome> {
        let source_changed = match &delta.change {
            Change::Measure(sources) => sources
                .iter()
                .any(|(id, file, _)| self.take(*id) != Some(file)),
            Change::New | Change::Unbind | Change::Open(_) | Change::Restore | Change::Save(_) => {
                false
            }
        };
        if self.session_binding() != delta.target.object
            || self.binding_generation() != delta.binding_generation
            || source_changed
        {
            return self.abort(
                delta,
                IoResult::Failed("the session or source changed before confirmation".into()),
                0,
                services,
            );
        }
        let mut outcome = Outcome::default();
        match delta.change {
            Change::New => {
                outcome.released = self.replace_session_binding().map(Box::new);
                self.set_checkpoint(Some(self.session_snapshot()));
            }
            Change::Unbind => outcome.released = self.unbind_session().map(Box::new),
            Change::Open(opened) => {
                outcome.released = self.replace_session_binding().map(Box::new);
                self.set_multitrack(opened.session.multitrack);
                self.set_takes(opened.resolved.takes);
                self.set_segments(opened.resolved.segments);
                for (source, sequence) in opened.resolved.sequences {
                    self.bind_sequence(
                        source,
                        std::sync::Arc::new(std::sync::Mutex::new(sequence)),
                    );
                }
            }
            Change::Restore => {}
            Change::Measure(sources) => {
                for (id, mut file, peaks) in sources {
                    file.peaks = Some(peaks.as_str().into());
                    self.set_take(id, file);
                }
            }
            Change::Save(saved) => outcome.saved = Some(saved),
        }
        // a session let go of takes its order with it: one session file per
        // window, and nothing walks a session nobody holds
        if let Some(released) = &outcome.released {
            services.histories.close(Object {
                member: delta.target.member,
                id: released.id,
            });
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
        // Summaries are caches beside user-owned sources. A partial cache
        // may be reused on the next open, but is never published on failure.
        Completion {
            committed: false,
            outcome: Outcome::default(),
            recovery: Vec::new(),
            returned: Vec::new(),
        }
    }
}
