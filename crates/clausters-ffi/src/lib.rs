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
use clausters_core::rng::WhiteNoise;
use clausters_core::tempoclock;

mod ws;

/// The C ABI version of this surface. Bump on any incompatible change. v2 added
/// the `clausters_ws_*` WebSocket client transport.
pub const CORE_ABI_VERSION: u32 = 2;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
