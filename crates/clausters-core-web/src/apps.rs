//! The applications over the document (`clausters_apps`): the editing
//! context a page holds, and what a take's samples measure.

#[cfg(target_arch = "wasm32")]
use crate::document::JsEventSequence;
#[cfg(target_arch = "wasm32")]
use crate::notation::JsScore;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// **An editing context**: one undo order over every editor opened in it. Its
/// verbs cross through `call`, as JSON.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = EditingCore)]
pub struct JsEditing(clausters_apps::editing::Editing);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = EditingCore)]
impl JsEditing {
    /// An empty context.
    #[wasm_bindgen(constructor)]
    pub fn new() -> JsEditing {
        JsEditing(clausters_apps::editing::Editing::new())
    }

    /// One verb, as `clausters_apps::editing::call_json` documents.
    pub fn call(&mut self, request: &str) -> String {
        clausters_apps::editing::call_json(&mut self.0, request)
    }

    /// Opens a notes editor over `sequence`, which it then edits in place --
    /// the C ABI's `clausters_apps_editing_open_notes`. `request` carries the
    /// `key` and what the editor is built from.
    #[wasm_bindgen(js_name = openNotes)]
    pub fn open_notes(&mut self, sequence: &JsEventSequence, request: &str) -> String {
        let key = serde_json::from_str::<serde_json::Value>(request)
            .ok()
            .and_then(|r| r.get("key").and_then(|k| k.as_str().map(str::to_owned)))
            .unwrap_or_default();
        self.0.open_notes(&key, sequence.0.clone(), request)
    }

    /// Opens a score editor over `score`, which it then edits in place -- the
    /// C ABI's `clausters_apps_editing_open_score`. `request` carries the `key`
    /// and what the editor is built from.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = openScore)]
    pub fn open_score(&mut self, score: &JsScore, request: &str) -> String {
        let key = serde_json::from_str::<serde_json::Value>(request)
            .ok()
            .and_then(|r| r.get("key").and_then(|k| k.as_str().map(str::to_owned)))
            .unwrap_or_default();
        self.0.open_score(&key, score.0.clone(), request)
    }

    /// Opens a score editor over `sequence`, on the page it is read into --
    /// the C ABI's `clausters_apps_editing_open_score_over`. `score` is loaded
    /// with the reading, and an edit on the page is then an edit of the
    /// sequence. `request` carries the sequence's `key`, the transcription
    /// (`how`) and the reading (`interp`).
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = openScoreOver)]
    pub fn open_score_over(
        &mut self,
        score: &JsScore,
        sequence: &JsEventSequence,
        request: &str,
    ) -> String {
        let key = serde_json::from_str::<serde_json::Value>(request)
            .ok()
            .and_then(|r| r.get("key").and_then(|k| k.as_str().map(str::to_owned)))
            .unwrap_or_default();
        self.0
            .open_score_over(&key, score.0.clone(), sequence.0.clone(), request)
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for JsEditing {
    fn default() -> Self {
        Self::new()
    }
}

/// JS face: **a measure stack, checked** -- `{"stack": [...]}` answers
/// `{"layers": [...]}` or `{"error"}` naming what is refused.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = samplesMeasures)]
pub fn samples_measures(request: &str) -> String {
    clausters_apps::samples::measures_json(request)
}
