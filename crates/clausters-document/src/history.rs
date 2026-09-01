//! A history, and the data it belongs to — **the pile, with no document in it**.
//!
//! [`log`](crate::log) placed the arrangement's undo beside the arrangement,
//! and gave the reason: a history that sees only one editor's gestures
//! describes data that has moved on. This module is that argument made
//! structural and made general. Nothing here knows what an edit *is*: a
//! [`History`] holds identities it minted, an ordered pile of [`Entry`]s over
//! them, and a cursor. The verbs are the domain's.
//!
//! # The registry is the scope
//!
//! One history is **one editing context**: one ordered pile, one cursor, and
//! however many structures were registered in it. That single sentence is what
//! the three shapes this has to serve come out of:
//!
//! - an **independent structure** — a curve, a buffer, a roll the caller built
//!   with no composition behind it — is a history with one structure in it, and
//!   has a working undo without a [`Document`](crate::Document) existing
//!   anywhere;
//! - a **combination** — an application composing several editable views — is
//!   one history with several structures in it, and the interleaved order its
//!   undo walks *is* the pile, with no second mechanism to produce it;
//! - **two views of one structure** are two views of one history, which is the
//!   arrangement this whole module exists to make the only expressible one.
//!
//! What decides what shares a history is therefore which history a structure
//! was registered in, never which view is looking at it. A structure belongs to
//! **exactly one** history, and that is enforced rather than asked for:
//! [`History::register`] mints the identity, and [`History::record`] refuses an
//! entry naming an identity this history did not mint.
//!
//! The alternative — a pile per structure, and a view filtering one shared
//! order down to the structures it shows — is *selective undo*: inverting an
//! entry that touched A and B while a later entry over B stands, which writes a
//! state nobody was in. A history is one order or it is not a history.
//!
//! # An entry's payload is opaque, and that is what buys one pile
//!
//! One pile holds entries from several domains, so it cannot hold any domain's
//! type: a payload is an [`Opaque`], the same JSON the crate already carries a
//! leaf's configuration in and never interprets. Each leg names the structure
//! it belongs to, so a caller routes what comes back to whatever reads that
//! vocabulary.
//!
//! It has a price worth stating: an edit whose payload is bulk — a span of
//! samples — is held as JSON rather than as its own bytes. The boundary already
//! pays that (an intent crosses the C ABI and the wasm seam as JSON), so what
//! is new is only the in-process case, and it buys the property the whole
//! module is for. What keeps it from being paid twice is [`Spill`]: above a
//! threshold a payload leaves the pile for a content-addressed store, so an
//! undo/redo pair naming the same bytes holds one copy.
//!
//! # What the history does not do
//!
//! It never applies anything. [`History::undo`] hands back the inverses and
//! [`History::redo`] the steps, and the caller applies them through whatever
//! door its domain has — which for the arrangement is
//! [`intent::apply`](crate::intent::apply), the one door. Two things that edit
//! is exactly what this crate exists to prevent, and a history that edited
//! would be the second.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::Opaque;

/// A structure's identity within one history.
///
/// **Minted by the history**, not carried by the data: the arrangement has node
/// ids only because `to_document` stamps them, and a curve or a buffer a caller
/// built has none and is not going to be given a stable one for this. So the
/// caller registers what it is about to edit and keeps the handle — which is
/// also the read-back path, since the identity that opened an editable view is
/// the one its edited state is read out through.
///
/// **Unique across histories**, not only within one, and that is not a nicety:
/// "a structure belongs to exactly one history" is enforced by a history
/// refusing an identity it did not mint, and a per-history counter would hand
/// the second history's first structure the same number as the first's — so
/// the check would pass on exactly the arrangement it exists to refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StructureId(pub u64);

/// The source of [`StructureId`]s, shared by every history in the process. See
/// the type's own note on why it cannot be per-history.
static NEXT_STRUCTURE: AtomicU64 = AtomicU64::new(1);

/// A blob in a [`Spill`] store, named by its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpillId(pub u64);

/// Somewhere to put an inverse whose content is data rather than parameters.
///
/// Content-addressed: [`Spill::put`] of the same bytes twice returns the same
/// id and holds one copy, which is what keeps an undo/redo pair — the same span
/// named from both sides — from doubling. Each `put` takes a reference and each
/// [`Spill::release`] drops one, so a store is free to discard a blob once the
/// history has forgotten it.
pub trait Spill {
    /// Store these bytes and take a reference to them.
    fn put(&mut self, bytes: &[u8]) -> SpillId;
    /// Read them back, or `None` if the store has lost them.
    fn get(&self, id: SpillId) -> Option<Vec<u8>>;
    /// Drop one reference.
    fn release(&mut self, id: SpillId);
}

/// The store the crate ships: blobs in memory, refcounted.
///
/// What a page uses, and what a test uses. A native deployment that wants a
/// temporary directory implements [`Spill`] over one — the trait exists so that
/// choice is not the crate's, which has no business picking a directory policy.
#[derive(Debug, Default)]
pub struct MemorySpill {
    blobs: HashMap<SpillId, (Vec<u8>, usize)>,
    next: u64,
}

impl MemorySpill {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many distinct blobs are held.
    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }
}

impl Spill for MemorySpill {
    fn put(&mut self, bytes: &[u8]) -> SpillId {
        if let Some((id, (_, refs))) = self
            .blobs
            .iter_mut()
            .find(|(_, (held, _))| held.as_slice() == bytes)
        {
            *refs += 1;
            return *id;
        }
        self.next += 1;
        let id = SpillId(self.next);
        self.blobs.insert(id, (bytes.to_vec(), 1));
        id
    }

    fn get(&self, id: SpillId) -> Option<Vec<u8>> {
        self.blobs.get(&id).map(|(bytes, _)| bytes.clone())
    }

    fn release(&mut self, id: SpillId) {
        let Some((_, refs)) = self.blobs.get_mut(&id) else {
            return;
        };
        *refs -= 1;
        if *refs == 0 {
            self.blobs.remove(&id);
        }
    }
}

/// One move in the forward direction.
///
/// Only the forward direction has two shapes: going **back** is always data —
/// undoing a normalize means writing the old samples, and no algorithm
/// reconstructs them — while going forward need not be, since a deterministic
/// operation can store its *parameters* and be re-run. That is what makes a
/// redo of an edit over a million samples cost a few bytes.
///
/// On the wire it is externally tagged — `{"edit": <payload>}`,
/// `{"recompute": <params>}` — rather than internally, because a domain's own
/// payload is already tagged however that domain tags it, and two tags in one
/// object is how a format grows a bug nobody can read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// An ordinary edit, in the structure's own vocabulary.
    Edit(Opaque),
    /// A deterministic operation to **re-run**, carried as the owner's own
    /// parameters and never interpreted here. The crate holds no algorithms, so
    /// a caller that stores these must be ready to perform them on redo.
    Recompute(Opaque),
}

impl Step {
    /// The edit's payload, for the ordinary case.
    pub fn payload(&self) -> Option<&Opaque> {
        match self {
            Step::Edit(payload) => Some(payload),
            Step::Recompute(_) => None,
        }
    }
}

/// One half of a change, with a bulk payload lifted out of it.
#[derive(Debug, Clone, PartialEq)]
struct Half {
    step: Step,
    blob: Option<SpillId>,
}

/// One structure's share of a transaction: how to redo it there, and how to
/// undo it there.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    structure: StructureId,
    key: Option<String>,
    forward: Half,
    backward: Half,
}

/// One transaction in the pile: a gesture, and what it takes to reverse it.
///
/// The unit is the **gesture**, not the edit, because that is what a person
/// means by "the last thing I did". A gesture may touch more than one structure
/// — a drag that moves a clip and rewrites the curve it carries — so an entry
/// is a list of legs, each naming its structure, applied in order forward and
/// inverted in reverse.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// What to call this in a menu. The history never reads it.
    ///
    /// It stops being decoration the moment several structures are on screen:
    /// with one pile over all of them, the label is how a person knows what a
    /// keystroke is about to move.
    pub label: String,
    /// Whether this may merge into the entry before it when they touch the same
    /// thing the same way — a run of small adjustments becoming one undo
    /// instead of two hundred. The caller decides when a run is continuous,
    /// because only the caller knows where the hand stopped.
    pub coalesce: bool,
    changes: Vec<Change>,
}

impl Entry {
    /// One edit over one structure, and its inverse.
    pub fn new(
        label: impl Into<String>,
        structure: StructureId,
        forward: Step,
        backward: Opaque,
    ) -> Self {
        Self {
            label: label.into(),
            coalesce: false,
            changes: vec![change(structure, forward, backward)],
        }
    }

    /// Adds a leg over another structure — or the same one — to the same
    /// transaction. Applied in order forward, in reverse order backward.
    pub fn and(mut self, structure: StructureId, forward: Step, backward: Opaque) -> Self {
        self.changes.push(change(structure, forward, backward));
        self
    }

    /// Marks this entry as continuing the one before it (see
    /// [`Entry::coalesce`]).
    pub fn continuing(mut self) -> Self {
        self.coalesce = true;
        self
    }

    /// What makes the last leg *the same thing done the same way* as another —
    /// the test a merge is decided by.
    ///
    /// The history cannot compute this: "the same thing" is a statement in the
    /// domain's vocabulary (for the arrangement it is the kind of edit and the
    /// node it names), and reading it here would be the pile knowing a
    /// vocabulary. A leg with no key never coalesces, which is what a
    /// [`Step::Recompute`] wants: its payload is parameters the crate cannot
    /// compare, and merging two operations into one is the caller's decision.
    pub fn keyed(mut self, key: impl Into<String>) -> Self {
        if let Some(last) = self.changes.last_mut() {
            last.key = Some(key.into());
        }
        self
    }

    /// How many legs this transaction holds.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The structures this entry touches, in the order its legs name them.
    pub fn structures(&self) -> Vec<StructureId> {
        self.changes.iter().map(|c| c.structure).collect()
    }

    /// Whether this entry and `other` touch the same structures the same way.
    fn matches(&self, other: &Entry) -> bool {
        self.changes.len() == other.changes.len()
            && self
                .changes
                .iter()
                .zip(&other.changes)
                .all(|(a, b)| a.structure == b.structure && a.key.is_some() && a.key == b.key)
    }
}

fn change(structure: StructureId, forward: Step, backward: Opaque) -> Change {
    Change {
        structure,
        key: None,
        forward: Half {
            step: forward,
            blob: None,
        },
        backward: Half {
            step: Step::Edit(backward),
            blob: None,
        },
    }
}

/// What a structure is, to a history: an identity and the name of the
/// vocabulary its payloads are written in.
#[derive(Debug, Clone, PartialEq)]
struct Structure {
    domain: String,
}

/// How many entries a history keeps before the oldest starts falling off.
pub const DEFAULT_BUDGET: usize = 256;

/// A payload at or above this many **bytes**, serialized, goes to the spill
/// store rather than staying in the pile. One kibibyte.
pub const DEFAULT_SPILL_ABOVE: usize = 1024;

/// One editing context's history: the structures it holds, and one order over
/// them.
///
/// Entries in the order they happened, plus a cursor: everything before it has
/// been applied, everything at or after it has been undone and is waiting to be
/// redone. Recording a new entry drops whatever the cursor was standing in
/// front of, which is what makes a new edit after an undo a fork rather than a
/// corruption.
pub struct History {
    structures: HashMap<StructureId, Structure>,
    entries: Vec<Entry>,
    cursor: usize,
    budget: usize,
    spill_above: usize,
    spill: Box<dyn Spill>,
}

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("structures", &self.structures.len())
            .field("entries", &self.entries.len())
            .field("cursor", &self.cursor)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// An empty history spilling to memory.
    pub fn new() -> Self {
        Self::with_spill(Box::new(MemorySpill::new()))
    }

    /// An empty history over a store of the caller's own.
    pub fn with_spill(spill: Box<dyn Spill>) -> Self {
        Self {
            structures: HashMap::new(),
            entries: Vec::new(),
            cursor: 0,
            budget: DEFAULT_BUDGET,
            spill_above: DEFAULT_SPILL_ABOVE,
            spill,
        }
    }

    /// Keeps at most `budget` entries, dropping the oldest past that.
    pub fn budget(mut self, budget: usize) -> Self {
        self.budget = budget.max(1);
        self
    }

    /// Spills a payload of at least this many serialized bytes.
    pub fn spill_above(mut self, bytes: usize) -> Self {
        self.spill_above = bytes;
        self
    }

    /// Takes a structure into this history and hands back its identity.
    ///
    /// `domain` names the vocabulary its payloads are written in — `"tree"` for
    /// the arrangement — and the history carries it so a caller routing what
    /// comes back knows which reader an entry's payload belongs to. Nothing
    /// here reads it.
    pub fn register(&mut self, domain: impl Into<String>) -> StructureId {
        let id = StructureId(NEXT_STRUCTURE.fetch_add(1, Ordering::Relaxed));
        self.structures.insert(
            id,
            Structure {
                domain: domain.into(),
            },
        );
        id
    }

    /// Whether this history minted that identity.
    pub fn holds(&self, structure: StructureId) -> bool {
        self.structures.contains_key(&structure)
    }

    /// The vocabulary a structure's payloads are written in.
    pub fn domain(&self, structure: StructureId) -> Option<&str> {
        self.structures.get(&structure).map(|s| s.domain.as_str())
    }

    /// Every structure registered here, in the order they were registered.
    pub fn structures(&self) -> Vec<StructureId> {
        let mut ids: Vec<StructureId> = self.structures.keys().copied().collect();
        ids.sort();
        ids
    }

    /// Records a transaction, dropping anything waiting to be redone.
    ///
    /// `false` when a leg names a structure this history did not mint — which
    /// is the rule "a structure belongs to exactly one history", enforced where
    /// it can be rather than asked for. Nothing is recorded in that case: an
    /// entry is one transaction, and half of one is worse than none.
    pub fn record(&mut self, mut entry: Entry) -> bool {
        if entry.is_empty() {
            return false;
        }
        if !entry.changes.iter().all(|c| self.holds(c.structure)) {
            return false;
        }
        self.forget_redo();
        for change in &mut entry.changes {
            lift(&mut change.forward, self.spill_above, self.spill.as_mut());
            lift(&mut change.backward, self.spill_above, self.spill.as_mut());
        }
        if entry.coalesce && self.merge(&entry) {
            return true;
        }
        self.entries.push(entry);
        self.cursor = self.entries.len();
        self.enforce_budget();
        true
    }

    /// What an undo *would* hand back, without moving the cursor: each leg's
    /// inverse, **in reverse order** — a transaction unwinds the way it was
    /// laid down.
    ///
    /// The pair [`History::peek_undo`]/[`History::step_back`] exists for callers
    /// that have to know the answer before committing to it — a binding whose
    /// protocol sizes a buffer and then fills it, where doing the work on the
    /// sizing call would undo twice and hand back the second answer. Inside
    /// Rust, [`History::undo`] is the two together and is what you want.
    pub fn peek_undo(&self) -> Option<Vec<(StructureId, Opaque)>> {
        let entry = self.entries.get(self.cursor.checked_sub(1)?)?;
        let mut out = Vec::with_capacity(entry.changes.len());
        for change in entry.changes.iter().rev() {
            // A backward half is always an edit -- `Entry`'s constructors take
            // a payload for it, so `Recompute` cannot get in here.
            if let Step::Edit(payload) = restore(&change.backward, self.spill.as_ref()) {
                out.push((change.structure, payload));
            }
        }
        Some(out)
    }

    /// What a redo *would* hand back, without moving the cursor. See
    /// [`History::peek_undo`].
    pub fn peek_redo(&self) -> Option<Vec<(StructureId, Step)>> {
        let entry = self.entries.get(self.cursor)?;
        Some(
            entry
                .changes
                .iter()
                .map(|change| {
                    (
                        change.structure,
                        restore(&change.forward, self.spill.as_ref()),
                    )
                })
                .collect(),
        )
    }

    /// Moves the cursor back one entry, if it can. The commit half of
    /// [`History::peek_undo`].
    pub fn step_back(&mut self) -> bool {
        let can = self.can_undo();
        if can {
            self.cursor -= 1;
        }
        can
    }

    /// Moves the cursor forward one entry, if it can. The commit half of
    /// [`History::peek_redo`].
    pub fn step_forward(&mut self) -> bool {
        let can = self.can_redo();
        if can {
            self.cursor += 1;
        }
        can
    }

    /// Undoes the last thing done, in any structure: the inverses of the entry
    /// before the cursor, each with the structure it belongs to, or `None` when
    /// there is nothing to undo.
    ///
    /// The caller applies them, through whatever door the domain has.
    pub fn undo(&mut self) -> Option<Vec<(StructureId, Opaque)>> {
        let out = self.peek_undo()?;
        self.step_back();
        Some(out)
    }

    /// Redoes what was last undone, in order, or `None` when there is nothing.
    ///
    /// Returns [`Step`]s rather than payloads: a deterministic operation stores
    /// its parameters instead of its result, and re-running it is the owner's
    /// to do.
    pub fn redo(&mut self) -> Option<Vec<(StructureId, Step)>> {
        let out = self.peek_redo()?;
        self.step_forward();
        Some(out)
    }

    /// What an undo would be called, for a menu.
    pub fn undo_label(&self) -> Option<&str> {
        self.entries
            .get(self.cursor.checked_sub(1)?)
            .map(|e| e.label.as_str())
    }

    /// What a redo would be called.
    pub fn redo_label(&self) -> Option<&str> {
        self.entries.get(self.cursor).map(|e| e.label.as_str())
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }

    /// How many entries are held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forgets every entry, releasing what was spilled — what closing an
    /// editing context leaves behind. The structures stay registered: it is the
    /// order that is gone, not the identities the caller still holds.
    pub fn clear(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        for entry in entries {
            self.release(&entry);
        }
        self.cursor = 0;
    }

    fn merge(&mut self, entry: &Entry) -> bool {
        let Some(last) = self.entries.last_mut() else {
            return false;
        };
        if !last.matches(entry) {
            return false;
        }
        // The merged entry keeps the **oldest** inverse and the **newest**
        // forward, which is what makes one undo of a hundred small adjustments
        // land where the run started rather than one step back into it.
        let mut released = Vec::new();
        for (old, new) in last.changes.iter_mut().zip(&entry.changes) {
            if let Some(id) = std::mem::replace(&mut old.forward.blob, new.forward.blob) {
                released.push(id);
            }
            old.forward.step = new.forward.step.clone();
            if let Some(id) = new.backward.blob {
                released.push(id);
            }
        }
        last.label = entry.label.clone();
        for id in released {
            self.spill.release(id);
        }
        true
    }

    fn forget_redo(&mut self) {
        if self.cursor >= self.entries.len() {
            return;
        }
        let dropped: Vec<Entry> = self.entries.drain(self.cursor..).collect();
        for entry in dropped {
            self.release(&entry);
        }
    }

    fn enforce_budget(&mut self) {
        while self.entries.len() > self.budget {
            let oldest = self.entries.remove(0);
            self.release(&oldest);
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    fn release(&mut self, entry: &Entry) {
        for change in &entry.changes {
            for blob in [change.forward.blob, change.backward.blob]
                .into_iter()
                .flatten()
            {
                self.spill.release(blob);
            }
        }
    }
}

/// Moves a payload out of a half and into the store, when it is big enough to
/// be worth it.
fn lift(half: &mut Half, threshold: usize, spill: &mut dyn Spill) {
    let (Step::Edit(payload) | Step::Recompute(payload)) = &mut half.step;
    let Ok(bytes) = serde_json::to_vec(payload) else {
        return;
    };
    if bytes.len() < threshold {
        return;
    }
    half.blob = Some(spill.put(&bytes));
    *payload = Opaque::none();
}

/// Puts a spilled payload back, giving the caller a step it can act on.
fn restore(half: &Half, spill: &dyn Spill) -> Step {
    let mut step = half.step.clone();
    let Some(id) = half.blob else {
        return step;
    };
    let (Step::Edit(payload) | Step::Recompute(payload)) = &mut step;
    if let Some(bytes) = spill.get(id)
        && let Ok(restored) = serde_json::from_slice::<Opaque>(&bytes)
    {
        *payload = restored;
    }
    step
}

#[cfg(test)]
mod tests;
