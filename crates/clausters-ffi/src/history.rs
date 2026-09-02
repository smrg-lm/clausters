//! The edit history's C surface: a **handle** naming one editing context, and
//! the structures registered in it.
//!
//! # Why a handle, when the document crosses by value
//!
//! The document's binding round-trips the format, and that is right for it: a
//! document is what the caller is editing, and the alternative was dozens of
//! accessors. A history is the opposite shape. It is bookkeeping the caller
//! never reads field by field, and — the deciding term — **a bulk payload
//! leaves the pile on purpose**. Sending it by value would carry every spilled
//! span back and forth on each call, which is precisely the cost spilling
//! exists to avoid: a history at its default budget holding sample inverses is
//! megabytes. So it stays in Rust with its spill store, and the caller holds a
//! pointer, exactly as it does for the id registry.
//!
//! # One handle is one editing context
//!
//! A history holds the structures registered in it and one ordered pile over
//! them, so what a caller decides by choosing a handle is *what shares an undo
//! order*. A structure the client built with no composition behind it is a
//! history with one structure in it; an application composing several editable
//! views registers them all in one; two views of one structure hold one handle
//! between them, which is the arrangement the crate exists to make the only
//! expressible one. [`clausters_history_register`] mints the identity, and every
//! call that names one takes it.
//!
//! # Applying and recording are one call; undoing is not
//!
//! [`clausters_history_apply`] is the arrangement's applying door, and the two
//! being one call is deliberate rather than convenient: the inverse has to be
//! read out of the document **before** the edit lands, so a surface that let a
//! caller apply first and record second would let it record the wrong thing.
//! It is the arrangement's alone because the document is the one state this
//! surface can reach; a caller editing anything else applies the edit itself
//! and hands the pair to [`clausters_history_record`].
//!
//! **Undo and redo apply nothing**, and that is the shape a history with
//! several structures forces. The crate can reach one document handle and no
//! curve, no buffer and no roll, so applying "what it can" would apply one
//! structure's legs and leave the rest to the caller — out of order, which is
//! how a transaction half-happens. So both directions hand back the payloads
//! with the structure each belongs to, in the order they must be applied, and
//! the caller applies them through whatever door each domain has. What stays
//! the crate's is the *decision*: a redo reports the leading run of ordinary
//! edits separately from the steps the owner must re-run, and stops at the
//! first of those rather than skipping it, so a later edit is never applied
//! over a state the operation before it was meant to produce.
//!
//! # Size-then-fill against a surface that mutates
//!
//! The rest of this crate's JSON surface sizes with a null `out` and fills with
//! a second call, which needs the payload to be **identical on both calls**.
//! A cursor moves, so a naive implementation would undo on the sizing call and
//! hand back "nothing to undo" on the fill.
//!
//! The rule that resolves it: **the mutation happens only when the bytes are
//! actually written.** Each call computes what it *would* do without touching
//! the pile, writes if the buffer fits, and commits only then. So a sizing pass
//! is free of consequence and a run of them is idempotent, which is what a
//! binding needs to be able to assume.

use std::sync::Mutex;

use clausters_document::history::{Entry, History, Step, StructureId};
use clausters_document::{Against, Intent, Opaque, Rules, apply as apply_intent};

use crate::document::{FfiDocument, fill, text, with_document};

/// A history handle safe to share across the binding's threads.
pub struct FfiHistory(Mutex<History>);

fn with_history<T>(h: *mut FfiHistory, default: T, f: impl FnOnce(&mut History) -> T) -> T {
    // SAFETY: caller guarantees `h` is a live history handle (or null).
    let Some(history) = (unsafe { h.as_ref() }) else {
        return default;
    };
    f(&mut history.0.lock().expect("history lock poisoned"))
}

/// A leg as it crosses: the structure it belongs to, and the payload.
fn leg(structure: StructureId, key: &str, load: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "structure": structure.0, key: load })
}

/// A new, empty history. `budget` is how many entries it keeps before the
/// oldest falls off (0 takes the default), `spill_above` how many **bytes** a
/// payload must reach, serialized, before it leaves the pile for the spill
/// store (0 takes the default). It is a byte count rather than a sample count
/// because the pile holds payloads it does not read: the size it can measure is
/// the payload's.
///
/// The store is **memory**. A file-backed one is a caller's to supply in Rust
/// (the `Spill` trait) and is not reachable from here yet — see the document
/// crate's `PLAN.md`, where it waits on a caller that actually needs it. Free
/// with [`clausters_history_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_history_new(budget: usize, spill_above: usize) -> *mut FfiHistory {
    let mut history = History::new();
    if budget > 0 {
        history = history.budget(budget);
    }
    if spill_above > 0 {
        history = history.spill_above(spill_above);
    }
    Box::into_raw(Box::new(FfiHistory(Mutex::new(history))))
}

/// Frees a history created by [`clausters_history_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_history_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_free(h: *mut FfiHistory) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Takes a structure into this history and returns its identity, or `0` when
/// the handle is null.
///
/// `domain` names the vocabulary its payloads are written in — `"tree"` for the
/// arrangement, `"points"` for a break-point curve — and the history carries it
/// so a caller routing what comes back knows which reader a leg's payload
/// belongs to. Nothing in the crate reads it.
///
/// The identity is minted here rather than carried by the data, because a
/// structure a client built has no id and is not going to be given a stable one
/// for this. It is also the read-back path: the identity that opened an
/// editable view is the one its edited state is read out through.
///
/// # Safety
/// `h` must be a live history handle and `domain` null or readable for
/// `domain_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_register(
    h: *mut FfiHistory,
    domain: *const u8,
    domain_len: usize,
) -> u64 {
    // SAFETY: forwarded from this function's own contract.
    let domain = unsafe { text(domain, domain_len) }.unwrap_or(std::borrow::Cow::Borrowed(""));
    with_history(h, 0, |history| history.register(domain.as_ref()).0)
}

/// Apply an edit to `doc` and record it against `structure`, in one call — the
/// arrangement's only door for an ordinary entry.
///
/// Arguments are [`crate::clausters_document_apply`]'s, plus the history
/// handle, the structure the document is registered as, and a `label` (what an
/// undo menu calls this). Writes the same outcome object and returns the byte
/// count it needs, or `0` when a handle is null or the intent will not parse.
/// The document is **not** in the reply — it stays in its handle, and a caller
/// that wants it asks [`crate::clausters_document_snapshot`].
///
/// Nothing is recorded unless the document actually changed, so a refusal —
/// stale or otherwise — leaves no entry, and neither does a resend. An entry
/// naming a structure this history did not mint is refused: the edit still
/// applies, because the caller asked for it against a document it holds, and
/// the history says so by recording nothing.
///
/// # Safety
/// `h` must be a live history handle and `doc` a live document handle; the
/// payloads must be null or readable for their lengths, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn clausters_history_apply(
    h: *mut FfiHistory,
    structure: u64,
    doc: *mut FfiDocument,
    intent: *const u8,
    intent_len: usize,
    against: *const u8,
    against_len: usize,
    quant: f64,
    label: *const u8,
    label_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(intent) = (unsafe { text(intent, intent_len) }) else {
        return 0;
    };
    let Ok(intent) = serde_json::from_str::<Intent>(&intent) else {
        return 0;
    };
    // SAFETY: as above.
    let against = unsafe { text(against, against_len) }
        .and_then(|raw| serde_json::from_str::<Against>(&raw).ok())
        .unwrap_or_else(Against::unstated);
    // SAFETY: as above.
    let label = unsafe { text(label, label_len) }.unwrap_or(std::borrow::Cow::Borrowed("edit"));
    let structure = StructureId(structure);

    with_document(doc, 0, |held| {
        // The inverse is read *before* the edit lands -- which is the whole
        // reason applying and recording are one call -- and it is also what
        // rolls the edit back if the caller's buffer did not fit, so the tree
        // is never cloned to protect a sizing pass. `document::edit_in_place`
        // carries the same reasoning at more length.
        // A `WriteSamples` is excluded by kind: it moves the **source's
        // generation** as well as the version, and an inverse write moves it
        // again, so rolling one back would leave the generation two ahead of
        // where it started. `document::edit_in_place` carries the full note.
        let inverse = match intent {
            Intent::WriteSamples { .. } => None,
            _ => clausters_document::inverse_of(&held.document, &intent),
        };
        let Some(inverse) = inverse else {
            // Nothing safe to roll back with, so it runs on a copy -- and it
            // records nothing either: a destructive write's entry comes from
            // the caller through `clausters_history_record`, which is the door
            // for exactly this case, because its overwritten samples are not in
            // the tree for the crate to read.
            let mut edited = held.document.clone();
            let outcome = apply_intent(&mut edited, &intent, &against, &Rules { quant });
            // SAFETY: forwarded from this function's own contract.
            return unsafe {
                fill(&outcome_bytes(&outcome), out, out_cap, || {
                    held.commit(edited)
                })
            };
        };

        let version = held.document.version;
        let outcome = apply_intent(&mut held.document, &intent, &against, &Rules { quant });
        let bytes = outcome_bytes(&outcome);
        if !out.is_null() && out_cap >= bytes.len() {
            // SAFETY: out is writable for out_cap >= bytes.len() bytes.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
            held.pending = Vec::new();
            if outcome.applied {
                let entry = Entry::new(
                    label.as_ref(),
                    structure,
                    Step::Edit(clausters_document::log::payload(&outcome.effective)),
                    clausters_document::log::payload(&inverse),
                )
                .keyed(clausters_document::log::coalesce_key(&outcome.effective));
                with_history(h, false, |history| history.record(entry));
            }
        } else if outcome.applied {
            apply_intent(
                &mut held.document,
                &inverse,
                &Against::unstated(),
                &Rules::default(),
            );
            held.document.version = version;
        }
        bytes.len()
    })
}

/// The outcome object every applying call answers with — small, and the reason
/// a second size-then-fill pass over one costs nothing.
fn outcome_bytes(outcome: &clausters_document::Outcome) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "effective": outcome.effective,
        "applied": outcome.applied,
        "reason": outcome.reason,
        "stale": outcome.stale,
    }))
    .unwrap_or_default()
}

/// Record one entry — the door for everything [`clausters_history_apply`]
/// cannot do: a destructive write, whose overwritten samples are not in the
/// tree, and every domain that is not the arrangement, whose state this surface
/// cannot reach.
///
/// `request` is the whole entry as JSON:
///
/// ```json
/// {"label": "drag the clip and its curve", "coalesce": false, "legs": [
///   {"structure": 1, "forward": {"edit": {…}}, "backward": {…}, "key": "place:7"},
///   {"structure": 2, "forward": {"edit": {…}}, "backward": {…}, "key": "points"}
/// ]}
/// ```
///
/// **Several legs are one transaction**: applied in the order given, inverted
/// in reverse, and undone in one step. That is what a gesture touching more
/// than one structure needs — a drag that moves a clip and rewrites the curve
/// it carries — and it is why the whole entry crosses in one call rather than
/// a leg at a time: half a transaction is worse than none. It is not
/// coalescing, which merges *successive* entries over one structure; the two
/// are kept apart so a merge cannot silently join two structures.
///
/// A leg's `forward` is a step (`{"edit": <payload>}` or
/// `{"recompute": <params>}`), its `backward` a payload in that structure's own
/// vocabulary, and its `key` what makes two edits *the same thing done the same
/// way* — the arrangement spells it `"place:7"`
/// ([`clausters_document_coalesce_key`](crate::clausters_document_coalesce_key)),
/// a curve has one verb and one key — with an absent or empty key never
/// coalescing. `coalesce` merges this entry into the one before it when every
/// leg's structure and key match, which is what makes a run of small
/// adjustments one undo; the caller decides, because only the caller knows
/// where the hand stopped.
///
/// Returns 0 on success, -1 when the request will not parse, holds no leg, or
/// names a structure this history did not mint. **This applies nothing** — the
/// caller has already made the edits; what is recorded is how to put them back.
/// The inverse of an arrangement leg comes from
/// [`clausters_document_inverse`](crate::clausters_document_inverse), read
/// before the edit lands.
///
/// # Safety
/// `h` must be a live history handle and `request` null or readable for
/// `request_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_record(
    h: *mut FfiHistory,
    request: *const u8,
    request_len: usize,
) -> i32 {
    // SAFETY: forwarded from this function's own contract.
    let Some(raw) = (unsafe { text(request, request_len) }) else {
        return -1;
    };
    let Ok(request) = serde_json::from_str::<Request>(&raw) else {
        return -1;
    };
    let Some(entry) = request.entry() else {
        return -1;
    };
    with_history(h, -1, |history| if history.record(entry) { 0 } else { -1 })
}

/// One entry as it crosses: what to call it, whether it continues the one
/// before it, and its legs.
#[derive(serde::Deserialize)]
struct Request {
    #[serde(default = "edit_label")]
    label: String,
    #[serde(default)]
    coalesce: bool,
    legs: Vec<Leg>,
}

/// One structure's share of a transaction.
#[derive(serde::Deserialize)]
struct Leg {
    structure: u64,
    forward: Step,
    /// How to put this leg back, or absent when nothing can — an act whose
    /// inverse the owner cannot write. The entry is still recorded, marked, and
    /// walked past in both directions.
    #[serde(default)]
    backward: Option<Opaque>,
    #[serde(default)]
    key: String,
}

fn edit_label() -> String {
    "edit".to_string()
}

impl Request {
    /// The entry, or `None` when it holds no leg — an empty transaction is not
    /// a gesture.
    fn entry(self) -> Option<Entry> {
        let mut legs = self.legs.into_iter();
        let first = legs.next()?;
        let mut entry = match first.backward {
            Some(backward) => Entry::new(
                self.label,
                StructureId(first.structure),
                first.forward,
                backward,
            ),
            None => Entry::uninvertible(self.label, StructureId(first.structure), first.forward),
        };
        if !first.key.is_empty() {
            entry = entry.keyed(first.key);
        }
        for leg in legs {
            entry = match leg.backward {
                Some(backward) => entry.and(StructureId(leg.structure), leg.forward, backward),
                None => entry.and_uninvertible(StructureId(leg.structure), leg.forward),
            };
            if !leg.key.is_empty() {
                entry = entry.keyed(leg.key);
            }
        }
        if self.coalesce {
            entry = entry.continuing();
        }
        Some(entry)
    }
}

/// Undo the last thing done: the inverses of the entry the walk lands on, each
/// with the structure it belongs to, **in the order they must be applied**.
///
/// Writes
/// `{"label": …, "inverses": [{"structure": <id>, "payload": <payload>}, …], "skipped": [<label>, …]}`.
/// `skipped` names the entries the walk had to pass over because nothing can
/// invert them — a hole in the history that announces itself, which is what
/// lets a person understand why an undo did not go where they expected.
/// Returns the byte count it needs, `0` when the handle is null, and `2`
/// (`{}`) when there was nothing to undo, which a caller distinguishes from a
/// failure.
///
/// It applies nothing: a history holds structures this surface cannot reach, so
/// applying the legs it *could* would leave the rest to the caller out of
/// order, which is how a transaction half-happens. The cursor moves when the
/// bytes are written, so a sizing pass is free of consequence.
///
/// # Safety
/// `h` must be a live history handle and `out` null or writable for `out_cap`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_undo(
    h: *mut FfiHistory,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let undone = with_history(h, None, |history| history.peek_undo());
    let payload = match &undone {
        // Nothing to undo. `{}` rather than an empty list, so a caller can tell
        // "the history is at its start" from "the call failed" (0).
        None => serde_json::json!({}),
        Some(undone) => serde_json::json!({
            "label": undone.label,
            "inverses": undone
                .legs
                .iter()
                .map(|(structure, load)| leg(*structure, "payload", &load.0))
                .collect::<Vec<_>>(),
            "skipped": undone.skipped,
        }),
    };
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        fill(
            &serde_json::to_vec(&payload).unwrap_or_default(),
            out,
            out_cap,
            || {
                if undone.is_some() {
                    with_history(h, false, |history| history.step_back());
                }
            },
        )
    }
}

/// Redo what was last undone: the steps of the entry the walk lands on, each
/// with the structure it belongs to, in order.
///
/// Writes `{"label": …, "edits": [{"structure": <id>, "payload": <payload>}, …],
/// "remaining": [{"structure": <id>, "step": <step>}, …], "skipped": [<label>, …]}`.
/// `edits` is the leading run of ordinary edits, for the caller to apply in
/// order; `remaining` holds the steps from the first one the crate cannot
/// describe as an edit onward — a deterministic operation stored as its
/// parameters, which the **owner** re-runs, because the crate holds no
/// algorithms. It stops at the first rather than skipping it, so a later edit is
/// never applied over a state the operation before it was meant to produce.
/// `skipped` is [`clausters_history_undo`]'s.
///
/// Returns the byte count needed, `0` when the handle is null, and `2` (`{}`)
/// when there was nothing to redo.
///
/// # Safety
/// As [`clausters_history_undo`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_redo(
    h: *mut FfiHistory,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let redone = with_history(h, None, |history| history.peek_redo());
    let payload = match &redone {
        None => serde_json::json!({}),
        Some(redone) => serde_json::json!({
            "label": redone.label,
            "edits": redone
                .edits
                .iter()
                .map(|(structure, load)| leg(*structure, "payload", &load.0))
                .collect::<Vec<_>>(),
            "remaining": redone
                .remaining
                .iter()
                .map(|(structure, step)| {
                    leg(
                        *structure,
                        "step",
                        &serde_json::to_value(step).unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>(),
            "skipped": redone.skipped,
        }),
    };
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        fill(
            &serde_json::to_vec(&payload).unwrap_or_default(),
            out,
            out_cap,
            || {
                if redone.is_some() {
                    with_history(h, false, |history| history.step_forward());
                }
            },
        )
    }
}

/// The data behind a structure is gone: drop it from the registry, and say
/// whether its memory may go now.
///
/// Returns 1 when nothing in the pile names it any more and the caller may free
/// at once, and 0 when it must wait for
/// [`clausters_history_released`] — because undoing a deletion has to be able to
/// give the data back, so a structure that is out of the tree stays alive while
/// an entry can still restore what referred to it.
///
/// It also **invalidates the entries that name it**: they cannot be applied to
/// data that is gone, so they become non-invertible — kept, marked, and walked
/// past with the walk saying so. Undoing a deletion returns the data, not its
/// history.
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_forget(h: *mut FfiHistory, structure: u64) -> i32 {
    with_history(h, 0, |history| {
        i32::from(history.forget(StructureId(structure)))
    })
}

/// The forgotten structures no entry names any more, as a JSON array of ids —
/// the caller may free their data now. Drains: each is reported once.
///
/// Returns the byte count it needs, or `0` when the handle is null. The drain
/// happens only when the bytes are written, so a sizing pass is free of
/// consequence.
///
/// # Safety
/// `h` must be a live history handle and `out` null or writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_released(
    h: *mut FfiHistory,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let ids = with_history(h, Vec::new(), |history| {
        history
            .pending_release()
            .iter()
            .map(|structure| structure.0)
            .collect::<Vec<_>>()
    });
    // SAFETY: forwarded from this function's own contract. The drain is the
    // commit, so a sizing pass hands back the same answer as the fill.
    unsafe {
        fill(
            &serde_json::to_vec(&ids).unwrap_or_default(),
            out,
            out_cap,
            || {
                with_history(h, (), |history| {
                    history.released();
                });
            },
        )
    }
}

/// Stamps the pile where it stands: this is what is on disk.
///
/// A save is an event of the whole editing context and the mark is the pile's,
/// so one save stamps one mark, and a structure registered later starts behind
/// it.
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_mark_saved(h: *mut FfiHistory) {
    with_history(h, (), |history| history.mark_saved())
}

/// Whether the work differs from what was last saved (1) or not (0).
///
/// Crossing the mark backwards is allowed, and this is the announcement — which
/// has to be accurate: nothing on disk changed, and the file still holds those
/// edits until the next save. Crossing forward again returns to clean.
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_dirty(h: *mut FfiHistory) -> i32 {
    with_history(h, 0, |history| i32::from(history.dirty()))
}

/// Whether the saved state can still be reached by walking this history (1) or
/// not (0).
///
/// `0` after the case the warning earns its place for: undo past the mark and
/// then edit, and the redo is truncated — so the saved state stops being
/// reachable, and [`clausters_history_dirty`] will never go quiet again on its
/// own.
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_saved_reachable(h: *mut FfiHistory) -> i32 {
    with_history(h, 0, |history| i32::from(history.saved_reachable()))
}

/// Whether there is anything to undo (1) or not (0).
/// Whether there is anything to undo (1) or not (0).
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_can_undo(h: *mut FfiHistory) -> i32 {
    with_history(h, 0, |history| i32::from(history.can_undo()))
}

/// Whether there is anything to redo (1) or not (0).
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_can_redo(h: *mut FfiHistory) -> i32 {
    with_history(h, 0, |history| i32::from(history.can_redo()))
}

/// What an undo would be called, written to `out`; the byte count it needs, or
/// `0` when there is nothing to undo. What a menu item reads — and what a
/// person needs when one pile holds several structures, since the label is the
/// only thing saying which one a keystroke is about to move.
///
/// # Safety
/// `h` must be a live history handle and `out` null or writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_undo_label(
    h: *mut FfiHistory,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let label = with_history(h, None, |history| history.undo_label());
    // SAFETY: caller guarantees `out` is null or writable for `out_cap`.
    unsafe { fill(label.unwrap_or_default().as_bytes(), out, out_cap, || {}) }
}

/// What a redo would be called. See [`clausters_history_undo_label`].
///
/// # Safety
/// As [`clausters_history_undo_label`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_redo_label(
    h: *mut FfiHistory,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let label = with_history(h, None, |history| history.redo_label());
    // SAFETY: caller guarantees `out` is null or writable for `out_cap`.
    unsafe { fill(label.unwrap_or_default().as_bytes(), out, out_cap, || {}) }
}

/// How many entries the history holds.
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_len(h: *mut FfiHistory) -> usize {
    with_history(h, 0, |history| history.len())
}

/// Forgets every entry, releasing what was spilled — what closing an editing
/// context leaves behind. The structures stay registered: it is the order that
/// is gone, not the identities the caller still holds.
///
/// # Safety
/// `h` must be a live history handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_history_clear(h: *mut FfiHistory) {
    with_history(h, (), |history| history.clear())
}

/// What makes two of a **domain's** edits *the same thing done the same way* —
/// the key a caller recording its own entry passes to
/// [`clausters_history_record`], or nothing when the payload is not written in
/// that vocabulary (or the domain is one the crate does not speak).
///
/// It is here rather than beside a structure because a caller across this
/// surface holds no structure to ask: a curve, a span of samples and a timeline
/// live in the caller's own memory, and only their *vocabulary* is the crate's.
/// [`clausters_document_coalesce_key`](crate::clausters_document_coalesce_key)
/// stays as it is — the arrangement's own door, on the surface its sentence
/// belongs to — and this is the same rule for the domains that have no handle
/// here.
///
/// Sizes with a null `out` and fills with a second call, like the rest of the
/// JSON surface. A pure read: nothing here mutates.
///
/// # Safety
/// `domain` must be null or readable for `domain_len` bytes, `payload` null or
/// readable for `payload_len` bytes, and `out` null or writable for `out_cap`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_domain_coalesce_key(
    domain: *const u8,
    domain_len: usize,
    payload: *const u8,
    payload_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(domain), Some(raw)) = (unsafe { text(domain, domain_len) }, unsafe {
        text(payload, payload_len)
    }) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return 0;
    };
    let Some(key) = clausters_document::domain::coalesce_key(&domain, &Opaque(value)) else {
        return 0;
    };
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { fill(key.as_bytes(), out, out_cap, || {}) }
}

#[cfg(test)]
mod tests;
