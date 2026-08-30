//! The notation layer's C surface: the pure logic every client shares, and —
//! behind the `verovio` feature — the engraver and the editable score.
//!
//! Two shapes, both already used elsewhere in this crate. Everything that
//! returns text or JSON is **size-then-fill**: the call returns the byte count
//! the result needs and writes it only if it fit, so a caller sizes with a null
//! (or short) `out` and fills with a second call — the
//! [`clausters_core_patch_compile`](crate::clausters_core_patch_compile)
//! pattern. Everything stateful carries an opaque handle, as the scheduler and
//! the sample-clock model do.
//!
//! Size-then-fill needs the payload to be **the same on both calls**, and that
//! is why there is no one-shot `engrave` here: the engraver mints fresh `xml:id`s
//! on every load, and their lengths vary, so sizing and filling would engrave two
//! different documents and the second could no longer fit the buffer measured for
//! the first. A binding's one-shot is [`clausters_score_open`] ->
//! [`clausters_score_display_list`] -> [`clausters_score_free`], where the ids
//! are minted once and the page is stable for as long as the handle lives.
//!
//! Strings cross as UTF-8 pointer+length; invalid UTF-8 is read lossily rather
//! than refused, so no binding has to validate before calling.

use clausters_core::notation::{
    Interpretation, Op, Sheet, Slot, apply, catalog, default_interpretation, mei_to_sheet, perform,
    sheet_to_mei, svg_to_display_list, voice_to_mei, voice_to_sheet,
};

/// Read a pointer+length as UTF-8 (lossily), or `None` when the pointer is null.
///
/// # Safety
/// `ptr` must be null or readable for `len` bytes.
unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<std::borrow::Cow<'a, str>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `ptr` is readable for `len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    Some(String::from_utf8_lossy(bytes))
}

/// Write `payload` into `out` if it fits, and return the byte count it needs.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
unsafe fn fill(payload: &[u8], out: *mut u8, out_cap: usize) -> usize {
    let n = payload.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), out, n) };
    }
    n
}

/// Walk a verovio SVG (`svg`/`svg_len`) into a `score` display list, written as
/// JSON to `out` (capacity `out_cap`). Returns the byte count the JSON needs, or
/// `0` when `svg` is null.
///
/// The producer of the SVG is interchangeable — native libverovio here, a wasm
/// verovio in a browser — which is what makes one host renderer serve every
/// client. A malformed SVG yields an empty display list, not an error.
///
/// # Safety
/// `svg` must be readable for `svg_len` bytes and `out` writable for `out_cap`
/// bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_svg_to_display_list(
    svg: *const u8,
    svg_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees the range.
    let Some(svg) = (unsafe { text(svg, svg_len) }) else {
        return 0;
    };
    let json = serde_json::to_vec(&svg_to_display_list(&svg)).unwrap_or_default();
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(&json, out, out_cap) }
}

/// Lay a **voice** — a JSON array of slots, `{"midis": [60], "ticks": 8}` per
/// note or chord and `{"ticks": 8}` per rest — out into barred, tied MEI,
/// written to `out` (capacity `out_cap`). Returns the byte count the MEI needs,
/// or `0` when the voice is not readable JSON.
///
/// `meter` is `"num/den"`, `clef` a shape+line like `"G2"`, and `key` selects the
/// key signature and the sharp-vs-flat spelling. Reducing a client's own
/// sequencing data to the voice stays in that client — it reads client-native
/// types; this is the language-agnostic step below it.
///
/// # Safety
/// Each pointer must be readable for its length, and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn clausters_core_voice_to_mei(
    voice: *const u8,
    voice_len: usize,
    meter: *const u8,
    meter_len: usize,
    clef: *const u8,
    clef_len: usize,
    key: *const u8,
    key_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees every range.
    let (Some(voice), Some(meter), Some(clef), Some(key)) = (unsafe {
        (
            text(voice, voice_len),
            text(meter, meter_len),
            text(clef, clef_len),
            text(key, key_len),
        )
    }) else {
        return 0;
    };
    let Ok(voice) = serde_json::from_str::<Vec<Slot>>(&voice) else {
        return 0;
    };
    let mei = voice_to_mei(&voice, &meter, &clef, &key);
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(mei.as_bytes(), out, out_cap) }
}

/// Lift a **voice** — the v1 wire form, a JSON array of slots — into the score
/// model, written to `out` as the same envelope the other sheet calls answer
/// in. Returns the byte count it needs, or `0` when a pointer is null.
///
/// This is the bridge a client crosses once: it reduces its own sequencing
/// types to slots (which reads client-native types and stays in the client),
/// and everything above that is the model. Ticks become exact durations and
/// MIDI numbers become spelled pitches in the world `key` implies.
///
/// # Safety
/// Each pointer must be readable for its length, and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn clausters_core_voice_to_sheet(
    voice: *const u8,
    voice_len: usize,
    meter: *const u8,
    meter_len: usize,
    clef: *const u8,
    clef_len: usize,
    key: *const u8,
    key_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees every range.
    let (Some(voice), Some(meter), Some(clef), Some(key)) = (unsafe {
        (
            text(voice, voice_len),
            text(meter, meter_len),
            text(clef, clef_len),
            text(key, key_len),
        )
    }) else {
        return 0;
    };
    let json = match serde_json::from_str::<Vec<Slot>>(&voice) {
        Err(e) => envelope_error(&format!("the voice could not be read: {e}")),
        Ok(voice) => {
            let sheet = voice_to_sheet(&voice, &meter, &clef, &key);
            serde_json::json!({ "ok": sheet }).to_string()
        }
    };
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(json.as_bytes(), out, out_cap) }
}

/// Apply one operation to a score model, both crossing as JSON, and write the
/// result to `out` (capacity `out_cap`). Returns the byte count the result
/// needs, or `0` when either pointer is null.
///
/// **One symbol for every operation there will ever be.** The verb and its
/// parameters are inside `op` (`{"op": "transpose", "semitones": 2}`), so a new
/// operation is a new entry in `clausters_core_sheet_ops` and nothing here
/// changes — no row of `docs/bindings.md`, no `CORE_ABI_VERSION` round. What
/// that costs is that this table cannot see the verbs, which is exactly what
/// the catalog is contrasted against.
///
/// The result is an **envelope**, so a refusal keeps its reason: `{"ok": …}`
/// carries the new sheet, `{"error": "…"}` a sentence saying what was refused.
/// A refused operation changes nothing — the caller still holds the sheet it
/// sent, because the model crossed by value and was never handed over.
///
/// # Safety
/// Each pointer must be readable for its length, and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_sheet_apply(
    sheet: *const u8,
    sheet_len: usize,
    op: *const u8,
    op_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees both ranges.
    let (Some(sheet), Some(op)) = (unsafe { (text(sheet, sheet_len), text(op, op_len)) }) else {
        return 0;
    };
    let json = match (
        serde_json::from_str::<Sheet>(&sheet),
        serde_json::from_str::<Op>(&op),
    ) {
        (Err(e), _) => envelope_error(&format!("the sheet could not be read: {e}")),
        (_, Err(e)) => envelope_error(&format!("the operation could not be read: {e}")),
        (Ok(sheet), Ok(op)) => match apply(sheet, &op) {
            Ok(sheet) => serde_json::json!({ "ok": sheet }).to_string(),
            Err(e) => envelope_error(&e),
        },
    };
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(json.as_bytes(), out, out_cap) }
}

/// Write a score model out as MEI, in the same envelope
/// [`clausters_core_sheet_apply`] uses: `{"ok": "<mei…>"}` or `{"error": "…"}`.
///
/// The refusals are the ones the emitter owes a caller: a duration that is not
/// an exact note value (a tuplet), an accidental past a double, and the
/// polyphony the model can hold before this emitter can write it. Each says
/// which it is, because a caller reading "cannot" needs to know whether it is
/// wrong or early.
///
/// # Safety
/// `sheet` must be readable for `sheet_len` bytes and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_sheet_to_mei(
    sheet: *const u8,
    sheet_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees the range.
    let Some(sheet) = (unsafe { text(sheet, sheet_len) }) else {
        return 0;
    };
    let json = match serde_json::from_str::<Sheet>(&sheet) {
        Err(e) => envelope_error(&format!("the sheet could not be read: {e}")),
        Ok(sheet) => match sheet_to_mei(&sheet) {
            Ok(mei) => serde_json::json!({ "ok": mei }).to_string(),
            Err(e) => envelope_error(&e),
        },
    };
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(json.as_bytes(), out, out_cap) }
}

/// Every operation this core knows, as JSON — each entry naming the verb and
/// the parameters it takes.
///
/// **This is the parity surface the ABI cannot provide.** Operations cross as
/// data through one symbol, so a binding table proves nothing about which verbs
/// a client offers; each client is contrasted against this list instead, and a
/// verb that reaches only one of them fails a test there.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_sheet_ops(out: *mut u8, out_cap: usize) -> usize {
    let json = serde_json::to_vec(catalog()).unwrap_or_default();
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(&json, out, out_cap) }
}

/// Read an MEI document into the score model, written to `out` in the same
/// envelope the other sheet calls answer in. Returns the byte count it needs,
/// or `0` when `mei` is null.
///
/// **The other return path**, and not the one `clausters_core_sheet_perform` is:
/// that one turns a model into sound, this turns a *document* into a model. A
/// score opened from typed text — ABC, MusicXML, a hand-written MEI — is a
/// document and nothing else until this reads one, which is why every verb in
/// the algebra is unavailable on it until then.
///
/// There is one input format rather than four: the engraver normalizes whatever
/// it loaded to MEI (`clausters_score_mei`), so a caller reads that.
///
/// # Safety
/// `mei` must be readable for `mei_len` bytes and `out` writable for `out_cap`
/// bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_mei_to_sheet(
    mei: *const u8,
    mei_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees the range.
    let Some(mei) = (unsafe { text(mei, mei_len) }) else {
        return 0;
    };
    let json = match mei_to_sheet(&mei) {
        Ok(sheet) => serde_json::json!({ "ok": sheet }).to_string(),
        Err(e) => envelope_error(&e),
    };
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(json.as_bytes(), out, out_cap) }
}

/// Read a score model into the notes it **sounds**, under `interp` — the
/// reading, as JSON — and write them to `out` (capacity `out_cap`). Returns the
/// byte count the result needs, or `0` when `sheet` is null.
///
/// The answer is the same envelope the other sheet calls use: `{"ok": [ … ]}`
/// with one entry per sounding note, or `{"error": "…"}`.
///
/// Each note carries **two lengths** — `dur`, what is written, and `sustain`,
/// what is heard — because they are different numbers whenever an articulation
/// is honoured, and a caller that collapsed them into one would move every
/// attack after a staccato. It also carries the `staff` and `voice` it was
/// written on, which is what a caller binds an instrument to: the notation does
/// not say what plays it.
///
/// `interp` may be null or `{}` for the default reading; any field left out of
/// it keeps its default, so overriding one is a one-key object. What the
/// default *is* — every number in it — comes back from
/// [`clausters_core_interpretation`], so no client writes those numbers down.
///
/// # Safety
/// Each pointer must be readable for its length, and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_sheet_perform(
    sheet: *const u8,
    sheet_len: usize,
    interp: *const u8,
    interp_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees the ranges.
    let Some(sheet) = (unsafe { text(sheet, sheet_len) }) else {
        return 0;
    };
    // SAFETY: caller guarantees the range; a null pointer is the default.
    let interp = unsafe { text(interp, interp_len) };
    let read = match interp.as_deref().filter(|s| !s.trim().is_empty()) {
        None => Ok(default_interpretation()),
        Some(text) => serde_json::from_str::<Interpretation>(text),
    };
    let json = match (serde_json::from_str::<Sheet>(&sheet), read) {
        (Err(e), _) => envelope_error(&format!("the sheet could not be read: {e}")),
        (_, Err(e)) => envelope_error(&format!("the interpretation could not be read: {e}")),
        (Ok(sheet), Ok(interp)) => match perform(sheet, &interp) {
            Ok(notes) => serde_json::json!({ "ok": notes }).to_string(),
            Err(e) => envelope_error(&e),
        },
    };
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(json.as_bytes(), out, out_cap) }
}

/// The default interpretation, as JSON — every number the reading depends on.
///
/// **The parity surface for the reading**, and the value an override starts
/// from. The interpretation crosses inside a payload, so the binding table sees
/// one symbol and none of its fields; each client is contrasted against this,
/// as it is against `clausters_core_sheet_ops` for the verbs. It is also why no
/// client carries its own copy of the dynamics table: two clients with two
/// copies play the same score at two amplitudes, and nothing compares them.
///
/// # Safety
/// `out` must be null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_interpretation(out: *mut u8, out_cap: usize) -> usize {
    let json = serde_json::to_vec(&default_interpretation()).unwrap_or_default();
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(&json, out, out_cap) }
}

/// A refusal, in the envelope both sheet calls answer in.
fn envelope_error(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

// ---- the engraver and the editable score (feature `verovio`) ---------------

#[cfg(feature = "verovio")]
use clausters_notation::{EngraveOptions, Score};

/// The engraver's layout options: the scale/width pair, plus any `extra` verovio
/// options as a JSON object merged over the defaults the whole layer shares.
#[cfg(feature = "verovio")]
fn options(scale: i32, page_width: i32, extra: Option<String>) -> EngraveOptions {
    EngraveOptions {
        scale,
        page_width,
        extra,
        ..Default::default()
    }
}

/// Open `data` as an editable score, kept laid out so edits land on the same
/// document the display list was drawn from. Returns an opaque handle, or
/// null when `data` is null or could not be loaded. Free with
/// [`clausters_score_free`].
///
/// `options` is a JSON object of extra engraver options (`{"unit": 6}`) merged
/// over the defaults, or null for none — the seam a caller reaches the engraver's
/// own vocabulary through without this ABI growing a parameter per knob.
///
/// # Safety
/// `data` must be readable for `data_len` bytes and `options` for
/// `options_len` (or be null).
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_open(
    data: *const u8,
    data_len: usize,
    scale: i32,
    page_width: i32,
    options: *const u8,
    options_len: usize,
) -> *mut Score {
    // SAFETY: caller guarantees the ranges.
    let (Some(data), extra) = (unsafe { (text(data, data_len), text(options, options_len)) })
    else {
        return std::ptr::null_mut();
    };
    let opts = self::options(scale, page_width, extra.map(|e| e.into_owned()));
    match clausters_notation::open(&data, &opts) {
        Ok(score) => Box::into_raw(Box::new(score)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Frees a score opened by [`clausters_score_open`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_score_open`, not yet freed.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_free(h: *mut Score) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// This score engraved into one page as JSON — the drawing layers plus
/// `cursors` and `notes` — from the live document, so it reflects every edit
/// applied so far. Returns the byte count the JSON needs, or `0` on a null
/// handle.
///
/// # Safety
/// `h` must be a live score handle and `out` writable for `out_cap` bytes
/// (or null, to size only).
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_display_list(
    h: *mut Score,
    page: i32,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if h.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees a live handle.
    let score = unsafe { &mut *h };
    let json = serde_json::to_vec(&score.display_list(page)).unwrap_or_default();
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(&json, out, out_cap) }
}

/// The score as MEI, ids and all — the format to persist. Returns the byte
/// count it needs, or `0` on a null handle.
///
/// # Safety
/// `h` must be a live score handle and `out` writable for `out_cap` bytes
/// (or null, to size only).
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_mei(h: *mut Score, out: *mut u8, out_cap: usize) -> usize {
    if h.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees a live handle.
    let mei = unsafe { &*h }.mei();
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(mei.as_bytes(), out, out_cap) }
}

/// Move the note `element_id` by `steps` **diatonic** steps along the staff —
/// up when positive — as one undo step. Returns `1` when the edit was
/// applied, `0` when it was rejected (and rolled back) or the handle is null.
///
/// # Safety
/// `h` must be a live score handle and `element_id` readable for `id_len`
/// bytes.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_transpose(
    h: *mut Score,
    element_id: *const u8,
    id_len: usize,
    steps: i32,
) -> i32 {
    // SAFETY: caller guarantees the handle and the range.
    let (false, Some(id)) = (h.is_null(), unsafe { text(element_id, id_len) }) else {
        return 0;
    };
    // SAFETY: caller guarantees a live handle.
    unsafe { &mut *h }.transpose(&id, steps) as i32
}

/// Move a note **to** the diatonic staff position `position` on `page` — whole
/// steps from its staff's top line, positive upward — as one undo step.
///
/// The absolute form, and the one an edit travels in: applying it twice leaves
/// the note where it is, and a page re-engraved under the gesture needs no
/// rebasing. Returns `1` when the note is now at `position` (including when it
/// already was), `0` when the element is not on that page, the page has no
/// staff, verovio refused, or a pointer is null.
///
/// # Safety
/// `h` must be a live score handle and `element_id` readable for `id_len`
/// bytes.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_transpose_to(
    h: *mut Score,
    element_id: *const u8,
    id_len: usize,
    position: i32,
    page: i32,
) -> i32 {
    // SAFETY: caller guarantees the handle and the range.
    let (false, Some(id)) = (h.is_null(), unsafe { text(element_id, id_len) }) else {
        return 0;
    };
    // SAFETY: caller guarantees a live handle.
    unsafe { &mut *h }.transpose_to(&id, position, page) as i32
}

/// Apply one **model** operation to an open score as a single undo step, and
/// re-engrave. `op` is the operation as JSON, the same payload
/// [`clausters_core_sheet_apply`] takes. Returns `1` when it was applied, `0`
/// when the document has no model behind it, the operation was refused, or a
/// pointer is null.
///
/// **This is the edit path**, and the reason it is not the engraver's editor:
/// there is one implementation of what an edit to a score means, it is the
/// algebra every client already binds, and a standalone host holding a sheet
/// performs the same operation through the same code. A refused operation
/// leaves both the page and the model as they were.
///
/// # Safety
/// `h` must be a live score handle and `op` readable for `op_len` bytes.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_apply(h: *mut Score, op: *const u8, op_len: usize) -> i32 {
    if h.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees the range.
    let Some(op) = (unsafe { text(op, op_len) }) else {
        return 0;
    };
    let Ok(op) = serde_json::from_str::<Op>(&op) else {
        return 0;
    };
    // SAFETY: caller guarantees a live handle.
    unsafe { &mut *h }.apply(&op) as i32
}

/// The open score as the **model**, written to `out` in the usual envelope:
/// `{"ok": …}` with the sheet, or `{"error": "…"}` when the document could not
/// be read into one — which is a state and not a failure, since the page still
/// draws and still plays and only the model's verbs are unavailable.
///
/// # Safety
/// `h` must be a live score handle and `out` writable for `out_cap` bytes (or
/// null, to size only).
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_sheet(
    h: *mut Score,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if h.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees a live handle.
    let json = match unsafe { &*h }.sheet() {
        Some(sheet) => serde_json::json!({ "ok": sheet }).to_string(),
        None => envelope_error(
            "this document could not be read into the score model, so the model's \
             verbs are not available on it; the page still draws and still plays",
        ),
    };
    // SAFETY: caller guarantees `out` is writable for `out_cap` bytes.
    unsafe { fill(json.as_bytes(), out, out_cap) }
}

/// Apply one raw editor action (`set`, `insert`, `delete`, ...) as a single
/// undo step — the escape hatch for what [`clausters_score_transpose`] does
/// not cover. `param` is the action's parameter object as JSON. Returns `1`
/// when the edit was applied, `0` when it was rejected or a pointer is null.
///
/// # Safety
/// `h` must be a live score handle; `action` and `param` readable for their
/// lengths.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_edit(
    h: *mut Score,
    action: *const u8,
    action_len: usize,
    param: *const u8,
    param_len: usize,
) -> i32 {
    if h.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees the ranges.
    let (Some(action), param) = (unsafe { (text(action, action_len), text(param, param_len)) })
    else {
        return 0;
    };
    // SAFETY: caller guarantees a live handle.
    unsafe { &mut *h }.edit(&action, param.as_deref().unwrap_or("{}")) as i32
}

/// Step back one edit. Returns `1` on success, `0` when there is nothing to
/// undo (never a crash).
///
/// # Safety
/// `h` must be a live score handle.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_undo(h: *mut Score) -> i32 {
    if h.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees a live handle.
    unsafe { &mut *h }.undo() as i32
}

/// Step forward again after [`clausters_score_undo`]. Returns `1` on success,
/// `0` when there is nothing to redo.
///
/// # Safety
/// `h` must be a live score handle.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_redo(h: *mut Score) -> i32 {
    if h.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees a live handle.
    unsafe { &mut *h }.redo() as i32
}

/// Whether there is an edit to step back over (`1`/`0`).
///
/// # Safety
/// `h` must be a live score handle.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_can_undo(h: *mut Score) -> i32 {
    // SAFETY: caller guarantees a live handle.
    (!h.is_null() && unsafe { &*h }.can_undo()) as i32
}

/// Whether there is an undone edit to step forward into (`1`/`0`).
///
/// # Safety
/// `h` must be a live score handle.
#[cfg(feature = "verovio")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_score_can_redo(h: *mut Score) -> i32 {
    // SAFETY: caller guarantees a live handle.
    (!h.is_null() && unsafe { &*h }.can_redo()) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Call a size-then-fill entry point the way a binding does: size, then fill.
    fn round_trip(call: impl Fn(*mut u8, usize) -> usize) -> String {
        let n = call(std::ptr::null_mut(), 0);
        let mut buf = vec![0u8; n];
        assert_eq!(call(buf.as_mut_ptr(), buf.len()), n, "the size is stable");
        String::from_utf8(buf).expect("UTF-8")
    }

    fn mei_of(voice: &str) -> String {
        round_trip(|out, cap| unsafe {
            clausters_core_voice_to_mei(
                voice.as_ptr(),
                voice.len(),
                c"4/4".to_bytes().as_ptr(),
                3,
                c"G2".to_bytes().as_ptr(),
                2,
                c"C".to_bytes().as_ptr(),
                1,
                out,
                cap,
            )
        })
    }

    #[test]
    fn a_voice_crosses_as_json_and_comes_back_as_mei() {
        let mei = mei_of(r#"[{"midis": [60], "ticks": 8}, {"ticks": 24}]"#);
        assert!(mei.contains("pname=\"c\""), "the note");
        assert!(mei.contains("<rest"), "the rest");
        assert!(mei.contains("meter.count=\"4\""), "the meter reached it");
    }

    #[test]
    fn a_malformed_voice_sizes_to_zero() {
        let voice = "not json";
        let n = unsafe {
            clausters_core_voice_to_mei(
                voice.as_ptr(),
                voice.len(),
                c"4/4".to_bytes().as_ptr(),
                3,
                c"G2".to_bytes().as_ptr(),
                2,
                c"C".to_bytes().as_ptr(),
                1,
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(n, 0);
    }

    #[test]
    fn an_svg_crosses_as_bytes_and_comes_back_as_a_display_list() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><svg class="definition-scale"
            viewBox="0 0 1000 500"><g class="note" id="n1"><path d="M0 0 L100 0"/></g></svg></svg>"#;
        let json = round_trip(|out, cap| unsafe {
            clausters_core_svg_to_display_list(svg.as_ptr(), svg.len(), out, cap)
        });
        let dl: serde_json::Value = serde_json::from_str(&json).expect("JSON");
        assert_eq!(dl["vb"], serde_json::json!([1000.0, 500.0]));
        assert_eq!(dl["prims"][0]["id"], "n1");
    }

    #[test]
    fn a_null_svg_sizes_to_zero() {
        let n = unsafe {
            clausters_core_svg_to_display_list(std::ptr::null(), 0, std::ptr::null_mut(), 0)
        };
        assert_eq!(n, 0);
    }

    #[cfg(feature = "verovio")]
    mod native {
        use super::super::*;
        use super::round_trip;

        /// A one-bar C major triad, written by the core's own encoder.
        fn mei() -> String {
            clausters_core::notation::voice_to_mei(
                &[clausters_core::notation::Slot::Note {
                    midis: vec![60, 64, 67],
                    ticks: 32,
                }],
                "4/4",
                "G2",
                "C",
            )
        }

        #[test]
        fn a_handle_edits_undoes_and_frees() {
            let data = mei();
            let h = unsafe {
                clausters_score_open(data.as_ptr(), data.len(), 40, 2100, std::ptr::null(), 0)
            };
            assert!(!h.is_null(), "the score opened");

            let json =
                round_trip(|out, cap| unsafe { clausters_score_display_list(h, 1, out, cap) });
            let page: serde_json::Value = serde_json::from_str(&json).expect("JSON");
            let id = page["notes"][0]["id"].as_str().expect("an id").to_owned();

            assert_eq!(unsafe { clausters_score_can_undo(h) }, 0);
            assert_eq!(
                unsafe { clausters_score_transpose(h, id.as_ptr(), id.len(), 1) },
                1
            );
            assert_eq!(unsafe { clausters_score_can_undo(h) }, 1);

            let mei = round_trip(|out, cap| unsafe { clausters_score_mei(h, out, cap) });
            assert!(mei.contains("<music"), "MEI came back");

            assert_eq!(unsafe { clausters_score_undo(h) }, 1);
            assert_eq!(unsafe { clausters_score_can_redo(h) }, 1);
            assert_eq!(unsafe { clausters_score_redo(h) }, 1);
            unsafe { clausters_score_free(h) };
        }

        #[test]
        fn the_raw_edit_hatch_crosses() {
            let data = mei();
            let h = unsafe {
                clausters_score_open(data.as_ptr(), data.len(), 40, 2100, std::ptr::null(), 0)
            };
            let json =
                round_trip(|out, cap| unsafe { clausters_score_display_list(h, 1, out, cap) });
            let page: serde_json::Value = serde_json::from_str(&json).expect("JSON");
            let id = page["notes"][0]["id"].as_str().expect("an id").to_owned();
            let param = format!(r#"{{"elementId": "{id}", "key": 38}}"#);
            assert_eq!(
                unsafe {
                    clausters_score_edit(
                        h,
                        c"keyDown".to_bytes().as_ptr(),
                        7,
                        param.as_ptr(),
                        param.len(),
                    )
                },
                1
            );
            unsafe { clausters_score_free(h) };
        }

        #[test]
        fn a_null_handle_is_never_a_crash() {
            let h = std::ptr::null_mut();
            assert_eq!(
                unsafe { clausters_score_display_list(h, 1, std::ptr::null_mut(), 0) },
                0
            );
            assert_eq!(
                unsafe { clausters_score_mei(h, std::ptr::null_mut(), 0) },
                0
            );
            assert_eq!(
                unsafe { clausters_score_transpose(h, c"x".to_bytes().as_ptr(), 1, 1) },
                0
            );
            assert_eq!(unsafe { clausters_score_undo(h) }, 0);
            assert_eq!(unsafe { clausters_score_redo(h) }, 0);
            assert_eq!(unsafe { clausters_score_can_undo(h) }, 0);
            assert_eq!(unsafe { clausters_score_can_redo(h) }, 0);
            unsafe { clausters_score_free(h) };
        }

        #[test]
        fn unloadable_data_opens_to_a_null_handle() {
            let data = "this is not a score";
            let h = unsafe {
                clausters_score_open(data.as_ptr(), data.len(), 40, 2100, std::ptr::null(), 0)
            };
            assert!(h.is_null());
        }
    }
}
