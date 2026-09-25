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
