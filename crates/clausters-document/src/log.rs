//! Undo, beside the data it inverts.
//!
//! The log lives with the document and not with the view, and that placement is
//! the whole of this module's design. A GUI host holding its own log knows only
//! the gestures *it* made — so a script editing the arrangement, a second
//! editor, or a re-render would leave that log describing a document that had
//! moved on, and undo would write a state nobody was ever in. Here the log sees
//! every edit, because every edit comes through [`crate::intent::apply`].
//!
//! # An entry is a transaction, and its inverse is an ordinary intent
//!
//! Undo needs no second code path: the inverse of an entry is an [`Intent`],
//! handed back to `apply` like any other. That is what the absolute vocabulary
//! buys — an edit states the resulting value, so the edit that states the
//! *previous* value is its inverse, and the document already knows how to
//! compute it (the same reader O4's staleness check uses).
//!
//! # Forward and backward are not the same kind of thing
//!
//! Going **back** is always data: undoing a normalize means writing the old
//! samples, and no algorithm reconstructs them. Going **forward** need not be —
//! a deterministic operation can store its *parameters* and be re-run, which is
//! how a redo of an edit over a million samples costs a few bytes instead of
//! four megabytes. That asymmetry is only available because the log sits with
//! the document: the owner has the algorithm, and the host — which was going to
//! hold this log — never did. So [`Log::redo`] hands back [`Step`]s, one of
//! which the caller must re-run itself, while [`Log::undo`] hands back plain
//! intents.
//!
//! # What never enters the log
//!
//! Only an edit that **changed the document** is recorded, and only when it
//! goes in through [`apply_logged`]. What comes *back* from an owner after an
//! edit — the state push that answers a gesture — is not an edit and is not
//! recorded: it is the document describing itself, and logging it would make
//! every undo two steps deep. A refused edit is not recorded either, for the
//! same reason a refusal does not move the version.
//!
//! # Big payloads leave the log
//!
//! A sample write's inverse is the span it overwrote, which is the one thing
//! here whose size follows the audio rather than the parameters. Above a
//! threshold it goes to a [`Spill`] store — content-addressed, so an undo/redo
//! pair that names the same bytes holds one copy — and the intent kept in the
//! log carries an empty payload until [`Log::undo`] puts it back. The store is
//! a trait because where it belongs follows the deployment (a temporary
//! directory natively, memory in a page), and [`MemorySpill`] is the one the
//! crate ships.

use std::collections::HashMap;

use crate::intent::{Against, Intent, Outcome, Rules, current};
use crate::{Document, Opaque};

/// A blob in a [`Spill`] store, named by its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpillId(pub u64);

/// Somewhere to put an inverse whose content is data rather than parameters.
///
/// Content-addressed: [`Spill::put`] of the same bytes twice returns the same
/// id and holds one copy, which is what keeps an undo/redo pair — the same span
/// named from both sides — from doubling. Each `put` takes a reference and each
/// [`Spill::release`] drops one, so a store is free to discard a blob once the
/// log has forgotten it.
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
    blobs: HashMap<u64, (Vec<u8>, u32)>,
}

impl MemorySpill {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many distinct blobs are held — what a budget test watches.
    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }
}

impl Spill for MemorySpill {
    fn put(&mut self, bytes: &[u8]) -> SpillId {
        // FNV-1a, so the crate carries no hash dependency for something whose
        // only job is to make identical payloads land on the same slot.
        let mut key = 0xcbf2_9ce4_8422_2325u64;
        for byte in bytes {
            key ^= u64::from(*byte);
            key = key.wrapping_mul(0x1000_0000_01b3);
        }
        // Probe rather than trust the hash: a collision would silently hand
        // back somebody else's samples, which is a corrupted undo.
        loop {
            match self.blobs.get_mut(&key) {
                Some((held, refs)) if held == bytes => {
                    *refs += 1;
                    return SpillId(key);
                }
                Some(_) => key = key.wrapping_add(1),
                None => {
                    self.blobs.insert(key, (bytes.to_vec(), 1));
                    return SpillId(key);
                }
            }
        }
    }

    fn get(&self, id: SpillId) -> Option<Vec<u8>> {
        self.blobs.get(&id.0).map(|(bytes, _)| bytes.clone())
    }

    fn release(&mut self, id: SpillId) {
        if let Some((_, refs)) = self.blobs.get_mut(&id.0) {
            *refs -= 1;
            if *refs == 0 {
                self.blobs.remove(&id.0);
            }
        }
    }
}

/// One move in the forward direction.
///
/// Only the forward direction has two shapes: see the module docs on why going
/// back is always data.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// An ordinary edit. Hand it to [`crate::intent::apply`].
    Edit(Intent),
    /// A deterministic operation to **re-run**, carried as the owner's own
    /// parameters and never interpreted here. The crate cannot execute one —
    /// it holds no algorithms — so a caller that stores these must be ready to
    /// perform them on redo. What it buys is a redo whose cost follows the
    /// parameters instead of the audio.
    Recompute(Opaque),
}

impl Step {
    /// The edit, for the ordinary case.
    pub fn intent(&self) -> Option<&Intent> {
        match self {
            Step::Edit(intent) => Some(intent),
            Step::Recompute(_) => None,
        }
    }
}

/// One half of a change, with any bulk payload lifted out of it.
#[derive(Debug, Clone, PartialEq)]
struct Half {
    step: Step,
    blob: Option<SpillId>,
}

/// A single reversible edit: how to redo it, and how to undo it.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    forward: Half,
    backward: Half,
}

/// One transaction in the log: a gesture, and what it takes to reverse it.
///
/// The unit is the **gesture**, not the intent, because that is what a person
/// means by "the last thing I did" — and because one gesture already produces
/// one intent by the vocabulary's own rule, an entry usually holds one change.
/// It holds several when a script applies a batch it wants undone as a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// What to call this in a menu. The log never reads it.
    pub label: String,
    /// Whether this may merge into the entry before it when they touch the
    /// same thing the same way — a run of small adjustments becoming one undo
    /// instead of two hundred. The caller decides when a run is continuous,
    /// because only the caller knows where the hand stopped.
    pub coalesce: bool,
    changes: Vec<Change>,
}

impl Entry {
    /// One edit and its inverse.
    pub fn new(label: impl Into<String>, forward: Step, backward: Intent) -> Self {
        Self {
            label: label.into(),
            coalesce: false,
            changes: vec![Change {
                forward: Half {
                    step: forward,
                    blob: None,
                },
                backward: Half {
                    step: Step::Edit(backward),
                    blob: None,
                },
            }],
        }
    }

    /// Marks this entry as continuing the one before it (see
    /// [`Entry::coalesce`]).
    pub fn continuing(mut self) -> Self {
        self.coalesce = true;
        self
    }

    /// Adds another edit to the same transaction. Applied in order forward, in
    /// reverse order backward.
    pub fn and(mut self, forward: Step, backward: Intent) -> Self {
        self.changes.push(Change {
            forward: Half {
                step: forward,
                blob: None,
            },
            backward: Half {
                step: Step::Edit(backward),
                blob: None,
            },
        });
        self
    }

    /// How many edits this transaction holds.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Whether this entry and `other` touch the same nodes the same way — the
    /// test for merging a continuing entry into the one before it.
    fn matches(&self, other: &Entry) -> bool {
        self.changes.len() == other.changes.len()
            && self
                .changes
                .iter()
                .zip(&other.changes)
                .all(|(a, b)| same_shape(&a.forward.step, &b.forward.step))
    }
}

fn same_shape(a: &Step, b: &Step) -> bool {
    match (a, b) {
        (Step::Edit(a), Step::Edit(b)) => {
            a.node() == b.node() && std::mem::discriminant(a) == std::mem::discriminant(b)
        }
        // A recompute carries an opaque payload the crate cannot compare
        // meaningfully, and merging two operations into one is the caller's
        // decision anyway. So it never coalesces.
        _ => false,
    }
}

/// The undo history of one document.
///
/// Entries in the order they happened, plus a cursor: everything before it has
/// been applied, everything at or after it has been undone and is waiting to be
/// redone. Recording a new entry drops whatever the cursor was standing in
/// front of, which is what makes a new edit after an undo a fork rather than a
/// corruption.
pub struct Log {
    entries: Vec<Entry>,
    cursor: usize,
    budget: usize,
    spill_above: usize,
    spill: Box<dyn Spill>,
}

/// How many entries a log keeps before the oldest starts falling off.
pub const DEFAULT_BUDGET: usize = 256;

/// Sample payloads at or above this many values go to the spill store rather
/// than staying in the log. One kibibyte of `f32`.
pub const DEFAULT_SPILL_ABOVE: usize = 256;

impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Log")
            .field("entries", &self.entries.len())
            .field("cursor", &self.cursor)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl Default for Log {
    fn default() -> Self {
        Self::new()
    }
}

impl Log {
    /// An empty log spilling to memory.
    pub fn new() -> Self {
        Self::with_spill(Box::new(MemorySpill::new()))
    }

    /// An empty log over a store of the caller's own.
    pub fn with_spill(spill: Box<dyn Spill>) -> Self {
        Self {
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

    /// Spills sample payloads of at least this many values.
    pub fn spill_above(mut self, values: usize) -> Self {
        self.spill_above = values;
        self
    }

    /// Records a transaction, dropping anything waiting to be redone.
    pub fn record(&mut self, mut entry: Entry) {
        if entry.is_empty() {
            return;
        }
        self.forget_redo();
        for change in &mut entry.changes {
            lift(&mut change.forward, self.spill_above, self.spill.as_mut());
            lift(&mut change.backward, self.spill_above, self.spill.as_mut());
        }
        if entry.coalesce && self.merge(&entry) {
            return;
        }
        self.entries.push(entry);
        self.cursor = self.entries.len();
        self.enforce_budget();
    }

    /// Undoes the last thing done: the inverses of the entry before the cursor,
    /// **in reverse order** (a transaction's edits unwind the way they were
    /// laid down), or `None` when there is nothing to undo.
    ///
    /// The caller applies them. The log does not touch the document, because
    /// applying is [`crate::intent::apply`]'s job and having two things that
    /// edit is exactly what this crate exists to prevent.
    pub fn undo(&mut self) -> Option<Vec<Intent>> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        let entry = &self.entries[self.cursor];
        let mut out = Vec::with_capacity(entry.changes.len());
        for change in entry.changes.iter().rev() {
            // A backward half is always an edit -- `Entry`'s constructors take
            // an `Intent` for it, so `Recompute` cannot get in here.
            if let Step::Edit(intent) = materialize(&change.backward, self.spill.as_ref()) {
                out.push(intent);
            }
        }
        Some(out)
    }

    /// Redoes what was last undone, in order, or `None` when there is nothing.
    ///
    /// Returns [`Step`]s rather than intents: a deterministic operation stores
    /// its parameters instead of its result, and re-running it is the owner's
    /// to do.
    pub fn redo(&mut self) -> Option<Vec<Step>> {
        let entry = self.entries.get(self.cursor)?;
        let out = entry
            .changes
            .iter()
            .map(|change| materialize(&change.forward, self.spill.as_ref()))
            .collect();
        self.cursor += 1;
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

    /// Forgets everything, releasing what was spilled — what closing a document
    /// or loading another one leaves behind. A history of edits to a document
    /// that is no longer open inverts nothing.
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
            if let Some(id) = old.forward.blob.replace_with(new.forward.blob) {
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

/// Swaps an `Option<SpillId>` in place, handing back what was there.
trait ReplaceWith {
    fn replace_with(&mut self, value: Option<SpillId>) -> Option<SpillId>;
}

impl ReplaceWith for Option<SpillId> {
    fn replace_with(&mut self, value: Option<SpillId>) -> Option<SpillId> {
        std::mem::replace(self, value)
    }
}

/// Moves a sample payload out of a half and into the store, when it is big
/// enough to be worth it.
fn lift(half: &mut Half, threshold: usize, spill: &mut dyn Spill) {
    let Step::Edit(Intent::WriteSamples { values, .. }) = &mut half.step else {
        return;
    };
    if values.len() < threshold {
        return;
    }
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values.iter() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    half.blob = Some(spill.put(&bytes));
    values.clear();
}

/// Puts a spilled payload back, giving the caller a step it can act on.
fn materialize(half: &Half, spill: &dyn Spill) -> Step {
    let mut step = half.step.clone();
    let Some(id) = half.blob else {
        return step;
    };
    let Step::Edit(Intent::WriteSamples { values, .. }) = &mut step else {
        return step;
    };
    if let Some(bytes) = spill.get(id) {
        *values = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
    }
    step
}

/// Apply an edit and record it, in one call.
///
/// The **only** way an entry gets into a log by itself, and that is what makes
/// the rule mechanical rather than a habit: the inverse is read out of the
/// document *before* the edit lands, and nothing is recorded unless the
/// document actually changed. A refusal — stale or otherwise — leaves no entry,
/// for the same reason it does not move the version.
///
/// `WriteSamples` is the one edit this cannot log on its own: the samples it
/// overwrote are not in the document, so there is nothing here to read the
/// inverse from. A caller doing destructive edits reads the span it is about to
/// write, applies, and records the pair itself with [`Log::record`].
pub fn apply_logged(
    document: &mut Document,
    intent: &Intent,
    against: &Against,
    rules: &Rules,
    log: &mut Log,
    label: impl Into<String>,
) -> Outcome {
    let before = current(document, intent);
    let outcome = crate::intent::apply(document, intent, against, rules);
    if !outcome.applied {
        return outcome;
    }
    if let Some(before) = before {
        log.record(Entry::new(
            label,
            Step::Edit(outcome.effective.clone()),
            before,
        ));
    }
    outcome
}

#[cfg(test)]
mod tests;
