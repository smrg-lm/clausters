//! The seeded value stream, so a take replays in any language.

use super::*;

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
