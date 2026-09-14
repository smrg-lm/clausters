//! The applications over the document.
//!
//! The C half of [`clausters_apps`]. An editing context is a handle, because it
//! keeps state between messages — the history, the version, the editors opened
//! in it and each one's end of the conversation — and its verbs cross through
//! one JSON door, sized with a null `out` and filled with a second call.

use std::sync::Mutex;

use clausters_apps::editing::{self as context, Editing};
use clausters_apps::samples;

/// **A measure stack, checked**: `{"stack": [...]}` answers `{"layers": [...]}`
/// or `{"error"}` naming what is refused. Sizes with a null `out` and fills with
/// a second call.
///
/// # Safety
/// `request` must be readable for `request_len` bytes, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_samples_measures(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    let answer = samples::measures_json(&request);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// An editing context safe to share across the binding's threads, and the
/// answer a sizing call computed and has not handed over yet.
pub struct FfiEditing(Mutex<(Editing, Option<(String, String)>)>);

/// **An editing context**: one undo order over every editor opened in it. Free
/// with [`clausters_apps_editing_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_apps_editing_new() -> *mut FfiEditing {
    Box::into_raw(Box::new(FfiEditing(Mutex::new((Editing::new(), None)))))
}

/// Frees a context created by [`clausters_apps_editing_new`], with every
/// editor opened in it (null is a no-op).
///
/// # Safety
/// `e` must be a pointer from `clausters_apps_editing_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_editing_free(e: *mut FfiEditing) {
    if !e.is_null() {
        // SAFETY: caller guarantees `e` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(e) });
    }
}

/// **One verb of the context**: `request` names the `verb` and carries its
/// arguments, as `clausters_apps::editing::call_json` documents, and the answer
/// is JSON.
///
/// **A verb runs once.** A context holds a history, which is not copied for a
/// sizing pass the way an editor is: the first call runs the verb and keeps its
/// answer, and a call with the same request hands that answer over instead of
/// running it again. The answer is let go once it has been filled.
///
/// Returns the byte count the answer needs, or 0 for a null handle or a request
/// that is not UTF-8.
///
/// # Safety
/// `e` must be a live context handle, `request` readable for `request_len`
/// bytes, and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_editing_call(
    e: *mut FfiEditing,
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees `e` is live or null.
    let Some(handle) = (unsafe { e.as_ref() }) else {
        return 0;
    };
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    let Ok(mut held) = handle.0.lock() else {
        return 0;
    };
    let (editing, pending) = &mut *held;
    let answer = match pending.take() {
        Some((asked, answer)) if asked == request => answer,
        _ => context::call_json(editing, &request),
    };
    let mut handed = false;
    // SAFETY: forwarded from this function's own contract.
    let n = unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || handed = true) };
    if !handed {
        *pending = Some((request.into_owned(), answer));
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(e: *mut FfiEditing, request: &str) -> String {
        let n = unsafe {
            clausters_apps_editing_call(e, request.as_ptr(), request.len(), std::ptr::null_mut(), 0)
        };
        let mut buf = vec![0u8; n];
        let wrote = unsafe {
            clausters_apps_editing_call(e, request.as_ptr(), request.len(), buf.as_mut_ptr(), n)
        };
        String::from_utf8(buf[..wrote].to_vec()).unwrap()
    }

    /// **A verb runs once across the sizing call and the filling one**: two
    /// opens are two members, not four.
    #[test]
    fn a_sized_verb_runs_once() {
        let e = clausters_apps_editing_new();
        let first = call(e, r#"{"verb": "openSamples", "key": "a", "buffer": 1}"#);
        let second = call(e, r#"{"verb": "openSamples", "key": "b", "buffer": 2}"#);
        assert!(first.starts_with(r#"{"member":0,"#), "{first}");
        assert!(second.starts_with(r#"{"member":1,"#), "{second}");
        unsafe { clausters_apps_editing_free(e) };
    }
}
