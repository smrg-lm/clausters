//! What an event's keys mean -- `clausters_core::event` over the C ABI.
//!
//! A family crosses as a fixed array of `f64`, NaN for a key the event does
//! not hold: the pitch keys as `[freq, midinote, degree, alter, octave, root]`
//! with the scale beside them, the level keys as `[amp, velocity, db]`. A key
//! is named by its index in that order, `6` being the scale.

use clausters_core::event::{
    self, Level, LevelKey, Pitch, PitchKey, Spelling, amp_of_velocity, velocity_of_amp,
};

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
}
