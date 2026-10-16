//! The plugin catalog's doors (`clausters_core::plugin_catalog`) and a node's
//! generic editor (`clausters_core::node_editor`): one JSON request in, one
//! JSON answer out, the same documents the wasm build takes.

use crate::event::json_door;
use serde_json::Value;

/// A catalog from scan rows: `{"rows": [...]}` in, `{"plugins": [...],
/// "errors": [...]}` out (`plugin_catalog::catalog_request`).
///
/// # Safety
/// `request` must be readable for `len` bytes, `out` null or writable for
/// `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_plugin_catalog(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from the caller.
    unsafe {
        json_door(request, len, out, out_cap, |req| {
            clausters_core::plugin_catalog::catalog_request(&Value::Object(req.clone()))
        })
    }
}

/// A search over a catalog's plugins: `{"plugins", "query", "one"}` in,
/// `{"index"}` or `{"indices"}` out (`plugin_catalog::search_request`).
///
/// # Safety
/// `request` must be readable for `len` bytes, `out` null or writable for
/// `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_plugin_search(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from the caller.
    unsafe {
        json_door(request, len, out, out_cap, |req| {
            clausters_core::plugin_catalog::search_request(&Value::Object(req.clone()))
        })
    }
}

/// A plugin def's reference to a plugin, its ports routed: `{"plugin",
/// "inputs", "outputs", "state"}` in, the reference out
/// (`plugin_catalog::reference_request`).
///
/// # Safety
/// `request` must be readable for `len` bytes, `out` null or writable for
/// `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_plugin_reference(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from the caller.
    unsafe {
        json_door(request, len, out, out_cap, |req| {
            clausters_core::plugin_catalog::reference_request(&Value::Object(req.clone()))
        })
    }
}

/// A node's generic editor: `{"node", "name", "controls", "query", "page"}`
/// in, `{"window", "contents", "pages", "page"}` out
/// (`node_editor::editor_request`).
///
/// # Safety
/// `request` must be readable for `len` bytes, `out` null or writable for
/// `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_node_editor(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from the caller.
    unsafe {
        json_door(request, len, out, out_cap, |req| {
            clausters_core::node_editor::editor_request(&Value::Object(req.clone()))
        })
    }
}
