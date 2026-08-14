//! The document's C surface: **hand over the document and the edit, take back
//! the document and what happened**.
//!
//! This is the whole binding, and its shape is the crate's central discipline
//! rather than a convenience. `clausters-document` is the only thing that
//! applies an intent, so a client does not apply and then report — it passes
//! the document and the intent across and receives the new document plus the
//! outcome. One implementation of the edit semantics, in one language, however
//! many clients there are.
//!
//! # Why no handles
//!
//! Every other stateful surface in this crate hands out an opaque handle (the
//! scheduler, the score, the registry). This one does not, and that was decided
//! before it was written: a handle would mean each client holding pointers into
//! a Rust object graph across a C ABI, and then every accessor a client wants —
//! and there are dozens, because a document is a tree — becomes a call to
//! design, bind and keep in step. Round-tripping the format costs a
//! serialization per edit and buys a binding that is *one function*, plus the
//! property that a client's document **is** the crate's document rather than a
//! parallel structure that synchronizes with it.
//!
//! # Size-then-fill, and why it is safe here
//!
//! Like the rest of the JSON surface: the call returns the byte count the
//! result needs and writes it only if it fit, so a caller sizes with a null
//! `out` and fills with a second call. That pattern needs the payload to be
//! **identical on both calls**, which holds here because applying an intent is
//! deterministic and the document crosses by value — the second call re-does
//! the same edit on the same input and produces the same bytes. (It is why
//! there is no one-shot engrave next door, where fresh ids make the two calls
//! differ.)

use clausters_document::{Against, Document, Intent, Mapping, Rules, Selection, Unit};

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

/// Write `payload` into `out` if it fits, and return the byte count it needs.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
unsafe fn fill(payload: &[u8], out: *mut u8, out_cap: usize) -> usize {
    let n = payload.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), out, n) };
    }
    n
}

/// Apply an edit to a document.
///
/// `document` is a document as JSON, `intent` an intent as JSON, `against` the
/// state the edit was made against (`{"version":N}`, or null for unstated), and
/// `quant` the musical grid a placement snaps to in beats (`0` snaps nothing).
///
/// Writes `{"document": …, "outcome": {"effective": …, "applied": bool,
/// "reason": …, "stale": bool}}` to `out` and returns the byte count it needs.
/// Returns `0` when the document or the intent is null or will not parse — the
/// caller's document is then untouched, which is the same thing a refusal means
/// and needs no separate error channel.
///
/// # Safety
/// The three inputs must be null or readable for their lengths, and `out` null
/// or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_apply(
    document: *const u8,
    document_len: usize,
    intent: *const u8,
    intent_len: usize,
    against: *const u8,
    against_len: usize,
    quant: f64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(document), Some(intent)) = (unsafe { text(document, document_len) }, unsafe {
        text(intent, intent_len)
    }) else {
        return 0;
    };
    let (Ok(mut document), Ok(intent)) = (
        serde_json::from_str::<Document>(&document),
        serde_json::from_str::<Intent>(&intent),
    ) else {
        return 0;
    };
    // SAFETY: as above.
    let against = unsafe { text(against, against_len) }
        .and_then(|raw| serde_json::from_str::<Against>(&raw).ok())
        .unwrap_or_else(Against::unstated);

    let outcome = clausters_document::apply(&mut document, &intent, &against, &Rules { quant });
    let payload = serde_json::json!({
        "document": document,
        "outcome": {
            "effective": outcome.effective,
            "applied": outcome.applied,
            "reason": outcome.reason,
            "stale": outcome.stale,
        }
    });
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    // SAFETY: as above.
    unsafe { fill(&bytes, out, out_cap) }
}

/// Resolve a selection to the spans of material underneath it.
///
/// `document` and `selection` are JSON; `frames_per_beat` is the bridge between
/// the arrangement's beats and the material's frames, supplied rather than
/// derived because tempo is the caller's; `in_beats` says whether the
/// selection's numbers are beats (non-zero) or frames on the shared axis
/// (zero).
///
/// Writes a JSON array of `{"node", "source", "generation", "range", "at"}` to
/// `out` and returns the byte count it needs. Returns `0` on a null or
/// unparseable input; an empty array is `2` bytes, which is how "nothing was
/// underneath" differs from "the call failed".
///
/// # Safety
/// Both inputs must be null or readable for their lengths, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_document_resolve(
    document: *const u8,
    document_len: usize,
    selection: *const u8,
    selection_len: usize,
    frames_per_beat: f64,
    in_beats: i32,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(document), Some(selection)) = (unsafe { text(document, document_len) }, unsafe {
        text(selection, selection_len)
    }) else {
        return 0;
    };
    let (Ok(document), Ok(selection)) = (
        serde_json::from_str::<Document>(&document),
        serde_json::from_str::<Selection>(&selection),
    ) else {
        return 0;
    };
    let mapping = Mapping {
        frames_per_beat,
        unit: if in_beats != 0 {
            Unit::Beats
        } else {
            Unit::Frames
        },
    };
    let resolved: Vec<_> = clausters_document::resolve(&document, &selection, &mapping)
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
    let bytes = serde_json::to_vec(&resolved).unwrap_or_default();
    // SAFETY: as above.
    unsafe { fill(&bytes, out, out_cap) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The size-then-fill dance a binding does, as one call.
    fn apply(document: &str, intent: &str, against: Option<&str>, quant: f64) -> String {
        let (a_ptr, a_len) = against.map_or((std::ptr::null(), 0), |a| (a.as_ptr(), a.len()));
        let n = unsafe {
            clausters_document_apply(
                document.as_ptr(),
                document.len(),
                intent.as_ptr(),
                intent.len(),
                a_ptr,
                a_len,
                quant,
                std::ptr::null_mut(),
                0,
            )
        };
        let mut buf = vec![0u8; n];
        let written = unsafe {
            clausters_document_apply(
                document.as_ptr(),
                document.len(),
                intent.as_ptr(),
                intent.len(),
                a_ptr,
                a_len,
                quant,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        assert_eq!(written, n, "sizing and filling must agree");
        String::from_utf8(buf).unwrap()
    }

    const DOC: &str = r#"{"version":1,"root":{"id":1,"kind":"set","grouping":"concrete",
        "members":[{"offset":0.0,"node":{"id":2,"kind":"event"}}]}}"#;

    #[test]
    fn an_edit_crosses_and_the_document_comes_back_changed() {
        let json: serde_json::Value = serde_json::from_str(&apply(
            DOC,
            r#"{"intent":"place","node":2,"offset":3.0}"#,
            None,
            0.0,
        ))
        .unwrap();
        assert_eq!(json["document"]["version"], 2);
        assert_eq!(json["document"]["root"]["members"][0]["offset"], 3.0);
        assert_eq!(json["outcome"]["applied"], true);
        assert_eq!(json["outcome"]["stale"], false);
    }

    #[test]
    fn the_rules_cross_too_and_the_outcome_reports_what_they_did() {
        let json: serde_json::Value = serde_json::from_str(&apply(
            DOC,
            r#"{"intent":"place","node":2,"offset":4.3}"#,
            None,
            1.0,
        ))
        .unwrap();
        assert_eq!(json["outcome"]["effective"]["offset"], 4.0);
        assert_eq!(json["outcome"]["reason"], "snapped to the grid");
    }

    #[test]
    fn a_stale_edit_crosses_as_stale_rather_than_as_an_error() {
        let json: serde_json::Value = serde_json::from_str(&apply(
            DOC,
            r#"{"intent":"place","node":2,"offset":3.0}"#,
            Some(r#"{"version":99}"#),
            0.0,
        ))
        .unwrap();
        assert_eq!(json["outcome"]["stale"], true);
        assert_eq!(json["outcome"]["applied"], false);
        assert_eq!(
            json["document"]["version"], 1,
            "and the document did not move"
        );
    }

    #[test]
    fn a_null_or_unparseable_input_writes_nothing() {
        // The caller's document is then untouched, which is what a refusal
        // means anyway -- so there is no second error channel to bind.
        let n = unsafe {
            clausters_document_apply(
                std::ptr::null(),
                0,
                b"{}".as_ptr(),
                2,
                std::ptr::null(),
                0,
                0.0,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(n, 0);
        assert_eq!(apply(DOC, "not json", None, 0.0), "");
    }

    #[test]
    fn a_selection_resolves_across_the_abi() {
        let document = r#"{"version":1,"root":{"id":1,"kind":"set","grouping":"concrete",
            "members":[{"offset":2.0,"dur":4.0,"node":{"id":2,"kind":"buffer",
            "source":{"source":100,"lifetime":"external","generation":2,
            "range":{"start":480000,"end":672000}}}}]}}"#;
        let selection = r#"{"start":144000.0,"len":48000.0}"#;
        let n = unsafe {
            clausters_document_resolve(
                document.as_ptr(),
                document.len(),
                selection.as_ptr(),
                selection.len(),
                48_000.0,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        let mut buf = vec![0u8; n];
        unsafe {
            clausters_document_resolve(
                document.as_ptr(),
                document.len(),
                selection.as_ptr(),
                selection.len(),
                48_000.0,
                0,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        let json: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(json[0]["source"], 100);
        assert_eq!(json[0]["generation"], 2);
        assert_eq!(json[0]["range"]["start"], 528_000);
        assert_eq!(json[0]["range"]["end"], 576_000);
    }

    #[test]
    fn nothing_underneath_is_an_empty_array_and_not_a_failure() {
        let selection = r#"{"start":0.0,"len":1.0}"#;
        let n = unsafe {
            clausters_document_resolve(
                DOC.as_ptr(),
                DOC.len(),
                selection.as_ptr(),
                selection.len(),
                48_000.0,
                1,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(n, 2, "`[]`, which a failure (0) is distinguishable from");
    }
}
