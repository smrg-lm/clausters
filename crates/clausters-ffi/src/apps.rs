//! The applications over the document.
//!
//! The C half of [`clausters_apps`]. Sizes with a null `out` and fills with a
//! second call, like the rest of the JSON surface here — composing a window is
//! a pure read, so a sizing pass changes nothing and can be repeated.

/// **The multitrack editor's window**, as a GuiDef rooted at a `window` node:
/// the time ruler, the piece under it and the transport row.
///
/// `request` is one JSON object: the projection's `piece`, `rate`, `defaultBpm`
/// and `sources`, and the window's `widget` and `ruler` ids, `link`, `cursor`
/// (in beats), `meters`, `transport` (`false`, `true`, or the row's ids),
/// `title`, `w` and `h`. The answer is `{}` for a request that names no piece.
///
/// Returns the byte count the answer needs, or 0 for a request that is not
/// UTF-8.
///
/// # Safety
/// `request` must be readable for `request_len` bytes, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_multitrack_window(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    let answer = clausters_apps::multitrack::window_json(&request);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **Everything one widget of that window should be drawing**, for a
/// correction: the ruler's cursor, or the piece's whole props.
///
/// `request` is the one [`clausters_apps_multitrack_window`] takes, with `for`
/// naming the widget.
///
/// # Safety
/// As [`clausters_apps_multitrack_window`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_apps_multitrack_props(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    let answer = clausters_apps::multitrack::props_json(&request);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}
