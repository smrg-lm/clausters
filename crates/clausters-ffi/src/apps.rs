//! The applications over the document.
//!
//! The C half of [`clausters_apps`]. An editor is a handle, because it keeps
//! state between messages — the conversation's floor, the window's ids, where
//! the reader put the cursor — and its verbs cross through one JSON door, sized
//! with a null `out` and filled with a second call.

use std::sync::Mutex;

use clausters_apps::multitrack::editor::{self, MultitrackEditor};

/// A multitrack editor safe to share across the binding's threads.
pub struct FfiMultitrackEditor(Mutex<MultitrackEditor>);

/// **A multitrack editor** over the piece a JSON request names, or null for a
/// request that names none.
///
/// `request` carries `piece`, `rate`, `defaultBpm`, `version` (the history's
/// counter), and the window's `link`, `transport` (`false`, `true`, or the
/// row's ids), `title`, `w` and `h`. Free with
/// [`clausters_apps_multitrack_editor_free`].
///
/// # Safety
/// `request` must be readable for `request_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_multitrack_editor_new(
    request: *const u8,
    request_len: usize,
) -> *mut FfiMultitrackEditor {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return std::ptr::null_mut();
    };
    match editor::new_json(&request) {
        Some(editor) => Box::into_raw(Box::new(FfiMultitrackEditor(Mutex::new(editor)))),
        None => std::ptr::null_mut(),
    }
}

/// Frees an editor created by [`clausters_apps_multitrack_editor_new`] (null is
/// a no-op).
///
/// # Safety
/// `e` must be a pointer from `clausters_apps_multitrack_editor_new`, not yet
/// freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_multitrack_editor_free(e: *mut FfiMultitrackEditor) {
    if !e.is_null() {
        // SAFETY: caller guarantees `e` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(e) });
    }
}

/// **One verb of the editor**: `request` names the `verb` and carries its
/// arguments, as `clausters_apps::multitrack::editor::call_json` documents, and
/// the answer is JSON.
///
/// The verb runs against a copy that is adopted when the answer is filled, so a
/// sizing pass changes nothing and can be repeated.
///
/// Returns the byte count the answer needs, or 0 for a null handle or a request
/// that is not UTF-8.
///
/// # Safety
/// `e` must be a live editor handle, `request` readable for `request_len` bytes,
/// and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_multitrack_editor_call(
    e: *mut FfiMultitrackEditor,
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
    let mut next = held.clone();
    let answer = editor::call_json(&mut next, &request);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::document::fill(answer.as_bytes(), out, out_cap, || {
            *held = next;
        })
    }
}
