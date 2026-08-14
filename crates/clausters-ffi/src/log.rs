//! The undo log's C surface: a **handle**, and edits that apply and record in
//! one call.
//!
//! # Why a handle, when the document crosses by value
//!
//! The document's binding round-trips the format, and that is right for it: a
//! document is what the caller is editing, and the alternative was dozens of
//! accessors. A log is the opposite shape. It is bookkeeping the caller never
//! reads field by field, and — the deciding term — **a bulk inverse leaves the
//! log on purpose**. Sending it by value would carry every spilled span back
//! and forth on each call, which is precisely the cost spilling exists to
//! avoid: a log at its default budget holding sample inverses is megabytes.
//! So the log stays in Rust with its spill store, and the caller holds a
//! pointer, exactly as it does for the id registry.
//!
//! # Applying and recording are one call
//!
//! [`clausters_log_apply`] is `log::apply_logged` across the ABI, and that is
//! deliberate rather than convenient: the inverse has to be read out of the
//! document **before** the edit lands, so a binding that let a caller apply
//! first and record second would let it record the wrong thing. Nothing else
//! can put an ordinary entry in.
//!
//! `WriteSamples` is the one edit this cannot do alone — the samples it
//! overwrote are not in the document — so a destructive caller reads the span
//! it is about to write and hands the pair to [`clausters_log_record`].
//!
//! # Undo applies; redo may not be able to
//!
//! Going back is always data, so [`clausters_log_undo`] applies the inverses
//! itself and hands back the document they left. Going forward need not be: a
//! deterministic operation stores its parameters and is **re-run by the
//! owner**, and the crate holds no algorithms. So [`clausters_log_redo`]
//! applies the leading run of ordinary edits, stops at the first operation it
//! cannot perform, and hands back what is left for the caller to carry out in
//! order. A redo with nothing to re-run leaves `remaining` empty, which is the
//! ordinary case.
//!
//! Both move the cursor and apply in one call for the same reason `apply`
//! records in one: two calls could half-happen, and a log that disagrees with
//! its document is worse than no log.
//!
//! # Size-then-fill against a surface that mutates
//!
//! The rest of this crate's JSON surface sizes with a null `out` and fills with
//! a second call, which needs the payload to be **identical on both calls**.
//! Everything here mutates, so a naive implementation would undo on the sizing
//! call and hand back "nothing to undo" on the fill — and record two entries
//! for one edit. (It is the same hazard that keeps a one-shot engrave out of
//! `notation`, met from the other side.)
//!
//! The rule that resolves it: **the mutation happens only when the bytes are
//! actually written.** Each call computes what it *would* do without touching
//! the log, writes if the buffer fits, and commits only then. So a sizing pass
//! is free of consequence and a run of them is idempotent, which is what a
//! binding needs to be able to assume.

use std::sync::Mutex;

use clausters_document::{Against, Entry, Intent, Log, Rules, Step, apply as apply_intent};

use crate::document::{FfiDocument, with_document};

/// A log handle safe to share across the binding's threads.
pub struct FfiLog(Mutex<Log>);

/// Read a pointer+length as UTF-8 (lossily), or `None` when the pointer is null.
///
/// # Safety
/// `ptr` must be null or readable for `len` bytes.
unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<std::borrow::Cow<'a, str>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `ptr` is readable for `len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    Some(String::from_utf8_lossy(bytes))
}

/// Write `payload` into `out` if it fits, run `commit` only if it was written,
/// and return the byte count it needs.
///
/// The commit is what makes size-then-fill safe over a mutating surface: a
/// sizing pass (null or short `out`) changes nothing, so it can be repeated.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
unsafe fn fill(payload: &[u8], out: *mut u8, out_cap: usize, commit: impl FnOnce()) -> usize {
    let n = payload.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), out, n) };
        commit();
    }
    n
}

fn with_log<T>(h: *mut FfiLog, default: T, f: impl FnOnce(&mut Log) -> T) -> T {
    // SAFETY: caller guarantees `h` is a live log handle (or null).
    let Some(log) = (unsafe { h.as_ref() }) else {
        return default;
    };
    f(&mut log.0.lock().expect("log lock poisoned"))
}

/// A new log. `budget` is how many entries it keeps before the oldest falls off
/// (0 takes the default), `spill_above` how many `f32` values a sample payload
/// must reach before it leaves the log for the spill store (0 takes the
/// default).
///
/// The store is **memory**. A file-backed one is a caller's to supply in Rust
/// (the `Spill` trait) and is not reachable from here yet — see the crate's
/// `PLAN.md`, where it waits on a caller that actually needs it. Free with
/// [`clausters_log_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_log_new(budget: usize, spill_above: usize) -> *mut FfiLog {
    let mut log = Log::new();
    if budget > 0 {
        log = log.budget(budget);
    }
    if spill_above > 0 {
        log = log.spill_above(spill_above);
    }
    Box::into_raw(Box::new(FfiLog(Mutex::new(log))))
}

/// Frees a log created by [`clausters_log_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_log_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_free(h: *mut FfiLog) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Apply an edit to `doc` and record it in `h`, in one call — the only door for
/// an ordinary entry.
///
/// Arguments are [`crate::clausters_document_apply`]'s, plus the log handle and
/// a `label` (what an undo menu calls this). Writes the same outcome object and
/// returns the byte count it needs, or `0` when a handle is null or the intent
/// will not parse. The document is **not** in the reply — it stays in its
/// handle, and a caller that wants it asks
/// [`crate::clausters_document_snapshot`].
///
/// Applying and recording are one call because the inverse has to be read out
/// of the document *before* the edit lands; a surface that let a caller apply
/// first and record second would let it record the wrong thing. Nothing is
/// recorded unless the document actually changed, so a refusal — stale or
/// otherwise — leaves no entry, and neither does a resend.
///
/// # Safety
/// `h` must be a live log handle and `doc` a live document handle; the payloads
/// must be null or readable for their lengths, and `out` null or writable for
/// `out_cap` bytes.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn clausters_log_apply(
    h: *mut FfiLog,
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

    with_document(doc, 0, |held| {
        // The inverse is read *before* the edit lands -- which is the whole
        // reason applying and recording are one call -- and it is also what
        // rolls the edit back if the caller's buffer did not fit, so the tree
        // is never cloned to protect a sizing pass. `document::edit_in_place`
        // carries the same reasoning at more length.
        let Some(inverse) = clausters_document::inverse_of(&held.document, &intent) else {
            // No inverse in the document -- a `WriteSamples`, whose overwritten
            // samples are not in the tree. It cannot be rolled back, so it runs
            // on a copy, and it records nothing either: its entry comes from
            // the caller through `clausters_log_record`, which is the door for
            // exactly this case.
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
                    Step::Edit(outcome.effective.clone()),
                    inverse,
                );
                with_log(h, (), |log| log.record(entry));
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

/// Record an entry the document cannot supply the inverse for — a destructive
/// write, whose overwritten samples are not in the tree.
///
/// `forward` is a step (`{"edit": <intent>}` or `{"recompute": <params>}`),
/// `backward` an ordinary intent. `coalesce` non-zero merges this into the
/// entry before it when both touch the same node the same way, which is what
/// makes a run of small adjustments one undo; the caller decides, because only
/// the caller knows where the hand stopped.
///
/// Returns 0 on success, -1 when a payload will not parse. **This does not
/// apply anything** — the caller has already written the samples; what is
/// recorded is how to put them back.
///
/// # Safety
/// `h` must be a live log handle and the payloads null or readable for their
/// lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_record(
    h: *mut FfiLog,
    forward: *const u8,
    forward_len: usize,
    backward: *const u8,
    backward_len: usize,
    label: *const u8,
    label_len: usize,
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
        serde_json::from_str::<Intent>(&backward),
    ) else {
        return -1;
    };
    // SAFETY: as above.
    let label = unsafe { text(label, label_len) }.unwrap_or(std::borrow::Cow::Borrowed("edit"));
    let mut entry = Entry::new(label.as_ref(), forward, backward);
    if coalesce != 0 {
        entry = entry.continuing();
    }
    with_log(h, -1, |log| {
        log.record(entry);
        0
    })
}

/// Undo the last transaction, applying its inverses to the document `doc`
/// holds.
///
/// Writes `{"undone": [<intent>, …]}` — what the inverses were, in the order
/// they were applied. Returns the byte count it needs, `0` when a handle is
/// null, and `2` (`{}`) when there was nothing to undo, which a caller
/// distinguishes from a failure.
///
/// It applies rather than handing the inverses back for the caller to apply,
/// because the cursor has already moved: two calls could half-happen, and a log
/// that disagrees with its document is worse than no log.
///
/// # Safety
/// `h` must be a live log handle and `doc` a live document handle; `out` null
/// or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_undo(
    h: *mut FfiLog,
    doc: *mut FfiDocument,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let undone = with_log(h, None, |log| log.peek_undo());
    with_document(doc, 0, |held| {
        let mut edited = held.document.clone();
        let payload = match undone {
            // Nothing to undo. `{}` rather than an empty object with a
            // document, so a caller can tell "the history is at its start" from
            // "the call failed" (0) and from "an undo that legitimately
            // changed nothing".
            None => serde_json::json!({}),
            Some(ref intents) => {
                for intent in intents {
                    // An undo is authoritative: it states what the document
                    // was, so it is not checked against a version it predates.
                    apply_intent(&mut edited, intent, &Against::unstated(), &Rules::default());
                }
                serde_json::json!({ "undone": intents })
            }
        };
        // SAFETY: forwarded from this function's own contract.
        unsafe {
            fill(
                &serde_json::to_vec(&payload).unwrap_or_default(),
                out,
                out_cap,
                || {
                    if undone.is_some() {
                        held.commit(edited);
                        with_log(h, false, |log| log.step_back());
                    }
                },
            )
        }
    })
}

/// Redo what was last undone, applying what it can.
///
/// Writes `{"remaining": [<step>, …]}`, applying what it can to the document
/// `doc` holds. The ordinary edits at the front have already been applied there;
/// `remaining`
/// holds the steps from the first one the crate cannot perform onward — a
/// deterministic operation stored as its parameters, which the **owner**
/// re-runs, because the crate holds no algorithms. It stops at the first rather
/// than skipping it, so the caller never sees a later edit applied over a state
/// the operation before it was meant to produce.
///
/// Returns the byte count needed, `0` when the document will not parse, and `2`
/// (`{}`) when there was nothing to redo.
///
/// # Safety
/// As [`clausters_log_undo`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_redo(
    h: *mut FfiLog,
    doc: *mut FfiDocument,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let steps = with_log(h, None, |log| log.peek_redo());
    let had_steps = steps.is_some();
    with_document(doc, 0, |held| {
        let mut edited = held.document.clone();
        let payload = match steps {
            None => serde_json::json!({}),
            Some(steps) => {
                let mut remaining = Vec::new();
                let mut stopped = false;
                for step in steps {
                    match (&step, stopped) {
                        (Step::Edit(intent), false) => {
                            apply_intent(
                                &mut edited,
                                intent,
                                &Against::unstated(),
                                &Rules::default(),
                            );
                        }
                        _ => {
                            stopped = true;
                            remaining.push(step);
                        }
                    }
                }
                serde_json::json!({ "remaining": remaining })
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
                        held.commit(edited);
                        with_log(h, false, |log| log.step_forward());
                    }
                },
            )
        }
    })
}

/// Whether there is anything to undo (1) or not (0).
///
/// # Safety
/// `h` must be a live log handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_can_undo(h: *mut FfiLog) -> i32 {
    with_log(h, 0, |log| i32::from(log.can_undo()))
}

/// Whether there is anything to redo (1) or not (0).
///
/// # Safety
/// `h` must be a live log handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_can_redo(h: *mut FfiLog) -> i32 {
    with_log(h, 0, |log| i32::from(log.can_redo()))
}

/// What an undo would be called, written to `out`; the byte count it needs, or
/// `0` when there is nothing to undo. What a menu item reads.
///
/// # Safety
/// `h` must be a live log handle and `out` null or writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_undo_label(
    h: *mut FfiLog,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let label = with_log(h, None, |log| log.undo_label().map(str::to_string));
    // SAFETY: caller guarantees `out` is null or writable for `out_cap`.
    unsafe { fill(label.unwrap_or_default().as_bytes(), out, out_cap, || {}) }
}

/// What a redo would be called. See [`clausters_log_undo_label`].
///
/// # Safety
/// As [`clausters_log_undo_label`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_redo_label(
    h: *mut FfiLog,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    let label = with_log(h, None, |log| log.redo_label().map(str::to_string));
    // SAFETY: caller guarantees `out` is null or writable for `out_cap`.
    unsafe { fill(label.unwrap_or_default().as_bytes(), out, out_cap, || {}) }
}

/// How many entries the log holds.
///
/// # Safety
/// `h` must be a live log handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_len(h: *mut FfiLog) -> usize {
    with_log(h, 0, |log| log.len())
}

/// Forgets everything, releasing what was spilled — what closing a document or
/// loading another one leaves behind.
///
/// # Safety
/// `h` must be a live log handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_log_clear(h: *mut FfiLog) {
    with_log(h, (), |log| log.clear())
}

#[cfg(test)]
mod tests;
