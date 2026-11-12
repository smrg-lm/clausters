//! The notation layer.
//!
//! The engraving logic is the core's and there is one of it: the SVG walk, the
//! MEI encoder, and the editable `Score` -- the order an edit is made in, the
//! reload that keeps the timemap honest, the undo stack of MEI snapshots. What
//! differs in a page is only *where verovio is*: not a linked C++ library but a
//! module the page loads, so the engraver arrives as a JS object and this door
//! makes it look like the port the core drives.
//!
//! The methods that object must have are exactly verovio's toolkit calls, in
//! this language's spelling: `loadData(data)`, `renderSvg(page)`, `mei()`,
//! `edit(actionJson)`, `timemap(optionsJson)`, `midiValues(id)`. The web client
//! wraps the published toolkit in one; anything else answering to those six is
//! as good, which is what makes this testable without an engraver at all.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The engraver as a page has it: a JS object with the six toolkit calls.
///
/// Every crossing is `Reflect::get` plus a call, and every failure -- a missing
/// method, a thrown exception, a value of the wrong shape -- reads as the same
/// thing a refusal reads as. That is the port's rule (failure is a value), and
/// it is what keeps a broken engraver from taking a page's edit half-applied:
/// the score rolls back either way.
#[cfg(target_arch = "wasm32")]
struct JsEngraver {
    object: js_sys::Object,
}

#[cfg(target_arch = "wasm32")]
impl JsEngraver {
    /// Call `name` with `args`, or `None` when the method is missing or threw.
    fn call(&self, name: &str, args: &[JsValue]) -> Option<JsValue> {
        let method = js_sys::Reflect::get(&self.object, &JsValue::from_str(name)).ok()?;
        let method = method.dyn_ref::<js_sys::Function>()?;
        let out = match args {
            [] => method.call0(&self.object),
            [a] => method.call1(&self.object, a),
            [a, b] => method.call2(&self.object, a, b),
            _ => return None,
        };
        out.ok()
    }

    fn text(&self, name: &str, args: &[JsValue]) -> Option<String> {
        self.call(name, args)?.as_string()
    }
}

// SAFETY: a page's wasm module runs on the one thread that made it, so the JS
// object is never reached from another; `Send` is what an application's score
// asks of every engraver, for the bindings that do have threads.
#[cfg(target_arch = "wasm32")]
unsafe impl Send for JsEngraver {}

#[cfg(target_arch = "wasm32")]
impl clausters_notation::Engraver for JsEngraver {
    /// A page has one thread and the engraver is reached from it alone, so
    /// there is nothing to serialize. The guard exists for the native binding,
    /// where libverovio's process-wide state makes it load-bearing.
    type Guard = ();

    fn lock(&self) -> Self::Guard {}

    fn load_data(&self, data: &str) -> bool {
        self.call("loadData", &[JsValue::from_str(data)])
            .map(|v| v.is_truthy())
            .unwrap_or(false)
    }

    fn render_svg(&self, page: i32) -> String {
        self.text("renderSvg", &[JsValue::from_f64(page as f64)])
            .unwrap_or_default()
    }

    fn mei(&self) -> String {
        self.text("mei", &[]).unwrap_or_default()
    }

    fn edit(&self, action: &str) -> bool {
        self.call("edit", &[JsValue::from_str(action)])
            .map(|v| v.is_truthy())
            .unwrap_or(false)
    }

    fn timemap(&self, options: &str) -> String {
        self.text("timemap", &[JsValue::from_str(options)])
            .unwrap_or_default()
    }

    fn midi_values(&self, xml_id: &str) -> Option<String> {
        self.text("midiValues", &[JsValue::from_str(xml_id)])
            .filter(|s| !s.is_empty())
    }

    fn set_options(&self, options: &str) -> bool {
        self.call("setOptions", &[JsValue::from_str(options)])
            .map(|v| v.is_truthy())
            .unwrap_or(false)
    }

    fn page_count(&self) -> i32 {
        self.call("pageCount", &[])
            .and_then(|v| v.as_f64())
            .map_or(1, |n| n as i32)
    }
}

/// A loaded score, held open in Rust so it can be edited and re-engraved -- the
/// JS face of [`clausters_notation::Score`].
///
/// The same object the Python client holds over the C ABI, running the same
/// state machine: a page that transposes a note and one that transposes it in a
/// window take the identical sequence of calls to verovio.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Score)]
pub struct JsScore(pub(crate) clausters_apps::score::Shared);

#[cfg(target_arch = "wasm32")]
impl JsScore {
    /// The score, held for one call.
    fn held(
        &self,
    ) -> std::sync::MutexGuard<'_, clausters_notation::Score<clausters_notation::AnyEngraver>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Score)]
impl JsScore {
    /// Load `data` (any format the engraver auto-detects) on `engraver`, or
    /// throw when it could not be read.
    ///
    /// Configuring the engraver -- its resource path, its options -- happens on
    /// the JS side before this, exactly as the native binding configures its
    /// toolkit before handing it over.
    #[wasm_bindgen(constructor)]
    pub fn new(engraver: js_sys::Object, data: &str) -> Result<JsScore, JsError> {
        clausters_notation::Score::open(
            clausters_notation::AnyEngraver::new(JsEngraver { object: engraver }),
            data,
        )
        .map(|score| JsScore(std::sync::Arc::new(std::sync::Mutex::new(score))))
        .ok_or_else(|| JsError::new("the engraver could not load the score data"))
    }

    /// This score engraved into a page: the display list the host draws, the
    /// cursor track a playhead follows, and the notes that sound.
    #[wasm_bindgen(js_name = displayList)]
    pub fn display_list(&mut self, page: i32) -> Result<String, JsError> {
        serde_json::to_string(&self.held().display_list(page))
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// The score as MEI, ids and all -- what to persist, and what an undo step
    /// is made of.
    pub fn mei(&self) -> String {
        self.held().mei()
    }

    /// Apply one **model** operation as a single undo step, and re-engrave.
    ///
    /// This is the edit path, and the reason it is not the engraver's editor:
    /// there is one implementation of what an edit to a score means, and it is
    /// vocabulary this package already binds. Returns whether it was applied;
    /// a refusal leaves both the page and the model as they were.
    pub fn apply(&mut self, op: &str) -> Result<bool, JsError> {
        let op: clausters_notation::Op =
            serde_json::from_str(op).map_err(|e| JsError::new(&format!("op: {e}")))?;
        Ok(self.held().apply(&op))
    }

    /// The open score as the **model**.
    ///
    /// Throws when the document could not be read into one -- a state and not a
    /// failure, since the page still draws and still plays and only the model's
    /// verbs are unavailable on it.
    pub fn sheet(&self) -> Result<String, JsError> {
        let held = self.held();
        let sheet = held.sheet().ok_or_else(|| {
            JsError::new(
                "this document could not be read into the score model, so the \
                 model's verbs are not available on it; the page still draws \
                 and still plays",
            )
        })?;
        serde_json::to_string(sheet).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Replace the document with `mei` -- **a state, not a step**.
    ///
    /// The door for a page whose editing context holds one history over several
    /// structures: a score's edit is recorded there as the MEI it produced, and
    /// a previous one is put back through here. It clears the score's own
    /// stack, so there is only ever one history over one score.
    pub fn load(&mut self, mei: &str) -> bool {
        self.held().load(mei)
    }

    /// Whether there is an edit to step back over.
    #[wasm_bindgen(getter, js_name = canUndo)]
    pub fn can_undo(&self) -> bool {
        self.held().can_undo()
    }

    /// Whether there is an undone edit to step forward into.
    #[wasm_bindgen(getter, js_name = canRedo)]
    pub fn can_redo(&self) -> bool {
        self.held().can_redo()
    }

    /// Step back one edit; `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.held().undo()
    }

    /// Step forward again after an undo; `false` when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        self.held().redo()
    }

    /// Move a note by `steps` diatonic steps along the staff, as one undo step.
    /// The relative form: reach for it only when the delta is what you have.
    pub fn transpose(&mut self, element_id: &str, steps: i32) -> bool {
        self.held().transpose(element_id, steps)
    }

    /// Move a note **to** a diatonic staff position, as one undo step -- the
    /// shape an edit travels in, so a resend cannot move the note twice.
    #[wasm_bindgen(js_name = transposeTo)]
    pub fn transpose_to(&mut self, element_id: &str, position: i32, page: i32) -> bool {
        self.held().transpose_to(element_id, position, page)
    }

    /// One raw editor action (`set`, `insert`, `delete`, ...) as a single undo
    /// step, `param` being its parameter object as JSON.
    pub fn edit(&mut self, action: &str, param: &str) -> bool {
        self.held().edit(action, param)
    }
}

/// The engraver's options for one page, as the JSON object it is configured
/// with: `scale` (staff size), `pageWidth` (the page units a score wraps into
/// systems at) and an optional JSON object merged over them.
///
/// A page configures its engraver through this rather than through a table of
/// its own, for the same reason the score model is shared: two clients that
/// configure verovio differently draw the same score two ways, and then no
/// display list from one is comparable with one from the other.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = engraveOptions)]
pub fn engrave_options(scale: i32, page_width: i32, extra: Option<String>) -> String {
    clausters_notation::engrave_options(scale, page_width, extra.as_deref())
}

/// Walk a verovio SVG into a `score` display list, as JSON.
///
/// The one-shot path: a page that only draws a score engraves once and walks
/// the SVG, with no document held open. A malformed SVG yields an empty display
/// list rather than an error, as the C ABI's twin does.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = svgToDisplayList)]
pub fn svg_to_display_list(svg: &str) -> Result<String, JsError> {
    serde_json::to_string(&clausters_notation::svg_to_display_list(svg))
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Lay a **voice** -- a JSON array of slots, `{"midis": [60], "ticks": 8}` per
/// note or chord and `{"ticks": 8}` per rest -- out into barred, tied MEI.
///
/// `meter` is `"num/den"`, `clef` a shape+line like `"G2"`, and `key` selects
/// the key signature and the sharp-vs-flat spelling. Reducing a client's own
/// sequencing data to that voice stays in the client, where the native types
/// are; this is the language-agnostic step below it, and the seam a richer
/// encoding extends for every client at once.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = voiceToMei)]
pub fn voice_to_mei(voice: &str, meter: &str, clef: &str, key: &str) -> Result<String, JsError> {
    let voice: Vec<clausters_notation::Slot> =
        serde_json::from_str(voice).map_err(|e| JsError::new(&format!("voice: {e}")))?;
    Ok(clausters_notation::voice_to_mei(&voice, meter, clef, key))
}

/// Lift a **voice** -- the v1 wire form, a JSON array of slots -- into the score
/// model.
///
/// The bridge a client crosses once: it reduces its own sequencing types to
/// slots, which reads client-native types and stays here, and everything above
/// that is the model. Ticks become exact durations and MIDI numbers become
/// spelled pitches in the world `key` implies.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = voiceToSheet)]
pub fn voice_to_sheet(voice: &str, meter: &str, clef: &str, key: &str) -> Result<String, JsError> {
    let voice: Vec<clausters_notation::Slot> =
        serde_json::from_str(voice).map_err(|e| JsError::new(&format!("voice: {e}")))?;
    let sheet = clausters_notation::voice_to_sheet(&voice, meter, clef, key);
    serde_json::to_string(&sheet).map_err(|e| JsError::new(&e.to_string()))
}

/// Apply one operation to a score model, both as JSON, returning the new model.
///
/// **One export for every operation there will ever be**: the verb and its
/// parameters are inside `op` (`{"op": "transpose", "semitones": 2}`), so a new
/// operation costs nothing here. What the C ABI answers in an envelope
/// (`{"ok": ...}` / `{"error": ...}`) this **throws** instead, which is the same
/// behaviour in the shape a page expects -- and the reason the refusal reaches
/// the caller either way, since a refused operation has to say why.
///
/// A refused operation changes nothing: the model crossed by value, so the
/// caller still holds what it sent.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetApply)]
pub fn sheet_apply(sheet: &str, op: &str) -> Result<String, JsError> {
    let sheet: clausters_notation::Sheet =
        serde_json::from_str(sheet).map_err(|e| JsError::new(&format!("sheet: {e}")))?;
    let op: clausters_notation::Op =
        serde_json::from_str(op).map_err(|e| JsError::new(&format!("op: {e}")))?;
    let out = clausters_notation::apply(sheet, &op).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}

/// Write a score model out as MEI.
///
/// Throws with the emitter's own reason when the model holds something MEI
/// cannot be written for yet -- a duration that is not an exact note value, an
/// accidental past a double, or polyphony -- each saying which it is, so a
/// caller knows whether it is wrong or early.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetToMei)]
pub fn sheet_to_mei(sheet: &str) -> Result<String, JsError> {
    let sheet: clausters_notation::Sheet =
        serde_json::from_str(sheet).map_err(|e| JsError::new(&format!("sheet: {e}")))?;
    clausters_notation::sheet_to_mei(&sheet).map_err(|e| JsError::new(&e))
}

/// Every operation this core knows, as JSON -- the verb and the parameters each
/// takes.
///
/// The parity surface the binding table cannot provide: operations cross as
/// data, so nothing fails when one client grows a verb the other lacks. This
/// package is contrasted against this list, as the Python client is.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetOps)]
pub fn sheet_ops() -> Result<String, JsError> {
    serde_json::to_string(clausters_notation::catalog()).map_err(|e| JsError::new(&e.to_string()))
}

/// Read an MEI document into the score model.
///
/// The other return path, and not the one {@link sheetPerform} is: that turns a
/// model into sound, this turns a *document* into a model. A score opened from
/// typed text is a document and nothing else until this reads one, which is why
/// the model's verbs cannot touch it until then. There is one input format rather than
/// four, because the engraver normalizes whatever it loaded to MEI.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = meiToSheet)]
pub fn mei_to_sheet(mei: &str) -> Result<String, JsError> {
    let sheet = clausters_notation::mei_to_sheet(mei).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&sheet).map_err(|e| JsError::new(&e.to_string()))
}

/// Read a score model into the notes it **sounds**, under `interp`.
///
/// Each note carries two lengths -- `dur`, what is written, and `sustain`, what
/// is heard -- because an honoured articulation makes them different numbers and
/// collapsing them would move every attack after a staccato. It also names the
/// `staff` and `voice` it was written on, which is what a caller binds an
/// instrument to: the notation does not say what plays it.
///
/// `interp` may be `""` or `"{}"` for the default reading, and any field left
/// out keeps its default. What the default *is* comes back from
/// [`interpretation`], so this package writes none of those numbers down.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetPerform)]
pub fn sheet_perform(sheet: &str, interp: &str) -> Result<String, JsError> {
    let sheet: clausters_notation::Sheet =
        serde_json::from_str(sheet).map_err(|e| JsError::new(&format!("sheet: {e}")))?;
    let interp: clausters_notation::Interpretation = if interp.trim().is_empty() {
        clausters_notation::default_interpretation()
    } else {
        serde_json::from_str(interp).map_err(|e| JsError::new(&format!("interpretation: {e}")))?
    };
    let notes = clausters_notation::perform(sheet, &interp).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&notes).map_err(|e| JsError::new(&e.to_string()))
}

/// **Render a sheet into a sequence**, one way, as JSON: the events it sounds,
/// each still saying what it is on the page, a channel to a voice, a staff's
/// dynamics as lanes and what is no note's as the sequence's `notation`
/// section (`clausters_document::events::score::render`). `interp` is the
/// reading, as [`sheet_perform`] takes it.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetRenderEvents)]
pub fn sheet_render_events(sheet: &str, interp: &str) -> Result<String, JsError> {
    let sheet: clausters_notation::Sheet =
        serde_json::from_str(sheet).map_err(|e| JsError::new(&format!("sheet: {e}")))?;
    let interp: clausters_notation::Interpretation = if interp.trim().is_empty() {
        clausters_notation::default_interpretation()
    } else {
        serde_json::from_str(interp).map_err(|e| JsError::new(&format!("interpretation: {e}")))?
    };
    let sequence =
        clausters_document::events::score::render(&sheet, &interp).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&sequence).map_err(|e| JsError::new(&e.to_string()))
}

/// **Read a sequence into a score**, as JSON `{"sheet", "items"}`: the way
/// back from [`sheet_render_events`] -- what the events say of their page
/// written as they say it, and what they do not decided by `how`, the
/// transcription (`clausters_document::events::transcription::read`).
/// `interp` is the reading whose dynamics name a level. Each of the two is
/// empty for its default -- `how` naming only what goes over the reading the
/// sequence states.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sheetReadEvents)]
pub fn sheet_read_events(sequence: &str, how: &str, interp: &str) -> Result<String, JsError> {
    use clausters_document::events::transcription::{Transcription, read};
    let sequence: clausters_document::events::EventSequence =
        serde_json::from_str(sequence).map_err(|e| JsError::new(&format!("sequence: {e}")))?;
    // what is given goes over the reading the sequence states
    let how: serde_json::Value = if how.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(how).map_err(|e| JsError::new(&format!("transcription: {e}")))?
    };
    let how = Transcription::over(&sequence, &how).map_err(|e| JsError::new(&e))?;
    let interp: clausters_notation::Interpretation = if interp.trim().is_empty() {
        clausters_notation::default_interpretation()
    } else {
        serde_json::from_str(interp).map_err(|e| JsError::new(&format!("interpretation: {e}")))?
    };
    let read = read(&sequence, &how, &interp).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&read).map_err(|e| JsError::new(&e.to_string()))
}

/// The default interpretation, as JSON -- every number the reading depends on,
/// and the value an override starts from.
///
/// The parity surface for the reading, as `sheetOps` is for the verbs: the
/// interpretation crosses inside a payload, so nothing structural notices when
/// one client's idea of `mf` drifts from the other's.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = interpretation)]
pub fn interpretation() -> Result<String, JsError> {
    serde_json::to_string(&clausters_notation::default_interpretation())
        .map_err(|e| JsError::new(&e.to_string()))
}

/// The **model** item an engraved element belongs to, or `-1` when the element
/// was not written from one.
///
/// The page names elements the way the emitter wrote them: `n7` is the item,
/// `n7-2` a part of it split across a barline, `n7-p1` one pitch of a chord.
/// All three are the same item, which is what lets a gesture anywhere on a note
/// reach the note -- and it is the step a client takes between a page's
/// selection and a model verb.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = itemId)]
pub fn item_id(element_id: &str) -> f64 {
    clausters_notation::item_id(element_id).map_or(-1.0, |id| id as f64)
}
