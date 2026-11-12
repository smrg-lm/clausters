//! Reading a caller's string and filling a caller's buffer: the two halves of
//! every call that takes text in and hands bytes back.
//!
//! A call that answers bytes follows **size-then-fill**: the caller passes a
//! buffer and its capacity, the call returns the byte count the answer needs,
//! and it writes only when the answer fits -- so a null or short buffer is a
//! sizing pass that changes nothing and can be repeated with a bigger one.

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

/// Write `payload` into `out` if it fits, and return the byte count it needs.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
pub(crate) unsafe fn fill(payload: &[u8], out: *mut u8, out_cap: usize) -> usize {
    // SAFETY: forwarded from this function's own contract.
    unsafe { fill_then(payload, out, out_cap, || {}) }
}

/// [`fill`], running `commit` only if the payload was written.
///
/// The commit is what makes size-then-fill safe over a mutating surface: a
/// sizing pass (null or short `out`) changes nothing, so it can be repeated.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
pub(crate) unsafe fn fill_then(
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

/// Run a verb and answer its JSON; **a panic inside it becomes an answer**,
/// `{"error": "internal: ..."}`, instead of crossing the boundary.
///
/// An unwind that reaches an `extern "C"` function aborts the process, and the
/// process is the caller's: a Python interpreter with an unsaved session in
/// it. A verb that panics has a bug, and the caller is owed the report of it
/// through the door every other refusal comes through, on a handle it can
/// still close.
///
/// What it does not promise is the state behind a verb that edits in place:
/// the doors that run on a copy and commit it (the document's, the runner's)
/// lose nothing, while a context caught mid-edit keeps whatever the verb had
/// done when it stopped. That is why the answer says `internal`.
pub(crate) fn guarded(verb: impl FnOnce() -> String) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(verb)) {
        Ok(answer) => answer,
        Err(payload) => {
            let said = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".to_string());
            serde_json::json!({ "error": format!("internal: {said}") }).to_string()
        }
    }
}

/// [`guarded`] for a call that answers a value rather than JSON: a panic
/// inside it answers `default`, the same value the call gives for a null
/// handle. The panic's own message has already gone to the caller's standard
/// error, where the panic hook writes it.
pub(crate) fn caught<T>(default: T, call: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).unwrap_or(default)
}

/// Lock a handle's state, **poisoned or not**.
///
/// A lock is poisoned when a thread panicked while holding it, which
/// [`guarded`] now survives; refusing the lock from then on would turn one
/// failed verb into a handle that answers nothing for the rest of its life.
pub(crate) fn held<T>(state: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verb_that_panics_answers_an_error() {
        let answer = guarded(|| panic!("the verb broke at {}", 7));
        let value: serde_json::Value = serde_json::from_str(&answer).expect("JSON");
        assert_eq!(value["error"], "internal: the verb broke at 7");
        assert_eq!(guarded(|| "{}".to_string()), "{}");
    }

    #[test]
    fn a_lock_a_panic_poisoned_is_still_handed_over() {
        let state = std::sync::Mutex::new(1);
        let answer = guarded(|| {
            let mut inside = held(&state);
            *inside = 2;
            panic!("while holding the lock");
        });
        assert!(answer.contains("internal"));
        assert!(state.is_poisoned());
        assert_eq!(*held(&state), 2, "the next call reads the state");
    }
}
