//! An event's keys.
//!
//! `clausters_core::event`, with each family crossing as the C ABI carries it:
//! `[freq, midinote, degree, alter, octave, root]` and `[amp, velocity, db]`,
//! NaN for a key the event does not hold, a key named by its index (`6` the
//! scale). An edit returns the rewritten family rather than writing in place.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: scale degree -> MIDI note number in the pitch space
/// `octave`/`root`, with floored octave wrapping (sclang semantics), `alter`
/// semitones added and a fractional degree read as an alteration. An empty
/// `scale` yields middle C.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn degree_to_midinote(degree: f64, alter: f64, octave: f64, root: f64, scale: &[f32]) -> f64 {
    clausters_core::event::degree_to_midinote(degree, alter, octave, root, scale)
}

/// JS face: MIDI note -> `[degree, alter]`; `spelling` below zero is flat.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn midinote_to_degree(
    midinote: f64,
    octave: f64,
    root: f64,
    scale: &[f32],
    spelling: i32,
) -> Vec<f64> {
    let (degree, alter) = clausters_core::event::midinote_to_degree(
        midinote,
        octave,
        root,
        scale,
        clausters_core::event::Spelling::from_i32(spelling),
    );
    vec![degree, alter]
}

/// JS face: a SuperCollider degree as `[degree, alter]`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn split_degree(degree: f64) -> Vec<f64> {
    let (whole, alter) = clausters_core::event::split_degree(degree);
    vec![whole, alter]
}

#[cfg(target_arch = "wasm32")]
fn keys<const N: usize>(values: &[f64]) -> [f64; N] {
    let mut out = [f64::NAN; N];
    for (slot, v) in out.iter_mut().zip(values) {
        *slot = *v;
    }
    out
}

/// JS face: `[midinote, freq]` the pitch keys sound.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn pitch_resolve(pitch: &[f64], scale: &[f32]) -> Vec<f64> {
    let pitch = clausters_core::event::Pitch::from_array(keys(pitch));
    vec![pitch.midinote(scale), pitch.freq(scale)]
}

/// JS face: the pitch keys with key `key` (its index) written to `value` and
/// the others the event holds rewritten to the same note.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn pitch_set(
    pitch: &[f64],
    key: u32,
    value: f64,
    scale: &[f32],
    spelling: i32,
) -> Result<Vec<f64>, JsError> {
    let key = clausters_core::event::PitchKey::from_index(key)
        .ok_or_else(|| JsError::new(&format!("no pitch key {key}")))?;
    let mut p = clausters_core::event::Pitch::from_array(keys(pitch));
    p.set(
        key,
        value,
        scale,
        clausters_core::event::Spelling::from_i32(spelling),
    );
    Ok(p.to_array().to_vec())
}

/// JS face: `[amp, velocity]` the level keys sound at.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn level_resolve(level: &[f64]) -> Vec<f64> {
    let level = clausters_core::event::Level::from_array(keys(level));
    vec![level.amp(), level.velocity()]
}

/// JS face: the level keys with key `key` (its index) written to `value` and
/// the others the event holds rewritten.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn level_set(level: &[f64], key: u32, value: f64) -> Result<Vec<f64>, JsError> {
    let key = clausters_core::event::LevelKey::from_index(key)
        .ok_or_else(|| JsError::new(&format!("no level key {key}")))?;
    let mut l = clausters_core::event::Level::from_array(keys(level));
    l.set(key, value);
    Ok(l.to_array().to_vec())
}

/// JS face: a MIDI velocity as its linear amplitude.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn amp_of_velocity(velocity: f64) -> f64 {
    clausters_core::event::amp_of_velocity(velocity)
}

/// JS face: a linear amplitude as a note-on's velocity (1..127).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn velocity_of_amp(amp: f64) -> f64 {
    clausters_core::event::velocity_of_amp(amp)
}

/// JS face: beats to the next event (`delta` NaN when not stated).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_delta(dur: f64, stretch: f64, delta: f64) -> f64 {
    clausters_core::event::delta(dur, stretch, (!delta.is_nan()).then_some(delta))
}

/// JS face: beats the event sounds (`sustain` NaN when not stated).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_sustain(dur: f64, legato: f64, stretch: f64, sustain: f64) -> f64 {
    clausters_core::event::sustain(dur, legato, stretch, (!sustain.is_nan()).then_some(sustain))
}

/// JS face: a note event as its synth's messages, `{"start", "release",
/// "sustain"}` with `[tag, value]` arguments, or `{"error"}` for anything that
/// is not a note. `event` is the event's keys as JSON, in the reference
/// client's spelling.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_synth(event: &str, node: i32) -> String {
    let keys = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(event)
        .unwrap_or_default();
    match clausters_core::event::render::synth(&keys, node) {
        Some(s) => s.to_json().to_string(),
        None => serde_json::json!({"error": "only a note renders a synth"}).to_string(),
    }
}

/// JS face: a note event whose instrument is a node as the `/node_event`
/// messages of note `note` on it, `{"start", "release", "sustain"}` with
/// `[tag, value]` arguments, or `{"error"}` for anything else.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_node_note(event: &str, note: i32) -> String {
    let keys = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(event)
        .unwrap_or_default();
    match clausters_core::event::render::node_note(&keys, note) {
        Some(s) => s.to_json().to_string(),
        None => {
            serde_json::json!({"error": "only a note on a node renders a node note"}).to_string()
        }
    }
}

/// JS face: an event as its MIDI messages, `{"messages": [[at, [bytes]]]}` or
/// `{"error"}`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_midi(event: &str, channel: u8) -> String {
    let keys = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(event)
        .unwrap_or_default();
    match clausters_core::event::render::midi(&keys, channel) {
        Ok(messages) => serde_json::json!({
            "messages": messages
                .iter()
                .map(|m| serde_json::json!([m.at, m.bytes]))
                .collect::<Vec<_>>()
        })
        .to_string(),
        Err(error) => serde_json::json!({ "error": error }).to_string(),
    }
}

/// JS face: MIDI bytes as the event that plays them back, as JSON.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_of_midi(bytes: &[u8]) -> String {
    serde_json::Value::Object(clausters_core::event::render::from_midi(bytes)).to_string()
}

/// JS face: the notation keys, as JSON -- the table of what an event says
/// about the page it is written on (`clausters_core::event::notation::KEYS`).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_notation_keys() -> String {
    clausters_core::event::notation::keys_json()
}

/// JS face: the MIDI note a `pitches` value sounds -- the notation key's JSON
/// -- or NaN when it is no list holding one.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn event_written_midinote(pitches: &str) -> f64 {
    serde_json::from_str::<serde_json::Value>(pitches)
        .ok()
        .and_then(|value| clausters_core::event::notation::written_midinote(&value))
        .unwrap_or(f64::NAN)
}
