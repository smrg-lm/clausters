//! The document's C surface: **the tree stays here, the edit and the outcome
//! cross**.
//!
//! `clausters-document` is the only thing that applies an intent, so a client
//! does not apply and then report — it hands an intent over and receives what
//! happened. One implementation of the edit semantics, in one language, however
//! many clients there are.
//!
//! # Why a handle, when the plan said no handles
//!
//! The first binding passed the whole document in and took the whole new
//! document back, and the reasoning was sound as far as it went: a handle would
//! mean each client holding pointers into a Rust object graph, and a tree has
//! dozens of accessors to design, bind and keep in step. What it did not do was
//! put a number on "a serialization per edit". The number is **205 ms** for one
//! placement on a 10240-event composition (3.3 MB of JSON), against 6 ms on the
//! 320-event one an example builds — linear in the whole document and
//! independent of the edit, so a destructive stroke touching fifty samples paid
//! the same as a drag.
//!
//! The objection was to *accessor* handles — a call per field — and not to
//! pointers. This handle answers it by having the same surface the by-value
//! binding had: `apply`, `resolve`, and a `snapshot` for whoever wants the tree.
//! Nothing about "the crate is the only applier" changes; what changes is that
//! an edit now costs the edit. A client that wants the by-value convenience
//! builds it in its own language out of open → apply → snapshot → free, and
//! pays the serialization only where it actually asked for it.
//!
//! # Size-then-fill, and the two rules it needs
//!
//! Like the rest of the JSON surface, a call returns the byte count its result
//! needs and writes it only if it fits, so a caller sizes with a null `out` and
//! fills with a second call. Over a surface that **mutates**, that needs the
//! payload identical on both passes, which is the hazard `log` records from the
//! other side. The two rules here:
//!
//! - **A mutating call commits only when the bytes are written** (O11's rule,
//!   unchanged), so a sizing pass is free of consequence and a run of them is
//!   idempotent. This is now cheap as well as safe, because what a mutating
//!   call returns is the *outcome* — a few hundred bytes — and no longer the
//!   document.
//! - **A pure read caches between the pair.** `snapshot` is the one call whose
//!   payload is still the size of the composition, and it changes nothing, so
//!   the sizing pass keeps the bytes it produced and the fill copies them out.
//!   Caching a *mutating* call this way would be wrong — the mutation would
//!   land on the sizing pass, and a caller that sized and then gave up would
//!   have edited the document without knowing it.

use std::sync::Mutex;

use clausters_document::{
    Against, Body, Document, Grouping, Intent, Mapping, Node, NodeId, Opaque, Rules, Selection,
    Unit, apply as apply_intent,
};

/// A document handle safe to share across the binding's threads.
pub struct FfiDocument(Mutex<Held>);

pub(crate) struct Held {
    pub(crate) document: Document,
    /// The bytes a size-then-fill pair over a **pure read** is in the middle of
    /// handing out. Only `snapshot` uses it; see the rules above.
    pub(crate) pending: Vec<u8>,
}

impl Held {
    /// Swap in an edited tree, dropping any snapshot bytes it invalidates.
    /// What a mutating call runs in its commit.
    pub(crate) fn commit(&mut self, edited: Document) {
        self.document = edited;
        self.pending = Vec::new();
    }
}

/// Read a pointer+length as UTF-8 (lossily), or `None` when the pointer is null.
///
/// # Safety
/// `ptr` must be null or readable for `len` bytes.
pub(crate) unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<std::borrow::Cow<'a, str>> {
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
pub(crate) unsafe fn fill(
    payload: &[u8],
    out: *mut u8,
    out_cap: usize,
    commit: impl FnOnce(),
) -> usize {
    let n = payload.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), out, n) };
        commit();
    }
    n
}

/// Run `f` with the handle's contents locked.
///
/// `history` reaches the document through this: an edit that is also *recorded*
/// still has to land on the one held tree. The lock order is always
/// **document, then history** — a commit closure may take the history's lock
/// while this one is held, and nothing takes them the other way round.
pub(crate) fn with_document<T>(
    h: *mut FfiDocument,
    default: T,
    f: impl FnOnce(&mut Held) -> T,
) -> T {
    // SAFETY: caller guarantees `h` is a live document handle (or null).
    let Some(held) = (unsafe { h.as_ref() }) else {
        return default;
    };
    f(&mut held.0.lock().expect("document lock poisoned"))
}

/// Open a document. `json` is a document as JSON, or null for an empty one (a
/// concrete set with no members, at `FIRST_VERSION`).
///
/// Returns null when the JSON will not parse. Free with
/// [`clausters_document_free`].
///
/// # Safety
/// `json` must be null or readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_open(json: *const u8, len: usize) -> *mut FfiDocument {
    // SAFETY: forwarded from this function's own contract.
    let document = match unsafe { text(json, len) } {
        Some(raw) => match serde_json::from_str::<Document>(&raw) {
            Ok(document) => document,
            Err(_) => return std::ptr::null_mut(),
        },
        None => Document::new(Node::new(
            NodeId(1),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members: Vec::new(),
                config: Opaque::none(),
            },
        )),
    };
    Box::into_raw(Box::new(FfiDocument(Mutex::new(Held {
        document,
        pending: Vec::new(),
    }))))
}

/// Frees a document created by [`clausters_document_open`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_document_open`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_free(h: *mut FfiDocument) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// The edit that would put this node back the way it is — the inverse of
/// `intent`, read out of the document **before** anything is applied. Written
/// to `out`; the byte count it needs, or `0` when the handle is null, the
/// intent will not parse, or the document cannot describe the inverse (the node
/// is gone, or its body holds nothing of that shape).
///
/// [`crate::clausters_history_apply`] does this for you and is what an ordinary
/// edit wants. This is for the caller that records its **own** entry — a leg of
/// a transaction spanning several structures, which nothing but the caller can
/// apply, since the crate reaches one document and no curve.
///
/// For a `writesamples` it is the empty write rather than the span, which is
/// why a destructive caller reads the samples it is about to overwrite instead
/// of asking here.
///
/// # Safety
/// `h` must be a live document handle or null, `intent` null or readable for
/// `intent_len` bytes, and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_inverse(
    h: *mut FfiDocument,
    intent: *const u8,
    intent_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(raw) = (unsafe { text(intent, intent_len) }) else {
        return 0;
    };
    let Ok(intent) = serde_json::from_str::<Intent>(&raw) else {
        return 0;
    };
    with_document(h, 0, |held| {
        let Some(inverse) = clausters_document::inverse_of(&held.document, &intent) else {
            return 0;
        };
        let bytes = serde_json::to_vec(&inverse).unwrap_or_default();
        // SAFETY: forwarded from this function's own contract. A pure read, so
        // there is nothing to commit.
        unsafe { fill(&bytes, out, out_cap, || {}) }
    })
}

/// What makes two edits over the arrangement *the same thing done the same way*
/// — the key a history coalesces on, written to `out`; the byte count it needs,
/// or `0` when the intent will not parse.
///
/// It is here rather than on the history's surface because it is a sentence in
/// **this** vocabulary: the kind of edit and the node it names. The pile reads
/// no vocabulary, so a caller recording its own entries asks the domain, which
/// is what keeps a second spelling of the rule out of every binding.
///
/// # Safety
/// `intent` must be null or readable for `intent_len` bytes, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_coalesce_key(
    intent: *const u8,
    intent_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(raw) = (unsafe { text(intent, intent_len) }) else {
        return 0;
    };
    let Ok(intent) = serde_json::from_str::<Intent>(&raw) else {
        return 0;
    };
    let key = clausters_document::log::coalesce_key(&intent);
    let bytes = key.as_bytes();
    if !out.is_null() && out_cap >= bytes.len() {
        // SAFETY: out is writable for out_cap >= bytes.len() bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}

/// The document's current version — monotonic, bumped by every applied edit,
/// never zero. `0` when the handle is null, which is also what *unstated*
/// means, so a caller that lost its handle names the state it cannot vouch for.
///
/// # Safety
/// `h` must be a live document handle or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_version(h: *mut FfiDocument) -> u64 {
    with_document(h, 0, |held| held.document.version)
}

/// The whole document as JSON — for saving it, for handing it to another
/// process, for a client that wants the tree.
///
/// A **pure read**, so the sizing pass keeps what it serialized and the fill
/// copies it out: the composition is serialized once per pair, not twice.
///
/// Returns the byte count the document needs, or `0` when the handle is null.
///
/// # Safety
/// `h` must be a live document handle, and `out` null or writable for `out_cap`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_snapshot(
    h: *mut FfiDocument,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    with_document(h, 0, |held| {
        if held.pending.is_empty() {
            held.pending = serde_json::to_vec(&held.document).unwrap_or_default();
        }
        let n = held.pending.len();
        if !out.is_null() && out_cap >= n {
            // SAFETY: out is writable for out_cap >= n bytes.
            unsafe { std::ptr::copy_nonoverlapping(held.pending.as_ptr(), out, n) };
            held.pending = Vec::new();
        }
        n
    })
}

/// Apply an edit to the document the handle holds.
///
/// `intent` is an intent as JSON, `against` the state the edit was made against
/// (`{"version":N}`, or null for unstated), and `quant` the musical grid a
/// placement snaps to in beats (`0` snaps nothing).
///
/// Writes `{"effective": …, "applied": bool, "reason": …, "stale": bool}` to
/// `out` and returns the byte count it needs. The **document is not in the
/// reply** — that is the whole point of the handle, and a caller that wants it
/// asks [`clausters_document_snapshot`]. Returns `0` when the handle is null or
/// the intent will not parse; the document is then untouched, which is the same
/// thing a refusal means and needs no separate error channel.
///
/// The edit lands only when the bytes are written, so a sizing pass changes
/// nothing and repeating one is harmless.
///
/// # Safety
/// `h` must be a live document handle; the payloads null or readable for their
/// lengths, and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_apply(
    h: *mut FfiDocument,
    intent: *const u8,
    intent_len: usize,
    against: *const u8,
    against_len: usize,
    quant: f64,
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

    with_document(h, 0, |held| {
        // SAFETY: forwarded from this function's own contract.
        unsafe { edit_in_place(held, &intent, &against, quant, out, out_cap) }
    })
}

/// Apply `intent`, write the outcome if it fits, and leave the document
/// untouched if it does not.
///
/// **The edit runs in place and is rolled back rather than run on a copy**,
/// which is the difference between costing the edit and costing the
/// composition: cloning the tree to protect a sizing pass is O(document), and
/// on a 10240-event piece that is 14 ms per gesture whatever the gesture
/// touched. The rollback is the intent's own inverse — the same one the log
/// records — plus restoring the version by hand, since applying an inverse
/// bumps the counter rather than rewinding it.
///
/// A `WriteSamples` is the one edit with no inverse in the document (its
/// overwritten samples are not in the tree), so that path still uses a copy.
/// It is the rare one, and paying there keeps the rule exact everywhere.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
unsafe fn edit_in_place(
    held: &mut Held,
    intent: &Intent,
    against: &Against,
    quant: f64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // `WriteSamples` is excluded by kind rather than by whether an inverse
    // exists, and finding out why is the reason this is spelled out. It bumps
    // the **source's generation** as well as the document's version, and an
    // inverse write bumps it again -- so rolling one back left the generation
    // two ahead instead of where it started, which the cross-binding vectors
    // caught and nothing else would have. The version is one field to restore;
    // a generation is one per source, and chasing them is exactly the kind of
    // bookkeeping a copy exists to avoid.
    let inverse = match intent {
        Intent::WriteSamples { .. } => None,
        _ => clausters_document::inverse_of(&held.document, intent),
    };
    let Some(inverse) = inverse else {
        // Nothing safe to roll back with: run on a copy and swap it in on
        // commit.
        let mut edited = held.document.clone();
        let outcome = apply_intent(&mut edited, intent, against, &Rules { quant });
        // SAFETY: forwarded from this function's own contract.
        return unsafe {
            fill(&outcome_bytes(&outcome), out, out_cap, || {
                held.commit(edited)
            })
        };
    };

    let version = held.document.version;
    let outcome = apply_intent(&mut held.document, intent, against, &Rules { quant });
    let applied = outcome.applied;
    let bytes = outcome_bytes(&outcome);
    let fits = !out.is_null() && out_cap >= bytes.len();
    if fits {
        // SAFETY: out is writable for out_cap >= bytes.len() bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
        held.pending = Vec::new();
    } else if applied {
        // A sizing pass, or a buffer the caller guessed too small. Put it back
        // exactly, version included, so the pass is free of consequence and a
        // run of them is idempotent -- what a binding has to be able to assume.
        apply_intent(
            &mut held.document,
            &inverse,
            &Against::unstated(),
            &Rules::default(),
        );
        held.document.version = version;
    }
    bytes.len()
}

fn outcome_bytes(outcome: &clausters_document::Outcome) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "effective": outcome.effective,
        "applied": outcome.applied,
        "reason": outcome.reason,
        "stale": outcome.stale,
    }))
    .unwrap_or_default()
}

/// Resolve a selection to the spans of samples underneath it.
///
/// `selection` is JSON; `frames_per_beat` and `frames_per_second` are the two
/// bridges between the document's units and the buffer's frames — a placement
/// is in beats and a take's length in seconds — supplied rather than derived
/// because tempo is the caller's; `in_beats` says whether the selection's
/// numbers are beats (non-zero) or frames on the shared axis (zero).
///
/// Writes a JSON array of `{"node", "source", "generation", "range", "at"}` to
/// `out` and returns the byte count it needs. Returns `0` on a null handle or
/// an unparseable selection; an empty array is `2` bytes, which is how "nothing
/// was underneath" differs from "the call failed".
///
/// # Safety
/// `h` must be a live document handle, `selection` null or readable for its
/// length, and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_resolve(
    h: *mut FfiDocument,
    selection: *const u8,
    selection_len: usize,
    frames_per_beat: f64,
    frames_per_second: f64,
    in_beats: i32,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(selection) = (unsafe { text(selection, selection_len) }) else {
        return 0;
    };
    let Ok(selection) = serde_json::from_str::<Selection>(&selection) else {
        return 0;
    };
    let mapping = Mapping {
        frames_per_beat,
        frames_per_second,
        unit: if in_beats != 0 {
            Unit::Beats
        } else {
            Unit::Frames
        },
    };
    with_document(h, 0, |held| {
        let resolved: Vec<_> = clausters_document::resolve(&held.document, &selection, &mapping)
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "node": r.node,
                    "source": r.source,
                    "generation": r.generation,
                    "range": r.range,
                    "at": r.at,
                })
            })
            .collect();
        // SAFETY: as above. A pure read with a small payload -- no caching
        // needed, and nothing to commit.
        unsafe {
            fill(
                &serde_json::to_vec(&resolved).unwrap_or_default(),
                out,
                out_cap,
                || {},
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"version":1,"root":{"id":1,"kind":"aggregate","grouping":"concrete",
        "members":[{"offset":0.0,"node":{"id":2,"kind":"clang"}}]}}"#;

    /// A handle that frees itself, so a failing assertion does not leak.
    struct Doc(*mut FfiDocument);

    impl Doc {
        fn new(json: &str) -> Self {
            let h = unsafe { clausters_document_open(json.as_ptr(), json.len()) };
            assert!(!h.is_null(), "the fixture parses");
            Self(h)
        }

        fn tree(&self) -> serde_json::Value {
            serde_json::from_str(&sized(|out, cap| unsafe {
                clausters_document_snapshot(self.0, out, cap)
            }))
            .unwrap()
        }
    }

    impl Drop for Doc {
        fn drop(&mut self) {
            unsafe { clausters_document_free(self.0) };
        }
    }

    /// The size-then-fill dance a binding does, as one call.
    fn sized(mut call: impl FnMut(*mut u8, usize) -> usize) -> String {
        let n = call(std::ptr::null_mut(), 0);
        let mut buf = vec![0u8; n];
        let written = call(buf.as_mut_ptr(), buf.len());
        assert_eq!(written, n, "sizing and filling must agree");
        String::from_utf8(buf).unwrap()
    }

    fn apply(doc: &Doc, intent: &str, against: Option<&str>, quant: f64) -> serde_json::Value {
        let (a_ptr, a_len) = against.map_or((std::ptr::null(), 0), |a| (a.as_ptr(), a.len()));
        let raw = sized(|out, cap| unsafe {
            clausters_document_apply(
                doc.0,
                intent.as_ptr(),
                intent.len(),
                a_ptr,
                a_len,
                quant,
                out,
                cap,
            )
        });
        serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null)
    }

    #[test]
    fn an_edit_crosses_and_the_document_changes_behind_the_handle() {
        let doc = Doc::new(DOC);
        let outcome = apply(
            &doc,
            r#"{"intent":"place","node":2,"offset":3.0}"#,
            None,
            0.0,
        );
        assert_eq!(outcome["applied"], true);
        assert_eq!(outcome["stale"], false);
        assert_eq!(doc.tree()["version"], 2);
        assert_eq!(doc.tree()["root"]["members"][0]["offset"], 3.0);
        assert_eq!(
            unsafe { clausters_document_version(doc.0) },
            2,
            "and the version is readable without serializing the tree"
        );
    }

    #[test]
    fn the_rules_cross_too_and_the_outcome_reports_what_they_did() {
        let doc = Doc::new(DOC);
        let outcome = apply(
            &doc,
            r#"{"intent":"place","node":2,"offset":4.3}"#,
            None,
            1.0,
        );
        assert_eq!(outcome["effective"]["offset"], 4.0);
        assert_eq!(outcome["reason"], "snapped to the grid");
    }

    #[test]
    fn a_stale_edit_crosses_as_stale_rather_than_as_an_error() {
        let doc = Doc::new(DOC);
        let outcome = apply(
            &doc,
            r#"{"intent":"place","node":2,"offset":3.0}"#,
            Some(r#"{"version":99}"#),
            0.0,
        );
        assert_eq!(outcome["stale"], true);
        assert_eq!(outcome["applied"], false);
        assert_eq!(doc.tree()["version"], 1, "and the document did not move");
    }

    #[test]
    fn a_null_or_unparseable_input_writes_nothing() {
        // The document is then untouched, which is what a refusal means anyway
        // -- so there is no second error channel to bind.
        let doc = Doc::new(DOC);
        let n = unsafe {
            clausters_document_apply(
                doc.0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                0.0,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(n, 0);
        assert_eq!(apply(&doc, "not json", None, 0.0), serde_json::Value::Null);
        assert_eq!(doc.tree()["version"], 1);

        // An unparseable document is a null handle rather than an empty one:
        // opening something that is not a document must not look like opening
        // an empty composition.
        let bad = "not json";
        assert!(unsafe { clausters_document_open(bad.as_ptr(), bad.len()) }.is_null());
    }

    #[test]
    fn an_opened_nothing_is_an_empty_composition() {
        let doc = unsafe { clausters_document_open(std::ptr::null(), 0) };
        assert!(!doc.is_null());
        assert_eq!(unsafe { clausters_document_version(doc) }, 1);
        unsafe { clausters_document_free(doc) };
    }

    /// The sizing pass caches, so a snapshot serializes the composition once
    /// per pair -- and the cache has to be dropped by an edit, or the second
    /// reader sees the tree from before it.
    #[test]
    fn a_snapshot_after_an_edit_is_the_edited_tree() {
        let doc = Doc::new(DOC);
        assert_eq!(doc.tree()["root"]["members"][0]["offset"], 0.0);
        // Size without filling, so the cache is warm and stale on purpose.
        unsafe { clausters_document_snapshot(doc.0, std::ptr::null_mut(), 0) };
        apply(
            &doc,
            r#"{"intent":"place","node":2,"offset":9.0}"#,
            None,
            0.0,
        );
        assert_eq!(doc.tree()["root"]["members"][0]["offset"], 9.0);
    }

    #[test]
    fn a_sizing_pass_does_not_edit_the_document() {
        // What the handle makes possible to get wrong: the tree is no longer
        // the caller's, so a mutating sizing pass would move it silently.
        let doc = Doc::new(DOC);
        let intent = r#"{"intent":"place","node":2,"offset":5.0}"#;
        for _ in 0..3 {
            unsafe {
                clausters_document_apply(
                    doc.0,
                    intent.as_ptr(),
                    intent.len(),
                    std::ptr::null(),
                    0,
                    0.0,
                    std::ptr::null_mut(),
                    0,
                )
            };
        }
        assert_eq!(doc.tree()["version"], 1, "nothing applied");
        apply(&doc, intent, None, 0.0);
        assert_eq!(doc.tree()["version"], 2, "and one real call applies once");
    }

    /// The other path through `apply`: an edit the document holds no inverse
    /// for cannot be rolled back, so it runs on a copy — and the rule it is
    /// protecting has to hold there too.
    #[test]
    fn a_sizing_pass_over_a_destructive_write_edits_nothing_either() {
        let doc = Doc::new(
            r#"{"version":1,"root":{"id":1,"kind":"vector",
            "source":{"source":7,"lifetime":"temporary","generation":4}}}"#,
        );
        let intent = r#"{"intent":"writesamples","node":1,"start":0,"values":[0.5,0.5]}"#;
        for _ in 0..3 {
            unsafe {
                clausters_document_apply(
                    doc.0,
                    intent.as_ptr(),
                    intent.len(),
                    std::ptr::null(),
                    0,
                    0.0,
                    std::ptr::null_mut(),
                    0,
                )
            };
        }
        assert_eq!(doc.tree()["version"], 1, "nothing applied");
        assert_eq!(
            doc.tree()["root"]["source"]["generation"],
            4,
            "and the source's generation is where it started"
        );
        let outcome = apply(&doc, intent, None, 0.0);
        assert_eq!(outcome["applied"], true);
        assert_eq!(doc.tree()["version"], 2, "and one real call applies once");
        assert_eq!(
            doc.tree()["root"]["source"]["generation"],
            5,
            "once, not twice -- the generation is the field a rollback cannot \
             restore, which is why this edit is never rolled back"
        );
    }

    /// The rollback has to restore the **version** as well as the tree: applying
    /// an inverse bumps the counter rather than rewinding it, so a sizing pass
    /// would otherwise leave the document a version ahead of itself and make
    /// every later edit look stale.
    #[test]
    fn a_rolled_back_sizing_pass_leaves_the_version_alone() {
        let doc = Doc::new(DOC);
        let intent = r#"{"intent":"place","node":2,"offset":6.0}"#;
        for _ in 0..4 {
            unsafe {
                clausters_document_apply(
                    doc.0,
                    intent.as_ptr(),
                    intent.len(),
                    std::ptr::null(),
                    0,
                    0.0,
                    std::ptr::null_mut(),
                    0,
                )
            };
        }
        assert_eq!(unsafe { clausters_document_version(doc.0) }, 1);
        // And an edit naming that version is still current, not stale.
        let outcome = apply(&doc, intent, Some(r#"{"version":1}"#), 0.0);
        assert_eq!(outcome["stale"], false);
        assert_eq!(outcome["applied"], true);
    }

    #[test]
    fn a_null_handle_is_answered_rather_than_a_crash() {
        let null: *mut FfiDocument = std::ptr::null_mut();
        assert_eq!(unsafe { clausters_document_version(null) }, 0);
        assert_eq!(
            unsafe { clausters_document_snapshot(null, std::ptr::null_mut(), 0) },
            0
        );
        unsafe { clausters_document_free(null) };
    }

    #[test]
    fn a_selection_resolves_across_the_abi() {
        let doc = Doc::new(
            r#"{"version":1,"root":{"id":1,"kind":"aggregate","grouping":"concrete",
            "members":[{"offset":2.0,"dur":4.0,"node":{"id":2,"kind":"vector",
            "source":{"source":100,"lifetime":"external","generation":2,
            "range":{"start":480000,"end":672000}}}}]}}"#,
        );
        let selection = r#"{"start":144000.0,"len":48000.0}"#;
        let raw = sized(|out, cap| unsafe {
            clausters_document_resolve(
                doc.0,
                selection.as_ptr(),
                selection.len(),
                48_000.0,
                48_000.0,
                0,
                out,
                cap,
            )
        });
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json[0]["source"], 100);
        assert_eq!(json[0]["generation"], 2);
        assert_eq!(json[0]["range"]["start"], 528_000);
        assert_eq!(json[0]["range"]["end"], 576_000);
    }

    #[test]
    fn nothing_underneath_is_an_empty_array_and_not_a_failure() {
        let doc = Doc::new(DOC);
        let selection = r#"{"start":0.0,"len":1.0}"#;
        let n = unsafe {
            clausters_document_resolve(
                doc.0,
                selection.as_ptr(),
                selection.len(),
                48_000.0,
                48_000.0,
                1,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(n, 2, "`[]`, which a failure (0) is distinguishable from");
    }
}
