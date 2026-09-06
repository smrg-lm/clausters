//! The difference between two GuiDef pictures.

use clausters_core::guidiff::{self, Update};

use crate::document::{fill, text};

// --- The redraw difference (ABI v40) ------------------------------------
//
// A pure read over two JSON documents, so it takes and gives text and holds
// nothing: the caller keeps the picture the host is drawing, hands it here with
// the one it would draw now, and gets back either the sets to send or the word
// that says the tree has to go whole.

/// What to send so a host drawing `old` draws `new` instead, as JSON:
/// `{"define": true}` when the shape changed, else
/// `{"sets": [[<id>, {<prop>: <value>, …}], …]}` in tree order (an empty list
/// meaning the two are identical and nothing need be sent).
///
/// Sizes with a null `out` and fills with a second call, like the rest of the
/// JSON surface. Unreadable input answers `{"define": true}` rather than
/// failing: a caller that cannot be diffed can always send the tree.
///
/// # Safety
/// `old` must be null or readable for `old_len` bytes, `new` null or readable
/// for `new_len` bytes, and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_gui_difference(
    old: *const u8,
    old_len: usize,
    new: *const u8,
    new_len: usize,
    root_id: i64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(old), Some(new)) = (unsafe { text(old, old_len) }, unsafe { text(new, new_len) })
    else {
        return 0;
    };
    let answer = match (
        serde_json::from_str::<serde_json::Value>(&old),
        serde_json::from_str::<serde_json::Value>(&new),
    ) {
        (Ok(old), Ok(new)) => guidiff::difference(&old, &new, root_id),
        _ => Update::Define,
    };
    let payload = match answer {
        Update::Define => serde_json::json!({"define": true}),
        Update::Sets(sets) => serde_json::json!({
            "sets": sets
                .into_iter()
                .map(|(id, props)| serde_json::json!([id, props]))
                .collect::<Vec<_>>()
        }),
    };
    let text = payload.to_string();
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { fill(text.as_bytes(), out, out_cap, || {}) }
}
