//! C ABI over [`clausters_core`] — the language-agnostic surface for client
//! bindings.
//!
//! Same contract as the server's embed ABI (`clausters::embed`): only flat
//! data crosses — `f32`/`f64`/integers and pointer+length arrays, never a
//! library type. A thin per-language wrapper (Python `ctypes` now, JS N-API or
//! wasm later) sits on top. Check [`clausters_core_abi_version`] first.
//!
//! Scope: the numeric builtins, the seeded RNG and the timing/sample-conversion
//! scalars, plus a **WebSocket client transport** (`clausters_ws_*`, in
//! [`ws`]) — the carrier a browser-less binding uses to reach a `--ws` server,
//! sharing the server's WebSocket implementation (`tungstenite`) instead of
//! re-implementing the framing per language. OSC bundle assembly stays in
//! `clausters_core::osc` (Rust-tested).

use clausters_core::builtins::{self, BinaryOp, UnaryOp};
use clausters_core::clocksync::SampleClockModel;
use clausters_core::measure;
use std::sync::Mutex;

use clausters_core::peaks::{self, MultiPyramid, Pyramid};
use clausters_core::registry::{self, NodeIdPartition, Registry, ReleaseError};
use clausters_core::rng::{Rng, WhiteNoise};
use clausters_core::scale;
use clausters_core::tempoclock::{self, Scheduler};
use clausters_core::window::Window;

mod ws;

/// The C ABI version of this surface. Bump on any incompatible change. v2 added
/// the `clausters_ws_*` WebSocket client transport; v3 the `clausters_core_peaks_*`
/// peak-pyramid cache builder; v4 the `clausters_core_window` smoothing windows
/// (shared with the server's FFT chain for bit-identical analysis); v5 the seam
/// audit pass — the `clausters_sched_*` beat queue, the `clausters_clocksync_*`
/// sample-clock model, the `clausters_rng_*` value stream, NTP timetag packing,
/// `quant_delay` and `degree_to_midinote` — so no value/time logic remains
/// per-language; v6 `clausters_rng_next_u64` (child-stream seed derivation for
/// the per-routine random context); v7 the `clausters_core_correlation` /
/// `clausters_core_lissajous` stereo-field measurements (shared with the GUI
/// phasescope so a headless client reads the identical numbers); v8 the
/// `clausters_core_peaks_multi_*` multichannel peak-pyramid cache (one cache
/// resource per buffer, all channels — the editor-grade waveform's format);
/// v9 the ruler/axis scalars — `clausters_core_hz_to_mel`/`_mel_to_hz`/
/// `_hz_to_bark`/`_bark_to_hz` (perceptual frequency scales, shared with the
/// GUI spectrogram axis) and `clausters_core_bar`/`_beat_in_bar` (the bar:beat
/// read of a quant grid, the display complement of `quant_delay`); v10 the
/// `clausters_registry_*` finite-resource id registry (node ids, buses,
/// buffers — every client's allocator and the server's reserved ranges share
/// the one occupancy-map model, internally locked per handle); v11 the
/// `clausters_core_patch_compile` cord→bus pass (a directed patch JSON in, its
/// GraphDef wiring JSON out — the GUI patcher's translation, shared so every
/// client compiles a patch identically).
pub const CORE_ABI_VERSION: u32 = 11;

/// Returns [`CORE_ABI_VERSION`]; call before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_abi_version() -> u32 {
    CORE_ABI_VERSION
}

/// Applies unary `op` to `input` (broadcast if `in_len == 1`) into `out`,
/// writing `n` samples. Returns 0 on success, -1 on an unknown op, -2 on a
/// null pointer or a non-broadcast length mismatch.
///
/// # Safety
/// `input` must be readable for `in_len` `f32`s and `out` writable for `n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_unary(
    op: u32,
    input: *const f32,
    in_len: usize,
    out: *mut f32,
    n: usize,
) -> i32 {
    let Some(op) = UnaryOp::from_u32(op) else {
        return -1;
    };
    if input.is_null() || out.is_null() || (in_len != 1 && in_len != n) {
        return -2;
    }
    // SAFETY: caller guarantees the ranges above.
    let a = unsafe { std::slice::from_raw_parts(input, in_len) };
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    builtins::unary_slice(op, a, o);
    0
}

/// Applies binary `op` to `a` and `b` (each broadcast if its length is 1) into
/// `out`, writing `n` samples. Same return codes as
/// [`clausters_core_unary`].
///
/// # Safety
/// `a`/`b` must be readable for `a_len`/`b_len` `f32`s, `out` writable for `n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_binary(
    op: u32,
    a: *const f32,
    a_len: usize,
    b: *const f32,
    b_len: usize,
    out: *mut f32,
    n: usize,
) -> i32 {
    let Some(op) = BinaryOp::from_u32(op) else {
        return -1;
    };
    let ok_len = |l: usize| l == 1 || l == n;
    if a.is_null() || b.is_null() || out.is_null() || !ok_len(a_len) || !ok_len(b_len) {
        return -2;
    }
    // SAFETY: caller guarantees the ranges above.
    let av = unsafe { std::slice::from_raw_parts(a, a_len) };
    let bv = unsafe { std::slice::from_raw_parts(b, b_len) };
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    builtins::binary_slice(op, av, bv, o);
    0
}

/// Fills `out` with `n` white-noise samples from `seed`, identical to the
/// server's `WhiteNoise` UGen seeded the same way.
///
/// # Safety
/// `out` must be writable for `n` `f32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_whitenoise(seed: u64, out: *mut f32, n: usize) {
    if out.is_null() {
        return;
    }
    // SAFETY: caller guarantees `out` is writable for `n`.
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    WhiteNoise::from_seed(seed).fill(o);
}

/// Fills `out` (`n` samples) with the smoothing window of type `wintype` — the
/// **same** `clausters_core::window::Window` the server's `FFT`/`IFFT` UGens
/// apply, so a client that pre-windows audio matches the server bit for bit.
/// `wintype`: -1 rectangular, 0 Hann, 1 sine, 2 Welch, 3 Hamming, 4 Blackman
/// (any other value falls back to Hann, as the server does).
///
/// # Safety
/// `out` must be writable for `n` `f32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_window(wintype: i32, out: *mut f32, n: usize) {
    if out.is_null() {
        return;
    }
    // SAFETY: caller guarantees `out` is writable for `n`.
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    Window::from_wintype(wintype).fill(o);
}

/// The exact byte length of the peak-pyramid cache for `n` samples at
/// `base_bucket` — call it to size the buffer for [`clausters_core_peaks_build`]
/// without building the pyramid. Returns 0 if `base_bucket == 0`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_peaks_cache_size(n: usize, base_bucket: usize) -> usize {
    if base_bucket == 0 {
        return 0;
    }
    peaks::cache_size(n, base_bucket)
}

/// Builds a min/max peak pyramid from `samples` (mono, `n` `f32`s) at
/// `base_bucket` and writes its cache bytes — the memory-mappable format the GUI
/// host maps to render a waveform without re-sending the samples — into `out`
/// (capacity `out_cap`). Returns the number of bytes written, or 0 on a null
/// pointer, `base_bucket == 0`, or `out_cap` below
/// [`clausters_core_peaks_cache_size`]`(n, base_bucket)`.
///
/// # Safety
/// `samples` must be readable for `n` `f32`s and `out` writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_peaks_build(
    samples: *const f32,
    n: usize,
    base_bucket: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if samples.is_null() || out.is_null() || base_bucket == 0 {
        return 0;
    }
    // SAFETY: caller guarantees `samples` is readable for `n` `f32`s.
    let s = unsafe { std::slice::from_raw_parts(samples, n) };
    let cache = Pyramid::build(s, base_bucket).to_bytes();
    if cache.len() > out_cap {
        return 0;
    }
    // SAFETY: out is writable for out_cap >= cache.len().
    let o = unsafe { std::slice::from_raw_parts_mut(out, cache.len()) };
    o.copy_from_slice(&cache);
    cache.len()
}

/// The exact byte length of the **multichannel** peak-pyramid cache for
/// `frames` samples per channel across `channels` channels at `base_bucket` —
/// sizes the buffer for [`clausters_core_peaks_multi_build`] without building.
/// Returns 0 if `base_bucket == 0`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_peaks_multi_cache_size(
    frames: usize,
    channels: usize,
    base_bucket: usize,
) -> usize {
    if base_bucket == 0 {
        return 0;
    }
    peaks::multi_cache_size(frames, channels, base_bucket)
}

/// Builds the multichannel peak-pyramid cache from `samples` (`n` `f32`s
/// holding `channels` interleaved channels; a trailing partial frame is
/// ignored) at `base_bucket`, writing the version-2 cache bytes — the single
/// mappable resource an editor-grade waveform names as its `cache` — into
/// `out` (capacity `out_cap`). Returns the bytes written, or 0 on a null
/// pointer, `base_bucket == 0` / `channels == 0`, or a too-small `out_cap`.
///
/// # Safety
/// `samples` must be readable for `n` `f32`s and `out` writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_peaks_multi_build(
    samples: *const f32,
    n: usize,
    channels: usize,
    base_bucket: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if samples.is_null() || out.is_null() || base_bucket == 0 || channels == 0 {
        return 0;
    }
    // SAFETY: caller guarantees `samples` is readable for `n` `f32`s.
    let s = unsafe { std::slice::from_raw_parts(samples, n) };
    let cache = MultiPyramid::build_interleaved(s, channels, base_bucket).to_bytes();
    if cache.len() > out_cap {
        return 0;
    }
    // SAFETY: out is writable for out_cap >= cache.len().
    let o = unsafe { std::slice::from_raw_parts_mut(out, cache.len()) };
    o.copy_from_slice(&cache);
    cache.len()
}

/// The stereo **correlation** (Pearson's r) of channels `left` and `right`
/// (each `n` `f32`s): `+1` mono/in-phase, `0` decorrelated, `-1` anti-phase —
/// the same measurement the GUI phasescope shows. Writes the coefficient into
/// `*out` and returns 0; returns -1 (leaving `*out` untouched) when it is
/// undefined (`n == 0` or a constant channel — silence/DC) or on a null pointer.
///
/// # Safety
/// `left`/`right` must be readable for `n` `f32`s and `out` writable for one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_correlation(
    left: *const f32,
    right: *const f32,
    n: usize,
    out: *mut f32,
) -> i32 {
    if left.is_null() || right.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: caller guarantees both channels are readable for `n` `f32`s.
    let l = unsafe { std::slice::from_raw_parts(left, n) };
    let r = unsafe { std::slice::from_raw_parts(right, n) };
    match measure::correlation(l, r) {
        Some(v) => {
            // SAFETY: `out` is writable for one `f32`.
            unsafe { *out = v };
            0
        }
        None => -1,
    }
}

/// Maps `n` stereo pairs (`left`, `right`) to their **Lissajous / goniometer**
/// coordinates, writing `2 * n` interleaved `f32`s `[x0, y0, x1, y1, …]` into
/// `out`, where `x` is the side component `(L − R)/√2` and `y` the mid
/// `(L + R)/√2` — the 45°-rotated stereo plane a goniometer draws. Returns 0, or
/// -1 (leaving `out` untouched) on a null pointer.
///
/// # Safety
/// `left`/`right` must be readable for `n` `f32`s and `out` writable for `2 * n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_lissajous(
    left: *const f32,
    right: *const f32,
    n: usize,
    out: *mut f32,
) -> i32 {
    if left.is_null() || right.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: caller guarantees the ranges; `[f32; 2]` is two consecutive
    // `f32`s, so the `out` buffer of `2 * n` floats aliases `n` pairs.
    let l = unsafe { std::slice::from_raw_parts(left, n) };
    let r = unsafe { std::slice::from_raw_parts(right, n) };
    let o = unsafe { std::slice::from_raw_parts_mut(out as *mut [f32; 2], n) };
    if measure::lissajous_into(l, r, o) {
        0
    } else {
        -1
    }
}

/// Seconds at `beats` for the affine clock `(tempo, base_beats, base_seconds)`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_beats_to_secs(
    tempo: f64,
    base_beats: f64,
    base_seconds: f64,
    beats: f64,
) -> f64 {
    base_seconds + (beats - base_beats) / tempo
}

/// Beats at `secs` for the affine clock `(tempo, base_beats, base_seconds)`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_secs_to_beats(
    tempo: f64,
    base_beats: f64,
    base_seconds: f64,
    secs: f64,
) -> f64 {
    base_beats + (secs - base_seconds) * tempo
}

/// Seconds → sample count at `sample_rate` (ties to even).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_secs_to_samples(secs: f64, sample_rate: f64) -> i64 {
    tempoclock::secs_to_samples(secs, sample_rate)
}

/// Sample count → seconds at `sample_rate`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_samples_to_secs(samples: i64, sample_rate: f64) -> f64 {
    tempoclock::samples_to_secs(samples, sample_rate)
}

/// The server's sample counter at Unix instant `unix_secs`, from an anchor
/// (`anchor_sample` at `anchor_unix`) and the sample rate — the `/sched`
/// target conversion.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_unix_to_sample(
    unix_secs: f64,
    anchor_unix: f64,
    anchor_sample: i64,
    sample_rate: f64,
) -> i64 {
    clausters_core::osc::unix_to_sample(unix_secs, anchor_unix, anchor_sample, sample_rate)
}

/// Beats to wait so a routine starts on the next `quant` boundary of a grid
/// currently at `pos` beats (`quant <= 0` → 0, i.e. now).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_quant_delay(pos: f64, quant: f64) -> f64 {
    tempoclock::quant_delay(pos, quant)
}

/// The bar index a beat position falls in on a grid of `quant` beats per bar
/// (0-based; `quant <= 0` → 0, no bar grid) — the display complement of
/// [`clausters_core_quant_delay`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_bar(beats: f64, quant: f64) -> f64 {
    tempoclock::bar(beats, quant)
}

/// The beat within its bar for a beat position on a grid of `quant` beats per
/// bar (0-based, in `[0, quant)`; `quant <= 0` returns the position itself).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_beat_in_bar(beats: f64, quant: f64) -> f64 {
    tempoclock::beat_in_bar(beats, quant)
}

/// Hertz → mel (O'Shaughnessy), the perceptual frequency scale shared with the
/// GUI spectrogram axis.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_hz_to_mel(hz: f64) -> f64 {
    scale::hz_to_mel(hz)
}

/// Mel → hertz, the exact inverse of [`clausters_core_hz_to_mel`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_mel_to_hz(mel: f64) -> f64 {
    scale::mel_to_hz(mel)
}

/// Hertz → bark (Traunmüller closed form; −0.53 at 0 Hz, the axis floor).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_hz_to_bark(hz: f64) -> f64 {
    scale::hz_to_bark(hz)
}

/// Bark → hertz, the analytic inverse of [`clausters_core_hz_to_bark`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_bark_to_hz(bark: f64) -> f64 {
    scale::bark_to_hz(bark)
}

/// Packs raw NTP-scale seconds (any epoch: Unix + offset for wire timetags,
/// seconds-from-start for an NRT score) into the 64 timetag bits
/// (`seconds << 32 | fractional`), rounding the fraction — the one packing rule
/// every client shares.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_ntp_timetag(ntp_secs: f64) -> u64 {
    clausters_core::osc::timetag_bits(clausters_core::osc::pack_timetag(ntp_secs))
}

/// A Unix timestamp → the 64 NTP timetag bits (adds the 1900→1970 offset,
/// then packs like [`clausters_core_ntp_timetag`]).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_unix_to_ntp(unix_secs: f64) -> u64 {
    clausters_core::osc::timetag_bits(clausters_core::osc::unix_to_ntp(unix_secs))
}

/// Scale-degree → MIDI note number in the pitch space `octave`/`root`, with
/// floored octave wrapping (sclang semantics). `scale` is `n` semitone offsets;
/// `n == 0` (or a null `scale`) yields middle C.
///
/// # Safety
/// `scale` must be readable for `n` `f32`s (or null with `n == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_degree_to_midinote(
    degree: f64,
    octave: f64,
    root: f64,
    scale: *const f32,
    n: usize,
) -> f64 {
    if scale.is_null() || n == 0 {
        return builtins::degree_to_midinote(degree, octave, root, &[]);
    }
    // SAFETY: caller guarantees `scale` is readable for `n`.
    let s = unsafe { std::slice::from_raw_parts(scale, n) };
    builtins::degree_to_midinote(degree, octave, root, s)
}

// ---- seeded value stream (patterns) ----
//
// Stateless across the boundary: the caller holds the one u64 state word and
// passes it by pointer, so the stream is resumable from any language with no
// handle to free.

/// The initial state word for `seed` (splitmix64-mixed, never zero) — the same
/// seeding as the server's `WhiteNoise`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_rng_seed(seed: u64) -> u64 {
    Rng::from_seed(seed).state()
}

/// Advances `*state` one step and returns a uniform `f64` in `[0, 1)` with
/// 53-bit resolution. A null `state` returns 0.
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_next_f64(state: *mut u64) -> f64 {
    if state.is_null() {
        return 0.0;
    }
    // SAFETY: caller guarantees `state` points to a u64.
    let mut rng = Rng::from_state(unsafe { *state });
    let v = rng.next_f64();
    unsafe { *state = rng.state() };
    v
}

/// Advances `*state` and returns a uniform integer in `[0, n)` (0 when `n` is
/// 0 or `state` is null).
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_next_below(state: *mut u64, n: u64) -> u64 {
    if state.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `state` points to a u64.
    let mut rng = Rng::from_state(unsafe { *state });
    let v = rng.next_below(n);
    unsafe { *state = rng.state() };
    v
}

/// Advances `*state` one step and returns the full-width random word (0 when
/// `state` is null). Used to derive a child stream's seed from a parent's —
/// the sclang-style inheritance where a routine's generator is seeded from the
/// context that creates it, so one root seed reproduces a whole script.
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_next_u64(state: *mut u64) -> u64 {
    if state.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `state` points to a u64.
    let mut rng = Rng::from_state(unsafe { *state });
    let v = rng.next_u64();
    unsafe { *state = rng.state() };
    v
}

// ---- beat-ordered scheduler queue ----
//
// An opaque handle (like `clausters_ws_*`): the host language maps the flat
// `u64` ids back to its routines; only times and ids cross.

/// A new, empty scheduler queue. Free with [`clausters_sched_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_sched_new() -> *mut Scheduler {
    Box::into_raw(Box::new(Scheduler::new()))
}

/// Frees a queue created by [`clausters_sched_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_sched_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_free(h: *mut Scheduler) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Queues `id` at beat `time`. Stable for equal times (insertion order).
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_push(h: *mut Scheduler, time: f64, id: u64) {
    if let Some(s) = unsafe { h.as_mut() } {
        s.push(time, id);
    }
}

/// Writes the earliest queued beat into `*out_time`; returns 0, or -1 when the
/// queue is empty (out untouched).
///
/// # Safety
/// `h` must be a live scheduler handle and `out_time` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_peek_time(h: *mut Scheduler, out_time: *mut f64) -> i32 {
    let Some(s) = (unsafe { h.as_ref() }) else {
        return -1;
    };
    match s.peek_time() {
        Some(t) if !out_time.is_null() => {
            // SAFETY: caller guarantees `out_time` is writable.
            unsafe { *out_time = t };
            0
        }
        _ => -1,
    }
}

/// Pops the earliest event with time `<= now` into `*out_time`/`*out_id`;
/// returns 0, or -1 when nothing is due.
///
/// # Safety
/// `h` must be a live scheduler handle; `out_time`/`out_id` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_pop_due(
    h: *mut Scheduler,
    now: f64,
    out_time: *mut f64,
    out_id: *mut u64,
) -> i32 {
    let Some(s) = (unsafe { h.as_mut() }) else {
        return -1;
    };
    match s.pop_due(now) {
        Some((t, id)) if !out_time.is_null() && !out_id.is_null() => {
            // SAFETY: caller guarantees the out pointers are writable.
            unsafe {
                *out_time = t;
                *out_id = id;
            }
            0
        }
        _ => -1,
    }
}

/// Removes every queued entry with `id`; returns how many were dropped.
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_remove(h: *mut Scheduler, id: u64) -> usize {
    match unsafe { h.as_mut() } {
        Some(s) => s.remove(id),
        None => 0,
    }
}

/// Number of queued entries (0 for a null handle).
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_len(h: *mut Scheduler) -> usize {
    unsafe { h.as_ref() }.map_or(0, Scheduler::len)
}

/// Drops every queued entry.
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_clear(h: *mut Scheduler) {
    if let Some(s) = unsafe { h.as_mut() } {
        s.clear();
    }
}

// ---- sample-clock tracking model ----

/// A new least-squares model at `nominal_rate` keeping `window` anchors.
/// Free with [`clausters_clocksync_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_clocksync_new(
    nominal_rate: f64,
    window: usize,
) -> *mut SampleClockModel {
    Box::into_raw(Box::new(SampleClockModel::new(nominal_rate, window)))
}

/// Frees a model created by [`clausters_clocksync_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_clocksync_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_free(h: *mut SampleClockModel) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Adds an anchor `(t_local, sample)` and refits; a positive finite `rate`
/// updates the nominal rate (pass `<= 0` to keep it).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_add_anchor(
    h: *mut SampleClockModel,
    t_local: f64,
    sample: i64,
    rate: f64,
) {
    if let Some(m) = unsafe { h.as_mut() } {
        m.add_anchor(t_local, sample, rate);
    }
}

/// The predicted counter at local time `t_local` (0 for a null handle).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_sample_at(
    h: *mut SampleClockModel,
    t_local: f64,
) -> i64 {
    unsafe { h.as_ref() }.map_or(0, |m| m.sample_at(t_local))
}

/// Inverse: the local time the counter reaches `sample`.
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_local_time_of(
    h: *mut SampleClockModel,
    sample: i64,
) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, |m| m.local_time_of(sample))
}

/// Fitted-slope deviation from the nominal rate, in ppm.
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_drift_ppm(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::drift_ppm)
}

/// Local-time span covered by the anchor window (0 below two anchors).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_span(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::span)
}

/// The nominal (or last reported) sample rate.
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_rate(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::rate)
}

/// Fitted slope `b` (samples per local second).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_slope(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::slope)
}

/// Fitted intercept `a` (samples at local time 0).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_intercept(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::intercept)
}

// --- Finite-resource registry (ABI v10) ---------------------------------
//
// The shared id allocator model (`clausters_core::registry`): node ids, buses
// and buffers are finite boot-time resources, the registry is the occupancy
// map. Handles are internally locked — a client's clock thread allocates
// while its reply thread releases on `/n_end` — and the registry is passive:
// events flow in, nothing calls back out.

/// A registry handle safe to share across the binding's threads.
pub struct FfiRegistry(Mutex<Registry>);

/// A new registry over `[base, base + capacity)`. `capacity` 0 means
/// **unbounded** (the NRT/score mode: allocation never fails, release only
/// keeps the live count). Free with [`clausters_registry_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_registry_new(base: i64, capacity: u64) -> *mut FfiRegistry {
    let reg = if capacity == 0 {
        Registry::unbounded(base)
    } else {
        Registry::new(base, capacity as usize)
    };
    Box::into_raw(Box::new(FfiRegistry(Mutex::new(reg))))
}

/// Frees a registry created by [`clausters_registry_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_registry_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_free(h: *mut FfiRegistry) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

fn with_registry<T>(h: *mut FfiRegistry, default: T, f: impl FnOnce(&mut Registry) -> T) -> T {
    // SAFETY: caller guarantees `h` is a live registry handle (or null).
    let Some(reg) = (unsafe { h.as_ref() }) else {
        return default;
    };
    f(&mut reg.0.lock().expect("registry lock poisoned"))
}

/// Allocates `width` contiguous ids; returns the first, or -1 when the space
/// is exhausted (never wraps). `width` 0 counts as 1.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_alloc(h: *mut FfiRegistry, width: u64) -> i64 {
    with_registry(h, -1, |r| r.alloc(width as usize).unwrap_or(-1))
}

/// Returns `width` ids starting at `first` to the pool. 0 on success, -1 when
/// some id is out of range, -2 when some id is not allocated (double release
/// or foreign id); on error nothing is released.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_release(
    h: *mut FfiRegistry,
    first: i64,
    width: u64,
) -> i32 {
    with_registry(h, -1, |r| match r.release(first, width as usize) {
        Ok(()) => 0,
        Err(ReleaseError::OutOfRange) => -1,
        Err(ReleaseError::NotAllocated) => -2,
    })
}

/// How many ids are currently allocated.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_in_use(h: *mut FfiRegistry) -> u64 {
    with_registry(h, 0, |r| r.in_use() as u64)
}

/// The registry's capacity; 0 when unbounded.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_capacity(h: *mut FfiRegistry) -> u64 {
    with_registry(h, 0, |r| r.capacity().unwrap_or(0) as u64)
}

/// Whether `id` falls inside the registry's space (allocated or not) — the
/// foreign-id filter for `/n_end` handling. 1 yes, 0 no.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_contains(h: *mut FfiRegistry, id: i64) -> i32 {
    with_registry(h, 0, |r| r.contains(id) as i32)
}

/// Releases everything back to the pool (a client reset).
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_clear(h: *mut FfiRegistry) {
    with_registry(h, (), Registry::clear)
}

/// The boot-derived node-id partition for a node table of `max_nodes` slots.
/// Writes six values into `out`: client base, client capacity, auto base,
/// auto capacity, MIDI base, MIDI capacity. Returns 0, or -1 on a null `out`.
///
/// # Safety
/// `out` must be writable for six `i64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_node_partition(max_nodes: u64, out: *mut i64) -> i32 {
    if out.is_null() {
        return -1;
    }
    let p = NodeIdPartition::from_max_nodes(max_nodes as usize);
    let vals = [
        p.client_base,
        p.client_capacity as i64,
        p.auto_base,
        p.auto_capacity as i64,
        p.midi_base,
        p.midi_capacity as i64,
    ];
    // SAFETY: caller guarantees `out` is writable for six i64s.
    unsafe { std::slice::from_raw_parts_mut(out, 6) }.copy_from_slice(&vals);
    0
}

/// Width of the GraphDef private-bus reservation at the top of the audio bus
/// space (before clamping to a smaller configured count).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_registry_graph_audio_reserved() -> u64 {
    registry::GRAPH_AUDIO_BUS_RESERVED as u64
}

/// Control-rate counterpart of [`clausters_registry_graph_audio_reserved`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_registry_graph_control_reserved() -> u64 {
    registry::GRAPH_CONTROL_BUS_RESERVED as u64
}

/// The GUI patcher's **cord → bus pass**: a directed
/// [`Patch`](clausters_core::patch::Patch) as JSON in (`patch`/`patch_len`), its
/// [`Compiled`](clausters_core::patch::Compiled) bus wiring as JSON written to
/// `out` (capacity `out_cap`). Returns the number of bytes the output JSON needs
/// — written iff it fit, so a caller sizes with a first call (a null/small `out`)
/// then fills — or `0` when the input is not readable JSON for a `Patch`.
///
/// A **compile** error (a malformed cord: reversed, mismatched rate, out of
/// range) is not a `0`: it comes back *as* the output JSON, the object
/// `{"error": "<message>"}`, so the caller reads one channel. Success is the
/// `Compiled` object (`{"buses": …, "members": …}`).
///
/// # Safety
/// `patch` must be readable for `patch_len` bytes and `out` writable for
/// `out_cap` bytes (or null, to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_patch_compile(
    patch: *const u8,
    patch_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_core::patch::{Patch, compile};

    if patch.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `patch` is readable for `patch_len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(patch, patch_len) };
    let Ok(p) = serde_json::from_slice::<Patch>(bytes) else {
        return 0;
    };
    let json = match compile(&p) {
        Ok(c) => serde_json::to_vec(&c).unwrap_or_default(),
        Err(e) => serde_json::to_vec(&serde_json::json!({ "error": e })).unwrap_or_default(),
    };
    let n = json.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(json.as_ptr(), out, n) };
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trip_over_the_c_surface() {
        let h = clausters_registry_new(1000, 4);
        unsafe {
            assert_eq!(clausters_registry_alloc(h, 2), 1000);
            assert_eq!(clausters_registry_alloc(h, 2), 1002);
            assert_eq!(clausters_registry_alloc(h, 1), -1, "exhausted, no wrap");
            assert_eq!(clausters_registry_in_use(h), 4);
            assert_eq!(clausters_registry_release(h, 1000, 2), 0);
            assert_eq!(clausters_registry_release(h, 1000, 2), -2, "double release");
            assert_eq!(clausters_registry_release(h, 999, 1), -1, "out of range");
            assert_eq!(clausters_registry_alloc(h, 2), 1000, "released ids reused");
            assert_eq!(clausters_registry_contains(h, 1003), 1);
            assert_eq!(clausters_registry_contains(h, 1004), 0);
            assert_eq!(clausters_registry_capacity(h), 4);
            clausters_registry_clear(h);
            assert_eq!(clausters_registry_in_use(h), 0);
            clausters_registry_free(h);
        }
        // Unbounded (NRT): capacity 0, allocation never fails.
        let h = clausters_registry_new(0, 0);
        unsafe {
            assert_eq!(clausters_registry_capacity(h), 0);
            for i in 0..10_000 {
                assert_eq!(clausters_registry_alloc(h, 1), i);
            }
            clausters_registry_free(h);
        }
    }

    #[test]
    fn unary_and_binary_over_arrays() {
        let a = [1.0f32, 2.0, 3.0];
        let b = [10.0f32];
        let mut out = [0.0f32; 3];
        // add with a broadcast constant
        let rc = unsafe {
            clausters_core_binary(
                BinaryOp::Add as u32,
                a.as_ptr(),
                3,
                b.as_ptr(),
                1,
                out.as_mut_ptr(),
                3,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(out, [11.0, 12.0, 13.0]);

        let rc = unsafe {
            clausters_core_unary(UnaryOp::Neg as u32, a.as_ptr(), 3, out.as_mut_ptr(), 3)
        };
        assert_eq!(rc, 0);
        assert_eq!(out, [-1.0, -2.0, -3.0]);
    }

    #[test]
    fn rejects_unknown_op_and_bad_length() {
        let mut out = [0.0f32; 2];
        let a = [1.0f32, 2.0];
        assert_eq!(
            unsafe { clausters_core_unary(9999, a.as_ptr(), 2, out.as_mut_ptr(), 2) },
            -1
        );
        // in_len neither 1 nor n
        assert_eq!(
            unsafe {
                clausters_core_unary(UnaryOp::Abs as u32, a.as_ptr(), 2, out.as_mut_ptr(), 5)
            },
            -2
        );
    }

    #[test]
    fn whitenoise_matches_core() {
        let mut out = [0.0f32; 8];
        unsafe { clausters_core_whitenoise(42, out.as_mut_ptr(), 8) };
        let mut expect = [0.0f32; 8];
        WhiteNoise::from_seed(42).fill(&mut expect);
        assert_eq!(out, expect);
    }

    #[test]
    fn clock_scalars() {
        // 120 bpm = 2 bps, beat 0 at second 0.
        assert_eq!(clausters_core_beats_to_secs(2.0, 0.0, 0.0, 2.0), 1.0);
        assert_eq!(clausters_core_secs_to_samples(1.0, 48_000.0), 48_000);
    }

    #[test]
    fn scheduler_round_trip_over_the_abi() {
        let h = clausters_sched_new();
        unsafe {
            clausters_sched_push(h, 2.0, 20);
            clausters_sched_push(h, 1.0, 10);
            clausters_sched_push(h, 1.0, 11);
            clausters_sched_push(h, 3.0, 10);
            assert_eq!(clausters_sched_len(h), 4);
            let mut t = 0.0;
            assert_eq!(clausters_sched_peek_time(h, &mut t), 0);
            assert_eq!(t, 1.0);
            assert_eq!(clausters_sched_remove(h, 10), 2);
            let mut id = 0u64;
            assert_eq!(clausters_sched_pop_due(h, 1.0, &mut t, &mut id), 0);
            assert_eq!((t, id), (1.0, 11));
            assert_eq!(clausters_sched_pop_due(h, 1.0, &mut t, &mut id), -1);
            clausters_sched_clear(h);
            assert_eq!(clausters_sched_len(h), 0);
            clausters_sched_free(h);
        }
    }

    #[test]
    fn clocksync_and_rng_and_timetags_over_the_abi() {
        let h = clausters_clocksync_new(48_000.0, 64);
        unsafe {
            for i in 0..6 {
                let t = i as f64 * 0.05;
                clausters_clocksync_add_anchor(h, t, (1000.0 + 48_000.0 * t) as i64, 48_000.0);
            }
            assert!((clausters_clocksync_sample_at(h, 1.0) - 49_000).abs() <= 1);
            assert!(clausters_clocksync_drift_ppm(h).abs() < 1.0);
            assert_eq!(clausters_clocksync_rate(h), 48_000.0);
            clausters_clocksync_free(h);
        }

        // The flat-state RNG resumes the same stream as the library type.
        let mut state = clausters_rng_seed(1);
        let mut expect = Rng::from_seed(1);
        for _ in 0..100 {
            assert_eq!(
                unsafe { clausters_rng_next_f64(&mut state) },
                expect.next_f64()
            );
        }
        // ...including the raw word used to derive child-stream seeds.
        assert_eq!(
            unsafe { clausters_rng_next_u64(&mut state) },
            expect.next_u64()
        );

        // Timetag packing matches the core's rounding rule.
        assert_eq!(
            clausters_core_ntp_timetag(10.75),
            (10u64 << 32) | ((3u64) << 30)
        );
        let bits = clausters_core_unix_to_ntp(0.0);
        assert_eq!(bits >> 32, 2_208_988_800);

        assert_eq!(clausters_core_quant_delay(3.5, 4.0), 0.5);
        let major = [0.0f32, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0];
        assert_eq!(
            unsafe { clausters_core_degree_to_midinote(-1.0, 5.0, 0.0, major.as_ptr(), 7) },
            59.0
        );
    }

    #[test]
    fn ruler_axis_scalars_cross_the_boundary() {
        // bar:beat reads of the quant grid (the quant_delay complement).
        assert_eq!(clausters_core_bar(9.5, 4.0), 2.0);
        assert!((clausters_core_beat_in_bar(9.5, 4.0) - 1.5).abs() < 1e-12);
        assert_eq!(clausters_core_beat_in_bar(9.5, 0.0), 9.5);
        // Perceptual frequency scales round-trip through the C surface.
        for hz in [100.0, 1000.0, 12_000.0] {
            assert!((clausters_core_mel_to_hz(clausters_core_hz_to_mel(hz)) - hz).abs() < 1e-6);
            assert!((clausters_core_bark_to_hz(clausters_core_hz_to_bark(hz)) - hz).abs() < 1e-6);
        }
        assert!((clausters_core_hz_to_mel(1000.0) - 1000.0).abs() < 0.1);
        assert!((clausters_core_hz_to_bark(1000.0) - 8.53).abs() < 0.05);
    }

    #[test]
    fn peaks_build_writes_a_parseable_cache() {
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let base = 64;
        let size = clausters_core_peaks_cache_size(samples.len(), base);
        assert!(size > 0);
        let mut out = vec![0u8; size];
        let written = unsafe {
            clausters_core_peaks_build(
                samples.as_ptr(),
                samples.len(),
                base,
                out.as_mut_ptr(),
                out.len(),
            )
        };
        assert_eq!(written, size, "writes exactly the predicted size");
        // The bytes are the same cache the GUI host parses, identical to a
        // pyramid built in-process (the algorithm lives once in the core).
        let from_ffi = Pyramid::from_bytes(&out).expect("parse");
        assert_eq!(from_ffi.total_samples(), samples.len());
        assert_eq!(
            Pyramid::build(&samples, base).to_bytes(),
            out,
            "FFI cache is byte-identical to the in-process build"
        );
        // A buffer one byte short writes nothing.
        let mut tiny = vec![0u8; size - 1];
        assert_eq!(
            unsafe {
                clausters_core_peaks_build(
                    samples.as_ptr(),
                    samples.len(),
                    base,
                    tiny.as_mut_ptr(),
                    tiny.len(),
                )
            },
            0
        );
    }

    #[test]
    fn peaks_multi_build_writes_a_parseable_multichannel_cache() {
        // Interleaved stereo; the FFI cache must be byte-identical to the
        // in-process multichannel build (one algorithm, in the core).
        let inter: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.01).sin()).collect();
        let (frames, channels, base) = (1000, 2, 64);
        let size = clausters_core_peaks_multi_cache_size(frames, channels, base);
        assert!(size > 0);
        let mut out = vec![0u8; size];
        let written = unsafe {
            clausters_core_peaks_multi_build(
                inter.as_ptr(),
                inter.len(),
                channels,
                base,
                out.as_mut_ptr(),
                out.len(),
            )
        };
        assert_eq!(written, size, "writes exactly the predicted size");
        let parsed = MultiPyramid::from_bytes(&out).expect("parse");
        assert_eq!(parsed.num_channels(), channels);
        assert_eq!(parsed.frames(), frames);
        assert_eq!(
            MultiPyramid::build_interleaved(&inter, channels, base).to_bytes(),
            out
        );
        // Refusals: zero channels/bucket and a too-small buffer.
        assert_eq!(
            clausters_core_peaks_multi_cache_size(frames, channels, 0),
            0
        );
        let mut tiny = vec![0u8; size - 1];
        assert_eq!(
            unsafe {
                clausters_core_peaks_multi_build(
                    inter.as_ptr(),
                    inter.len(),
                    channels,
                    base,
                    tiny.as_mut_ptr(),
                    tiny.len(),
                )
            },
            0
        );
    }

    #[test]
    fn correlation_matches_the_core_and_flags_the_undefined_case() {
        let l: Vec<f32> = (0..128).map(|i| (i as f32 * 0.2).sin()).collect();
        let neg: Vec<f32> = l.iter().map(|s| -s).collect();
        let mut r = 0.0f32;
        assert_eq!(
            unsafe { clausters_core_correlation(l.as_ptr(), l.as_ptr(), l.len(), &mut r) },
            0
        );
        assert!((r - 1.0).abs() < 1e-5, "mono reads +1");
        assert_eq!(
            unsafe { clausters_core_correlation(l.as_ptr(), neg.as_ptr(), l.len(), &mut r) },
            0
        );
        assert!((r + 1.0).abs() < 1e-5, "anti-phase reads -1");
        // A constant channel is undefined: -1, and `r` is left as it was.
        let flat = vec![0.5f32; 128];
        let before = r;
        assert_eq!(
            unsafe { clausters_core_correlation(flat.as_ptr(), l.as_ptr(), l.len(), &mut r) },
            -1
        );
        assert_eq!(r, before, "out untouched on the undefined case");
    }

    #[test]
    fn lissajous_writes_interleaved_side_mid_pairs() {
        let l = [1.0f32, 0.7, 0.0];
        let r = [1.0f32, -0.7, 0.0];
        let mut out = vec![0.0f32; l.len() * 2];
        assert_eq!(
            unsafe { clausters_core_lissajous(l.as_ptr(), r.as_ptr(), l.len(), out.as_mut_ptr()) },
            0
        );
        // Mono pair (1,1): side 0, mid √2. Anti pair (0.7,-0.7): side √2·0.7, mid 0.
        assert!(out[0].abs() < 1e-6 && (out[1] - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!((out[2] - std::f32::consts::SQRT_2 * 0.7).abs() < 1e-6 && out[3].abs() < 1e-6);
    }

    fn compile_json(patch: &str) -> (usize, serde_json::Value) {
        let b = patch.as_bytes();
        // Size query first (null out), then fill.
        let need =
            unsafe { clausters_core_patch_compile(b.as_ptr(), b.len(), std::ptr::null_mut(), 0) };
        if need == 0 {
            return (0, serde_json::Value::Null);
        }
        let mut buf = vec![0u8; need];
        let n = unsafe {
            clausters_core_patch_compile(b.as_ptr(), b.len(), buf.as_mut_ptr(), buf.len())
        };
        assert_eq!(n, need, "the fill matches the size query");
        (need, serde_json::from_slice(&buf).unwrap())
    }

    #[test]
    fn patch_compile_wires_a_chain() {
        let (need, v) = compile_json(
            r#"{"boxes":[
                {"def":"tone","ports":[{"name":"out","dir":"out","rate":"audio"}]},
                {"def":"dac","ports":[{"name":"in","dir":"in","rate":"audio"},
                                      {"name":"out","dir":"out","rate":"audio"}]},
                {"def":"OUT","hardware":true,"ports":[{"name":"in","dir":"in","rate":"audio"}]}],
              "cords":[{"from_box":0,"from_port":0,"to_box":1,"to_port":0},
                       {"from_box":1,"from_port":1,"to_box":2,"to_port":0}]}"#,
        );
        assert!(need > 0);
        assert_eq!(
            v["buses"].as_array().unwrap().len(),
            1,
            "one private bus (tone->dac)"
        );
        let members = v["members"].as_array().unwrap();
        assert_eq!(members.len(), 2, "the hardware OUT is not a member");
        let dac = &members[1];
        assert_eq!(dac["def"], "dac");
        assert!(
            dac["controls"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["bus"] == "OUT"),
            "dac's out is wired to the hardware OUT"
        );
    }

    #[test]
    fn patch_compile_reports_a_bad_cord_as_an_error_object() {
        // A reversed cord (inlet -> outlet) comes back as an error channel, not 0.
        let (_need, v) = compile_json(
            r#"{"boxes":[
                {"def":"tone","ports":[{"name":"out","dir":"out","rate":"audio"}]},
                {"def":"dac","ports":[{"name":"in","dir":"in","rate":"audio"}]}],
              "cords":[{"from_box":1,"from_port":0,"to_box":0,"to_port":0}]}"#,
        );
        assert!(v["error"].is_string());
    }

    #[test]
    fn patch_compile_rejects_malformed_input_with_zero() {
        let bad = b"not a patch";
        let n = unsafe {
            clausters_core_patch_compile(bad.as_ptr(), bad.len(), std::ptr::null_mut(), 0)
        };
        assert_eq!(n, 0);
    }
}
