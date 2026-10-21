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
use crate::application::resources::{
    ResourceId, Resources, Root, RootKind, ServerInstance, Source,
};
use crate::audio::editor::{self as audio, AudioEditor};
use crate::multitrack::editor::{self as multitrack, MultitrackEditor};
use crate::notes::Shared;
use crate::notes::editor::{self as notes, NotesEditor};
use crate::points::editor::{self as points, PointsEditor};
#[cfg(feature = "notation")]
use crate::score::editor::{self as score, ScoreEditor};
use crate::turn::{Event, Kind, Record, int};

mod audio_operations;
mod exchange;

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
    /// **The takes to write to disk**, once the turn's own steps are carried
    /// out: past the resident budget, the oldest takes only the history holds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stored: Vec<Stored>,
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
    /// The steps an audio editor's take is stitched again with, for the list
    /// the step handed it.
    #[serde(rename_all = "camelCase")]
    Audio {
        /// The member.
        member: MemberId,
        /// The steps, in the JSON a runner walks.
        steps: Value,
        /// The edited object's playback projection, independent of view focus.
        file: Value,
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
    /// Those of them whose take was on disk: the number goes back, and there
    /// is no buffer on the server to free.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spilled: Vec<i64>,
    /// **A multitrack's joins**, by source id: no region reads one and no
    /// entry can put one back, so the buffer the caller built for it is freed
    /// and the source leaves its table. The multitrack's editor has already
    /// forgotten them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<u64>,
}

/// **A take leaving memory**: only the history holds it, and the resident
/// budget is past. The member's steps write it to disk and free its buffer; a
/// caller that cannot carry them out tells the member it was `kept`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stored {
    /// The member whose take it is.
    pub member: MemberId,
    /// Its buffer, which stays its number while it is on disk.
    pub buffer: i64,
    /// The steps that write it and free the buffer.
    pub steps: Value,
    /// A prepared storage operation whose waits and confirmation must be executed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::application::operations::Prepared>,
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
    /// The takes to write to disk. See [`Turned::stored`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stored: Vec<Stored>,
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
    /// The most of those bytes kept in memory, when there is a limit; the rest
    /// is on disk.
    resident: Option<u64>,
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
            resident: None,
        }
    }

    /// **Keeps at most `bytes` of the takes only the history holds in
    /// memory**, or lifts the limit with `None`. Past it the oldest go to disk,
    /// through the member that made them, and a step that needs one reads it
    /// back.
    pub fn set_resident(&mut self, bytes: Option<u64>) {
        self.resident = bytes;
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
    fn release(&mut self) -> (Vec<Freed>, Vec<Stored>) {
        self.release_with_storage(true)
    }

    fn release_with_storage(&mut self, schedule_storage: bool) -> (Vec<Freed>, Vec<Stored>) {
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
                    spilled: Vec::new(),
                    sources: Vec::new(),
                });
                out.len() - 1
            }
        };
        for source in released {
            let at = at_member(&mut out, source.member);
            out[at].buffers.push(source.id.buffer);
            if source.stored.is_some() {
                out[at].spilled.push(source.id.buffer);
            }
            for seat in &mut self.seats {
                if seat.server == source.id.server
                    && let Member::Audio(editor) = &mut seat.member
                {
                    editor.forget(source.id.buffer);
                }
            }
        }
        for source in released_joins {
            if rooted.contains(&source) {
                continue;
            }
            for (owner, seat) in self.seats.iter_mut().enumerate() {
                if let Member::Multitrack(editor) = &mut seat.member
                    && editor.is_join(source)
                {
                    let at = at_member(&mut out, owner as MemberId);
                    out[at].sources.push(source.0);
                    editor.forget_join(source);
                }
            }
        }
        let stored = if schedule_storage {
            self.store()
        } else {
            Vec::new()
        };
        (out, stored)
    }

    /// **The takes past the resident budget, out of memory**: of the takes
    /// only the history holds and still in a buffer, the oldest go to disk
    /// until what is left fits.
    fn store(&mut self) -> Vec<Stored> {
        let Some(limit) = self.resident else {
            return Vec::new();
        };
        let candidates = self.services.resources.spill_candidates(limit, |id| {
            self.services.resources.source(id).is_some_and(|source| {
                matches!(&self.seats[source.member as usize].member,
                    Member::Audio(editor) if editor.storage_available())
            })
        });
        let selected = self.services.histories.selected();
        let mut out = Vec::new();
        for id in candidates {
            let Some(source) = self.services.resources.source(id) else {
                continue;
            };
            let owner = source.member as usize;
            let buffer = id.buffer;
            if let Ok(ticket) = self.prepare_audio(
                owner as MemberId,
                audio::operations::Command::Spill(buffer),
                0,
            ) && let Some((batch, work)) = self.audio_runtime.operations.next(ticket)
            {
                out.push(Stored {
                    member: owner as MemberId,
                    buffer,
                    steps: clausters_editing::apply::steps_json(&work.steps),
                    operation: Some(crate::application::operations::Prepared {
                        member: work.member,
                        ticket,
                        batch,
                        server: work.server,
                        steps: work.steps,
                    }),
                });
            }
        }
        self.services.histories.select(selected);
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
                json!(editor.actions(member, seat.server, can_undo, can_redo, pending))
            }
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

    fn admit(&mut self, member: Member, shared: Option<&str>) -> MemberId {
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
        let (freed, stored) = self.release();
        Some(Turned {
            outcome,
            stepped,
            corrections,
            freed,
            stored,
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
        let (freed, stored) = self.release();
        Some(Turned {
            outcome,
            stepped: None,
            corrections,
            freed,
            stored,
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
            stored: Vec::new(),
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
                                .and_then(|p| editor.apply(&p.0))
                                .map(|steps| (steps, editor.playback_file()));
                            (before, result)
                        }) {
                            kept.push((id, Kept::Audio(target, before)));
                            if let Some((steps, file)) = result {
                                stepped = true;
                                out.effects.push(Effect::Audio {
                                    member,
                                    steps,
                                    file,
                                });
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
        (out.freed, out.stored) = self.release();
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

/// A record as a client hands one over for an external member.
#[derive(Deserialize)]
struct Recorded {
    #[serde(default)]
    label: String,
    #[serde(default)]
    legs: Vec<RecordedLeg>,
}

#[derive(Deserialize)]
struct RecordedLeg {
    forward: Value,
    backward: Value,
    #[serde(default)]
    key: Option<String>,
    #[serde(default, rename = "holdsForward")]
    holds_forward: Vec<u64>,
    #[serde(default, rename = "holdsBackward")]
    holds_backward: Vec<u64>,
}

/// **One verb of a context, over JSON** -- the door both clients bind.
///
/// `request` names the `verb` and carries its arguments:
///
/// - `openMultitrack` -- `key`, and what a multitrack editor is built from
///   (`clausters_apps::multitrack::editor::new_json`, the version aside):
///   `{"member", "structure"}`, or `{"error"}`.
/// - `openAudio` -- `key`, and what an audio editor is built from
///   (`clausters_apps::audio::editor::new_json`): `{"member", "structure"}`,
///   or `{"error"}`. Its file has an undo order of its own; `sharedHistory`
///   `true` joins it to the context's shared order under `key` instead.
/// - `bytes` -- `bytes`, or `null` for none: the most bytes of takes only the
///   history may hold. Answers `{}`.
/// - `resident` -- `bytes`, or `null` for none: the most of those kept in
///   memory; past it the oldest go to disk (a turn's and a step's `stored`).
///   Answers `{}`.
/// - `openNotes` -- `key`, `sequence` (the sequence as data, which the
///   editor then holds alone -- a caller that shares one opens it with
///   [`Editing::open_notes`]) and what a notes editor is built from
///   (`clausters_apps::notes::editor::new_json`): `{"member", "structure"}`.
/// - `openPoints` -- `key`, `points` (the curve as flat quads) and `name`,
///   and what a points editor is built from
///   (`clausters_apps::points::editor::new_json`): `{"member", "structure"}`.
///   A second points editor under a key one already has edits that one's
///   curve.
/// - `external` -- `key`, `domain`: `{"member", "structure"}`.
/// - `event` -- `member`, `addr`, `args`: a [`Turned`], or `null`.
/// - `act` -- `member`, `call`: a verb a client calls on a member, which
///   edits -- a [`Turned`], or `null` (the score editor's verbs, built with the
///   `notation` feature).
/// - `step` -- `direction` (`"undo"` or `"redo"`): a [`Stepped`].
/// - `record` -- `member`, `label`, `legs` (`forward`, `backward`, `key`),
///   `coalesce`: `{"recorded", "version"}`.
/// - `member` -- `member`, and a verb of that member's own door with its
///   arguments, the version filled in: what that door answers. `event` and
///   `apply` are the context's and answer `{}` here.
/// - `moved` -- an edit that leaves no entry: `{"version"}`, moved on.
/// - `actions` -- `member`: each action the member answers, by verb, as
///   [`clausters_editing::verbs::ActionState`] reads it now. A member's
///   `window` answer carries the same object as `actions`.
/// - `state` -- `{"version", "canUndo", "canRedo", "undoLabel", "redoLabel"}`.
///
/// An unknown verb answers `{}`.
pub fn call_json(editing: &mut Editing, request: &str) -> String {
    let Ok(mut request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let get = |request: &Value, key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let member = int(&get(&request, "member")) as MemberId;
    let key = get(&request, "key")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let verb = request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(map) = request.as_object_mut() {
        map.insert("version".into(), json!(editing.version));
    }
    match verb.as_str() {
        "openMultitrack" => match multitrack::new_json(&request.to_string()) {
            Some(editor) => joined(editing, &key, Member::Multitrack(Box::new(editor))),
            None => json!({ "error": "the request names no multitrack" }).to_string(),
        },
        "openAudio" => match audio::new_json(&request.to_string()) {
            Ok(editor) => {
                // An audio file's history is its own unless the caller
                // composes it into the context's shared order.
                let editor = Member::Audio(Box::new(editor));
                let member = if request["sharedHistory"] == true {
                    editing.join(&key, editor)
                } else {
                    editing.open_independent(editor)
                };
                if let Ok(server) =
                    serde_json::from_value::<ServerInstance>(get(&request, "server"))
                {
                    editing.set_server(member, server);
                }
                json!({"member": member, "structure": editing.structure(member)}).to_string()
            }
            Err(error) => json!({ "error": error }).to_string(),
        },
        "openNotes" => {
            let request = request.to_string();
            let editor = notes::new_json(notes::shared_of(&request), &request);
            joined(editing, &key, Member::Notes(Box::new(editor)))
        }
        "openPoints" => {
            let curve = editing
                .shared_curve(&key)
                .unwrap_or_else(|| crate::points::shared_of(&request));
            let editor = points::new_json(curve, &request.to_string());
            joined(editing, &key, Member::Points(Box::new(editor)))
        }
        "operation" => editing.audio_operation_json(&request).to_string(),
        "cleanup" => {
            let value = match request["action"].as_str().unwrap_or_default() {
                "prepare" => serde_json::from_value::<ResourceId>(get(&request, "resource"))
                    .ok()
                    .and_then(|id| editing.services.resources.begin_release(id))
                    .map(|release| serde_json::to_value(release).unwrap())
                    .unwrap_or_else(|| json!({"accepted":false})),
                "complete" => {
                    let accepted = serde_json::from_value::<
                        crate::application::resources::ReleaseTicket,
                    >(get(&request, "ticket"))
                    .is_ok_and(|ticket| {
                        editing.services.resources.finish_release(
                            ticket,
                            if request["success"] == true {
                                Ok(())
                            } else {
                                Err(request["reason"]
                                    .as_str()
                                    .unwrap_or("cleanup was refused")
                                    .to_string())
                            },
                        )
                    });
                    json!({"accepted":accepted})
                }
                "pending" => {
                    json!({"tickets":editing.services.resources.pending_releases().into_iter().map(|ticket|json!({"ticket":ticket,"reason":editing.services.resources.release_error(ticket)})).collect::<Vec<_>>()})
                }
                _ => json!({"error":"unknown cleanup action"}),
            };
            value.to_string()
        }
        "exchange" => editing.exchange_json(&request).to_string(),
        "resources" => {
            editing.sync_resources();
            serde_json::to_string(&editing.services.resources.sources()).unwrap()
        }
        "retainMember" => {
            editing.sync_resources();
            let retained = if let Some(seat) = editing.seats.get(member as usize)
                && let Member::Audio(editor) = &seat.member
                && let Ok(kind) = serde_json::from_value::<RootKind>(get(&request, "kind"))
                && !matches!(kind, RootKind::Contents | RootKind::History)
            {
                editing.services.resources.retain(
                    Root {
                        kind,
                        owner: u64::from(member),
                    },
                    editor.rooted().into_iter().map(|source| ResourceId {
                        server: seat.server,
                        buffer: source.0 as i64,
                    }),
                )
            } else {
                false
            };
            let (freed, stored) = editing.release();
            json!({"retained": retained, "freed": freed, "stored": stored}).to_string()
        }
        "retain" => {
            editing.sync_resources();
            let root = serde_json::from_value::<Root>(get(&request, "root"));
            let sources = serde_json::from_value::<Vec<ResourceId>>(get(&request, "sources"));
            let retained = match (root, sources) {
                (Ok(root), Ok(sources)) => editing.services.resources.retain(root, sources),
                _ => false,
            };
            json!({"retained": retained}).to_string()
        }
        "dropRoot" => {
            if let Ok(root) = serde_json::from_value::<Root>(get(&request, "root")) {
                editing.services.resources.drop_root(root);
            }
            let (freed, stored) = editing.release();
            json!({"freed": freed, "stored": stored}).to_string()
        }
        "invalidate" => {
            let invalidated = serde_json::from_value::<ServerInstance>(get(&request, "server"))
                .map(|server| editing.invalidate_server(server))
                .unwrap_or_default();
            json!({"invalidated": invalidated}).to_string()
        }
        "bytes" => {
            editing.set_bytes(get(&request, "bytes").as_u64());
            "{}".into()
        }
        "resident" => {
            editing.set_resident(get(&request, "bytes").as_u64());
            "{}".into()
        }
        "external" => {
            let domain = get(&request, "domain")
                .as_str()
                .unwrap_or_default()
                .to_string();
            joined(editing, &key, Member::External { domain })
        }
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            let prepared = editing.prepare_audio_event(member, &event);
            to_json(&prepared.or_else(|| editing.event(member, &event)))
        }
        "act" => {
            let call = get(&request, "call");
            let prepared = editing.prepare_audio_action(member, &call);
            to_json(&prepared.or_else(|| editing.act(member, &call)))
        }
        "step" => {
            let target = match request.get("member") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let target = value
                        .as_u64()
                        .and_then(|id| MemberId::try_from(id).ok())
                        .filter(|id| editing.member(*id).is_some());
                    let Some(target) = target else {
                        return to_json(&Stepped {
                            stepped: false,
                            operation: None,
                            reason: Some("the history step member is invalid".into()),
                            effects: Vec::new(),
                            corrections: Vec::new(),
                            freed: Vec::new(),
                            stored: Vec::new(),
                            version: editing.version(),
                        });
                    };
                    Some(target)
                }
            };
            editing.select(target);
            let direction = get(&request, "direction")
                .as_str()
                .and_then(Direction::parse)
                .unwrap_or(Direction::Undo);
            let prepared = editing.prepare_audio_walk(direction);
            to_json(&prepared.unwrap_or_else(|| editing.step(direction)))
        }
        "record" => {
            let coalesce = get(&request, "coalesce").as_bool().unwrap_or(false);
            let recorded = serde_json::from_value::<Recorded>(request.clone()).ok();
            let taken = recorded.is_some_and(|r| {
                let record = Record {
                    label: r.label,
                    legs: r
                        .legs
                        .into_iter()
                        .map(|leg| crate::turn::Leg {
                            forward: leg.forward,
                            backward: leg.backward,
                            key: leg.key.unwrap_or_default(),
                            holds_forward: leg.holds_forward,
                            holds_backward: leg.holds_backward,
                        })
                        .collect(),
                };
                editing.record(member, &record, coalesce)
            });
            json!({ "recorded": taken, "version": editing.version() }).to_string()
        }
        "member" => {
            editing.select_member(member);
            editing.sync_resources();
            let before = match editing.member(member) {
                Some(Member::Audio(editor)) => Some(editor.focused_file()),
                _ => None,
            };
            let inner = get(&request, "call")
                .get("verb")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if inner == "close_file"
                && let Some(seat) = editing.seats.get(member as usize)
                && let Member::Audio(editor) = &seat.member
            {
                let file = if let Some(index) = request["call"]["index"].as_u64() {
                    editor.file_at(index as usize)
                } else {
                    Some(
                        request["call"]["file"]
                            .as_i64()
                            .unwrap_or(editor.focused_file()),
                    )
                };
                if file.is_some_and(|object| {
                    editing
                        .audio_runtime
                        .operations
                        .busy(crate::application::operations::Target {
                            member,
                            object,
                            server: seat.server,
                        })
                }) {
                    return json!({"error":"the file has pending work; close after it completes"})
                        .to_string();
                }
            }
            if inner == "add_file" && matches!(editing.member(member), Some(Member::Audio(_))) {
                return editing
                    .prepare_add_file(member, &get(&request, "call"))
                    .to_string();
            }
            if matches!(inner.as_str(), "event" | "apply") {
                return "{}".into();
            }
            let mut call = get(&request, "call");
            if let Some(map) = call.as_object_mut() {
                map.insert("version".into(), json!(editing.version));
            }
            let answer = match editing.member_mut(member) {
                Some(Member::Multitrack(editor)) => {
                    multitrack::call_json(editor, &call.to_string())
                }
                Some(Member::Audio(editor)) => audio::call_json(editor, &call.to_string()),
                Some(Member::Notes(editor)) => notes::call_json(editor, &call.to_string()),
                Some(Member::Points(editor)) => points::call_json(editor, &call.to_string()),
                #[cfg(feature = "notation")]
                Some(Member::Score(editor)) => score::call_json(editor, &call.to_string()),
                _ => "{}".into(),
            };
            editing.select_member(member);
            let after = match editing.member(member) {
                Some(Member::Audio(editor)) => Some(editor.focused_file()),
                _ => None,
            };
            if before != after {
                editing.version += 1;
            }
            let mut value: Value = serde_json::from_str(&answer).unwrap_or_default();
            if inner == "window" {
                value["actions"] = editing.action_states(member);
            }
            if value.get("binding").is_some() {
                value["binding"]["structure"] = json!(editing.structure(member));
                value["binding"]["version"] = json!(editing.version);
            }
            // The switch moved the version, and the answer is how the host
            // learns it: its next gesture names this one.
            if let Some(answer) = value.get_mut("answer").filter(|a| a.is_object()) {
                answer["docVersion"] = json!(editing.version);
            }
            if inner == "close_file"
                && value.get("error").is_none()
                && let Some(file) = value["fileAction"]["file"].as_i64()
            {
                editing.close_audio_file(member, file);
                editing.sync_resources();
                let server = editing.seats[member as usize].server;
                let protected: Vec<i64> = value["freedBuffers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .filter(|&buffer| {
                        editing
                            .services
                            .resources
                            .retained(ResourceId { server, buffer })
                    })
                    .collect();
                if let Some(buffers) = value["freedBuffers"].as_array_mut() {
                    buffers.retain(|buffer| !protected.contains(&buffer.as_i64().unwrap_or(-1)));
                    for buffer in buffers.iter().filter_map(Value::as_i64) {
                        let id = ResourceId { server, buffer };
                        if editing.services.resources.source(id).is_none() {
                            editing.services.resources.register(Source {
                                id,
                                member,
                                bytes: 0,
                                owned: true,
                                stored: None,
                            });
                        }
                        editing.services.resources.released(id);
                    }
                }
                value["steps"] = json!([]);
                if value["close"] != true {
                    editing.select_member(member);
                }
            }
            value.to_string()
        }
        "moved" => json!({ "version": editing.moved() }).to_string(),
        "actions" => match request
            .get("member")
            .and_then(Value::as_u64)
            .and_then(|id| MemberId::try_from(id).ok())
            .filter(|id| editing.member(*id).is_some())
        {
            Some(member) => editing.action_states(member).to_string(),
            None => json!({"error": "the action member is invalid"}).to_string(),
        },
        "state" => {
            editing.select(
                request
                    .get("member")
                    .and_then(Value::as_u64)
                    .map(|m| m as MemberId),
            );
            json!({
            "version": editing.version(),
            "canUndo": editing.can_undo(),
            "canRedo": editing.can_redo(),
            "undoLabel": editing.undo_label(),
            "redoLabel": editing.redo_label(),
            })
            .to_string()
        }
        _ => "{}".into(),
    }
}

impl Editing {
    /// **Opens a notes editor over `sequence`**, shared with the caller -- the
    /// door a binding's sequence handle opens through, so the editor edits the
    /// very sequence the script holds. `request` is what
    /// [`notes::new_json`] reads; the answer is the `openNotes` one.
    pub fn open_notes(&mut self, key: &str, sequence: Shared, request: &str) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        let editor = notes::new_json(sequence, &request.to_string());
        joined(self, key, Member::Notes(Box::new(editor)))
    }
}

#[cfg(feature = "notation")]
impl Editing {
    /// **Opens a score editor over `score`**, shared with the caller -- the
    /// door a binding's score handle opens through, so the editor edits the
    /// very score the script holds. `request` is what [`score::new_json`]
    /// reads; the answer is the `openNotes` one's shape.
    pub fn open_score(&mut self, key: &str, shared: crate::score::Shared, request: &str) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        let editor = score::new_json(shared, &request.to_string());
        joined(self, key, Member::Score(Box::new(editor)))
    }

    /// **Opens a score editor over `sequence`**, on the page it is read
    /// into: `shared` is the score the caller's handle holds, loaded here
    /// with the reading, and `sequence` the one a roll over it shares. `key`
    /// is the sequence's, so the editor's entries are the sequence's and one
    /// order with every roll over it. `request` is what [`score::over_json`]
    /// reads; the answer is the `openNotes` one's shape, or `{"error"}` when
    /// the sequence cannot be read.
    pub fn open_score_over(
        &mut self,
        key: &str,
        shared: crate::score::Shared,
        sequence: Shared,
        request: &str,
    ) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        match score::over_json(shared, sequence, &request.to_string()) {
            Ok(editor) => joined(self, key, Member::Score(Box::new(editor))),
            Err(error) => json!({ "error": error }).to_string(),
        }
    }
}

impl Editing {
    /// **Opens a multitrack editor over `multitrack`**, shared with the caller:
    /// the door a binding's multitrack handle opens through, so the editor
    /// edits the very multitrack the script holds. `request` is what
    /// [`multitrack::over_json`] reads; the answer is the `openMultitrack` one.
    pub fn open_multitrack(
        &mut self,
        key: &str,
        multitrack: crate::multitrack::Shared,
        request: &str,
    ) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        match multitrack::over_json(multitrack, &request.to_string()) {
            Some(editor) => joined(self, key, Member::Multitrack(Box::new(editor))),
            None => json!({ "error": "the request does not read" }).to_string(),
        }
    }
}

impl Editing {
    /// The curve a points editor under `key` already edits, so a second
    /// window over it edits the same one.
    fn shared_curve(&self, key: &str) -> Option<crate::points::Shared> {
        let structure = self.services.histories.named(key)?;
        self.seats.iter().find_map(|seat| match &seat.member {
            Member::Points(editor) if seat.structure == structure => Some(editor.curve().clone()),
            _ => None,
        })
    }
}

impl Editing {
    /// **Binds a multitrack member's source to a sequence** shared with the
    /// caller: a region over `source` draws the sequence's notes, and a notes
    /// editor opened over the same handle edits what that region draws. The
    /// answer is the member's picture corrected with it (the `Answer` a
    /// correction is), or `{}` when `member` is not a multitrack.
    pub fn bind_sequence(
        &mut self,
        member: MemberId,
        source: SourceId,
        sequence: Shared,
    ) -> String {
        let version = self.version;
        match self.member_mut(member) {
            Some(Member::Multitrack(editor)) => {
                editor.bind_sequence(source, sequence);
                to_json(&editor.resync_all(version))
            }
            _ => "{}".into(),
        }
    }

    /// **Hands a multitrack member the engraver its boxes' pages are
    /// engraved with**: a score the caller made for that and reads no more,
    /// which the editor loads with each sequence in turn. The answer is the
    /// member's picture corrected with it, or `{}` when `member` is not a
    /// multitrack.
    #[cfg(feature = "notation")]
    pub fn bind_engraver(&mut self, member: MemberId, score: crate::score::Shared) -> String {
        let version = self.version;
        match self.member_mut(member) {
            Some(Member::Multitrack(editor)) => {
                editor.bind_engraver(score);
                to_json(&editor.resync_all(version))
            }
            _ => "{}".into(),
        }
    }

    /// [`Self::bind_sequence`] off a `{"member", "source"}` request: the one
    /// door every binding's sequence handle binds through.
    pub fn bind_sequence_json(&mut self, sequence: Shared, request: &str) -> String {
        let request = serde_json::from_str::<Value>(request).unwrap_or_default();
        let number = |key: &str| request.get(key).and_then(Value::as_u64);
        match (number("member"), number("source")) {
            (Some(member), Some(source)) => {
                self.bind_sequence(member as MemberId, SourceId(source), sequence)
            }
            _ => "{}".into(),
        }
    }
}

/// A member taken in, as the door answers it: its number and the structure it
/// is in the order.
fn joined(editing: &mut Editing, key: &str, member: Member) -> String {
    let member = editing.join(key, member);
    let structure = editing.structure(member).map(|s| s.0);
    json!({ "member": member, "structure": structure }).to_string()
}

/// A turn's answer as JSON.
fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::Window;
    use clausters_document::multitrack::{Content, Multitrack, Region, Track};
    use clausters_document::points::{Points, payload as points_payload};
    use clausters_document::{Lifetime, NodeId, Second, SourceId, SourceRef};
    use std::collections::HashMap;

    const SR: f64 = 48_000.0;

    fn region(id: u64, at: f64) -> Region {
        let mut region = Region::new(
            NodeId(id),
            Second(at),
            Second(2.0),
            Content::Unknown(Value::Null),
        );
        region.content = Content::window(Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: 2.0,
        });
        region
    }

    /// **A notes editor edits the sequence its holder shares**, and an undo
    /// through the context puts it back there.
    #[test]
    fn owned_file_buffers_stay_rooted_until_close_even_behind_another_file() {
        let mut editing = Editing::new();
        let member = call(&mut editing, json!({"verb":"openAudio", "sharedHistory":true,"key":"owned","take":3,"frames":100,"channels":1,"rate":48000,"display":9,"buffers":[20,21,22],"path":"owned.wav"}))["member"].clone();
        call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"open"}}),
        );
        let retained = call(
            &mut editing,
            json!({"verb":"retainMember","member":member,"kind":"playback"}),
        );
        assert!(retained["freed"].as_array().is_none_or(|f| f.is_empty()));
        call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"add_file","take":50,"frames":100,"display":51,"buffers":[52,53],"untitled":true}}),
        );
        let retained = call(
            &mut editing,
            json!({"verb":"retainMember","member":member,"kind":"playback"}),
        );
        assert!(retained["freed"].as_array().is_none_or(|f| f.is_empty()));
        let closing = call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"close_file","file":1}}),
        );
        assert!(
            !closing["freedBuffers"]
                .as_array()
                .unwrap()
                .contains(&json!(3)),
            "playback lease defers final release"
        );
        let released = call(
            &mut editing,
            json!({"verb":"retainMember","member":member,"kind":"playback"}),
        );
        assert!(
            released["freed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["buffers"].as_array().unwrap().contains(&json!(3)))
        );
    }

    #[test]
    fn a_notes_editor_edits_the_shared_sequence_and_undoes_through_the_context() {
        use clausters_document::EventSequence;
        use clausters_document::events::Event as SeqEvent;
        use std::sync::{Arc, Mutex};

        let sequence = Arc::new(Mutex::new(EventSequence::new(vec![SeqEvent::new(
            0.0,
            json!({"midinote": 60, "sustain": 1.0, "amp": 0.1}),
        )])));
        let mut editing = Editing::new();
        let opened: Value = serde_json::from_str(&editing.open_notes(
            "seq",
            sequence.clone(),
            r#"{"rate": 100.0}"#,
        ))
        .unwrap();
        let member = opened["member"].as_u64().unwrap() as MemberId;
        call_json(
            &mut editing,
            &json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": 40}})
                .to_string(),
        );
        let moved = vec![
            json!(1),
            json!(50.0),
            json!(100.0),
            json!(67.0),
            json!(13.0),
            json!(0.0),
        ];
        let turned = editing
            .event(member, &event(40, 1, "notes", moved))
            .unwrap();
        assert!(turned.outcome.changed());
        assert_eq!(
            sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
            json!(67.0)
        );
        let stepped = editing.step(Direction::Undo);
        assert!(stepped.stepped);
        assert!(matches!(
            stepped.effects[0],
            Effect::Notes { applied: true, .. }
        ));
        assert_eq!(sequence.lock().unwrap().get(1).unwrap().at.0, 0.0);
        assert_eq!(
            sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
            json!(60)
        );
        assert!(editing.step(Direction::Redo).stepped);
        assert_eq!(
            sequence.lock().unwrap().get(1).unwrap().data.0["midinote"],
            json!(67.0)
        );
    }

    /// **A region over a bound sequence draws what a notes editor edits**: the
    /// binding answers the multitrack's picture with the note in box 12, and an
    /// edit in the roll opened on the same handle corrects that picture.
    /// An engraver that keeps the document it is handed and draws nothing.
    #[cfg(feature = "notation")]
    struct Kept(std::sync::Mutex<String>);

    #[cfg(feature = "notation")]
    impl clausters_core::notation::Engraver for Kept {
        type Guard = ();
        fn lock(&self) -> Self::Guard {}
        fn load_data(&self, data: &str) -> bool {
            *self.0.lock().unwrap() = data.to_string();
            !data.is_empty()
        }
        fn render_svg(&self, _page: i32) -> String {
            String::new()
        }
        fn mei(&self) -> String {
            self.0.lock().unwrap().clone()
        }
        fn edit(&self, _action: &str) -> bool {
            false
        }
        fn timemap(&self, _options: &str) -> String {
            "[]".into()
        }
        fn midi_values(&self, _xml_id: &str) -> Option<String> {
            None
        }
        fn set_options(&self, _options: &str) -> bool {
            true
        }
    }

    /// A box of notes is drawn as its page where the window says so and an
    /// engraver was handed over: the page, and the time of each of its notes
    /// in the box's own frames.
    #[cfg(feature = "notation")]
    #[test]
    fn a_box_of_notes_is_stated_as_its_page_once_an_engraver_is_bound() {
        use clausters_core::notation::{AnyEngraver, Score, sheet_to_mei, voice_to_sheet};
        use clausters_document::EventSequence;
        use clausters_document::events::Event as SeqEvent;
        use std::sync::{Arc, Mutex};

        let sequence = Arc::new(Mutex::new(EventSequence::new(vec![
            SeqEvent::new(0.0, json!({"midinote": 60, "dur": 1.0})),
            SeqEvent::new(1.0, json!({"midinote": 64, "dur": 1.0})),
        ])));
        let mut editing = Editing::new();
        let multitrack = editing.join("multitrack", a_multitrack());
        editing.bind_sequence(multitrack, SourceId(1), sequence.clone());
        let picture = |editing: &mut Editing| match editing.member_mut(multitrack) {
            Some(Member::Multitrack(editor)) => editor.picture(40),
            _ => panic!("a multitrack editor"),
        };
        // as rolls, and with nothing to engrave with, there is no page
        assert_eq!(picture(&mut editing)["notes_view"], "roll");
        assert!(!picture(&mut editing).contains_key("scores"));

        let blank = sheet_to_mei(&voice_to_sheet(&[], "4/4", "G2", "C")).unwrap();
        let engraver = AnyEngraver::new(Kept(Mutex::new(String::new())));
        let score = Arc::new(Mutex::new(Score::open(engraver, &blank).expect("opens")));
        editing.bind_engraver(multitrack, score.clone());
        assert!(
            !picture(&mut editing).contains_key("scores"),
            "still as rolls"
        );
        match editing.member_mut(multitrack) {
            Some(Member::Multitrack(editor)) => assert!(editor.set_notes_view("score")),
            _ => panic!("a multitrack editor"),
        }
        let props = picture(&mut editing);
        assert_eq!(props["notes_view"], "score");
        let pages = props["scores"].as_object().expect("the pages");
        let (_, page) = pages.iter().next().expect("the box's page");
        // the second note stands a beat in: a second, in the axis's frames
        let anchors = page["anchors"].as_array().unwrap();
        assert_eq!(anchors[0], "n1");
        assert_eq!(anchors[1], 0.0);
        assert_eq!(anchors[4], "n2");
        assert_eq!(anchors[5], SR);
        // what was engraved is the sequence, read
        assert!(score.lock().unwrap().mei().contains("pname=\"e\""));
        // and a note moved is the page engraved again
        sequence.lock().unwrap().events[1].at.0 = 2.0;
        let again = picture(&mut editing);
        let (_, page) = again["scores"].as_object().unwrap().iter().next().unwrap();
        assert_eq!(page["anchors"][5], 2.0 * SR);
    }

    #[test]
    fn a_region_over_a_bound_sequence_follows_the_roll() {
        use clausters_document::EventSequence;
        use clausters_document::events::Event as SeqEvent;
        use std::sync::{Arc, Mutex};

        let sequence = Arc::new(Mutex::new(EventSequence::new(vec![SeqEvent::new(
            0.0,
            json!({"midinote": 60, "sustain": 1.0}),
        )])));
        let mut editing = Editing::new();
        let multitrack = editing.join("multitrack", a_multitrack());
        let bound: Value =
            serde_json::from_str(&editing.bind_sequence(multitrack, SourceId(1), sequence.clone()))
                .unwrap();
        let text = bound.to_string();
        assert!(
            text.contains("notes"),
            "the picture carries the notes: {text}"
        );

        let opened: Value = serde_json::from_str(&editing.open_notes(
            "seq",
            sequence.clone(),
            r#"{"rate": 100.0}"#,
        ))
        .unwrap();
        let notes = opened["member"].as_u64().unwrap() as MemberId;
        call_json(
            &mut editing,
            &json!({"verb": "member", "member": notes, "call": {"verb": "window", "widget": 60}})
                .to_string(),
        );
        let moved = vec![
            json!(1),
            json!(0.0),
            json!(100.0),
            json!(67.0),
            json!(100.0),
            json!(0.0),
        ];
        let turned = editing.event(notes, &event(60, 1, "notes", moved)).unwrap();
        let corrected = turned
            .corrections
            .iter()
            .find(|c| c.member == multitrack)
            .expect("the multitrack is corrected");
        let text = serde_json::to_string(&corrected.answer).unwrap();
        assert!(text.contains("67"), "box 12 draws the moved note: {text}");

        // A double click on box 12 asks for the roll over source 1.
        let opened = editing
            .event(multitrack, &event(40, 2, "open", vec![json!("12")]))
            .unwrap();
        let Outcome::Multitrack(outcome) = &opened.outcome else {
            panic!("the multitrack's outcome");
        };
        assert_eq!(outcome.open, Some(1));
        assert!(!opened.outcome.changed(), "and nothing is edited");

        // A member that is not a multitrack binds nothing.
        assert_eq!(editing.bind_sequence(notes, SourceId(1), sequence), "{}");
    }

    /// A multitrack of one track holding box 12, drawn by widget 40 in window 39.
    fn a_multitrack() -> Member {
        let mut track = Track::new(NodeId(10), NodeId(11));
        track.take_lanes[0].regions = vec![region(12, 0.0)];
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let mut editor = MultitrackEditor::new(multitrack, SR, FIRST_VERSION);
        editor.set_sources(HashMap::from([(SourceId(1), 7)]));
        editor.chrome(
            None,
            crate::multitrack::Transport::Unnumbered,
            "multitrack",
            (1000, 560),
        );
        editor.window(40, 41);
        editor.set_window(Some(39));
        Member::Multitrack(Box::new(editor))
    }

    /// **A join no region reads and no entry can put back is handed back to
    /// free**, under the multitrack that minted it: an undo keeps it for the
    /// redo, and the edit after the undo lets it go.
    #[test]
    fn a_join_undone_and_overtaken_is_freed() {
        let half = |id: u64, at: f64, start: f64| {
            let mut region = region(id, at);
            region.length = Second(1.0);
            region.content = Content::window(Window {
                source: SourceRef {
                    source: SourceId(1),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                start,
                duration: 1.0,
            });
            region
        };
        // The take's halves swapped: no window onto the take reads them in
        // this order, so the join mints a source.
        let mut track = Track::new(NodeId(10), NodeId(11));
        track.take_lanes[0].regions = vec![half(12, 0.0, 1.0), half(13, 1.0, 0.0)];
        let mut editor = MultitrackEditor::new(
            Multitrack {
                tracks: vec![track],
                ..Multitrack::default()
            },
            SR,
            FIRST_VERSION,
        );
        editor.set_sources(HashMap::from([(SourceId(1), 7)]));
        editor.chrome(
            None,
            crate::multitrack::Transport::Unnumbered,
            "multitrack",
            (1000, 560),
        );
        editor.window(40, 41);
        editor.set_window(Some(39));
        let mut editing = Editing::default();
        let member = editing.join("multitrack", Member::Multitrack(Box::new(editor)));

        let joined = editing
            .event(
                member,
                &event(40, 1, "join", vec![json!("12"), json!("13")]),
            )
            .unwrap();
        let Outcome::Multitrack(outcome) = &joined.outcome else {
            panic!("a multitrack's outcome");
        };
        let join = SourceId(outcome.minted[0]["id"].as_u64().unwrap());
        assert!(joined.freed.is_empty(), "the joined box reads it");

        let undone = editing.step(Direction::Undo);
        assert!(undone.freed.is_empty(), "a redo can still name it");

        // The two boxes are back as they were; the first moves to 3 s.
        let boxes = vec![
            json!("12"),
            json!("10"),
            json!(3.0 * SR),
            json!(SR),
            json!(SR),
            json!(""),
            json!(7),
            json!("13"),
            json!("10"),
            json!(SR),
            json!(SR),
            json!(0.0),
            json!(""),
            json!(7),
        ];
        let overtaken = editing
            .event(member, &event(40, 2, "clips", boxes))
            .unwrap();
        assert!(overtaken.outcome.changed());
        assert_eq!(
            overtaken.freed,
            [Freed {
                member,
                buffers: Vec::new(),
                spilled: Vec::new(),
                sources: vec![join.0],
            }],
            "the edit after the undo dropped the only entry that named it"
        );
        let Some(Member::Multitrack(editor)) = editing.member_mut(member) else {
            panic!("the multitrack is still there");
        };
        assert!(!editor.is_join(join), "and the editor forgot it");
    }

    /// A mono take in buffer 7, a hundred frames, edited by an audio editor
    /// drawn by widget 50 in window 49, with buffers for new takes.
    fn a_take() -> Member {
        let mut editor = audio::new_json(
            r#"{"take": 7, "frames": 100, "display": 9, "buffers": [20, 21, 22, 23],
                "version": 1}"#,
        )
        .unwrap();
        audio::call_json(&mut editor, r#"{"verb": "window", "widget": 50}"#);
        audio::call_json(&mut editor, r#"{"verb": "sync", "window": 49}"#);
        Member::Audio(Box::new(editor))
    }

    fn event(widget: i64, seq: i64, tag: &str, values: Vec<Value>) -> Event {
        let mut args = vec![json!(widget), json!(seq), json!(0), json!(tag)];
        args.extend(values);
        Event {
            addr: "/gui_event".into(),
            args,
        }
    }

    fn moved_to(at: f64) -> Vec<Value> {
        vec![
            json!("12"),
            json!("10"),
            json!(at * SR),
            json!(2.0 * SR),
            json!(0.0),
            json!(""),
            json!(7),
        ]
    }

    fn stroke(value: f64, was: f64) -> Vec<Value> {
        vec![json!(0), json!(2), json!([value]), json!([was])]
    }

    fn position(editing: &mut Editing, member: MemberId) -> f64 {
        match editing.member_mut(member) {
            Some(Member::Multitrack(editor)) => {
                editor.multitrack().tracks[0].take_lanes[0].regions[0]
                    .position
                    .0
            }
            _ => f64::NAN,
        }
    }

    /// The takes an audio member's list reads now, in order.
    fn reads(editing: &Editing, member: MemberId) -> Vec<u64> {
        match editing.member(member) {
            Some(Member::Audio(editor)) => editor
                .list()
                .iter()
                .map(|part| part.source.source.0)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// How many audio members a step stitched again.
    fn stitched(stepped: &Stepped) -> usize {
        stepped
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::Audio { .. }))
            .count()
    }

    /// **An editor open alone is a context of one**: its stroke is recorded
    /// there, and an undo hands back the list that puts the take back.
    #[test]
    fn an_editor_open_alone_undoes_through_a_context_of_one() {
        let mut editing = Editing::default();
        let take = editing.join("buffer:7", a_take());
        let turned = editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        assert!(turned.outcome.changed());
        assert_eq!(turned.version, 2);
        assert!(editing.can_undo());
        assert_eq!(editing.undo_label().as_deref(), Some("draw the samples"));

        assert_eq!(reads(&editing, take), [7, 20, 7]);
        let undone = editing.step(Direction::Undo);
        assert!(undone.stepped);
        assert_eq!(undone.version, 3);
        assert_eq!(stitched(&undone), 1);
        assert_eq!(reads(&editing, take), [7]);
        assert_eq!(
            undone.corrections,
            [Corrected {
                member: take,
                answer: Answer::Push {
                    seq: 0,
                    doc_version: 3,
                    reason: None,
                    corrections: vec![clausters_editing::conversation::Correction {
                        widget: 50,
                        props: json!({"reload": 1}),
                    }],
                },
            }],
            "the window reads the take again"
        );
        editing.step(Direction::Redo);
        assert_eq!(reads(&editing, take), [7, 20, 7]);
    }

    /// **Two applications in one context walk one order**: a box moved, a
    /// stroke drawn, a box moved again, and undo walks the three back in that
    /// order, each handed to its own member.
    #[test]
    fn two_applications_in_one_context_walk_one_order() {
        let mut editing = Editing::default();
        let multitrack = editing.join("multitrack", a_multitrack());
        let take = editing.join("buffer:7", a_take());

        let first = editing
            .event(multitrack, &event(40, 1, "clips", moved_to(2.0)))
            .unwrap();
        assert!(first.outcome.changed());
        assert!(
            first.corrections.iter().any(|c| c.member == take),
            "the other window is corrected"
        );
        editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        editing
            .event(multitrack, &event(40, 2, "clips", moved_to(4.0)))
            .unwrap();
        assert_eq!(position(&mut editing, multitrack), 4.0);

        let back = editing.step(Direction::Undo);
        assert!(matches!(back.effects[..], [Effect::Multitrack { .. }]));
        assert_eq!(position(&mut editing, multitrack), 2.0);

        let back = editing.step(Direction::Undo);
        assert_eq!(stitched(&back), 1, "the stroke, in between");
        assert_eq!(reads(&editing, take), [7]);
        assert_eq!(position(&mut editing, multitrack), 2.0);

        editing.step(Direction::Undo);
        assert_eq!(position(&mut editing, multitrack), 0.0);
        assert!(!editing.can_undo());

        editing.step(Direction::Redo);
        editing.step(Direction::Redo);
        assert_eq!(reads(&editing, take), [7, 20, 7]);
    }

    /// **Two windows over one take are one structure**: a stroke in one is
    /// one entry, undone once whichever asks, and each window's join is
    /// stitched again over the list the step handed back.
    #[test]
    fn two_windows_over_one_take_are_one_structure() {
        let mut editing = Editing::default();
        let left = editing.join("buffer:7", a_take());
        let right = editing.join("buffer:7", a_take());
        editing
            .event(left, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        let undone = editing.step(Direction::Undo);
        assert!(undone.stepped);
        assert_eq!(stitched(&undone), 2, "each window draws its own join");
        assert_eq!(
            (reads(&editing, left), reads(&editing, right)),
            (vec![7], vec![7])
        );
        assert!(!editing.can_undo(), "one entry");
        assert!(undone.corrections.iter().any(|c| c.member == right));
    }

    /// **An external member's legs are handed back**, in the same order as the
    /// rest.
    #[test]
    fn an_external_members_legs_are_handed_back() {
        let mut editing = Editing::default();
        let curve = editing.join(
            "curve",
            Member::External {
                domain: "points".into(),
            },
        );
        let take = editing.join("buffer:7", a_take());
        let record = Record {
            label: "draw the curve".into(),
            legs: vec![crate::turn::Leg {
                forward: json!({"edit": {"points": [1.0]}}),
                backward: json!({"points": [0.0]}),
                key: String::new(),
                ..Default::default()
            }],
        };
        assert!(editing.record(curve, &record, false));
        assert_eq!(editing.version(), 2);
        editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();

        editing.step(Direction::Undo);
        let back = editing.step(Direction::Undo);
        assert_eq!(
            back.effects,
            [Effect::External {
                member: curve,
                payloads: vec![json!({"points": [0.0]})],
            }]
        );
    }

    /// **An entry an embedder built** is recorded under its structure, moves the
    /// version, and walks back like any other.
    #[test]
    fn an_entry_built_in_rust_joins_the_order() {
        let mut editing = Editing::default();
        let curve = editing.join(
            "curve",
            Member::External {
                domain: "points".into(),
            },
        );
        let structure = editing.structure(curve).unwrap();
        assert!(editing.member(curve).is_some());
        let entry = Entry::new(
            "bend",
            structure,
            Step::Edit(Opaque(json!({"points": [1.0]}))),
            Opaque(json!({"points": [0.0]})),
        );
        assert!(editing.record_entry(entry));
        assert_eq!(editing.version(), 2);
        let back = editing.step(Direction::Undo);
        assert_eq!(
            back.effects,
            [Effect::External {
                member: curve,
                payloads: vec![json!({"points": [0.0]})]
            }]
        );
    }

    /// **A vocabulary that applies and records in one call** does it through
    /// the context's history, and the version moves only when it recorded.
    #[test]
    fn an_edit_recorded_through_a_door_of_its_own_moves_the_version() {
        let mut editing = Editing::default();
        let tree = editing.join(
            "tree",
            Member::External {
                domain: "tree".into(),
            },
        );
        assert!(!editing.record_with(tree, |_, _| false));
        assert_eq!(
            editing.version(),
            FIRST_VERSION,
            "nothing recorded, nothing moved"
        );
        assert!(editing.record_with(tree, |history, structure| {
            history.record(Entry::new(
                "move",
                structure,
                Step::Edit(Opaque(json!({"x": 1}))),
                Opaque(json!({"x": 0})),
            ))
        }));
        assert_eq!(editing.version(), 2);
        assert!(editing.can_undo());
    }

    /// **A step nothing could apply is not a step**: the cursor goes back, the
    /// entry is still on top, and the reason names it.
    #[test]
    fn a_step_nothing_could_apply_puts_the_cursor_back() {
        let mut editing = Editing::default();
        let multitrack = editing.join("multitrack", a_multitrack());
        let record = Record {
            label: "move nothing".into(),
            legs: vec![crate::turn::Leg {
                forward: json!({"edit": {"nonsense": true}}),
                backward: json!({"nonsense": true}),
                key: String::new(),
                ..Default::default()
            }],
        };
        editing.record(multitrack, &record, false);
        let version = editing.version();
        let refused = editing.step(Direction::Undo);
        assert!(!refused.stepped);
        assert!(
            refused
                .reason
                .as_deref()
                .unwrap()
                .starts_with("move nothing")
        );
        assert!(editing.can_undo(), "still there");
        assert_eq!(editing.version(), version);
    }

    /// **A step is whole across structures**: one whose second structure
    /// refuses puts back the first, which it had already stepped.
    #[test]
    fn a_step_refused_on_one_structure_puts_back_the_others() {
        let mut editing = Editing::default();
        let multitrack = editing.join("multitrack", a_multitrack());
        let curve = call(
            &mut editing,
            json!({"verb": "openPoints", "key": "curve", "rate": 100.0,
                   "points": [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0]}),
        )["member"]
            .clone();
        let curve = int(&curve) as MemberId;
        let held = |editing: &Editing| match &editing.seats[curve as usize].member {
            Member::Points(editor) => editor.curve().lock().unwrap().points.clone(),
            _ => unreachable!(),
        };
        let before = held(&editing);
        let mut bent = Points(before.clone());
        bent.0[1].value = 0.5;
        let entry = Entry::new(
            "both",
            editing.structure(multitrack).unwrap(),
            Step::Edit(Opaque(json!({"edit": {"nonsense": true}}))),
            Opaque(json!({"nonsense": true})),
        )
        .and(
            editing.structure(curve).unwrap(),
            Step::Edit(points_payload(&Points(before.clone()).state())),
            points_payload(&bent.state()),
        );
        assert!(editing.record_entry(entry));
        let version = editing.version();
        let refused = editing.step(Direction::Undo);
        assert!(!refused.stepped);
        assert!(refused.effects.is_empty());
        assert_eq!(held(&editing), before, "the curve is put back");
        assert!(editing.can_undo(), "still there");
        assert_eq!(editing.version(), version);
    }

    /// **An undo asked of a window is the context's step**, answered with the
    /// acknowledgement of the key that asked.
    #[test]
    fn an_undo_from_a_window_steps_the_context() {
        let mut editing = Editing::default();
        let take = editing.join("buffer:7", a_take());
        editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        let undo = Event {
            addr: "/gui_event".into(),
            args: vec![json!(49), json!(2), json!(0), json!("undo")],
        };
        let turned = editing.event(take, &undo).unwrap();
        let stepped = turned.stepped.expect("a step");
        assert!(stepped.stepped);
        let Outcome::Audio(outcome) = &turned.outcome else {
            panic!()
        };
        assert!(outcome.answer.is_none(), "the receipt waits for I/O");
        let prepared = serde_json::to_value(stepped.operation.unwrap()).unwrap();
        let (completed, _) = settle_operation(&mut editing, prepared);
        assert_eq!(completed["echo"]["answer"]["seq"], 2);
        assert_eq!(completed["echo"]["answer"]["docVersion"], 3);
    }

    /// The JSON door opens members, turns, steps and reports the state.
    #[test]
    fn failed_add_file_keeps_the_open_list_and_returns_only_settled_ids() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        let opening = raw_call(
            &mut editing,
            json!({"verb":"member","member":member,
            "call":{"verb":"add_file","take":50,"frames":100,"display":51,"buffers":[52,53],"untitled":true}}),
        );
        assert!(opening["operation"].is_object());
        let files = raw_call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"files"}}),
        );
        assert_eq!(files["files"].as_array().unwrap().len(), 1);
        let prepared = &opening["operation"];
        let failed = raw_call(
            &mut editing,
            json!({"verb":"operation","action":"complete",
            "ticket":prepared["ticket"],"batch":prepared["batch"],"result":{"status":"failed","reason":"allocation refused"}}),
        );
        assert!(failed["repair"].is_object());
        assert!(
            failed.get("returned").is_none(),
            "ids remain reserved during recovery"
        );
        let repaired = raw_call(
            &mut editing,
            json!({"verb":"operation","action":"repaired","ticket":failed["repair"]}),
        );
        assert_eq!(repaired["returned"]["buffers"], json!([51, 53]));
        let freed = repaired["freed"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|entry| entry["buffers"].as_array().unwrap())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(freed, [json!(50), json!(52)]);
        assert_eq!(
            raw_call(
                &mut editing,
                json!({"verb":"member","member":member,"call":{"verb":"files"}})
            )["files"],
            files["files"]
        );
        assert_eq!(
            raw_call(
                &mut editing,
                json!({"verb":"operation","action":"repaired","ticket":failed["repair"]})
            )["accepted"],
            false
        );
    }

    #[test]
    fn window_save_keeps_its_destination_and_dirty_checkpoint_until_confirmation() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        let opening = raw_call(
            &mut editing,
            json!({"verb":"operation","action":"prepare","member":member,"command":{"kind":"open"}}),
        );
        settle_operation(&mut editing, opening);
        draw(&mut editing, member, 1, 10);
        let window = 49;
        raw_call(
            &mut editing,
            json!({"verb":"member","member":member,"call":{"verb":"sync","window":window}}),
        );
        let version = editing.version();
        let pending = raw_call(
            &mut editing,
            json!({"verb":"event","member":member,"addr":"/gui_event",
            "args":[window,2,version,"save"]}),
        );
        let prepared = pending["outcome"]["operation"].clone();
        assert!(
            prepared.is_object(),
            "window save uses prepared batches: {pending}"
        );
        assert_eq!(
            raw_call(
                &mut editing,
                json!({"verb":"member","member":member,"call":{"verb":"binding"}})
            )["dirty"],
            true
        );
        let (saved, _) = settle_operation(&mut editing, prepared);
        assert_eq!(saved["committed"], true);
        assert_eq!(
            raw_call(
                &mut editing,
                json!({"verb":"member","member":member,"call":{"verb":"binding"}})
            )["dirty"],
            false
        );
    }

    #[test]
    fn saving_one_audio_window_does_not_acknowledge_in_another_context() {
        let mut origin = Editing::new();
        let mut other = Editing::new();
        let member = an_audio_take(&mut origin);
        let unrelated = an_audio_take(&mut other);
        for (context, member, window) in [(&mut origin, member, 49), (&mut other, unrelated, 99)] {
            let opening = raw_call(
                context,
                json!({
                    "verb":"operation", "action":"prepare", "member":member,
                    "command":{"kind":"open"}
                }),
            );
            settle_operation(context, opening);
            raw_call(
                context,
                json!({"verb":"member", "member":member,
                "call":{"verb":"sync", "window":window}}),
            );
            draw(context, member, 1, 10);
        }
        for args in [json!([49, 2, 0, "save"]), json!([49, 3, 0, "menu", "save"])] {
            let wrong = raw_call(
                &mut other,
                json!({"verb":"event", "member":unrelated,
                "addr":"/gui_event", "args":args}),
            );
            assert!(
                wrong["outcome"]["answer"].is_null(),
                "foreign window must not acknowledge: {wrong}"
            );
            assert!(wrong["outcome"]["operation"].is_null());
            let own = raw_call(
                &mut origin,
                json!({"verb":"event", "member":member,
                "addr":"/gui_event", "args":args}),
            );
            assert!(own["outcome"]["operation"].is_object(), "{own}");
            let (saved, _) = settle_operation(&mut origin, own["outcome"]["operation"].clone());
            assert_eq!(saved["committed"], true);
            assert_eq!(
                raw_call(
                    &mut other,
                    json!({"verb":"member", "member":unrelated,
                "call":{"verb":"binding"}})
                )["dirty"],
                true
            );
        }
    }

    #[test]
    fn the_door_opens_turns_and_steps() {
        let mut editing = Editing::default();
        let take = call(
            &mut editing,
            json!({"verb": "openAudio", "sharedHistory": true, "key": "buffer:7", "take": 7, "frames": 100,
                   "display": 9, "buffers": [20]}),
        )["member"]
            .clone();
        call(
            &mut editing,
            json!({"verb": "member", "member": take, "call": {"verb": "window", "widget": 50}}),
        );
        let turned = call(
            &mut editing,
            json!({"verb": "event", "member": take, "addr": "/gui_event",
                   "args": [50, 1, 0, "draw", 0, 2, [0.5], [0.0]]}),
        );
        assert_eq!(turned["version"], 2);
        assert_eq!(turned["outcome"]["record"]["label"], "draw the samples");
        let state = call(&mut editing, json!({"verb": "state"}));
        assert_eq!(
            call(&mut editing, json!({"verb": "moved"}))["version"],
            3,
            "an edit with no entry moves the version and records nothing"
        );
        assert_eq!(state["undoLabel"], "draw the samples");
        let stepped = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        assert_eq!(stepped["effects"][0]["kind"], "audio");
        assert_eq!(
            call(
                &mut editing,
                json!({"verb": "openAudio", "sharedHistory": true, "key": "x", "layers": []})
            )["error"]
                .as_str()
                .map(|e| e.contains("measures something")),
            Some(true)
        );
    }

    fn raw_call(editing: &mut Editing, request: Value) -> Value {
        serde_json::from_str(&call_json(editing, &request.to_string())).unwrap()
    }

    // A headless executor: acknowledge declared batches, keeping their steps
    // for ordering assertions. The document is inspected only after commit.
    fn settle_operation(editing: &mut Editing, mut prepared: Value) -> (Value, Vec<Value>) {
        let mut steps = Vec::new();
        while prepared["complete"] != true {
            steps.extend(prepared["steps"].as_array().unwrap().iter().cloned());
            prepared = raw_call(
                editing,
                json!({"verb":"operation", "action":"complete",
                "ticket":prepared["ticket"], "batch":prepared["batch"], "result":{"status":"success"}}),
            );
        }
        (prepared, steps)
    }

    fn call(editing: &mut Editing, request: Value) -> Value {
        let mut answer = raw_call(editing, request);
        if answer["outcome"]["operation"].is_object() {
            let (completed, _) = settle_operation(editing, answer["outcome"]["operation"].clone());
            for key in ["outcome", "freed", "stored", "version"] {
                if let Some(value) = completed.get(key) {
                    answer[key] = value.clone();
                }
            }
        } else if answer["operation"].is_object() {
            let (completed, steps) = settle_operation(editing, answer["operation"].clone());
            answer = if completed["outcome"].is_object() {
                completed["outcome"].clone()
            } else {
                completed["stepped"].clone()
            };
            if let Some(effects) = answer["effects"].as_array_mut() {
                for effect in effects {
                    if effect["kind"] == "audio" {
                        effect["steps"] = json!(steps);
                    }
                }
            }
            for key in ["freed", "stored", "version"] {
                if let Some(value) = completed.get(key) {
                    answer[key] = value.clone();
                }
            }
        }
        if let Some(stored) = answer["stored"].as_array_mut() {
            for entry in stored {
                if entry["operation"].is_object() {
                    let (_, steps) = settle_operation(editing, entry["operation"].clone());
                    entry["steps"] = json!(steps);
                }
            }
        }
        // Keep the original sparse encoding used by the assertions below.
        for key in ["freed", "stored"] {
            if answer[key].as_array().is_some_and(Vec::is_empty) {
                answer.as_object_mut().unwrap().remove(key);
            }
        }
        answer
    }

    /// Two windows over one curve edit one curve, and an undo in either hands
    /// the holder the points to write back once.
    #[test]
    fn two_points_editors_over_one_curve_share_it_and_one_undo() {
        let mut editing = Editing::default();
        let open = |editing: &mut Editing, widget: i64| -> Value {
            let member = call(
                editing,
                json!({"verb": "openPoints", "key": "object:1", "rate": 100.0,
                       "points": [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0]}),
            )["member"]
                .clone();
            call(
                editing,
                json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": widget}}),
            );
            member
        };
        let first = open(&mut editing, 40);
        let second = open(&mut editing, 41);
        assert_eq!(
            editing.structure(int(&first) as MemberId),
            editing.structure(int(&second) as MemberId)
        );
        let turned = call(
            &mut editing,
            json!({"verb": "event", "member": first, "addr": "/gui_event",
                   "args": [40, 1, 0, "points", 0.0, 0.0, 1, 0.0, 1.0, 0.5, 1, 0.0]}),
        );
        assert_eq!(
            turned["outcome"]["points"],
            json!([0.0, 0.0, 1.0, 0.0, 1.0, 0.5, 1.0, 0.0])
        );
        let state = call(
            &mut editing,
            json!({"verb": "member", "member": second, "call": {"verb": "state"}}),
        );
        assert_eq!(
            state["points"][5], 0.5,
            "the other window's curve is the same one"
        );
        let corrected = &turned["corrections"][0];
        assert_eq!(corrected["member"], second);
        let stepped = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        assert_eq!(stepped["effects"].as_array().unwrap().len(), 1);
        assert_eq!(stepped["effects"][0]["kind"], "points");
        assert_eq!(stepped["effects"][0]["points"][5], 1.0);
        assert_eq!(stepped["corrections"].as_array().unwrap().len(), 2);
    }

    /// An audio editor over take 3 (100 frames), drawn through join 9 by
    /// widget 12, with buffers 20..24 for new takes.
    fn an_audio_take(editing: &mut Editing) -> MemberId {
        let opened = call(
            editing,
            json!({"verb": "openAudio", "sharedHistory": true, "key": "take:3", "take": 3, "frames": 100,
                   "rate": 48000, "display": 9, "buffers": [20, 21, 22, 23]}),
        );
        let member = opened["member"].as_u64().unwrap() as MemberId;
        call(
            editing,
            json!({"verb": "member", "member": member, "call": {"verb": "window", "widget": 12}}),
        );
        member
    }

    fn draw(editing: &mut Editing, member: MemberId, seq: i64, at: i64) -> Value {
        let version = editing.version();
        call(
            editing,
            json!({"verb": "event", "member": member, "addr": "/gui_event",
                   "args": [12, seq, version, "draw", 0, at, [0.5], [0.0]]}),
        )
    }

    /// **A take the history can no longer reach is handed back to free**, and
    /// one it can is not: an undo keeps the stroke's take for the redo, and
    /// the edit after it lets it go.
    #[test]
    fn a_stroke_s_take_is_freed_when_no_entry_can_reach_it() {
        let mut editing = Editing::default();
        let take = an_audio_take(&mut editing);
        let first = draw(&mut editing, take, 1, 10);
        assert!(first.get("freed").is_none());
        let undone = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        assert_eq!(undone["effects"][0]["kind"], "audio");
        assert!(undone.get("freed").is_none(), "a redo can still find it");
        let second = draw(&mut editing, take, 2, 30);
        assert_eq!(
            second["freed"],
            json!([{"member": take, "buffers": [20]}]),
            "the redo is gone, and 20 with it"
        );
    }

    /// **The byte limit trims what only the history holds**, oldest first, and
    /// never a take the list still reads.
    #[test]
    fn the_byte_limit_frees_the_oldest_takes_only_the_history_holds() {
        let mut editing = Editing::default();
        let take = an_audio_take(&mut editing);
        // One frame of mono is four bytes. Three strokes over one frame leave
        // two takes only the history reads -- the first and the second, each
        // replaced by the next -- which is eight: the oldest entries go until
        // the second is all that is left, and the first is freed.
        call(&mut editing, json!({"verb": "bytes", "bytes": 4}));
        draw(&mut editing, take, 1, 10);
        draw(&mut editing, take, 2, 10);
        let third = draw(&mut editing, take, 3, 10);
        assert_eq!(third["freed"], json!([{"member": take, "buffers": [20]}]));
        assert_eq!(
            call(&mut editing, json!({"verb": "state"}))["canUndo"],
            true
        );
        let list = call(
            &mut editing,
            json!({"verb": "member", "member": take, "call": {"verb": "parts"}}),
        );
        let reads: Vec<u64> = list["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["source"]["source"].as_u64().unwrap())
            .collect();
        assert_eq!(reads, [3, 22, 3]);
    }

    /// **Past the resident budget the oldest take only the history holds goes
    /// to disk**, and the step that needs it reads it back before stitching.
    #[test]
    fn a_take_past_the_resident_budget_goes_to_disk_and_comes_back() {
        let mut editing = Editing::default();
        let take = an_audio_take(&mut editing);
        call(
            &mut editing,
            json!({"verb": "member", "member": take,
                   "call": {"verb": "sync", "scratch": "/scratch"}}),
        );
        // One frame is four bytes: nothing only the history holds stays in
        // memory.
        call(&mut editing, json!({"verb": "resident", "bytes": 0}));
        draw(&mut editing, take, 1, 10);
        let second = draw(&mut editing, take, 2, 10);
        assert_eq!(second["stored"][0]["buffer"], 20, "the first stroke's take");
        let steps = second["stored"][0]["steps"].as_array().unwrap();
        assert_eq!(steps[0]["send"]["addr"], "/buffer_write");
        let stored_path = steps[0]["send"]["args"][1].clone();
        assert!(
            stored_path["s"]
                .as_str()
                .unwrap()
                .starts_with("/scratch/take-")
        );
        assert!(stored_path["s"].as_str().unwrap().ends_with("-20.wav"));
        assert_eq!(steps[2]["send"]["addr"], "/buffer_free");

        // Undo the second stroke: the list reads the first one's take again,
        // so it is read back before the join is stitched.
        let undone = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        let steps = undone["effects"][0]["steps"].as_array().unwrap();
        assert_eq!(steps[0]["send"]["addr"], "/buffer_allocRead");
        assert_eq!(steps[0]["send"]["args"][1], stored_path);
        assert_eq!(steps[2]["send"]["addr"], "/buffer_stitch");
        // ...and the take the undo left behind is now the one on disk.
        assert_eq!(undone["stored"][0]["buffer"], 21);
    }

    /// **A take on disk that the history lets go of frees no buffer**: its
    /// number goes back, and there is nothing on the server to free.
    #[test]
    fn a_take_freed_from_disk_frees_no_buffer() {
        let mut editing = Editing::default();
        let take = an_audio_take(&mut editing);
        call(
            &mut editing,
            json!({"verb": "member", "member": take,
                   "call": {"verb": "sync", "scratch": "/scratch"}}),
        );
        call(&mut editing, json!({"verb": "resident", "bytes": 0}));
        call(&mut editing, json!({"verb": "bytes", "bytes": 4}));
        draw(&mut editing, take, 1, 10);
        draw(&mut editing, take, 2, 10);
        let third = draw(&mut editing, take, 3, 10);
        assert_eq!(
            third["freed"],
            json!([{"member": take, "buffers": [20], "spilled": [20]}])
        );
    }
    /// **An empty file in focus has nothing to undo**, whether the edited one
    /// was opened before it or after it: two windows, or two files of one
    /// window, never fall back to each other's order or the context's.
    #[test]
    fn an_empty_focused_file_never_undoes_another_whichever_opened_first() {
        let open = |editing: &mut Editing, n: i64| {
            let member = call(
                editing,
                json!({"verb": "openAudio", "key": format!("file:{n}"), "take": 3 + n,
                       "frames": 100, "rate": 48000, "display": 9 + n,
                       "buffers": [20 + n * 10, 21 + n * 10, 22 + n * 10]}),
            )["member"]
                .as_u64()
                .unwrap() as MemberId;
            call(
                editing,
                json!({"verb": "member", "member": member,
                       "call": {"verb": "window", "widget": 12}}),
            );
            member
        };
        for edited_first in [true, false] {
            // two windows
            let mut editing = Editing::new();
            let members = [open(&mut editing, 0), open(&mut editing, 1)];
            let (edited, empty) = if edited_first {
                (members[0], members[1])
            } else {
                (members[1], members[0])
            };
            draw(&mut editing, edited, 1, 10);
            let before = reads(&editing, edited);
            let state = call(&mut editing, json!({"verb": "actions", "member": empty}));
            assert_eq!(state["undo"]["enabled"], false, "{state}");
            assert_eq!(state["undo"]["target"]["member"], empty);
            for target in [json!(empty), Value::Null] {
                let stepped = call(
                    &mut editing,
                    json!({"verb": "step", "member": target, "direction": "undo"}),
                );
                assert_ne!(stepped["stepped"], true, "{target}: {stepped}");
            }
            assert_eq!(reads(&editing, edited), before);
            let state = call(&mut editing, json!({"verb": "actions", "member": edited}));
            assert_eq!(state["undo"]["enabled"], true);

            // two files of one window: the added one is edited or left empty
            let mut editing = Editing::new();
            let member = open(&mut editing, 0);
            call(
                &mut editing,
                json!({"verb": "member", "member": member, "call": {"verb": "add_file",
                       "take": 50, "frames": 100, "channels": 1, "rate": 48000,
                       "display": 51, "buffers": [52, 53, 54], "untitled": true}}),
            );
            let (first, second) = (0, 1);
            let (edited, empty) = if edited_first {
                (first, second)
            } else {
                (second, first)
            };
            let focus = |editing: &mut Editing, index: i64| {
                call(
                    editing,
                    json!({"verb": "member", "member": member,
                           "call": {"verb": "focus_file", "index": index}}),
                );
            };
            focus(&mut editing, edited);
            draw(&mut editing, member, 1, 10);
            let before = reads(&editing, member);
            focus(&mut editing, empty);
            let state = call(&mut editing, json!({"verb": "actions", "member": member}));
            assert_eq!(state["undo"]["enabled"], false, "{state}");
            let stepped = call(
                &mut editing,
                json!({"verb": "step", "member": member, "direction": "undo"}),
            );
            assert_ne!(stepped["stepped"], true, "{stepped}");
            focus(&mut editing, edited);
            assert_eq!(reads(&editing, member), before, "the edited file is intact");
            let state = call(&mut editing, json!({"verb": "actions", "member": member}));
            assert_eq!(state["undo"]["enabled"], true);
        }
    }

    #[test]
    fn actions_read_current_presentation_and_pending_operations() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        let state = call(&mut editing, json!({"verb":"actions", "member":member}));
        assert_eq!(state["copy"]["enabled"], false);
        assert_eq!(
            state["copy"]["unselected"], true,
            "the host reads its own selection"
        );
        assert_eq!(state["view_wave"]["checked"], true);
        call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"view_both"}}),
        );
        let state = call(&mut editing, json!({"verb":"actions", "member":member}));
        assert_eq!(state["view_wave"]["checked"], false);
        assert_eq!(state["view_both"]["checked"], true);
        let prepared = raw_call(
            &mut editing,
            json!({"verb":"operation", "action":"prepare", "member":member,
            "command":{"kind":"save", "path":"/tmp/action.wav"}}),
        );
        assert!(prepared["ticket"].is_object(), "{prepared}");
        let state = call(&mut editing, json!({"verb":"actions", "member":member}));
        assert_eq!(state["save"]["pending"], true);
        assert_eq!(state["save"]["enabled"], false);
        // pending work outweighs an empty selection, which a host reads itself
        assert_eq!(state["cut"]["reason"], "the file has pending work");
        assert_eq!(state["cut"].get("unselected"), None);
        assert_eq!(state["view_wave"]["enabled"], true);
        settle_operation(&mut editing, prepared);
        assert_eq!(
            call(&mut editing, json!({"verb":"actions", "member":member}))["save"]["pending"],
            false
        );
    }

    #[test]
    fn context_history_steps_accept_an_omitted_or_null_member() {
        for target in [None, Some(Value::Null)] {
            let mut editing = Editing::new();
            let member = an_audio_take(&mut editing);
            draw(&mut editing, member, 1, 10);
            let mut request = json!({"verb":"step", "direction":"undo"});
            if let Some(target) = target {
                request["member"] = target;
            }
            let stepped = call(&mut editing, request);
            assert_eq!(stepped["stepped"], true, "{stepped}");
            assert_eq!(stepped["effects"][0]["member"], member);
        }
    }

    #[test]
    fn invalid_history_step_members_never_use_another_order() {
        for direction in ["undo", "redo"] {
            let mut editing = Editing::new();
            let member = an_audio_take(&mut editing);
            draw(&mut editing, member, 1, 10);
            call(
                &mut editing,
                json!({"verb":"member", "member":member,
                "call":{"verb":"add_file", "take":50, "frames":100,
                    "display":51, "buffers":[52,53,54]}}),
            );
            draw(&mut editing, member, 2, 20);
            if direction == "redo" {
                call(
                    &mut editing,
                    json!({"verb":"step", "member":member,
                    "direction":"undo"}),
                );
            }
            let selected = editing.services.histories.selected();
            let version = editing.version();
            let before = reads(&editing, member);
            for target in [
                json!(999),
                json!(1_u64 << 32),
                json!(u64::MAX),
                json!(-1),
                json!(0.5),
                json!("0"),
                json!(false),
            ] {
                let refused = raw_call(
                    &mut editing,
                    json!({"verb":"step",
                    "member":target, "direction":direction}),
                );
                assert_eq!(refused["stepped"], false, "{target}: {refused}");
                assert_eq!(refused["reason"], "the history step member is invalid");
                assert!(refused["operation"].is_null());
                assert_eq!(refused["effects"], json!([]));
                assert_eq!(editing.services.histories.selected(), selected);
                assert_eq!(editing.version(), version);
                assert_eq!(reads(&editing, member), before);
            }
            let valid = call(
                &mut editing,
                json!({"verb":"step", "member":member,
                "direction":direction}),
            );
            assert_eq!(valid["stepped"], true, "{valid}");
        }
    }

    /// **A window's undo walks the order of its file in focus**, whichever
    /// member had the last turn, and the context's order is the other window's.
    #[test]
    fn a_step_named_for_a_window_walks_its_focused_file_beside_another_window() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        call(
            &mut editing,
            json!({"verb": "member", "member": member, "call": {"verb": "add_file",
            "take": 50, "frames": 200, "channels": 1, "rate": 48000, "display": 51,
            "buffers": [52, 53, 54], "untitled": true}}),
        );
        assert_eq!(
            draw(&mut editing, member, 1, 30)["outcome"]["changed"],
            true
        );
        let other = call(
            &mut editing,
            json!({"verb": "openAudio", "sharedHistory": true, "key": "take:4", "take": 4, "frames": 100,
                   "rate": 48000, "display": 19, "buffers": [40, 41, 42, 43]}),
        )["member"]
            .as_u64()
            .unwrap() as MemberId;
        call(
            &mut editing,
            json!({"verb": "member", "member": other, "call": {"verb": "window", "widget": 13}}),
        );
        let version = editing.version();
        call(
            &mut editing,
            json!({"verb": "event", "member": other, "addr": "/gui_event",
                   "args": [13, 1, version, "draw", 0, 10, [0.5], [0.0]]}),
        );
        let state = |editing: &mut Editing, member: Option<MemberId>| {
            call(editing, json!({"verb": "state", "member": member}))
        };
        assert_eq!(state(&mut editing, Some(member))["canUndo"], true);
        let undone = call(
            &mut editing,
            json!({"verb": "step", "direction": "undo", "member": member}),
        );
        assert_eq!(undone["stepped"], true);
        assert_eq!(undone["effects"][0]["member"], member);
        assert_eq!(state(&mut editing, Some(member))["canUndo"], false);
        assert_eq!(
            state(&mut editing, Some(other))["canUndo"],
            true,
            "the other window's edit stays"
        );
        let context = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        assert_eq!(context["effects"][0]["member"], other);
    }

    /// **Closing the file an editor was opened over** forgets its edits in
    /// the context's order, and the context's order is left whole when the
    /// editor's last file closes.
    #[test]
    fn closing_the_opened_file_forgets_its_edits_and_keeps_the_context_order() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        draw(&mut editing, member, 1, 10);
        call(
            &mut editing,
            json!({"verb": "member", "member": member, "call": {"verb": "add_file",
            "take": 50, "frames": 200, "channels": 1, "rate": 48000, "display": 51,
            "buffers": [52, 53, 54], "untitled": true}}),
        );
        call(
            &mut editing,
            json!({"verb": "member", "member": member, "call": {"verb": "close_file", "file": 1}}),
        );
        let context = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        assert_ne!(
            context["stepped"], true,
            "nothing applies a closed file's edit"
        );
        let closed = call(
            &mut editing,
            json!({"verb": "member", "member": member, "call": {"verb": "close_file"}}),
        );
        assert_eq!(closed["close"], true);
        assert_eq!(editing.services.histories.orders_len(), 1);
        assert_eq!(editing.services.histories.selected(), ScopeId(0));
    }

    #[test]
    fn audio_files_keep_independent_histories_and_restore_their_state() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        let first = draw(&mut editing, member, 1, 10);
        assert!(first["outcome"]["changed"].as_bool().unwrap());
        let member_call = |editing: &mut Editing, inner: Value| {
            call(
                editing,
                json!({"verb": "member", "member": member, "call": inner}),
            )
        };
        member_call(&mut editing, json!({"verb": "locate", "at": 0.0005}));
        member_call(
            &mut editing,
            json!({"verb": "span", "span": [0.0002, 0.0008]}),
        );
        let first_parts = member_call(&mut editing, json!({"verb": "parts"}));
        let added = member_call(
            &mut editing,
            json!({"verb": "add_file", "take": 50, "frames": 200,
            "channels": 2, "rate": 48000, "display": 51, "buffers": [52, 53, 54], "untitled": true}),
        );
        assert_eq!(added["binding"]["display"], 51);
        assert!(!editing.can_undo(), "a new file has no edits");
        let second = draw(&mut editing, member, 2, 30);
        assert_eq!(second["outcome"]["changed"], true);
        assert!(editing.step(Direction::Undo).stepped);
        assert!(!editing.can_undo());
        assert!(editing.can_redo());
        let restored = member_call(&mut editing, json!({"verb": "focus_file", "file": 1}));
        assert_eq!(restored["binding"]["cursor"], 24);
        // the host learns the version the switch moved to, with the cursor
        // and selection the incoming file left
        assert_eq!(restored["answer"]["answer"], "push");
        assert_eq!(restored["answer"]["docVersion"], editing.version());
        assert_eq!(
            restored["answer"]["corrections"][0]["props"]["sel_start"],
            10
        );
        assert_eq!(restored["answer"]["corrections"][0]["props"]["sel_len"], 28);
        assert_eq!(
            restored["binding"]["span"],
            json!([10.0 / 48000.0, 38.0 / 48000.0])
        );
        assert!(editing.can_undo());
        assert!(!editing.can_redo());
        assert_eq!(
            member_call(&mut editing, json!({"verb": "parts"})),
            first_parts
        );
        assert!(editing.step(Direction::Undo).stepped);
        assert!(!editing.can_undo());
        member_call(&mut editing, json!({"verb": "focus_file", "file": 2}));
        assert!(editing.can_redo(), "the other file retains its redo");
        assert!(editing.step(Direction::Redo).stepped);
        let closed = member_call(&mut editing, json!({"verb": "close_file", "file": 1}));
        assert_eq!(
            closed["binding"]["display"], 51,
            "closing a background file keeps focus"
        );
        assert_eq!(
            member_call(&mut editing, json!({"verb": "files"}))["files"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            editing.can_undo(),
            "closing another file keeps this history"
        );
    }
    #[test]
    fn a_gesture_from_the_previous_file_cannot_edit_the_new_binding() {
        let mut editing = Editing::new();
        editing.moved();
        let member = an_audio_take(&mut editing);
        let old_version = editing.version();
        call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{
            "verb":"add_file", "take":50, "frames":200, "channels":2,
            "rate":48000, "display":51, "buffers":[52,53,54]}}),
        );
        let before = call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"parts"}}),
        );
        let late = call(
            &mut editing,
            json!({"verb":"event", "member":member,
            "addr":"/gui_event", "args":[12, 9, old_version, "draw", 0, 10, [0.5], [0.0]]}),
        );
        assert_eq!(late["outcome"]["turn"], "stale");
        assert_eq!(
            call(
                &mut editing,
                json!({"verb":"member", "member":member, "call":{"verb":"parts"}})
            ),
            before
        );
        assert!(!editing.can_undo());
    }
    #[test]
    fn a_pending_consumer_outlives_redo_and_releases_exactly_once() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        draw(&mut editing, member, 1, 10);
        let source = json!({"server": {"connection":0,"generation":0}, "buffer":20});
        let root = json!({"kind":"pending", "owner":100});
        assert_eq!(
            call(
                &mut editing,
                json!({"verb":"retain", "root":root, "sources":[source]})
            )["retained"],
            true
        );
        call(&mut editing, json!({"verb":"step", "direction":"undo"}));
        let replaced = draw(&mut editing, member, 2, 30);
        assert!(replaced.get("freed").is_none());
        let released = call(&mut editing, json!({"verb":"dropRoot", "root":root}));
        assert_eq!(
            released["freed"],
            json!([{"member":member, "buffers":[20]}])
        );
        assert_eq!(
            raw_call(&mut editing, json!({"verb":"dropRoot", "root":root}))["freed"],
            json!([])
        );
    }

    #[test]
    fn closing_a_file_defers_source_release_until_playback_drops_its_root() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        draw(&mut editing, member, 1, 10);
        assert_eq!(
            call(
                &mut editing,
                json!({"verb":"retainMember", "member":member, "kind":"playback"})
            )["retained"],
            true
        );
        let closed = call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"close_file", "file":1}}),
        );
        assert!(
            !closed["freedBuffers"]
                .as_array()
                .unwrap()
                .contains(&json!(20))
        );
        assert!(
            !closed["steps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step["send"]["addr"] == "/buffer_free"
                    && step["send"]["args"][0]["i"] == 20)
        );
        let released = call(
            &mut editing,
            json!({"verb":"dropRoot", "root":{"kind":"playback", "owner":member}}),
        );
        assert!(
            released["freed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["buffers"].as_array().unwrap().contains(&json!(20)))
        );
    }

    #[test]
    fn an_inactive_files_history_keeps_sources_and_can_spill_through_its_owner() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"sync", "scratch":"scratch"}}),
        );
        draw(&mut editing, member, 1, 10);
        draw(&mut editing, member, 2, 10);
        call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"add_file", "take":50, "frames":200, "display":51,"buffers":[52,53,54]}}),
        );
        editing.set_resident(Some(0));
        let (_, stored) = editing.release();
        assert_eq!(
            stored
                .iter()
                .map(|stored| stored.buffer)
                .collect::<Vec<_>>(),
            [20]
        );
        let prepared = stored[0].operation.as_ref().unwrap();
        let refused = raw_call(
            &mut editing,
            json!({"verb":"operation","action":"complete",
            "ticket":prepared.ticket,"batch":prepared.batch,"result":{"status":"failed","reason":"quota"}}),
        );
        raw_call(
            &mut editing,
            json!({"verb":"operation","action":"repaired", "ticket":refused["repair"]}),
        );
        assert!(
            matches!(editing.member(member), Some(Member::Audio(editor)) if !editor.is_spilled(20))
        );
        let (freed, _) = editing.release();
        assert!(freed.is_empty());
    }
    #[test]
    fn shared_undo_routes_to_an_unfocused_object_without_rebinding_the_window() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        draw(&mut editing, member, 1, 10);
        call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"add_file", "take":50,"frames":200,"display":51,"buffers":[52,53,54]}}),
        );
        let binding = call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"binding"}}),
        );
        let stepped = call(&mut editing, json!({"verb":"step", "direction":"undo"}));
        assert_eq!(stepped["stepped"], true);
        assert_eq!(stepped["effects"][0]["file"]["id"], 1);
        assert_eq!(stepped["effects"][0]["file"]["display"], 9);
        let after = call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"binding"}}),
        );
        assert_eq!(after["id"], binding["id"]);
        assert_eq!(after["generation"], binding["generation"]);
        call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"focus_file", "file":1}}),
        );
        let parts = call(
            &mut editing,
            json!({"verb":"member", "member":member, "call":{"verb":"parts"}}),
        );
        assert_eq!(parts["parts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn equal_local_buffer_ids_on_different_servers_release_on_their_own_endpoint() {
        let mut editing = Editing::new();
        let a = an_audio_take(&mut editing);
        editing.set_server(
            a,
            ServerInstance {
                connection: 1,
                generation: 1,
            },
        );
        let b = call(
            &mut editing,
            json!({"verb":"openAudio", "sharedHistory":true, "key":"another-server", "take":3,"frames":100,"display":9,
            "buffers":[20,21,22,23], "server":{"connection":2,"generation":1}}),
        )["member"]
            .as_u64()
            .unwrap() as u32;
        call(
            &mut editing,
            json!({"verb":"member", "member":b,"call":{"verb":"window", "widget":12}}),
        );
        draw(&mut editing, a, 1, 10);
        draw(&mut editing, b, 2, 10);
        call(&mut editing, json!({"verb":"step", "direction":"undo"}));
        let replaced = draw(&mut editing, a, 3, 30);
        assert_eq!(replaced["freed"], json!([{"member":b,"buffers":[20]}]));
        assert!(
            matches!(editing.member(a), Some(Member::Audio(editor)) if editor.bytes(20).is_some())
        );
    }
    #[test]
    fn a_lost_endpoint_refuses_its_step_without_blocking_unrelated_context_edits() {
        let mut editing = Editing::new();
        let member = an_audio_take(&mut editing);
        draw(&mut editing, member, 1, 10);
        let external = editing.join(
            "local",
            Member::External {
                domain: "local".into(),
            },
        );
        call(
            &mut editing,
            json!({"verb":"record", "member":external, "label":"local edit",
            "legs":[{"forward":{"edit":{"x":1}},"backward":{"x":0}}]}),
        );
        call(
            &mut editing,
            json!({"verb":"invalidate", "server":{"connection":0,"generation":0}}),
        );
        assert_eq!(
            call(&mut editing, json!({"verb":"step","direction":"undo"}))["stepped"],
            true
        );
        let refused = call(&mut editing, json!({"verb":"step","direction":"undo"}));
        assert_eq!(refused["stepped"], false);
        assert!(
            refused["reason"]
                .as_str()
                .unwrap()
                .contains("previous server instance")
        );
        assert!(editing.can_undo());
    }
}
