//! The document.
//!
//! A class rather than free functions taking the tree, and the reason is a
//! measurement rather than a preference. The first binding passed the whole
//! document in and took the whole new one back: 205 ms for one placement on a
//! 10240-event multitrack, linear in the document and independent of the edit.
//! The tree now stays in Rust and only the intent and the outcome cross, which
//! is the same shape `History` already had and for the same reason.
//!
//! What has *not* changed is the discipline: the crate is the only thing that
//! applies an intent. This is not an accessor handle -- there is no call per
//! field of the tree -- it is the same three verbs the by-value binding had,
//! with `snapshot` for whoever wants the JSON.

#[cfg(target_arch = "wasm32")]
use crate::tempomap::JsTempoMap;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// One document, held in Rust -- the JS face of
/// [`clausters_document::Document`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Document)]
pub struct JsDocument(pub(crate) clausters_document::Document);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Document)]
impl JsDocument {
    /// Open a document from its JSON, or an empty one from `undefined`.
    #[wasm_bindgen(constructor)]
    pub fn new(json: Option<String>) -> Result<JsDocument, JsError> {
        let document = match json {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| JsError::new(&format!("document: {e}")))?
            }
            None => clausters_document::Document::new(clausters_document::Node::new(
                clausters_document::NodeId(1),
                clausters_document::Body::Aggregate {
                    grouping: clausters_document::Grouping::Concrete,
                    members: Vec::new(),
                    config: clausters_document::Opaque::none(),
                },
            )),
        };
        Ok(JsDocument(document))
    }

    /// The whole tree as JSON -- for saving it, or for a caller that wants it.
    /// The one call that still costs the size of the document, and it is
    /// asked for rather than paid on every edit.
    pub fn snapshot(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.0).map_err(|e| JsError::new(&e.to_string()))
    }

    /// The monotonic version, bumped by every applied edit. Never zero.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> u64 {
        self.0.version
    }

    /// What makes two edits over the arrangement *the same thing done the same
    /// way* -- the key a {@link History} coalesces on, or an empty string when
    /// the intent will not parse.
    ///
    /// It is here rather than on the history because it is a sentence in **this**
    /// vocabulary: the kind of edit and the node it names. The pile reads no
    /// vocabulary, so a caller recording its own entries asks the domain, which
    /// is what keeps a second spelling of the rule out of every binding.
    #[wasm_bindgen(js_name = coalesceKey)]
    pub fn coalesce_key(intent: &str) -> String {
        serde_json::from_str::<clausters_document::Intent>(intent)
            .map(|intent| clausters_document::log::coalesce_key(&intent))
            .unwrap_or_default()
    }

    /// The edit that would put this node back the way it is -- the inverse of
    /// `intent`, read out of the document **before** anything is applied, or
    /// `undefined` when the document cannot describe it (the node is gone, or
    /// its body holds nothing of that shape).
    ///
    /// {@link History.apply} does this for you and is what an ordinary edit
    /// wants. This is for the caller that records its **own** entry -- a leg of
    /// a transaction spanning several structures, which nothing but the caller
    /// can apply. For a `writesamples` it is the empty write rather than the
    /// span, which is why a destructive caller reads the samples it is about to
    /// overwrite instead of asking here.
    pub fn inverse(&self, intent: &str) -> Result<Option<String>, JsError> {
        let intent = serde_json::from_str::<clausters_document::Intent>(intent)
            .map_err(|e| JsError::new(&format!("document.inverse: {e}")))?;
        let Some(inverse) = clausters_document::inverse_of(&self.0, &intent) else {
            return Ok(None);
        };
        serde_json::to_string(&inverse)
            .map(Some)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Apply an edit. `apply(requestJson) -> outcomeJson`, the request carrying
    /// `{ intent, against?, quant? }` and the result the outcome alone --
    /// the document stays here and `snapshot` is how it leaves.
    ///
    /// One object rather than three arguments because the boundary is JSON
    /// either way, and a request that grows a field then costs no signature.
    pub fn apply(&mut self, request: &str) -> Result<String, JsError> {
        #[derive(serde::Deserialize)]
        struct Request {
            intent: clausters_document::Intent,
            #[serde(default)]
            against: Option<clausters_document::Against>,
            #[serde(default)]
            quant: f64,
        }
        let request: Request =
            serde_json::from_str(request).map_err(|e| JsError::new(&format!("apply: {e}")))?;
        let against = request
            .against
            .unwrap_or_else(clausters_document::Against::unstated);
        let outcome = clausters_document::apply(
            &mut self.0,
            &request.intent,
            &against,
            &clausters_document::Rules {
                quant: request.quant,
            },
        );
        serde_json::to_string(&serde_json::json!({
            "effective": outcome.effective,
            "applied": outcome.applied,
            "reason": outcome.reason,
            "stale": outcome.stale,
        }))
        .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Resolve a selection to the spans of samples underneath it.
    /// `resolve(requestJson, tempo) -> resolvedJson`, the request carrying
    /// `{ selection, framesPerSecond, inBeats? }` and `tempo` being the map
    /// of the clock the document is played on -- a map and a rate because a
    /// placement is in beats and a take's length is in seconds, and a map
    /// rather than a ratio so a selection may cross a tempo change.
    pub fn resolve(&self, request: &str, tempo: &JsTempoMap) -> Result<String, JsError> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Request {
            selection: clausters_document::Selection,
            frames_per_second: f64,
            #[serde(default)]
            in_beats: bool,
        }
        let request: Request =
            serde_json::from_str(request).map_err(|e| JsError::new(&format!("resolve: {e}")))?;
        let mapping = clausters_document::Mapping {
            tempo: &tempo.0,
            frames_per_second: request.frames_per_second,
            unit: if request.in_beats {
                clausters_document::Unit::Beats
            } else {
                clausters_document::Unit::Frames
            },
        };
        let resolved: Vec<_> = clausters_document::resolve(&self.0, &request.selection, &mapping)
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "node": r.node,
                    "source": r.source,
                    "generation": r.generation,
                    "range": r.range,
                    "at": r.at,
                })
            })
            .collect();
        serde_json::to_string(&resolved).map_err(|e| JsError::new(&e.to_string()))
    }
}

/// JS face: a sequence of events (`clausters_document::EventSequence`), every
/// verb through one JSON door, as the C ABI's `clausters_document_sequence_*`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub struct JsEventSequence(pub(crate) clausters_apps::notes::Shared);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl JsEventSequence {
    /// A sequence read from `json` (the sequence, or a bare list of events);
    /// empty for an empty string.
    #[wasm_bindgen(constructor)]
    pub fn new(json: &str) -> Result<JsEventSequence, JsError> {
        let sequence = if json.is_empty() {
            clausters_document::EventSequence::default()
        } else {
            serde_json::from_str(json)
                .map_err(|e| JsError::new(&format!("not a sequence of events: {e}")))?
        };
        Ok(Self(std::sync::Arc::new(std::sync::Mutex::new(sequence))))
    }

    /// One verb, as `clausters_document::events::call_json` answers it.
    pub fn call(&mut self, request: &str) -> String {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        clausters_document::events::call_json(&mut held, request)
    }
}

/// JS face: a multitrack (`clausters_document::Multitrack`) a script holds,
/// every verb through one JSON door, as the C ABI's
/// `clausters_document_multitrack_*`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub struct JsMultitrack(clausters_apps::multitrack::Shared);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
impl JsMultitrack {
    /// A multitrack read from `json`; empty for an empty string.
    #[wasm_bindgen(constructor)]
    pub fn new(json: &str) -> Result<JsMultitrack, JsError> {
        let multitrack = if json.is_empty() {
            clausters_document::Multitrack::default()
        } else {
            serde_json::from_str(json)
                .map_err(|e| JsError::new(&format!("not a multitrack: {e}")))?
        };
        Ok(Self(clausters_apps::multitrack::shared(multitrack)))
    }

    /// One verb, as `clausters_document::multitrack::handle::call_json`
    /// answers it.
    pub fn call(&mut self, request: &str) -> String {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        clausters_document::multitrack::handle::call_json(&mut held, request)
    }
}

/// **The defs a multitrack of these widths is played by** -- `{"synth": [...],
/// "graph": [...]}`, each list in the order it must be sent, or an empty string
/// for a width nothing is written for.
///
/// `widths` is a JSON array of `[source channels, track channels]` pairs and
/// `master` the multitrack's own width. What a track and a clip *are* on the server
/// is `clausters_core::mixer`'s and there is one of it: two clients writing
/// their own channel strips is two mixers, which is how the same multitrack comes to
/// sound different in two places.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = mixerDefs)]
pub fn mixer_defs(widths: &str, master: usize) -> String {
    let Ok(widths) = serde_json::from_str::<Vec<(usize, usize)>>(widths) else {
        return String::new();
    };
    let Ok(defs) = clausters_core::mixer::defs_for(&widths, master) else {
        return String::new();
    };
    serde_json::to_string(&serde_json::json!({
        "synth": defs.synth,
        "graph": defs.graph,
        // The one def a caller instantiates by name rather than by the plan:
        // a curve is a node of the caller's own, beside the multitrack.
        "curve": clausters_core::mixer::curve_name(),
        // **How long a meter's mark waits**, in seconds: a meter is added by
        // the caller (it is the caller who allocated the buses it writes), and
        // the level and the mark are the same def with this as the difference.
        // A number, so it is not the same taste twice in two clients.
        "meterHold": clausters_core::mixer::METER_HOLD,
    }))
    .unwrap_or_default()
}

/// **One catalogue view's props**, as JSON -- the widget a waveform, a curve or
/// a roll *is*, and what is on it; `{"error": reason}` for a kind this crate
/// does not draw or facts that will not read as that kind's.
///
/// `kind` is the view's name (`"waveform"`, `"bpf"`, `"pianoroll"`) and `facts`
/// the JSON that kind is written from. The answer carries the widget's `type`
/// and its props and **no id**: which number a widget gets is the page's, and
/// nothing in the crate knows it.
///
/// One door with the kind named rather than one export per view, the way
/// {@link domainCoalesceKey} names its vocabulary: bound once, and every view
/// the crate learns to draw arrives without a new export.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = viewProps)]
pub fn view_props(kind: &str, facts: &str) -> String {
    let answer = match serde_json::from_str::<serde_json::Value>(facts)
        .map_err(|e| format!("the facts are not JSON: {e}"))
        .and_then(|facts| clausters_document::view::catalogue::props(kind, &facts))
    {
        Ok(props) => serde_json::Value::Object(props),
        Err(reason) => serde_json::json!({ "error": reason }),
    };
    answer.to_string()
}

/// **The tags a view reports that are not edits**, as a JSON array of strings.
///
/// A page routes an incoming `/gui_event` by its tag: screen state is answered
/// generically and never reaches a domain, and everything else is the domain's
/// to read. Which tags those are is one list, and it was written once per
/// client until this call existed -- a table small enough that two copies look
/// harmless and drift silently, since a tag missing from one makes that client
/// *edit* with a gesture the other one merely looks at.
///
/// Read once and kept, not called per event: it answers a constant.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = viewNotAnEdit)]
pub fn view_not_an_edit() -> String {
    serde_json::to_string(&clausters_document::view::NOT_AN_EDIT).unwrap_or_default()
}
