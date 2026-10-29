//! **One undo order over every application open in it**: the editing context.
//!
//! A multitrack, a take and whatever else is on screen walk one history, and no
//! application holds one of its own. An editor is always opened *in* an
//! [`Editing`]; one opened alone is an `Editing` with one member, and two opened
//! in the same one walk one order -- which is the whole of how two applications
//! share an undo.
//!
//! The one exception is a file an audio editor **adds** beside the one it was
//! opened over: no other window reaches it, so it walks an order of its own
//! while it is in focus ([`Editing::select`]).
//!
//! # What the context does, and what it hands back
//!
//! It holds the [`History`], the version a host names back, and its
//! **members**: the editors it opened, and the structures the crate does not
//! apply (a client's curve, its notes) as external members. A **turn** is its:
//! the member reads the gesture, the context records the entry under that
//! member's structure and moves the version. A **step** is its too: it walks the
//! pile, hands each leg to the member that owns the structure, puts the cursor
//! back when nothing could apply it, and moves the version.
//!
//! What a running system does is handed back ([`Effect`]): a multitrack to write
//! back, the steps a write takes on a buffer, the payloads an external member
//! applies, and the answers every window is corrected with.
//!
//! # A structure is named by its key
//!
//! A member declares what it edits -- a take by its buffer, a multitrack by the multitrack
//! -- and a second member declaring the same key is the same structure in the
//! order. Two windows over one take are one structure, so an undo in either
//! walks the edit the other made.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_document::history::{Direction, Entry, History, Step, StructureId};
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::{Opaque, SourceId};
use clausters_editing::conversation::Answer;

use crate::application::histories::{Histories, Object, ScopeId};
use crate::application::resources::{ResourceId, Resources, Root, RootKind, ServerInstance};
use crate::audio::editor::{self as audio, AudioEditor};
use crate::multitrack::editor::{self as multitrack, MultitrackEditor};
use crate::notes::Shared;
use crate::notes::editor::{self as notes, NotesEditor};
use crate::points::editor::{self as points, PointsEditor};
#[cfg(feature = "notation")]
use crate::score::editor::{self as score, ScoreEditor};
use crate::turn::{Event, Kind, Record, int};

mod audio_calls;
mod audio_operations;
mod caller;
mod coordinator;
mod door;
mod exchange;
pub mod held;
mod multitrack_runs;
mod roll_runs;

pub use caller::{Caller, Header, Raised, Ran, Render};
pub use door::call_json;

/// The version an unedited context is at. One rather than zero, because zero is
/// what an edit means by *unstated* when it names the state it was made against.
pub const FIRST_VERSION: i64 = 1;

/// A member's number in its context.
pub type MemberId = u32;

/// **What sits in a context.**
#[derive(Clone, Debug)]
pub enum Member {
    /// A multitrack editor over a multitrack.
    Multitrack(Box<MultitrackEditor>),
    /// An audio editor over a take made of parts.
    Audio(Box<AudioEditor>),
    /// A notes editor over an event sequence it shares with its holder.
    Notes(Box<NotesEditor>),
    /// A points editor over a curve it shares with every points editor opened
    /// under the same key.
    Points(Box<PointsEditor>),
    /// A score editor over a score it shares with its holder.
    #[cfg(feature = "notation")]
    Score(Box<ScoreEditor>),
    /// A structure the crate does not apply: the context records and walks for
    /// it, and hands its legs back to be applied.
    External {
        /// The vocabulary its payloads are written in.
        domain: String,
    },
}

impl Member {
    fn domain(&self) -> String {
        match self {
            Member::Multitrack(_) => MULTITRACK.into(),
            Member::Audio(_) => audio::DOMAIN.into(),
            Member::Notes(_) => notes::DOMAIN.into(),
            Member::Points(_) => points::DOMAIN.into(),
            // a score editor over a sequence records the sequence's edits
            #[cfg(feature = "notation")]
            Member::Score(editor) if editor.is_over() => notes::DOMAIN.into(),
            #[cfg(feature = "notation")]
            Member::Score(_) => score::DOMAIN.into(),
            Member::External { domain } => domain.clone(),
        }
    }
}

/// One member, and the structure it is in the order.
#[derive(Clone, Debug)]
struct Seat {
    member: Member,
    structure: StructureId,
    server: ServerInstance,
}

/// **What a member's turn came to**, in that member's own terms.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Outcome {
    /// A multitrack editor's.
    Multitrack(multitrack::Outcome),
    /// An audio editor's.
    Audio(Box<audio::Outcome>),
    /// A notes editor's.
    Notes(notes::Outcome),
    /// A points editor's.
    Points(points::Outcome),
    /// A score editor's.
    #[cfg(feature = "notation")]
    Score(score::Outcome),
}

impl Outcome {
    fn kind(&self) -> Kind {
        match self {
            Outcome::Multitrack(o) => o.turn,
            Outcome::Audio(o) => o.turn,
            Outcome::Notes(o) => o.turn,
            Outcome::Points(o) => o.turn,
            #[cfg(feature = "notation")]
            Outcome::Score(o) => o.turn,
        }
    }

    fn record(&self) -> Option<&Record> {
        match self {
            Outcome::Multitrack(o) => o.record.as_ref(),
            Outcome::Audio(o) => o.record.as_ref(),
            Outcome::Notes(o) => o.record.as_ref(),
            Outcome::Points(o) => o.record.as_ref(),
            #[cfg(feature = "notation")]
            Outcome::Score(o) => o.record.as_ref(),
        }
    }

    fn changed(&self) -> bool {
        match self {
            Outcome::Multitrack(o) => o.changed,
            Outcome::Audio(o) => o.changed,
            Outcome::Notes(o) => o.changed,
            Outcome::Points(o) => o.changed,
            #[cfg(feature = "notation")]
            Outcome::Score(o) => o.changed,
        }
    }

    fn version(&self) -> i64 {
        match self {
            Outcome::Multitrack(o) => o.version,
            Outcome::Audio(o) => o.version,
            Outcome::Notes(o) => o.version,
            Outcome::Points(o) => o.version,
            #[cfg(feature = "notation")]
            Outcome::Score(o) => o.version,
        }
    }

    fn step(&self) -> (i64, bool) {
        match self {
            Outcome::Multitrack(o) => (o.seq, o.redo),
            Outcome::Audio(o) => (o.seq, o.redo),
            Outcome::Notes(o) => (o.seq, o.redo),
            Outcome::Points(o) => (o.seq, o.redo),
            #[cfg(feature = "notation")]
            Outcome::Score(o) => (o.seq, o.redo),
        }
    }

    fn answer(&mut self, answer: Answer) {
        match self {
            Outcome::Multitrack(o) => o.answer = Some(answer),
            Outcome::Audio(o) => o.answer = Some(answer),
            Outcome::Notes(o) => o.answer = Some(answer),
            Outcome::Points(o) => o.answer = Some(answer),
            #[cfg(feature = "notation")]
            Outcome::Score(o) => o.answer = Some(answer),
        }
    }
}

/// **What one message to a member came to.**
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Turned {
    /// The member's own outcome. Its entry is already recorded; its answer is
    /// the acknowledgement to send.
    pub outcome: Outcome,
    /// The step of the history the message asked for, when it was an undo or a
    /// redo, already taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stepped: Option<Stepped>,
    /// What every **other** member's window is corrected with, when the turn
    /// changed what they draw.
    pub corrections: Vec<Corrected>,
    /// **The takes to free**: buffers no entry of the history and no member
    /// reaches any more, each under the member that made it -- whose server
    /// it is on. The caller frees them after carrying out the turn's steps,
    /// which stop the joins from reading them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub freed: Vec<Freed>,
    /// The version after the turn.
    pub version: i64,
}

/// One member's window, and what it is told.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Corrected {
    /// The member.
    pub member: MemberId,
    /// The answer to send its window.
    pub answer: Answer,
}

/// **What a step does**, for the caller to carry out.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Effect {
    /// A multitrack editor applied a payload to its multitrack.
    #[serde(rename_all = "camelCase")]
    Multitrack {
        /// The member.
        member: MemberId,
        /// What applying it did: the multitrack as it now stands, a source minted.
        applied: multitrack::Applied,
    },
    /// An audio editor put the list the step names back on one of its
    /// files: the file, as its playback sounds it.
    #[serde(rename_all = "camelCase")]
    Audio {
        /// The member.
        member: MemberId,
        /// The edited file's playback projection, independent of view focus.
        file: audio::PlaybackFile,
    },
    /// A notes editor applied the step to the sequence it shares: its holder
    /// reads the change through its own handle, and there is nothing to carry
    /// out but the corrections.
    #[serde(rename_all = "camelCase")]
    Notes {
        /// The member.
        member: MemberId,
        /// Whether the sequence changed.
        applied: bool,
    },
    /// A points editor applied the step to the curve it shares: the points it
    /// leaves, as flat `t v shape curve` quads, for a holder whose curve is an
    /// object of its own to write back.
    #[serde(rename_all = "camelCase")]
    Points {
        /// The member.
        member: MemberId,
        /// The curve's points.
        points: Vec<f64>,
    },
    /// A score editor put the page the step names back on the score it
    /// shares: its holder reads the change through its own handle.
    #[cfg(feature = "notation")]
    #[serde(rename_all = "camelCase")]
    Score {
        /// The member.
        member: MemberId,
        /// Whether the score changed.
        applied: bool,
    },
    /// Payloads an external member applies, in order.
    #[serde(rename_all = "camelCase")]
    External {
        /// The member.
        member: MemberId,
        /// The payloads.
        payloads: Vec<Value>,
    },
}

/// **Takes a member made that nothing reaches any more**: the buffers to free,
/// on that member's server -- or, for a multitrack, the joins.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Freed {
    /// The member whose take they were.
    pub member: MemberId,
    /// The buffer numbers, to give back: an audio editor's takes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub buffers: Vec<i64>,
    /// **A multitrack's joins**, by source id: no region reads one and no
    /// entry can put one back, so the buffer the caller built for it is freed
    /// and the source leaves its table. The multitrack's editor has already
    /// forgotten them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<u64>,
    /// **An audio editor's renders**, by path, and their peaks: no list
    /// reads them, no entry can walk back to them and the clipboard does not
    /// name them, so the caller deletes the files.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// **What a step of the history came to.**
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stepped {
    /// Whether anything moved.
    pub stepped: bool,
    /// A prepared walk whose cursor is still uncommitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::application::operations::Prepared>,
    /// Why nothing did, where it is worth saying: the entry named structures no
    /// member could apply it to, and it is still there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What each member carries out.
    pub effects: Vec<Effect>,
    /// What every member's window is corrected with.
    pub corrections: Vec<Corrected>,
    /// The takes to free, once the effects are carried out. See
    /// [`Turned::freed`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub freed: Vec<Freed>,
    /// The version after the step.
    pub version: i64,
}

/// **The editing context**: a history, its version, and the members it holds.
#[derive(Debug)]
pub struct Editing {
    services: crate::application::services::Services,
    audio_runtime: crate::application::runtime::Runtime<audio::operations::Commit, audio::Outcome>,
    version: i64,
    seats: Vec<Seat>,
    /// The most bytes of takes only the history holds, when there is a limit.
    bytes: Option<u64>,
    /// Each audio member's coordinator ([`coordinator`]).
    sessions: std::collections::BTreeMap<MemberId, coordinator::Session>,
    /// Each multitrack member's playback and its tracks' readers.
    tracks: std::collections::BTreeMap<MemberId, multitrack_runs::Tracks>,
    /// Each roll's playback, on a transport of its own.
    rolls: std::collections::BTreeMap<MemberId, roll_runs::Roll>,
    /// The file each audio member plays, kept beside its session since a
    /// run holds the session while it decides whether an edit may start.
    audio_playing: std::collections::BTreeMap<MemberId, u64>,
    /// The engine the application holds, when it holds one: an audio member
    /// opened from then on plays on it.
    engine: Option<Box<dyn held::HeldEngine>>,
    /// What its audio members hand on by reference: the lists a window draws,
    /// the copies, the takes the histories name.
    shared: crate::audio::shared::Shared,
}

impl Default for Editing {
    fn default() -> Self {
        Self::new()
    }
}

impl Editing {
    /// An empty context.
    pub fn new() -> Self {
        Self::with_services(Histories::new(), Resources::default())
    }

    /// Compose explicit editing-order and source-ownership services.
    /// Neither service needs a renderer, a server connection or playback.
    pub fn with_services(history: Histories, resources: Resources) -> Self {
        Self {
            services: crate::application::services::Services::new(history, resources),
            audio_runtime: Default::default(),
            version: FIRST_VERSION,
            seats: Vec::new(),
            bytes: None,
            sessions: Default::default(),
            tracks: Default::default(),
            rolls: Default::default(),
            audio_playing: Default::default(),
            engine: None,
            shared: Default::default(),
        }
    }

    /// **Plays its audio members on `engine`**, an engine the application
    /// holds: their playback's ops become the engine's commands, and each
    /// file's list is handed to the engine to read rather than stitched into
    /// a join on a server. Set before the first audio member opens.
    pub fn hold_engine(&mut self, engine: Box<dyn held::HeldEngine>) {
        self.engine = Some(engine);
    }

    /// **Limits the takes only the history holds to `bytes`**, or lifts the
    /// limit with `None`. The oldest entries go first when a turn passes it,
    /// and what they held comes back as takes to free.
    pub fn set_bytes(&mut self, bytes: Option<u64>) {
        self.bytes = bytes;
    }

    /// **The takes no entry and no member reaches any more**, forgotten by
    /// every member and handed back as the buffers to free -- after the byte
    /// limit, when there is one, has trimmed the pile.
    fn release(&mut self) -> Vec<Freed> {
        let editors: Vec<&AudioEditor> = self
            .seats
            .iter()
            .filter_map(|seat| match &seat.member {
                Member::Audio(editor) => Some(editor.as_ref()),
                _ => None,
            })
            .collect();
        // A join a region still windows is a root the history does not see,
        // as a take an audio editor shows is.
        let rooted: Vec<SourceId> = editors
            .iter()
            .flat_map(|e| e.rooted())
            .chain(self.seats.iter().flat_map(|seat| match &seat.member {
                Member::Multitrack(editor) => editor.joins_read(),
                _ => Vec::new(),
            }))
            .collect();
        self.sync_resources();
        if let Some(bytes) = self.bytes {
            let endpoints: std::collections::HashMap<_, _> = self
                .seats
                .iter()
                .enumerate()
                .flat_map(|(member, seat)| {
                    self.services
                        .histories
                        .structures_for(member as MemberId)
                        .into_iter()
                        .map(move |structure| (structure, seat.server))
                })
                .collect();
            let resources = &self.services.resources;
            self.services.histories.trim_orders(bytes, &|history| {
                let ids: std::collections::BTreeSet<_> = history
                    .structures()
                    .into_iter()
                    .filter_map(|structure| {
                        endpoints.get(&structure).map(|server| (structure, *server))
                    })
                    .flat_map(|(structure, server)| {
                        history
                            .held_sources_for(structure)
                            .into_iter()
                            .map(move |source| ResourceId {
                                server,
                                buffer: source.0 as i64,
                            })
                    })
                    .collect();
                ids.into_iter()
                    .filter(|id| !resources.retained_outside_history(*id))
                    .filter_map(|id| resources.source(id).map(|source| source.bytes))
                    .sum()
            });
            self.sync_resources();
        }
        let released = self.services.resources.collect();
        let released_joins = self.services.histories.released_sources();
        // Each take goes back under the member that made it: the one that
        // knows its size is the one whose server it is on.
        let mut out: Vec<Freed> = Vec::new();
        let at_member = |out: &mut Vec<Freed>, member: MemberId| match out
            .iter()
            .position(|f| f.member == member)
        {
            Some(at) => at,
            None => {
                out.push(Freed {
                    member,
                    buffers: Vec::new(),
                    sources: Vec::new(),
                    files: Vec::new(),
                });
                out.len() - 1
            }
        };
        for source in released {
            let at = at_member(&mut out, source.member);
            out[at].buffers.push(source.id.buffer);
        }
        for source in released_joins {
            if rooted.contains(&source) {
                continue;
            }
            for (owner, seat) in self.seats.iter_mut().enumerate() {
                match &mut seat.member {
                    Member::Multitrack(editor) if editor.is_join(source) => {
                        let at = at_member(&mut out, owner as MemberId);
                        out[at].sources.push(source.0);
                        editor.forget_join(source);
                    }
                    // **A render nothing names any more**: its file and its
                    // peaks go, unless the clipboard names the file.
                    Member::Audio(editor) => {
                        let named = self.services.clipboard.files();
                        let files: Vec<String> = editor
                            .release(source)
                            .into_iter()
                            .filter(|f| !named.iter().any(|n| n == f))
                            .collect();
                        if !files.is_empty() {
                            let at = at_member(&mut out, owner as MemberId);
                            out[at].files.extend(files);
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    fn sync_resources(&mut self) {
        for (member, seat) in self.seats.iter().enumerate() {
            let Member::Audio(editor) = &seat.member else {
                continue;
            };
            let member = member as MemberId;
            editor.sync_resources(member, seat.server, &mut self.services);
        }
    }

    fn invalidate_server(&mut self, server: ServerInstance) -> Vec<ResourceId> {
        self.audio_runtime
            .invalidate(&mut self.services.resources, server);
        self.services.resources.invalidate(server)
    }

    /// Resources explicitly supplied to this composition, independent of views.
    pub fn resources(&mut self) -> &mut Resources {
        &mut self.services.resources
    }

    /// Associate a member with a connection incarnation before it allocates.
    pub fn set_server(&mut self, member: MemberId, server: ServerInstance) -> bool {
        let Some(seat) = self.seats.get_mut(member as usize) else {
            return false;
        };
        seat.server = server;
        if let Member::Audio(editor) = &mut seat.member {
            editor.set_server(server);
        }
        true
    }

    /// **Whether file `file` of audio member `member` is playing**: what an
    /// edit of it is refused for.
    pub(crate) fn audio_plays(&self, member: MemberId, file: i64) -> bool {
        self.audio_playing.get(&member).copied() == u64::try_from(file).ok()
    }

    /// Says which file audio member `member` plays now, if one.
    pub(super) fn note_playing(&mut self, member: MemberId, file: Option<u64>) {
        match file {
            Some(file) => self.audio_playing.insert(member, file),
            None => self.audio_playing.remove(&member),
        };
    }

    /// **The actions `member` answers, as they read now**: the editor's own
    /// facts, the history of the object in focus and the work pending on it.
    /// Read on every call and kept nowhere, so a menu or a tool never holds a
    /// second copy of the state. An empty object for a member with no action
    /// readings.
    pub fn action_states(&mut self, member: MemberId) -> Value {
        if self.member(member).is_none() {
            return json!({});
        }
        self.select_member(member);
        let (can_undo, can_redo) = (self.can_undo(), self.can_redo());
        let seat = &self.seats[member as usize];
        match &seat.member {
            Member::Audio(editor) => {
                let pending =
                    self.audio_runtime
                        .operations
                        .busy(crate::application::operations::Target {
                            member,
                            object: editor.focused_file(),
                            server: seat.server,
                        });
                let playing = self.audio_plays(member, editor.focused_file());
                json!(editor.actions(member, seat.server, can_undo, can_redo, pending, playing))
            }
            Member::Multitrack(editor) => {
                let stops = self
                    .tracks
                    .get(&member)
                    .map(multitrack_runs::Tracks::stops_at_end);
                json!(editor.actions(can_undo, can_redo, stops))
            }
            Member::Notes(editor) => json!(editor.actions(can_undo, can_redo)),
            _ => json!({}),
        }
    }

    /// The version a host names back.
    pub fn version(&self) -> i64 {
        self.version
    }

    /// Whether there is an edit to step back over.
    pub fn can_undo(&self) -> bool {
        self.services.histories.can_undo()
    }

    /// Whether there is an undone edit to step forward into.
    pub fn can_redo(&self) -> bool {
        self.services.histories.can_redo()
    }

    /// What an undo would be called.
    pub fn undo_label(&self) -> Option<String> {
        self.services.histories.undo_label()
    }

    /// What a redo would be called.
    pub fn redo_label(&self) -> Option<String> {
        self.services.histories.redo_label()
    }

    /// The structure `member` is in the order: the identity every member with
    /// its key shares, and what a client names that structure's widgets by.
    pub fn structure(&self, member: MemberId) -> Option<StructureId> {
        self.seats.get(member as usize).map(|seat| seat.structure)
    }

    /// **Takes a member in**, as the structure `key` names: the identity a
    /// member with the same key already has, or a new one -- in the
    /// context's shared order.
    pub fn join(&mut self, key: &str, member: Member) -> MemberId {
        self.admit(member, Some(key))
    }

    /// **Takes a member in with an undo order of its own**: its object shares
    /// no history with any other member's, whatever was opened before it.
    /// Sharing an order is a composition the caller asks for ([`join`]).
    ///
    /// [`join`]: Self::join
    pub fn open_independent(&mut self, member: Member) -> MemberId {
        self.admit(member, None)
    }

    fn admit(&mut self, mut member: Member, shared: Option<&str>) -> MemberId {
        if let Member::Audio(editor) = &mut member {
            editor.share(self.shared.clone());
        }
        let id = self.seats.len() as MemberId;
        let object = Object {
            member: id,
            id: match &member {
                Member::Audio(editor) => editor.focused_file(),
                _ => 0,
            },
        };
        let domain = member.domain();
        let structure = match shared {
            Some(key) => self.services.histories.join(object, key, &domain),
            None => self.services.histories.focus(object, &domain),
        };
        self.seats.push(Seat {
            member,
            structure,
            server: ServerInstance::default(),
        });
        id
    }

    /// **Selects whose order the next step and state read**: `member`'s (for
    /// an audio editor, the order of the file in focus) or, with `None`, the
    /// context's. A turn of a member selects its own; a caller stepping
    /// for a window names it here first.
    pub fn select(&mut self, member: Option<MemberId>) {
        match member {
            Some(member) => self.select_member(member),
            None => self.services.histories.select(ScopeId(0)),
        }
    }

    fn close_audio_file(&mut self, member: MemberId, file: i64) {
        self.services.histories.close(Object { member, id: file });
    }

    fn select_member(&mut self, member: MemberId) {
        let Some(seat) = self.seats.get(member as usize) else {
            return;
        };
        // Domain adapters supply object identity; order membership is generic.
        let object = match &seat.member {
            // an editor with no file open has no order to focus
            Member::Audio(editor) if !editor.has_file() => return,
            Member::Audio(editor) => editor.focused_file(),
            _ => 0,
        };
        let domain = seat.member.domain();
        let structure = self
            .services
            .histories
            .focus(Object { member, id: object }, &domain);
        self.seats[member as usize].structure = structure;
    }

    /// A member, to read.
    pub fn member(&self, member: MemberId) -> Option<&Member> {
        self.seats.get(member as usize).map(|seat| &seat.member)
    }

    /// A member, to read or to hand the facts it draws from.
    pub fn member_mut(&mut self, member: MemberId) -> Option<&mut Member> {
        self.seats
            .get_mut(member as usize)
            .map(|seat| &mut seat.member)
    }

    /// What its audio members hand on by reference ([`crate::audio::shared`]).
    #[cfg(test)]
    pub(crate) fn shared(&self) -> &crate::audio::shared::Shared {
        &self.shared
    }

    /// **The lists its audio members named since the last call**, for the
    /// host: what their answers name by number, handed over by reference.
    pub fn said(&self) -> Vec<(u64, clausters_document::parts::FileList)> {
        self.shared.said()
    }

    /// **An audio member whose window is gone leaves the context**: every
    /// file it holds is closed, its history with it, so the takes and the
    /// renders only it named are let go; its session goes, and what is left
    /// of it is an editor with no file. Its seat stays, so no other member's
    /// number moves. Answers what that freed, the renders to delete among it.
    pub fn forget_audio(&mut self, member: MemberId) -> Vec<Freed> {
        self.sessions.remove(&member);
        let files = match self.member_mut(member) {
            Some(Member::Audio(editor)) => editor.let_go_files(),
            _ => return Vec::new(),
        };
        for id in files {
            self.services.histories.close(Object { member, id });
        }
        let freed = self.release();
        let shared = self.shared.clone();
        if let Some(Member::Audio(editor)) = self.member_mut(member) {
            **editor = AudioEditor::empty();
            editor.share(shared);
        }
        self.sync_resources();
        freed
    }

    /// **An edit that leaves no entry still moves the version**: one applied
    /// with no inverse to record, which a host must stop naming the version it
    /// was made against. Answers the version it moved to.
    pub fn moved(&mut self) -> i64 {
        self.version += 1;
        self.version
    }

    /// **Records an entry** under `member`'s structure -- what an external
    /// member hands over for an edit it applied itself -- continuing the entry
    /// before it when `coalesce` says the hand has not stopped. Answers whether
    /// the history took it, and moves the version when it did.
    pub fn record(&mut self, member: MemberId, record: &Record, coalesce: bool) -> bool {
        self.select_member(member);
        let Some(structure) = self.seats.get(member as usize).map(|s| s.structure) else {
            return false;
        };
        let taken = self.record_at(structure, record, coalesce);
        if taken {
            self.version += 1;
        }
        taken
    }

    /// **Records an entry already built over structures of this context** --
    /// what an embedder in Rust hands over when it builds its entries with a
    /// vocabulary's own types (the document's log, say) rather than as JSON.
    /// Answers whether the history took it, and moves the version when it did.
    pub fn record_entry(&mut self, entry: Entry) -> bool {
        let taken = self.services.histories.record(entry);
        if taken {
            self.version += 1;
        }
        taken
    }

    /// **Applies and records through a vocabulary's own door**: `apply` is
    /// handed the history and `member`'s structure, and answers whether it
    /// recorded -- what an embedder in Rust does when the vocabulary applies and
    /// records in one call (the document's `apply_logged_in`). The version moves
    /// when it did.
    pub fn record_with(
        &mut self,
        member: MemberId,
        apply: impl FnOnce(&mut History, StructureId) -> bool,
    ) -> bool {
        self.select_member(member);
        let Some(structure) = self.structure(member) else {
            return false;
        };
        let taken = apply(self.services.histories.selected_history_mut(), structure);
        if taken {
            self.version += 1;
        }
        taken
    }

    fn record_at(&mut self, structure: StructureId, record: &Record, coalesce: bool) -> bool {
        let mut entry: Option<Entry> = None;
        for leg in &record.legs {
            let Ok(forward) = serde_json::from_value::<Step>(leg.forward.clone()) else {
                continue;
            };
            let backward = Opaque(leg.backward.clone());
            let next = match entry.take() {
                Some(e) => e.and(structure, forward, backward),
                None => Entry::new(record.label.clone(), structure, forward, backward),
            };
            let next = next.holding(
                leg.holds_forward.iter().map(|s| SourceId(*s)),
                leg.holds_backward.iter().map(|s| SourceId(*s)),
            );
            entry = Some(if leg.key.is_empty() {
                next
            } else {
                next.keyed(leg.key.clone())
            });
        }
        entry.is_some_and(|entry| {
            self.services
                .histories
                .record(if coalesce { entry.continuing() } else { entry })
        })
    }

    /// **One message to a member**, read, recorded and answered.
    ///
    /// `None` for a member that is not there or does not read messages (an
    /// external one).
    pub fn event(&mut self, member: MemberId, event: &Event) -> Option<Turned> {
        self.select_member(member);
        let version = self.version;
        let seat = self.seats.get_mut(member as usize)?;
        let structure = seat.structure;
        let mut outcome = match &mut seat.member {
            Member::Multitrack(editor) => Outcome::Multitrack(editor.event(event, version)),
            Member::Audio(editor) => Outcome::Audio(Box::new(editor.event(event, version))),
            Member::Notes(editor) => Outcome::Notes(editor.event(event, version)),
            Member::Points(editor) => Outcome::Points(editor.event(event, version)),
            #[cfg(feature = "notation")]
            Member::Score(editor) => Outcome::Score(editor.event(event, version)),
            Member::External { .. } => return None,
        };
        if let Some(record) = outcome.record().cloned() {
            self.record_at(structure, &record, false);
        }
        let mut corrections = Vec::new();
        if outcome.changed() {
            self.version = outcome.version();
            corrections = self.corrections(Some(member));
        }
        let mut stepped = None;
        if outcome.kind() == Kind::Step {
            let (seq, redo) = outcome.step();
            let direction = if redo {
                Direction::Redo
            } else {
                Direction::Undo
            };
            let step = self
                .prepare_audio_walk(direction)
                .unwrap_or_else(|| self.step(direction));
            let pending = step.operation.as_ref().map(|prepared| prepared.ticket);
            if let Some(ticket) = pending {
                self.audio_runtime.acknowledge_walk(ticket, member, seq);
            }
            let version = self.version;
            let reason = step.reason.clone();
            if let Some(seat) = self.seats.get(member as usize) {
                match &seat.member {
                    Member::Multitrack(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::Audio(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::Notes(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::Points(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    #[cfg(feature = "notation")]
                    Member::Score(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::External { .. } => {}
                }
            }
            if pending.is_some()
                && let Outcome::Audio(audio) = &mut outcome
            {
                audio.answer = None;
            }
            stepped = Some(step);
        }
        let freed = self.release();
        Some(Turned {
            outcome,
            stepped,
            corrections,
            freed,
            version: self.version,
        })
    }

    /// **One verb a client calls on a member** -- `request` as that member's
    /// own verbs read it -- read, recorded and answered like a message: an edit
    /// the hand asked for through the client rather than on the page.
    ///
    /// `None` for a member that is not there or takes no verbs this way.
    pub fn act(&mut self, member: MemberId, request: &Value) -> Option<Turned> {
        self.select_member(member);
        let version = self.version;
        let seat = self.seats.get_mut(member as usize)?;
        let structure = seat.structure;
        let outcome = match &mut seat.member {
            Member::Audio(editor) => Outcome::Audio(Box::new(editor.act(request, version))),
            #[cfg(feature = "notation")]
            Member::Score(editor) => Outcome::Score(editor.act(request, version)),
            _ => return None,
        };
        if let Some(record) = outcome.record().cloned() {
            self.record_at(structure, &record, false);
        }
        let mut corrections = Vec::new();
        if outcome.changed() {
            self.version = outcome.version();
            corrections = self.corrections(Some(member));
        }
        let freed = self.release();
        Some(Turned {
            outcome,
            stepped: None,
            corrections,
            freed,
            version: self.version,
        })
    }

    /// **One step of the history**, handed round the members.
    ///
    /// Each leg goes to the members holding its structure: every multitrack
    /// editor over the multitrack applies it to the multitrack it draws; a write to a take
    /// and an external member's payloads are carried out once, by the caller. A
    /// step nothing could apply is not a step: the cursor goes back, and the
    /// entry is still there.
    pub fn step(&mut self, direction: Direction) -> Stepped {
        let label = match direction {
            Direction::Undo => self.services.histories.undo_label(),
            Direction::Redo => self.services.histories.redo_label(),
        }
        .unwrap_or_default();
        let mut out = Stepped {
            stepped: false,
            operation: None,
            reason: None,
            effects: Vec::new(),
            corrections: Vec::new(),
            freed: Vec::new(),
            version: self.version,
        };
        let targets: Vec<_> = self
            .services
            .histories
            .peek_walk(direction)
            .into_iter()
            .flat_map(|walked| walked.legs.into_iter().map(|(structure, _)| structure))
            .collect();
        if self.seats.iter().enumerate().any(|(member, seat)| {
            matches!(seat.member, Member::Audio(_))
                && !self.services.resources.valid(seat.server)
                && self
                    .services
                    .histories
                    .structures_for(member as MemberId)
                    .iter()
                    .any(|structure| targets.contains(structure))
        }) {
            out.reason = Some("the editing order references a previous server instance".into());
            return out;
        }
        let Some(walked) = self.services.histories.walk(direction) else {
            return out;
        };
        // **Whole on every structure, or on none**: what each member was before
        // the step is kept, so a structure that refuses its legs puts back
        // the ones the step already went through.
        let mut kept: Vec<(usize, Kept)> = Vec::new();
        let mut refused = false;
        let mut applied = false;
        for (structure, payloads) in &walked.legs {
            let mut written = false;
            let mut seated = false;
            let mut stepped = false;
            for (id, seat) in self.seats.iter_mut().enumerate() {
                let member = id as MemberId;
                let object = self.services.histories.object_for(member, *structure);
                if seat.structure != *structure && object.is_none() {
                    continue;
                }
                match &mut seat.member {
                    Member::Multitrack(editor) => {
                        seated = true;
                        kept.push((id, Kept::Multitrack(editor.before_step())));
                        let loads: Vec<Value> = payloads.iter().map(|p| p.0.clone()).collect();
                        for done in editor.apply_step(&loads) {
                            stepped = true;
                            out.effects.push(Effect::Multitrack {
                                member,
                                applied: done,
                            });
                        }
                    }
                    Member::Audio(editor) => {
                        seated = true;
                        let target = object.map_or(editor.focused_file(), |object| object.id);
                        if let Some((before, result)) = editor.over_file(target, |editor| {
                            let before = editor.before_step();
                            let result = payloads
                                .last()
                                .is_some_and(|p| editor.apply(&p.0))
                                .then(|| editor.playback_file());
                            (before, result)
                        }) {
                            kept.push((id, Kept::Audio(target, before)));
                            if let Some(file) = result {
                                stepped = true;
                                out.effects
                                    .extend(file.map(|file| Effect::Audio { member, file }));
                            }
                        }
                    }
                    // One structure, one sequence: every notes editor over it
                    // shares the sequence, so the first applies the step and
                    // the rest are corrected.
                    Member::Notes(editor) if !written => {
                        written = true;
                        seated = true;
                        kept.push((id, Kept::Notes(editor.before_step())));
                        let mut done = false;
                        for payload in payloads {
                            done |= editor.apply(&payload.0);
                        }
                        stepped |= done;
                        out.effects.push(Effect::Notes {
                            member,
                            applied: done,
                        });
                    }
                    // One curve, shared by every points editor over it: the
                    // first applies the step and the rest are corrected.
                    Member::Points(editor) if !written => {
                        written = true;
                        seated = true;
                        kept.push((id, Kept::Points(editor.before_step())));
                        let mut left = None;
                        for payload in payloads {
                            left = editor.apply(&payload.0).or(left);
                        }
                        if let Some(points) = left {
                            stepped = true;
                            out.effects.push(Effect::Points { member, points });
                        }
                    }
                    // One score, shared by every score editor over it: the
                    // first loads the page the step names and the rest are
                    // corrected.
                    #[cfg(feature = "notation")]
                    Member::Score(editor) if !written => {
                        written = true;
                        seated = true;
                        kept.push((id, Kept::Score(editor.before_step())));
                        let mut done = false;
                        for payload in payloads {
                            done |= editor.apply(&payload.0);
                        }
                        stepped |= done;
                        out.effects.push(Effect::Score {
                            member,
                            applied: done,
                        });
                    }
                    // Carried out by the caller, after the step is taken:
                    // nothing here to put back, and nothing that refuses.
                    Member::External { .. } if !written => {
                        written = true;
                        seated = true;
                        stepped |= !payloads.is_empty();
                        out.effects.push(Effect::External {
                            member,
                            payloads: payloads.iter().map(|p| p.0.clone()).collect(),
                        });
                    }
                    _ => {}
                }
            }
            // A structure nobody holds any more has nobody to refuse it; one
            // that is held and moved nothing refused the step.
            if seated && !stepped {
                refused = true;
                break;
            }
            applied |= stepped;
        }
        if refused || !applied {
            for (id, state) in kept.into_iter().rev() {
                if let Some(seat) = self.seats.get_mut(id) {
                    state.put_back(&mut seat.member);
                }
            }
            self.services.histories.step(match direction {
                Direction::Undo => Direction::Redo,
                Direction::Redo => Direction::Undo,
            });
            out.effects.clear();
            out.reason = Some(format!("{label}: nothing here can put that edit back"));
            return out;
        }
        self.version += 1;
        out.stepped = true;
        out.version = self.version;
        out.corrections = self.corrections(None);
        out.freed = self.release();
        out
    }

    /// What every member's window but `except` is corrected with.
    fn corrections(&mut self, except: Option<MemberId>) -> Vec<Corrected> {
        let version = self.version;
        let mut out = Vec::new();
        for (id, seat) in self.seats.iter_mut().enumerate() {
            let member = id as MemberId;
            if Some(member) == except {
                continue;
            }
            let answer = match &mut seat.member {
                Member::Multitrack(editor) => editor.resync_all(version),
                Member::Audio(editor) => editor.resync_all(version),
                Member::Notes(editor) => editor.resync_all(version),
                Member::Points(editor) => editor.resync_all(version),
                #[cfg(feature = "notation")]
                Member::Score(editor) => editor.resync_all(version),
                Member::External { .. } => continue,
            };
            if answer != Answer::Silent {
                out.push(Corrected { member, answer });
            }
        }
        out
    }
}

/// **What a member was before a history step**, put back when another
/// structure of the same step refuses it.
enum Kept {
    Multitrack(crate::multitrack::editor::Kept),
    Audio(i64, audio::Kept),
    Notes(clausters_document::EventSequence),
    Points(clausters_document::multitrack::Automation),
    #[cfg(feature = "notation")]
    Score(crate::score::editor::Kept),
}

impl Kept {
    fn put_back(self, member: &mut Member) {
        match (self, member) {
            (Kept::Multitrack(state), Member::Multitrack(editor)) => editor.put_back(state),
            (Kept::Audio(file, state), Member::Audio(editor)) => {
                editor.over_file(file, |editor| editor.put_back(state));
            }
            (Kept::Notes(state), Member::Notes(editor)) => editor.put_back(state),
            (Kept::Points(state), Member::Points(editor)) => editor.put_back(state),
            #[cfg(feature = "notation")]
            (Kept::Score(state), Member::Score(editor)) => editor.put_back(state),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;

/// An answer as a value, for a door to write out.
fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| json!({}))
}
