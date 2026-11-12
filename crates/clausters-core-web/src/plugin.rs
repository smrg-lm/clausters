//! The plugin catalog's doors (`clausters_core::plugin_catalog`) and a
//! node's generic editor.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: the plugin catalog's doors (`clausters_core::plugin_catalog`),
/// one JSON request in and one JSON answer out, the documents the C ABI
/// takes; an error is `{"error"}`. A request that is not JSON is one too.
#[cfg(target_arch = "wasm32")]
fn plugin_door(request: &str, door: fn(&serde_json::Value) -> serde_json::Value) -> String {
    match serde_json::from_str::<serde_json::Value>(request) {
        Ok(value) if value.is_object() => door(&value).to_string(),
        _ => serde_json::json!({"error": "the request is not a JSON object"}).to_string(),
    }
}

/// JS face: a catalog from scan rows, `{"rows"}` in, `{"plugins", "errors"}` out.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn plugin_catalog(request: &str) -> String {
    plugin_door(request, clausters_core::plugin_catalog::catalog_request)
}

/// JS face: a search over a catalog's plugins, `{"plugins", "query", "one"}`
/// in, `{"index"}` or `{"indices"}` out.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn plugin_search(request: &str) -> String {
    plugin_door(request, clausters_core::plugin_catalog::search_request)
}

/// JS face: a plugin def's reference, `{"plugin", "inputs", "outputs",
/// "state"}` in, the reference out.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn plugin_reference(request: &str) -> String {
    plugin_door(request, clausters_core::plugin_catalog::reference_request)
}

/// JS face: a node's generic editor, `{"node", "name", "controls", "query",
/// "page"}` in, `{"window", "contents", "pages", "page"}` out.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn node_editor(request: &str) -> String {
    plugin_door(request, clausters_core::node_editor::editor_request)
}
