//! What an event's keys mean -- `clausters_core::event` over the C ABI.
//!
//! A family crosses as a fixed array of `f64`, NaN for a key the event does
//! not hold: the pitch keys as `[freq, midinote, degree, alter, octave, root]`
//! with the scale beside them, the level keys as `[amp, velocity, db]`. A key
//! is named by its index in that order, `6` being the scale.
//!
//! Rendering an event crosses as JSON, size-then-fill: the event is its map of
//! keys, and the answer is the messages its destination plays, or
//! `{"error": ...}`.

use clausters_core::event::{
    self, Level, LevelKey, Pitch, PitchKey, Spelling, amp_of_velocity, render, velocity_of_amp,
};
use serde_json::{Map, Value, json};

use crate::out::{fill, text};

/// The scale a door was handed, empty when null.
///
/// # Safety
/// `scale` must be readable for `n` `f32`s, or null.
unsafe fn scale<'a>(scale: *const f32, n: usize) -> &'a [f32] {
    if scale.is_null() || n == 0 {
        return &[];
    }
    // SAFETY: the caller's guarantee.
    unsafe { std::slice::from_raw_parts(scale, n) }
}

/// Scale degree -> MIDI note number in the pitch space `octave` / `root`, with
/// floored octave wrapping (sclang semantics), `alter` semitones added and a
/// fractional degree read as SuperCollider writes an alteration. `scale` is `n`
/// semitone offsets; `n == 0` (or a null `scale`) yields middle C.
///
/// # Safety
/// `scale` must be readable for `n` `f32`s (or null with `n == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_degree_to_midinote(
    degree: f64,
    alter: f64,
    octave: f64,
    root: f64,
    scale_ptr: *const f32,
    n: usize,
) -> f64 {
    // SAFETY: the caller's guarantee.
    let s = unsafe { scale(scale_ptr, n) };
    event::degree_to_midinote(degree, alter, octave, root, s)
}

/// MIDI note number -> the degree and alteration that write it, into `out[0]`
/// and `out[1]`. `spelling` below zero is flat, anything else sharp. Returns 0,
/// or -2 on a null `out`.
///
/// # Safety
/// `scale` must be readable for `n` `f32`s (or null with `n == 0`), `out`
/// writable for 2.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_midinote_to_degree(
    midinote: f64,
    octave: f64,
    root: f64,
    scale_ptr: *const f32,
    n: usize,
    spelling: i32,
    out: *mut f64,
) -> i32 {
    if out.is_null() {
        return -2;
    }
    // SAFETY: the caller's guarantee.
    let s = unsafe { scale(scale_ptr, n) };
    let (degree, alter) =
        event::midinote_to_degree(midinote, octave, root, s, Spelling::from_i32(spelling));
    // SAFETY: `out` is writable for 2.
    unsafe {
        *out = degree;
        *out.add(1) = alter;
    }
    0
}

/// A SuperCollider degree split into `out[0]` (the degree) and `out[1]` (the
/// alteration its fraction states). Returns 0, or -2 on a null `out`.
///
/// # Safety
/// `out` must be writable for 2.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_split_degree(degree: f64, out: *mut f64) -> i32 {
    if out.is_null() {
        return -2;
    }
    let (whole, alter) = event::split_degree(degree);
    // SAFETY: `out` is writable for 2.
    unsafe {
        *out = whole;
        *out.add(1) = alter;
    }
    0
}

/// The note and the frequency the pitch keys sound, into `out[0]` and
/// `out[1]`. Returns 0, or -2 on a null pointer.
///
/// # Safety
/// `keys` must be readable for 6 `f64`s, `scale` for `n` `f32`s (or null with
/// `n == 0`), `out` writable for 2.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_pitch_resolve(
    keys: *const f64,
    scale_ptr: *const f32,
    n: usize,
    out: *mut f64,
) -> i32 {
    if keys.is_null() || out.is_null() {
        return -2;
    }
    // SAFETY: the caller's guarantee.
    let (pitch, s) = unsafe {
        (
            Pitch::from_array(*keys.cast::<[f64; 6]>()),
            scale(scale_ptr, n),
        )
    };
    // SAFETY: `out` is writable for 2.
    unsafe {
        *out = pitch.midinote(s);
        *out.add(1) = pitch.freq(s);
    }
    0
}

/// Writes pitch key `key` (its index; `6` is the scale, whose new value is
/// `scale`) to `value` in `keys`, rewriting the other keys the event holds so
/// they say the same note. `spelling` below zero is flat. Returns 0, -1 on an
/// unknown key, -2 on a null `keys`.
///
/// # Safety
/// `keys` must be readable and writable for 6 `f64`s, `scale` readable for `n`
/// `f32`s (or null with `n == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_pitch_set(
    keys: *mut f64,
    key: u32,
    value: f64,
    scale_ptr: *const f32,
    n: usize,
    spelling: i32,
) -> i32 {
    let Some(key) = PitchKey::from_index(key) else {
        return -1;
    };
    if keys.is_null() {
        return -2;
    }
    // SAFETY: the caller's guarantee.
    let (array, s) = unsafe { (&mut *keys.cast::<[f64; 6]>(), scale(scale_ptr, n)) };
    let mut pitch = Pitch::from_array(*array);
    pitch.set(key, value, s, Spelling::from_i32(spelling));
    *array = pitch.to_array();
    0
}

/// The amplitude and the velocity the level keys sound at, into `out[0]` and
/// `out[1]`. Returns 0, or -2 on a null pointer.
///
/// # Safety
/// `keys` must be readable for 3 `f64`s and `out` writable for 2.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_level_resolve(keys: *const f64, out: *mut f64) -> i32 {
    if keys.is_null() || out.is_null() {
        return -2;
    }
    // SAFETY: the caller's guarantee.
    let level = Level::from_array(unsafe { *keys.cast::<[f64; 3]>() });
    // SAFETY: `out` is writable for 2.
    unsafe {
        *out = level.amp();
        *out.add(1) = level.velocity();
    }
    0
}

/// Writes level key `key` (its index) to `value` in `keys`, rewriting the other
/// keys the event holds. Returns 0, -1 on an unknown key, -2 on a null `keys`.
///
/// # Safety
/// `keys` must be readable and writable for 3 `f64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_level_set(keys: *mut f64, key: u32, value: f64) -> i32 {
    let Some(key) = LevelKey::from_index(key) else {
        return -1;
    };
    if keys.is_null() {
        return -2;
    }
    // SAFETY: the caller's guarantee.
    let array = unsafe { &mut *keys.cast::<[f64; 3]>() };
    let mut level = Level::from_array(*array);
    level.set(key, value);
    *array = level.to_array();
    0
}

/// A MIDI velocity as the linear amplitude that goes with it.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_amp_of_velocity(velocity: f64) -> f64 {
    amp_of_velocity(velocity)
}

/// A linear amplitude as the velocity a note-on carries (1..127).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_velocity_of_amp(amp: f64) -> f64 {
    velocity_of_amp(amp)
}

/// Beats until the next event: `delta` when it is not NaN, else
/// `dur * stretch`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_event_delta(dur: f64, stretch: f64, delta: f64) -> f64 {
    event::delta(dur, stretch, (!delta.is_nan()).then_some(delta))
}

/// Beats the event sounds: `sustain` when it is not NaN, else
/// `dur * legato * stretch`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_event_sustain(
    dur: f64,
    legato: f64,
    stretch: f64,
    sustain: f64,
) -> f64 {
    event::sustain(dur, legato, stretch, (!sustain.is_nan()).then_some(sustain))
}

/// Reads a JSON request, answers with `answer`'s JSON, size-then-fill. A
/// request that does not read answers `{"error": ...}` too.
///
/// # Safety
/// `request` must be null or readable for `len` bytes, `out` null or writable
/// for `out_cap`.
unsafe fn json_door(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
    answer: impl FnOnce(&Map<String, Value>) -> Value,
) -> usize {
    // SAFETY: forwarded from the caller.
    let parsed = unsafe { text(request, len) }
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned());
    let reply = match parsed {
        Some(req) => answer(&req),
        None => json!({"error": "the request is not a JSON object"}),
    };
    // SAFETY: forwarded from the caller.
    unsafe { fill(reply.to_string().as_bytes(), out, out_cap) }
}

fn event_of(req: &Map<String, Value>) -> Map<String, Value> {
    req.get("event")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// A note event as the messages of its synth: `{"event": {...}, "node": n}`
/// in, `{"start": [..], "release": [..], "sustain": beats}` out, each message a
/// list of `[tag, value]` arguments with the address first
/// (`clausters_core::event::render::synth`).
///
/// # Safety
/// `request` must be readable for `len` bytes, `out` null or writable for
/// `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_event_synth(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from the caller.
    unsafe {
        json_door(request, len, out, out_cap, |req| {
            let node = req.get("node").and_then(Value::as_i64).unwrap_or(0) as i32;
            match render::synth(&event_of(req), node) {
                Some(s) => s.to_json(),
                None => json!({"error": "only a note renders a synth"}),
            }
        })
    }
}

/// An event as the MIDI messages it plays: `{"event": {...}, "channel": c}` in,
/// `{"messages": [[at, [bytes]], ...]}` out (`render::midi`).
///
/// # Safety
/// `request` must be readable for `len` bytes, `out` null or writable for
/// `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_event_midi(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from the caller.
    unsafe { json_door(request, len, out, out_cap, midi_answer) }
}

fn midi_answer(req: &Map<String, Value>) -> Value {
    let channel = req.get("channel").and_then(Value::as_u64).unwrap_or(0) as u8;
    match render::midi(&event_of(req), channel) {
        Ok(messages) => json!({
            "messages": messages.iter().map(|m| json!([m.at, m.bytes])).collect::<Vec<_>>()
        }),
        Err(error) => json!({ "error": error }),
    }
}

/// MIDI bytes as the event that plays them back: `{"bytes": [..]}` in, the
/// event's keys out (`render::from_midi`).
///
/// # Safety
/// `request` must be readable for `len` bytes, `out` null or writable for
/// `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_event_of_midi(
    request: *const u8,
    len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from the caller.
    unsafe {
        json_door(request, len, out, out_cap, |req| {
            let bytes: Vec<u8> = req
                .get("bytes")
                .and_then(Value::as_array)
                .map(|b| {
                    b.iter()
                        .filter_map(Value::as_u64)
                        .map(|v| v as u8)
                        .collect()
                })
                .unwrap_or_default();
            Value::Object(render::from_midi(&bytes))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pitch_edit_crosses_and_comes_back_coherent() {
        let major = event::MAJOR;
        let mut keys = [440.0, 69.0, f64::NAN, f64::NAN, 5.0, 0.0];
        let rc =
            unsafe { clausters_core_pitch_set(keys.as_mut_ptr(), 1, 72.0, major.as_ptr(), 7, 0) };
        assert_eq!(rc, 0);
        assert!((keys[0] - 523.251_130_601_197_3).abs() < 1e-9);
        let mut out = [0.0; 2];
        unsafe { clausters_core_pitch_resolve(keys.as_ptr(), major.as_ptr(), 7, out.as_mut_ptr()) };
        assert!((out[0] - 72.0).abs() < 1e-9);
        assert_eq!(
            unsafe { clausters_core_pitch_set(keys.as_mut_ptr(), 9, 0.0, major.as_ptr(), 7, 0) },
            -1
        );
    }

    #[test]
    fn a_level_edit_crosses_and_comes_back_coherent() {
        let mut keys = [0.1, f64::NAN, f64::NAN];
        unsafe { clausters_core_level_set(keys.as_mut_ptr(), 1, 100.0) };
        assert!((keys[0] - 100.0 / 127.0).abs() < 1e-12);
        assert_eq!(keys[1], 100.0);
        assert!(keys[2].is_nan());
    }

    #[test]
    fn the_degree_doors() {
        let major = event::MAJOR;
        let mut out = [0.0; 2];
        unsafe {
            clausters_core_midinote_to_degree(
                61.0,
                5.0,
                0.0,
                major.as_ptr(),
                7,
                -1,
                out.as_mut_ptr(),
            )
        };
        assert_eq!(out, [1.0, -1.0]);
        unsafe { clausters_core_split_degree(1.1, out.as_mut_ptr()) };
        assert_eq!(out, [1.0, 1.0]);
        assert_eq!(clausters_core_event_sustain(1.0, 0.8, 1.0, f64::NAN), 0.8);
        assert_eq!(clausters_core_event_delta(1.0, 2.0, 0.25), 0.25);
    }

    fn call(
        door: unsafe extern "C" fn(*const u8, usize, *mut u8, usize) -> usize,
        req: &str,
    ) -> Value {
        let need = unsafe { door(req.as_ptr(), req.len(), std::ptr::null_mut(), 0) };
        let mut buf = vec![0u8; need];
        unsafe { door(req.as_ptr(), req.len(), buf.as_mut_ptr(), need) };
        serde_json::from_slice(&buf).unwrap()
    }

    #[test]
    fn the_render_doors() {
        let s = call(
            clausters_core_event_synth,
            r#"{"event":{"midinote":69,"amp":0.5},"node":7}"#,
        );
        assert_eq!(s["start"][0], json!(["s", "/synth_new"]));
        assert_eq!(s["start"][2], json!(["i", 7]));
        assert_eq!(s["start"][6], json!(["f", 440.0]));
        let m = call(
            clausters_core_event_midi,
            r#"{"event":{"midinote":60},"channel":2}"#,
        );
        assert_eq!(m["messages"][0], json!([0.0, [0x92, 60, 13]]));
        let e = call(clausters_core_event_of_midi, r#"{"bytes":[176,7,99]}"#);
        assert_eq!(e["midicmd"], "cc");
        let bad = call(clausters_core_event_midi, r#"{"event":{"type":"osc"}}"#);
        assert!(bad["error"].is_string());
        assert!(call(clausters_core_event_synth, "nope")["error"].is_string());
    }
}
