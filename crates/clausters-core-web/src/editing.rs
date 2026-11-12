//! The projections an editable structure owes its endpoints
//! (`clausters_editing`): the props a view is drawn with, the intake of an
//! edit, a played event's curves, the step runner, the notes' playback and
//! the conversation a gesture holds with its owner.

#[cfg(target_arch = "wasm32")]
use crate::document::JsEventSequence;
#[cfg(target_arch = "wasm32")]
use crate::registry::JsIdSpaces;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: **the props a break-point curve is drawn with**, as a JSON string --
/// `{"points": [...], "min": .., "max": .., "duration": ..}`.
///
/// The projection, not the rule: `curveAxis` above answers what a curve is
/// drawn against, and this assembles the whole payload a `bpf` widget is set
/// with, so a page and a script send the same props for the same curve. Pass
/// the axis and span the view already has as `keptLo`/`keptHi`/`held`; both are
/// widened and never narrowed. `duration` is absent when the curve spans
/// nothing.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = pointsProps)]
pub fn points_props(
    points: &[f64],
    kept_lo: Option<f64>,
    kept_hi: Option<f64>,
    held: Option<f64>,
) -> String {
    let kept = match (kept_lo, kept_hi) {
        (Some(lo), Some(hi)) => Some((lo, hi)),
        _ => None,
    };
    clausters_editing::points::props_json(points, kept, held.unwrap_or(0.0))
}

/// JS face: **a multitrack as the props the multitrack widget is drawn with**, as a
/// JSON string.
///
/// The rows, the boxes, the automations over both, their break-points, which
/// are hidden and which boxes loop -- everything a multitrack has from the document
/// alone. `sources` is the same table `multitrack_plan` takes, source id to
/// `{"buffer", "channels"}`, because what a box is drawn from and what it is
/// played from are the same samples.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = multitrackProps)]
pub fn multitrack_props(multitrack: &str, sample_rate: f64, sources: &str) -> String {
    clausters_editing::multitrack::props_json(multitrack, sample_rate, sources)
}

/// JS face: **what a gesture means, in a structure's own vocabulary** -- the
/// edit ingestion, as a JSON string.
///
/// A host reports a gesture as a tag and a flat list of values, and this is what
/// turns one into the payloads a structure's own vocabulary states. `domain` is
/// `"points"`, `"samples"`, `"events"` or `"multitrack"`; `request` is the one
/// JSON object `clausters_editing::intake_json` documents, carrying the report
/// and whatever that domain needs beside it.
///
/// The answer is `{"payloads": [...], "label": "...."}`, with `inverse` where
/// the gesture carried one and `refusal` where the gesture is this domain's and
/// cannot be written. A domain or a tag nobody answers for comes back with no
/// payloads and no refusal, which is "nothing to say" rather than a failure.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = editingIntake)]
pub fn editing_intake(domain: &str, tag: &str, request: &str) -> String {
    clausters_editing::intake_json(domain, tag, request)
}

/// JS face: **a played event's curves, as the values a client sends** -- the
/// `/node_set`s of every instant of one window.
///
/// `request` is the step, the stretch of time, the notes that sound with
/// their own curves and the channels' curves, as JSON; the answer is
/// `{"bundles", "states", "over", "idle"}`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = editingEventCurves)]
pub fn editing_event_curves(request: &str) -> String {
    clausters_editing::event_curves::window_json(request)
}

/// JS face: **what is sounding of a multitrack**, held across edits.
///
/// The instance projection's state. The other two projections are functions of
/// a structure alone and this one is a function of a structure *and* of what a
/// server already holds: a multitrack plays itself from the transport, so the nodes
/// have to stay, and what this answers is the **difference**.
/// The tempo a multitrack that states none is drawn at, in beats per second:
/// what its ruler reads, and nothing it places.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = editingDefaultTempo)]
pub fn editing_default_tempo() -> f64 {
    clausters_editing::multitrack::DEFAULT_TEMPO
}

/// **Steps being carried out**: the queue a playback's answers are walked
/// through, as `clausters_editing::run::call_json` documents.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = StepRunner)]
pub struct JsStepRunner(clausters_editing::run::Runner);

#[cfg(target_arch = "wasm32")]
impl Default for JsStepRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = StepRunner)]
impl JsStepRunner {
    /// A runner holding nothing.
    #[wasm_bindgen(constructor)]
    pub fn new() -> JsStepRunner {
        JsStepRunner(clausters_editing::run::Runner::new())
    }

    /// One verb: `push`, `ready`, `reply`, `rendered`, `idle`, `retire`.
    pub fn call(&mut self, request: &str) -> String {
        clausters_editing::run::call_json(&mut self.0, request)
    }
}

/// JS face: the notes editor as it is playing, every verb through one JSON door
/// over the sequence it plays -- the C ABI's `clausters_editing_notes_playback_*`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = NotesPlayback)]
pub struct JsNotesPlayback(clausters_editing::notes_playback::NotesPlayback);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = NotesPlayback)]
impl JsNotesPlayback {
    /// A playback on `transport`; negative for one of its own, which it takes
    /// from the id spaces with its `open` verb or its first play.
    #[wasm_bindgen(constructor)]
    pub fn new(transport: i32) -> JsNotesPlayback {
        use clausters_editing::notes_playback::NotesPlayback;
        JsNotesPlayback(if transport < 0 {
            NotesPlayback::new()
        } else {
            NotesPlayback::on(transport)
        })
    }

    /// One verb over `sequence`, allocating from `ids`.
    pub fn call(
        &mut self,
        sequence: &JsEventSequence,
        request: &str,
        ids: &mut JsIdSpaces,
    ) -> String {
        let held = sequence.0.lock().unwrap_or_else(|e| e.into_inner());
        clausters_editing::notes_playback::call_json(&mut self.0, &held, request, &mut ids.0)
    }
}

/// JS face: **what one message from the host is** -- the conversation's first
/// decision.
///
/// `state` is the conversation's two integers (`{"floor", "applied"}`) and
/// `message` the event's *envelope* -- the address, the stamp, the version it
/// was made against, the tag, and whether this editor owns the widget and the
/// window. The payload is deliberately not here: what a report means is
/// `editingIntake`'s and already crosses once.
///
/// The answer is `{"turn": {...}, "state": {...}}`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = conversationRead)]
pub fn conversation_read(state: &str, message: &str) -> String {
    clausters_editing::conversation::read_json(state, message)
}

/// JS face: **what to answer the host with** -- the conversation's second
/// decision.
///
/// `request` is `{"seq", "docVersion", "reason", "corrections"}` and the answer
/// is one of `silent`, `ack` or `push`. Applied, transformed and refused are
/// one message: there is no success flag in it, and a refusal is simply the
/// previous value among the corrections.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = conversationAnswer)]
pub fn conversation_answer(request: &str) -> String {
    clausters_editing::conversation::answer_json(request)
}

/// JS face: **what a multitrack calls its rows and its boxes** -- `{"rows": [...],
/// "boxes": [...]}`, by the names the wire carries them under.
///
/// The minting correction's half that is a fact about the multitrack: a host that
/// made a track or split a box minted the *word* while the document minted the
/// *id*, so a view keeps what it was last told and answers with the picture
/// when the two stop agreeing.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = multitrackNames)]
pub fn multitrack_names(multitrack: &str) -> String {
    clausters_editing::multitrack::names_json(multitrack)
}
