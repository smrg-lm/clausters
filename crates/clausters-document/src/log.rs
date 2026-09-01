//! Undo for the arrangement, beside the data it inverts.
//!
//! The pile itself is [`history`](crate::history), which knows no vocabulary at
//! all. This module is its arrangement-shaped face: a [`Log`] is a
//! [`History`](crate::history::History) with one structure registered in it, and
//! an [`Entry`] states its halves as [`Intent`]s rather than as opaque payloads.
//! There is one implementation of the pile, and this is not a second one.
//!
//! The placement is the whole of the design. A GUI host holding its own log
//! knows only the gestures *it* made — so a script editing the arrangement, a
//! second editor, or a re-render would leave that log describing a document
//! that had moved on, and undo would write a state nobody was ever in. Here the
//! log sees every edit, because every edit comes through
//! [`crate::intent::apply`].
//!
//! # An entry is a transaction, and its inverse is an ordinary intent
//!
//! Undo needs no second code path: the inverse of an entry is an [`Intent`],
//! handed back to `apply` like any other. That is what the absolute vocabulary
//! is for — an edit states the resulting value, so the edit that states the
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
//! pair that names the same bytes holds one copy. See
//! [`history`](crate::history) for the mechanism; what is arrangement-specific
//! is only which edit gets big.

use crate::history::{self, History, StructureId};
use crate::intent::{Against, Intent, Outcome, Rules, current};
use crate::{Document, Opaque};

pub use crate::history::{DEFAULT_BUDGET, DEFAULT_SPILL_ABOVE, MemorySpill, Spill, SpillId};

/// The domain name a document's structure is registered under.
///
/// The arrangement's vocabulary is [`Intent`], and this is what a caller
/// routing a history's payloads matches on to know it is holding one.
pub const TREE: &str = "tree";

/// One move in the forward direction, in the arrangement's vocabulary.
///
/// [`history::Step`] with an [`Intent`] in place of the opaque payload, and the
/// same wire shape: externally tagged — `{"edit": <intent>}`,
/// `{"recompute": <params>}` — because [`Intent`] is already internally tagged
/// on `"intent"` and two tags in one object is how a format grows a bug nobody
/// can read.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    /// An ordinary edit. Hand it to [`crate::intent::apply`].
    Edit(Intent),
    /// A deterministic operation to **re-run**, carried as the owner's own
    /// parameters and never interpreted here. The crate cannot execute one —
    /// it holds no algorithms — so a caller that stores these must be ready to
    /// perform them on redo. What it gives is a redo whose cost follows the
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

    fn generic(&self) -> history::Step {
        match self {
            Step::Edit(intent) => history::Step::Edit(payload(intent)),
            Step::Recompute(params) => history::Step::Recompute(params.clone()),
        }
    }

    fn typed(step: history::Step) -> Option<Step> {
        match step {
            history::Step::Edit(payload) => intent_of(&payload).map(Step::Edit),
            history::Step::Recompute(params) => Some(Step::Recompute(params)),
        }
    }
}

/// An intent as the pile carries it. Total: every [`Intent`] is serde data.
fn payload(intent: &Intent) -> Opaque {
    Opaque(serde_json::to_value(intent).unwrap_or(serde_json::Value::Null))
}

/// The intent a payload holds, or `None` if it will not read as one — which
/// only a payload some other domain wrote can be.
fn intent_of(payload: &Opaque) -> Option<Intent> {
    serde_json::from_value(payload.0.clone()).ok()
}

/// What makes two edits *the same thing done the same way*: the kind of edit
/// and the node it names. The pile cannot compute it — that is a sentence in
/// this vocabulary — so an entry carries it.
fn coalesce_key(step: &Step) -> Option<String> {
    let intent = step.intent()?;
    let kind = match intent {
        Intent::Place { .. } => "place",
        Intent::Configure { .. } => "configure",
        Intent::SetMembers { .. } => "setmembers",
        Intent::WriteSamples { .. } => "writesamples",
    };
    Some(format!("{kind}:{}", intent.node().0))
}

/// A single reversible edit: how to redo it, and how to undo it.
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
    changes: Vec<(Step, Intent)>,
}

impl Entry {
    /// One edit and its inverse.
    pub fn new(label: impl Into<String>, forward: Step, backward: Intent) -> Self {
        Self {
            label: label.into(),
            coalesce: false,
            changes: vec![(forward, backward)],
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
        self.changes.push((forward, backward));
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

    /// This entry as the pile holds it: every leg over the one structure a
    /// [`Log`] has, each carrying the key that decides a merge.
    fn generic(&self, structure: StructureId) -> history::Entry {
        let mut legs = self.changes.iter();
        // An empty entry is refused by `History::record`, so the fold below
        // needs a first leg and this is where that is decided.
        let Some((forward, backward)) = legs.next() else {
            return history::Entry::new(
                self.label.clone(),
                structure,
                history::Step::Edit(Opaque::none()),
                Opaque::none(),
            );
        };
        let mut entry =
            history::Entry::new(&self.label, structure, forward.generic(), payload(backward));
        if let Some(key) = coalesce_key(forward) {
            entry = entry.keyed(key);
        }
        for (forward, backward) in legs {
            entry = entry.and(structure, forward.generic(), payload(backward));
            if let Some(key) = coalesce_key(forward) {
                entry = entry.keyed(key);
            }
        }
        if self.coalesce {
            entry.continuing()
        } else {
            entry
        }
    }
}

/// The undo history of one document.
///
/// A [`History`] with one structure registered in it, read in the arrangement's
/// vocabulary. Two views of one document share a `Log`; a context holding
/// *several* editable structures registers them in a `History` directly, which
/// is where the general shape is.
pub struct Log {
    history: History,
    structure: StructureId,
}

impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Log")
            .field("entries", &self.history.len())
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
        Self::over(History::new())
    }

    /// An empty log over a store of the caller's own.
    pub fn with_spill(spill: Box<dyn Spill>) -> Self {
        Self::over(History::with_spill(spill))
    }

    /// A log over a history of the caller's own, registering the document in
    /// it.
    pub fn over(mut history: History) -> Self {
        let structure = history.register(TREE);
        Self { history, structure }
    }

    /// Keeps at most `budget` entries, dropping the oldest past that.
    pub fn budget(mut self, budget: usize) -> Self {
        self.history = self.history.budget(budget);
        self
    }

    /// Spills a payload of at least this many serialized **bytes**.
    pub fn spill_above(mut self, bytes: usize) -> Self {
        self.history = self.history.spill_above(bytes);
        self
    }

    /// The pile this log is a face of, and the structure the document is
    /// registered as — what a caller composing several editable structures in
    /// one context reaches for.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The pile, to record over more than the document. See [`Log::history`].
    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// The document's identity within this history.
    pub fn structure(&self) -> StructureId {
        self.structure
    }

    /// Records a transaction, dropping anything waiting to be redone.
    pub fn record(&mut self, entry: Entry) {
        if entry.is_empty() {
            return;
        }
        self.history.record(entry.generic(self.structure));
    }

    /// What an undo *would* hand back, without moving the cursor.
    ///
    /// The pair [`Log::peek_undo`]/[`Log::step_back`] exists for callers that
    /// have to know the answer before committing to it — a binding whose
    /// protocol sizes a buffer and then fills it, where doing the work on the
    /// sizing call would undo twice and hand back the second answer. Inside
    /// Rust, [`Log::undo`] is the two together and is what you want.
    pub fn peek_undo(&self) -> Option<Vec<Intent>> {
        Some(
            self.history
                .peek_undo()?
                .iter()
                .filter_map(|(_, payload)| intent_of(payload))
                .collect(),
        )
    }

    /// What a redo *would* hand back, without moving the cursor. See
    /// [`Log::peek_undo`].
    pub fn peek_redo(&self) -> Option<Vec<Step>> {
        Some(
            self.history
                .peek_redo()?
                .into_iter()
                .filter_map(|(_, step)| Step::typed(step))
                .collect(),
        )
    }

    /// Moves the cursor back one entry, if it can. The commit half of
    /// [`Log::peek_undo`].
    pub fn step_back(&mut self) -> bool {
        self.history.step_back()
    }

    /// Moves the cursor forward one entry, if it can. The commit half of
    /// [`Log::peek_redo`].
    pub fn step_forward(&mut self) -> bool {
        self.history.step_forward()
    }

    /// Undoes the last thing done: the inverses of the entry before the cursor,
    /// **in reverse order** (a transaction's edits unwind the way they were
    /// laid down), or `None` when there is nothing to undo.
    ///
    /// The caller applies them. The log does not touch the document, because
    /// applying is [`crate::intent::apply`]'s job and having two things that
    /// edit is exactly what this crate exists to prevent.
    pub fn undo(&mut self) -> Option<Vec<Intent>> {
        let out = self.peek_undo()?;
        self.step_back();
        Some(out)
    }

    /// Redoes what was last undone, in order, or `None` when there is nothing.
    ///
    /// Returns [`Step`]s rather than intents: a deterministic operation stores
    /// its parameters instead of its result, and re-running it is the owner's
    /// to do.
    pub fn redo(&mut self) -> Option<Vec<Step>> {
        let out = self.peek_redo()?;
        self.step_forward();
        Some(out)
    }

    /// What an undo would be called, for a menu.
    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    /// What a redo would be called.
    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_label()
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// How many entries are held.
    pub fn len(&self) -> usize {
        self.history.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    /// Forgets everything, releasing what was spilled — what closing a document
    /// or loading another one leaves behind. A history of edits to a document
    /// that is no longer open inverts nothing.
    pub fn clear(&mut self) {
        self.history.clear();
    }
}

/// The edit that would put this node back the way it is — the inverse of
/// `intent`, read out of the document before anything is applied.
///
/// The whole of what makes undo cheap here: an absolute intent states a value,
/// so the edit stating the *previous* value is its inverse, and the document
/// already knows it. `None` when the document cannot describe it — the node is
/// gone, or its body holds nothing of that shape — and for
/// [`Intent::WriteSamples`], where the samples are not in the document, it is
/// the empty write rather than the span, which is why a destructive caller
/// reads its own span before writing.
///
/// Public because a caller that records its own entries needs exactly this, and
/// because a GUI host's "every intent carries its previous value" is this
/// function on the owner's side.
pub fn inverse_of(document: &Document, intent: &Intent) -> Option<Intent> {
    current(document, intent)
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
