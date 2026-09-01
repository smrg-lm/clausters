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

/// Record an entry against `structure` — the door for everything
/// [`clausters_history_apply`] cannot do: a destructive write, whose
/// overwritten samples are not in the tree, and every domain that is not the
/// arrangement, whose state this surface cannot reach.
///
/// `forward` is a step (`{"edit": <payload>}` or `{"recompute": <params>}`),
/// `backward` a payload in the structure's own vocabulary. `key` is what makes
/// two edits *the same thing done the same way* — the arrangement spells it
/// `"place:7"`, a curve has one verb and one key — and an empty one never
/// coalesces. `coalesce` non-zero merges this into the entry before it when
/// both keys match, which is what makes a run of small adjustments one undo;
/// the caller decides, because only the caller knows where the hand stopped.
///
/// Returns 0 on success, -1 when a payload will not parse or when `structure`
/// is one this history did not mint. **This does not apply anything** — the
/// caller has already made the edit; what is recorded is how to put it back.
///
/// # Safety
/// `h` must be a live history handle and the payloads null or readable for
/// their lengths.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn clausters_history_record(
    h: *mut FfiHistory,
    structure: u64,
    forward: *const u8,
    forward_len: usize,
    backward: *const u8,
    backward_len: usize,
    label: *const u8,
    label_len: usize,
    key: *const u8,
    key_len: usize,
    coalesce: i32,
) -> i32 {
    // SAFETY: forwarded from this function's own contract.
    let (Some(forward), Some(backward)) = (unsafe { text(forward, forward_len) }, unsafe {
        text(backward, backward_len)
    }) else {
        return -1;
    };
    let (Ok(forward), Ok(backward)) = (
        serde_json::from_str::<Step>(&forward),
        serde_json::from_str::<Opaque>(&backward),
    ) else {
        return -1;
    };
    // SAFETY: as above.
    let label = unsafe { text(label, label_len) }.unwrap_or(std::borrow::Cow::Borrowed("edit"));
    // SAFETY: as above.
    let key = unsafe { text(key, key_len) }.unwrap_or_default();
    let mut entry = Entry::new(label.as_ref(), StructureId(structure), forward, backward);
    if !key.is_empty() {
        entry = entry.keyed(key.as_ref());
    }
    if coalesce != 0 {
        entry = entry.continuing();
    }
    with_history(h, -1, |history| if history.record(entry) { 0 } else { -1 })
}

/// Undo the last transaction: the inverses of the entry before the cursor, each
/// with the structure it belongs to, **in the order they must be applied**.
///
/// Writes `{"inverses": [{"structure": <id>, "payload": <payload>}, …]}`.
/// Returns the byte count it needs, `0` when the handle is null, and `2` (`{}`)
/// when there was nothing to undo, which a caller distinguishes from a failure.
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
    let inverses = with_history(h, None, |history| history.peek_undo());
    let payload = match inverses {
        // Nothing to undo. `{}` rather than an empty list, so a caller can tell
        // "the history is at its start" from "the call failed" (0).
        None => serde_json::json!({}),
        Some(ref legs) => serde_json::json!({
            "inverses": legs
                .iter()
                .map(|(structure, load)| leg(*structure, "payload", &load.0))
                .collect::<Vec<_>>()
        }),
    };
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        fill(
            &serde_json::to_vec(&payload).unwrap_or_default(),
            out,
            out_cap,
            || {
                if inverses.is_some() {
                    with_history(h, false, |history| history.step_back());
                }
            },
        )
    }
}

/// Redo what was last undone: the steps of the entry at the cursor, each with
/// the structure it belongs to, in order.
///
/// Writes `{"edits": [{"structure": <id>, "payload": <payload>}, …],
/// "remaining": [{"structure": <id>, "step": <step>}, …]}`. `edits` is the
/// leading run of ordinary edits, for the caller to apply in order;
/// `remaining` holds the steps from the first one the crate cannot describe as
/// an edit onward — a deterministic operation stored as its parameters, which
/// the **owner** re-runs, because the crate holds no algorithms. It stops at
/// the first rather than skipping it, so a later edit is never applied over a
/// state the operation before it was meant to produce.
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
    let steps = with_history(h, None, |history| history.peek_redo());
    let had_steps = steps.is_some();
    let payload = match steps {
        None => serde_json::json!({}),
        Some(steps) => {
            let mut edits = Vec::new();
            let mut remaining = Vec::new();
            for (structure, step) in steps {
                match (&step, remaining.is_empty()) {
                    (Step::Edit(load), true) => edits.push(leg(structure, "payload", &load.0)),
                    _ => remaining.push(leg(
                        structure,
                        "step",
                        &serde_json::to_value(&step).unwrap_or_default(),
                    )),
                }
            }
            serde_json::json!({ "edits": edits, "remaining": remaining })
        }
    };
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        fill(
            &serde_json::to_vec(&payload).unwrap_or_default(),
            out,
            out_cap,
            || {
                if had_steps {
                    with_history(h, false, |history| history.step_forward());
                }
            },
        )
    }
}

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
    let label = with_history(h, None, |history| history.undo_label().map(str::to_string));
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
    let label = with_history(h, None, |history| history.redo_label().map(str::to_string));
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

#[cfg(test)]
mod tests;
