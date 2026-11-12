//! The patcher's cord-to-bus compile: a patch JSON in, its GraphDef wiring
//! out.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The patcher's **cord->bus pass**: a directed patch (`{boxes, cords}`) in, the
/// buses and wired members it compiles to out, both as JSON.
///
/// One bus per connected net, its writers summing, and a bad cord -- reversed,
/// rate-mismatched, out of range -- comes back as `{"error": ...}` naming it. The
/// same door the C ABI opens as `clausters_core_patch_compile`: a patcher is a
/// model with one compilation, and a second implementation of it in TypeScript
/// would be a second answer to "what does this cord mean".
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = patchCompile)]
pub fn patch_compile(patch: &str) -> Result<String, JsError> {
    use clausters_core::patch::{Patch, compile};

    let parsed: Patch =
        serde_json::from_str(patch).map_err(|e| JsError::new(&format!("patch: {e}")))?;
    match compile(&parsed) {
        Ok(compiled) => serde_json::to_string(&compiled).map_err(|e| JsError::new(&e.to_string())),
        Err(e) => Err(JsError::new(&e)),
    }
}
