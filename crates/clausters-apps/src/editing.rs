//! **One undo order over every application open in it**: the editing context.
//!
//! A multitrack, a take and whatever else is on screen walk one history, and no
//! application holds one of its own. An editor is always opened *in* an
//! [`Editing`]; one opened alone is an `Editing` with one member, and two opened
//! in the same one walk one order -- which is the whole of how two applications
//! share an undo.
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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_document::history::{Direction, Entry, History, Step, StructureId};
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::samples::SAMPLES;
use clausters_document::{Opaque, SourceId};
use clausters_editing::conversation::Answer;

use crate::audio::editor::{self as audio, AudioEditor};
use crate::multitrack::editor::{self as multitrack, MultitrackEditor};
use crate::samples::editor::{self as samples, SamplesEditor};
use crate::turn::{Event, Kind, Record, int};

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
    /// A samples editor over a take.
    Samples(SamplesEditor),
    /// An audio editor over a take made of parts.
    Audio(Box<AudioEditor>),
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
            Member::Samples(_) => SAMPLES.into(),
            Member::Audio(_) => audio::DOMAIN.into(),
            Member::External { domain } => domain.clone(),
        }
    }
}

/// One member, and the structure it is in the order.
#[derive(Clone, Debug)]
struct Seat {
    member: Member,
    structure: StructureId,
}

/// **What a member's turn came to**, in that member's own terms.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Outcome {
    /// A multitrack editor's.
    Multitrack(multitrack::Outcome),
    /// A samples editor's.
    Samples(samples::Outcome),
    /// An audio editor's.
    Audio(audio::Outcome),
}

impl Outcome {
    fn kind(&self) -> Kind {
        match self {
            Outcome::Multitrack(o) => o.turn,
            Outcome::Samples(o) => o.turn,
            Outcome::Audio(o) => o.turn,
        }
    }

    fn record(&self) -> Option<&Record> {
        match self {
            Outcome::Multitrack(o) => o.record.as_ref(),
            Outcome::Samples(o) => o.record.as_ref(),
            Outcome::Audio(o) => o.record.as_ref(),
        }
    }

    fn changed(&self) -> bool {
        match self {
            Outcome::Multitrack(o) => o.changed,
            Outcome::Samples(o) => o.changed,
            Outcome::Audio(o) => o.changed,
        }
    }

    fn version(&self) -> i64 {
        match self {
            Outcome::Multitrack(o) => o.version,
            Outcome::Samples(o) => o.version,
            Outcome::Audio(o) => o.version,
        }
    }

    fn step(&self) -> (i64, bool) {
        match self {
            Outcome::Multitrack(o) => (o.seq, o.redo),
            Outcome::Samples(o) => (o.seq, o.redo),
            Outcome::Audio(o) => (o.seq, o.redo),
        }
    }

    fn answer(&mut self, answer: Answer) {
        match self {
            Outcome::Multitrack(o) => o.answer = Some(answer),
            Outcome::Samples(o) => o.answer = Some(answer),
            Outcome::Audio(o) => o.answer = Some(answer),
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
    /// Writes to a take's buffer, in order: `write` payloads, which the member
    /// turns into steps (its `write` verb) with the bound of the server the take
    /// is on.
    #[serde(rename_all = "camelCase")]
    Samples {
        /// The member.
        member: MemberId,
        /// The payloads.
        payloads: Vec<Value>,
    },
    /// The steps an audio editor's take is stitched again with, for the list
    /// the step handed it.
    #[serde(rename_all = "camelCase")]
    Audio {
        /// The member.
        member: MemberId,
        /// The steps, in the JSON a runner walks.
        steps: Value,
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
/// on that member's server.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Freed {
    /// The member whose take they were.
    pub member: MemberId,
    /// The buffer numbers, to give back.
    pub buffers: Vec<i64>,
    /// Those of them whose take was on disk: the number goes back, and there
    /// is no buffer on the server to free.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spilled: Vec<i64>,
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
}

/// **What a step of the history came to.**
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stepped {
    /// Whether anything moved.
    pub stepped: bool,
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
    history: History,
    version: i64,
    seats: Vec<Seat>,
    keys: HashMap<String, StructureId>,
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
        Self {
            history: History::new(),
            version: FIRST_VERSION,
            seats: Vec::new(),
            keys: HashMap::new(),
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
        let editors: Vec<&AudioEditor> = self
            .seats
            .iter()
            .filter_map(|seat| match &seat.member {
                Member::Audio(editor) => Some(editor.as_ref()),
                _ => None,
            })
            .collect();
        let rooted: Vec<SourceId> = editors.iter().flat_map(|e| e.rooted()).collect();
        if let Some(bytes) = self.bytes {
            let size_of = |source: SourceId| {
                editors
                    .iter()
                    .find_map(|e| e.bytes(source.0 as i64))
                    .unwrap_or(0)
            };
            let is_rooted = |source: SourceId| rooted.contains(&source);
            self.history.trim_to_bytes(bytes, &size_of, &is_rooted);
        }
        let released: Vec<i64> = self
            .history
            .released_sources()
            .into_iter()
            .filter(|source| !rooted.contains(source))
            .map(|source| source.0 as i64)
            .collect();
        // Each take goes back under the member that made it: the one that
        // knows its size is the one whose server it is on.
        let mut out: Vec<Freed> = Vec::new();
        for buffer in released {
            let owner = self.seats.iter().position(|seat| {
                matches!(&seat.member, Member::Audio(editor) if editor.bytes(buffer).is_some())
            });
            let Some(owner) = owner else {
                continue;
            };
            let member = owner as MemberId;
            let spilled = matches!(
                &self.seats[owner].member,
                Member::Audio(editor) if editor.is_spilled(buffer)
            );
            let at = match out.iter().position(|f| f.member == member) {
                Some(at) => at,
                None => {
                    out.push(Freed {
                        member,
                        buffers: Vec::new(),
                        spilled: Vec::new(),
                    });
                    out.len() - 1
                }
            };
            out[at].buffers.push(buffer);
            if spilled {
                out[at].spilled.push(buffer);
            }
        }
        for seat in &mut self.seats {
            if let Member::Audio(editor) = &mut seat.member {
                for freed in &out {
                    for buffer in &freed.buffers {
                        editor.forget(*buffer);
                    }
                }
            }
        }
        let stored = self.store();
        (out, stored)
    }

    /// **The takes past the resident budget, out of memory**: of the takes
    /// only the history holds and still in a buffer, the oldest go to disk
    /// until what is left fits.
    fn store(&mut self) -> Vec<Stored> {
        let Some(limit) = self.resident else {
            return Vec::new();
        };
        let rooted: Vec<SourceId> = self
            .seats
            .iter()
            .filter_map(|seat| match &seat.member {
                Member::Audio(editor) => Some(editor.rooted()),
                _ => None,
            })
            .flatten()
            .collect();
        // Oldest first: the order the pile names them in.
        let mut resident: Vec<(usize, i64, u64)> = Vec::new();
        for source in self.history.held_sources() {
            if rooted.contains(&source) {
                continue;
            }
            let buffer = source.0 as i64;
            let found = self.seats.iter().position(
                |seat| matches!(&seat.member, Member::Audio(e) if e.bytes(buffer).is_some()),
            );
            let Some(owner) = found else {
                continue;
            };
            if let Member::Audio(editor) = &self.seats[owner].member
                && !editor.is_spilled(buffer)
                && let Some(bytes) = editor.bytes(buffer)
            {
                resident.push((owner, buffer, bytes));
            }
        }
        let mut total: u64 = resident.iter().map(|(_, _, b)| b).sum();
        let mut out = Vec::new();
        for (owner, buffer, bytes) in resident {
            if total <= limit {
                break;
            }
            if let Member::Audio(editor) = &mut self.seats[owner].member
                && let Some(steps) = editor.spill(buffer)
            {
                total -= bytes;
                out.push(Stored {
                    member: owner as MemberId,
                    buffer,
                    steps,
                });
            }
        }
        out
    }

    /// The version a host names back.
    pub fn version(&self) -> i64 {
        self.version
    }

    /// Whether there is an edit to step back over.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether there is an undone edit to step forward into.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// What an undo would be called.
    pub fn undo_label(&self) -> Option<String> {
        self.history.undo_label()
    }

    /// What a redo would be called.
    pub fn redo_label(&self) -> Option<String> {
        self.history.redo_label()
    }

    /// The structure `member` is in the order: the identity every member with
    /// its key shares, and what a client names that structure's widgets by.
    pub fn structure(&self, member: MemberId) -> Option<StructureId> {
        self.seats.get(member as usize).map(|seat| seat.structure)
    }

    /// **Takes a member in**, as the structure `key` names: the identity a
    /// member with the same key already has, or a new one.
    pub fn join(&mut self, key: &str, member: Member) -> MemberId {
        let history = &mut self.history;
        let structure = *self
            .keys
            .entry(key.to_string())
            .or_insert_with(|| history.register(member.domain()));
        self.seats.push(Seat { member, structure });
        (self.seats.len() - 1) as MemberId
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
        let taken = self.history.record(entry);
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
        let Some(structure) = self.structure(member) else {
            return false;
        };
        let taken = apply(&mut self.history, structure);
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
            self.history
                .record(if coalesce { entry.continuing() } else { entry })
        })
    }

    /// **One message to a member**, read, recorded and answered.
    ///
    /// `None` for a member that is not there or does not read messages (an
    /// external one).
    pub fn event(&mut self, member: MemberId, event: &Event) -> Option<Turned> {
        let version = self.version;
        let seat = self.seats.get_mut(member as usize)?;
        let structure = seat.structure;
        let mut outcome = match &mut seat.member {
            Member::Multitrack(editor) => Outcome::Multitrack(editor.event(event, version)),
            Member::Samples(editor) => Outcome::Samples(editor.event(event, version)),
            Member::Audio(editor) => Outcome::Audio(editor.event(event, version)),
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
            let step = self.step(if redo {
                Direction::Redo
            } else {
                Direction::Undo
            });
            let version = self.version;
            let reason = step.reason.clone();
            if let Some(seat) = self.seats.get(member as usize) {
                match &seat.member {
                    Member::Multitrack(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::Samples(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::Audio(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::External { .. } => {}
                }
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

    /// **One step of the history**, handed round the members.
    ///
    /// Each leg goes to the members holding its structure: every multitrack
    /// editor over the multitrack applies it to the multitrack it draws; a write to a take
    /// and an external member's payloads are carried out once, by the caller. A
    /// step nothing could apply is not a step: the cursor goes back, and the
    /// entry is still there.
    pub fn step(&mut self, direction: Direction) -> Stepped {
        let label = match direction {
            Direction::Undo => self.history.undo_label(),
            Direction::Redo => self.history.redo_label(),
        }
        .unwrap_or_default();
        let mut out = Stepped {
            stepped: false,
            reason: None,
            effects: Vec::new(),
            corrections: Vec::new(),
            freed: Vec::new(),
            stored: Vec::new(),
            version: self.version,
        };
        let Some(walked) = self.history.walk(direction) else {
            return out;
        };
        let mut applied = false;
        for (structure, payloads) in &walked.legs {
            let mut written = false;
            for (id, seat) in self.seats.iter_mut().enumerate() {
                if seat.structure != *structure {
                    continue;
                }
                let member = id as MemberId;
                match &mut seat.member {
                    Member::Multitrack(editor) => {
                        for payload in payloads {
                            let done = editor.apply(&payload.0);
                            applied |= done.applied;
                            out.effects.push(Effect::Multitrack {
                                member,
                                applied: done,
                            });
                        }
                    }
                    Member::Audio(editor) => {
                        // The last list is the take: a step names one list per
                        // entry, and what it leaves is what is drawn.
                        if let Some(steps) = payloads.last().and_then(|p| editor.apply(&p.0)) {
                            applied = true;
                            out.effects.push(Effect::Audio { member, steps });
                        }
                    }
                    Member::Samples(_) if !written => {
                        written = true;
                        applied |= !payloads.is_empty();
                        out.effects.push(Effect::Samples {
                            member,
                            payloads: payloads.iter().map(|p| p.0.clone()).collect(),
                        });
                    }
                    Member::External { .. } if !written => {
                        written = true;
                        applied |= !payloads.is_empty();
                        out.effects.push(Effect::External {
                            member,
                            payloads: payloads.iter().map(|p| p.0.clone()).collect(),
                        });
                    }
                    _ => {}
                }
            }
        }
        if !applied {
            self.history.step(match direction {
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
                Member::Samples(editor) => editor.resync_all(version),
                Member::Audio(editor) => editor.resync_all(version),
                Member::External { .. } => continue,
            };
            if answer != Answer::Silent {
                out.push(Corrected { member, answer });
            }
        }
        out
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
/// - `openSamples` -- `key`, and what a samples editor is built from
///   (`clausters_apps::samples::editor::new_json`): `{"member", "structure"}`,
///   or `{"error"}`.
/// - `openAudio` -- `key`, and what an audio editor is built from
///   (`clausters_apps::audio::editor::new_json`): `{"member", "structure"}`,
///   or `{"error"}`.
/// - `bytes` -- `bytes`, or `null` for none: the most bytes of takes only the
///   history may hold. Answers `{}`.
/// - `resident` -- `bytes`, or `null` for none: the most of those kept in
///   memory; past it the oldest go to disk (a turn's and a step's `stored`).
///   Answers `{}`.
/// - `external` -- `key`, `domain`: `{"member", "structure"}`.
/// - `event` -- `member`, `addr`, `args`: a [`Turned`], or `null`.
/// - `step` -- `direction` (`"undo"` or `"redo"`): a [`Stepped`].
/// - `record` -- `member`, `label`, `legs` (`forward`, `backward`, `key`),
///   `coalesce`: `{"recorded", "version"}`.
/// - `member` -- `member`, and a verb of that member's own door with its
///   arguments, the version filled in: what that door answers. `event` and
///   `apply` are the context's and answer `{}` here.
/// - `moved` -- an edit that leaves no entry: `{"version"}`, moved on.
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
        "openSamples" => match samples::new_json(&request.to_string()) {
            Ok(editor) => joined(editing, &key, Member::Samples(editor)),
            Err(error) => json!({ "error": error }).to_string(),
        },
        "openAudio" => match audio::new_json(&request.to_string()) {
            Ok(editor) => joined(editing, &key, Member::Audio(Box::new(editor))),
            Err(error) => json!({ "error": error }).to_string(),
        },
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
            to_json(&editing.event(member, &event))
        }
        "step" => {
            let direction = get(&request, "direction")
                .as_str()
                .and_then(Direction::parse)
                .unwrap_or(Direction::Undo);
            to_json(&editing.step(direction))
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
            let inner = get(&request, "call")
                .get("verb")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if matches!(inner.as_str(), "event" | "apply") {
                return "{}".into();
            }
            let mut call = get(&request, "call");
            if let Some(map) = call.as_object_mut() {
                map.insert("version".into(), json!(editing.version));
            }
            match editing.member_mut(member) {
                Some(Member::Multitrack(editor)) => {
                    multitrack::call_json(editor, &call.to_string())
                }
                Some(Member::Samples(editor)) => samples::call_json(editor, &call.to_string()),
                Some(Member::Audio(editor)) => audio::call_json(editor, &call.to_string()),
                _ => "{}".into(),
            }
        }
        "moved" => json!({ "version": editing.moved() }).to_string(),
        "state" => json!({
            "version": editing.version(),
            "canUndo": editing.can_undo(),
            "canRedo": editing.can_redo(),
            "undoLabel": editing.undo_label(),
            "redoLabel": editing.redo_label(),
        })
        .to_string(),
        _ => "{}".into(),
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
    use clausters_document::multitrack::{Content, Multitrack, Region, Track};
    use clausters_document::{
        Lifetime, NodeId, Second, SegmentRef, SegmentSource, SourceId, SourceRef,
    };

    const SR: f64 = 48_000.0;

    fn region(id: u64, at: f64) -> Region {
        let mut region = Region::new(
            NodeId(id),
            Second(at),
            Second(2.0),
            Content::Unknown(Value::Null),
        );
        region.content = Content::window(SegmentRef {
            source: SegmentSource::Samples(SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 2.0,
        });
        region
    }

    /// A multitrack of one track holding box 12, drawn by widget 40 in window 39.
    fn a_multitrack() -> Member {
        let mut track = Track::new(NodeId(10), NodeId(11));
        track.lanes[0].regions = vec![region(12, 0.0)];
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

    /// A mono take in buffer 7, drawn by widget 50 in window 49.
    fn a_take() -> Member {
        let mut editor = samples::new_json(r#"{"buffer": 7, "version": 1}"#).unwrap();
        samples::call_json(&mut editor, r#"{"verb": "window", "widget": 50}"#);
        samples::call_json(&mut editor, r#"{"verb": "sync", "window": 49}"#);
        Member::Samples(editor)
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
                editor.multitrack().tracks[0].lanes[0].regions[0].position.0
            }
            _ => f64::NAN,
        }
    }

    fn written(stepped: &Stepped) -> Vec<Value> {
        stepped
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::Samples { payloads, .. } => Some(payloads[0]["values"].clone()),
                _ => None,
            })
            .collect()
    }

    /// **An editor open alone is a context of one**: its stroke is recorded
    /// there, and an undo hands back the write that puts the take back.
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

        let undone = editing.step(Direction::Undo);
        assert!(undone.stepped);
        assert_eq!(undone.version, 3);
        assert_eq!(written(&undone), [json!([0.0])]);
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
        let redone = editing.step(Direction::Redo);
        assert_eq!(written(&redone), [json!([0.5])]);
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
        assert_eq!(written(&back), [json!([0.0])], "the stroke, in between");
        assert_eq!(position(&mut editing, multitrack), 2.0);

        editing.step(Direction::Undo);
        assert_eq!(position(&mut editing, multitrack), 0.0);
        assert!(!editing.can_undo());

        editing.step(Direction::Redo);
        let forward = editing.step(Direction::Redo);
        assert_eq!(written(&forward), [json!([0.5])]);
    }

    /// **Two windows over one take are one structure**: a stroke in one is
    /// undone once, whichever asks.
    #[test]
    fn two_windows_over_one_take_are_one_structure() {
        let mut editing = Editing::default();
        let left = editing.join("buffer:7", a_take());
        let right = editing.join("buffer:7", a_take());
        editing
            .event(left, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        let undone = editing.step(Direction::Undo);
        assert_eq!(written(&undone).len(), 1, "one write, not one per window");
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
        let Outcome::Samples(outcome) = &turned.outcome else {
            panic!()
        };
        assert_eq!(
            outcome.answer,
            Some(Answer::Ack {
                seq: 2,
                doc_version: 3,
                reason: None
            })
        );
    }

    /// The JSON door opens members, turns, steps and reports the state.
    #[test]
    fn the_door_opens_turns_and_steps() {
        let mut editing = Editing::default();
        let call = |editing: &mut Editing, request: Value| -> Value {
            serde_json::from_str(&call_json(editing, &request.to_string())).unwrap()
        };
        let take = call(
            &mut editing,
            json!({"verb": "openSamples", "key": "buffer:7", "buffer": 7}),
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
        assert_eq!(turned["outcome"]["edit"]["values"], json!([0.5]));
        let state = call(&mut editing, json!({"verb": "state"}));
        assert_eq!(
            call(&mut editing, json!({"verb": "moved"}))["version"],
            3,
            "an edit with no entry moves the version and records nothing"
        );
        assert_eq!(state["undoLabel"], "draw the samples");
        let stepped = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        assert_eq!(stepped["effects"][0]["kind"], "samples");
        assert_eq!(
            call(
                &mut editing,
                json!({"verb": "openSamples", "key": "x", "layers": []})
            )["error"]
                .as_str()
                .map(|e| e.contains("measures something")),
            Some(true)
        );
    }

    fn call(editing: &mut Editing, request: Value) -> Value {
        serde_json::from_str(&call_json(editing, &request.to_string())).unwrap()
    }

    /// An audio editor over take 3 (100 frames), drawn through join 9 by
    /// widget 12, with buffers 20..24 for new takes.
    fn an_audio_take(editing: &mut Editing) -> MemberId {
        let opened = call(
            editing,
            json!({"verb": "openAudio", "key": "take:3", "take": 3, "frames": 100,
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
        assert_eq!(
            steps[0]["send"]["args"][1],
            json!({"s": "/scratch/take-20.wav"})
        );
        assert_eq!(steps[2]["send"]["addr"], "/buffer_free");

        // Undo the second stroke: the list reads the first one's take again,
        // so it is read back before the join is stitched.
        let undone = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        let steps = undone["effects"][0]["steps"].as_array().unwrap();
        assert_eq!(steps[0]["send"]["addr"], "/buffer_allocRead");
        assert_eq!(
            steps[0]["send"]["args"][1],
            json!({"s": "/scratch/take-20.wav"})
        );
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
}
