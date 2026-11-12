//! The component bundle.
//!
//! The bundle mount, opened to the page: a persisted bundle is a template, and the
//! page turns it into N non-colliding instances. The pass itself is
//! `clausters_core::bundle` -- pure, natively tested, and the same one the native
//! `--standalone` leg runs -- so these three are only the JSON boundary. The
//! page allocates between `bundle_requirements` and `bundle_resolve`, from its
//! own `Server`/`GuiHost` allocators; nothing here allocates or keeps state.

#[cfg(target_arch = "wasm32")]
use clausters_core::bundle;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: what one instance of a bundle needs allocated.
/// `bundle_requirements(requestJson) -> requirementsJson`, the request holding
/// the manifest and -- for a bundle written before the contract, whose widget
/// ids are whatever its author picked -- the template its id block is measured
/// from.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bundle_requirements(request: &str) -> Result<String, JsError> {
    let request: bundle::RequirementsRequest =
        serde_json::from_str(request).map_err(|e| JsError::new(&format!("requirements: {e}")))?;
    serde_json::to_string(&bundle::requirements_request(&request))
        .map_err(|e| JsError::new(&e.to_string()))
}

/// JS face: one mounted instance, from the allocation the page just made.
/// `bundle_resolve(requestJson) -> resolvedJson`, the request carrying the
/// manifest, the template, the allocation and the supplied parameters
/// (`{ attributes, preset }`) in one object.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bundle_resolve(request: &str) -> Result<String, JsError> {
    let request: bundle::ResolveRequest =
        serde_json::from_str(request).map_err(|e| JsError::new(&format!("resolve: {e}")))?;
    let resolved = bundle::resolve_request(&request).map_err(|e| JsError::new(&e.to_string()))?;
    serde_json::to_string(&resolved).map_err(|e| JsError::new(&e.to_string()))
}

/// JS face: the writers' pre-flight -- the mount dry-run over the declared
/// defaults, plus the no-holes check on every def payload.
/// `bundle_validate(requestJson)`, throwing on the first problem.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bundle_validate(request: &str) -> Result<(), JsError> {
    let request: bundle::ValidateRequest =
        serde_json::from_str(request).map_err(|e| JsError::new(&format!("validate: {e}")))?;
    bundle::validate_request(&request).map_err(|e| JsError::new(&e.to_string()))
}
