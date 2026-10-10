//! The seeded value stream, so a take replays in any language.

use super::*;

// ---- seeded value stream (patterns) ----
//
// Stateless across the boundary: the caller holds the one u64 state word and
// passes it by pointer, so the stream is resumable from any language with no
// handle to free.

/// The initial state word for `seed` (splitmix64-mixed, never zero) -- the same
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
/// `state` is null). Used to derive a child stream's seed from a parent's --
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

/// Advances `*state` and returns one draw from distribution `draw` (a
/// `clausters_core::rng::Draw`) with its arguments `a` and `b` -- `rand`,
/// `rrand`, `exprand`, `linrand`, `coin`, `irand`, `gauss` and the rest, one
/// door for the whole table. NaN on an unknown distribution or a null `state`.
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_draw(state: *mut u64, draw: u32, a: f64, b: f64) -> f64 {
    let Some(draw) = clausters_core::rng::Draw::from_u32(draw) else {
        return f64::NAN;
    };
    if state.is_null() {
        return f64::NAN;
    }
    // SAFETY: caller guarantees `state` points to a u64.
    let mut rng = Rng::from_state(unsafe { *state });
    let v = rng.draw(draw, a, b);
    unsafe { *state = rng.state() };
    v
}

/// Advances `*state` and returns the index `wchoose` picks from the `n`
/// relative `weights` (0 when `n` is 0, `weights` is null or `state` is null).
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null), and `weights`
/// readable for `n` `f64`s (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_windex(
    state: *mut u64,
    weights: *const f64,
    n: usize,
) -> u64 {
    if state.is_null() || weights.is_null() || n == 0 {
        return 0;
    }
    // SAFETY: caller guarantees the ranges above.
    let w = unsafe { std::slice::from_raw_parts(weights, n) };
    let mut rng = Rng::from_state(unsafe { *state });
    let i = rng.windex(w);
    unsafe { *state = rng.state() };
    i as u64
}

/// Advances `*state` and returns one draw shaped by the `n`-value `table` an
/// `asrandomtable` built (NaN when `n` is 0 or a pointer is null).
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null), and `table` readable
/// for `n` `f64`s (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_tablerand(
    state: *mut u64,
    table: *const f64,
    n: usize,
) -> f64 {
    if state.is_null() || table.is_null() || n == 0 {
        return f64::NAN;
    }
    // SAFETY: caller guarantees the ranges above.
    let t = unsafe { std::slice::from_raw_parts(table, n) };
    let mut rng = Rng::from_state(unsafe { *state });
    let v = rng.tablerand(t);
    unsafe { *state = rng.state() };
    v
}

/// Advances `*state` and writes sclang's `sputter` of a list of `n` values --
/// its indices, each repeated while a `probability` coin lands -- into `out`,
/// at most `maxlen` of them; returns how many it wrote (0 when a pointer is
/// null).
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null), and `out` writable
/// for `maxlen` `u32`s (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_sputter(
    state: *mut u64,
    n: usize,
    probability: f64,
    maxlen: usize,
    out: *mut u32,
) -> usize {
    if state.is_null() || (out.is_null() && maxlen > 0) {
        return 0;
    }
    let mut rng = Rng::from_state(unsafe { *state });
    let indices = rng.sputter(n, probability, maxlen);
    unsafe { *state = rng.state() };
    if !indices.is_empty() {
        // SAFETY: caller guarantees `out` holds `maxlen`, and sputter never
        // writes more.
        let o = unsafe { std::slice::from_raw_parts_mut(out, indices.len()) };
        o.copy_from_slice(&indices);
    }
    indices.len()
}

/// Advances `*state` and writes a uniformly random permutation of `0..n`
/// into `out` (nothing when `n` is 0 or a pointer is null).
///
/// # Safety
/// `state` must be a valid pointer to a `u64` (or null), and `out` writable
/// for `n` `u32`s (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_rng_scramble(state: *mut u64, out: *mut u32, n: usize) {
    if state.is_null() || out.is_null() || n == 0 {
        return;
    }
    // SAFETY: caller guarantees the ranges above.
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    let mut rng = Rng::from_state(unsafe { *state });
    rng.scramble(o);
    unsafe { *state = rng.state() };
}
