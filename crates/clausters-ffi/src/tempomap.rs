//! The piece's beat↔second time map, as an opaque handle.
//!
//! Like `clausters_sched_*`, the structure stays in Rust and only flat data
//! crosses: beats, seconds and tempos as `f64`, a curve as a small integer.
//! There is one implementation of the integral, and every client calls it.

use clausters_core::tempomap::{Curve, Extent, Shape, TempoMap};

/// A new map of one constant-tempo segment (beat 0 at second 0). A tempo that
/// is not finite and positive falls back to 1.0. Free with
/// [`clausters_tempomap_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_tempomap_new(tempo: f64) -> *mut TempoMap {
    Box::into_raw(Box::new(TempoMap::new(tempo)))
}

/// The map's edit count, bumped by every write that lands (`0` for a null
/// handle, and a live map never reads 0).
///
/// What a holder of a **shared** map compares to learn that something moved:
/// every reader re-evaluates from the map itself, so this is all the
/// machinery a second clock on one map needs.
///
/// # Safety
/// `h` must be a live handle from `clausters_tempomap_new` (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_version(h: *const TempoMap) -> u64 {
    // SAFETY: caller guarantees `h` is live or null.
    match unsafe { h.as_ref() } {
        Some(m) => m.version(),
        None => 0,
    }
}

/// The map written out as JSON — its breakpoints, without the derived seconds.
/// Follows the `clausters_core_bundle_*` convention: JSON out into a caller
/// buffer, returning the size it needs (`0` for a null handle).
///
/// # Safety
/// `h` must be live or null, and `out` writable for `out_cap` bytes (or null,
/// to size only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_dump(
    h: *const TempoMap,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees `h` is live or null.
    let Some(map) = (unsafe { h.as_ref() }) else {
        return 0;
    };
    let json = serde_json::to_vec(map).unwrap_or_default();
    let n = json.len();
    if !out.is_null() && out_cap >= n {
        // SAFETY: out is non-null and writable for out_cap >= n bytes.
        unsafe { std::ptr::copy_nonoverlapping(json.as_ptr(), out, n) };
    }
    n
}

/// A map read back from the JSON [`clausters_tempomap_dump`] writes. Null when
/// the bytes are not a map this client could have written — the breakpoints
/// are replayed through the ordinary writers, so every rule a live gesture
/// obeys is checked here.
///
/// # Safety
/// `json` must be readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_load(json: *const u8, len: usize) -> *mut TempoMap {
    if json.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees `json` is readable for `len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(json, len) };
    match serde_json::from_slice::<TempoMap>(bytes) {
        Ok(m) => Box::into_raw(Box::new(m)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// A new map of one constant-tempo segment with `base_beats` falling on
/// `base_seconds` — the affine triple a running clock already holds, so
/// adopting a map changes no result. Null on invalid arguments.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_tempomap_anchored(
    tempo: f64,
    base_beats: f64,
    base_seconds: f64,
) -> *mut TempoMap {
    match TempoMap::anchored(tempo, base_beats, base_seconds) {
        Ok(m) => Box::into_raw(Box::new(m)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Frees a map created by [`clausters_tempomap_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_tempomap_new`/`_anchored`/`_clone`,
/// not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_free(h: *mut TempoMap) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// An independent copy of `h` — what handing a piece's map to a clock takes,
/// so neither one's edits reach the other. Null when `h` is null.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_clone(h: *const TempoMap) -> *mut TempoMap {
    match unsafe { h.as_ref() } {
        Some(m) => Box::into_raw(Box::new(m.clone())),
        None => std::ptr::null_mut(),
    }
}

/// **The time map**: the second beat `b` falls on. 0.0 for a null handle.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_secs_at(h: *const TempoMap, b: f64) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, |m| m.secs_at(b))
}

/// The inverse: the beat falling on second `s`. 0.0 for a null handle.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_beats_at(h: *const TempoMap, s: f64) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, |m| m.beats_at(s))
}

/// The tempo (beats per second) in effect at beat `b`. 0.0 for a null handle.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_tempo_at(h: *const TempoMap, b: f64) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, |m| m.tempo_at(b))
}

/// How long the stretch from `b0` to `b1` lasts, in seconds — the only correct
/// way to turn a length in beats into a length in time, since the same span
/// lasts differently depending on where it sits.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_span_secs(h: *const TempoMap, b0: f64, b1: f64) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, |m| m.span_secs(b0, b1))
}

/// How many beats fit in `secs` seconds starting at beat `b0`.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_span_beats(
    h: *const TempoMap,
    b0: f64,
    secs: f64,
) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, |m| m.span_beats(b0, secs))
}

/// Appends a constant-tempo change at beat `b`. Returns 0, or -1 when the
/// tempo or the breakpoint is refused (the map is left as it was).
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_push(h: *mut TempoMap, b: f64, tempo: f64) -> i32 {
    match unsafe { h.as_mut() } {
        Some(m) => match m.push(b, tempo) {
            Ok(()) => 0,
            Err(_) => -1,
        },
        None => -1,
    }
}

/// Writes a tempo ramp over `[from_beats, to_beats]` from `from_tempo` to
/// `to_tempo`, holding `to_tempo` after it. Returns 0, or -1 when refused.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_ramp(
    h: *mut TempoMap,
    from_beats: f64,
    to_beats: f64,
    from_tempo: f64,
    to_tempo: f64,
) -> i32 {
    match unsafe { h.as_mut() } {
        Some(m) => match m.ramp(from_beats, to_beats, from_tempo, to_tempo) {
            Ok(()) => 0,
            Err(_) => -1,
        },
        None => -1,
    }
}

/// [`clausters_tempomap_ramp`] in an explicit shape: `shape` is the envelope
/// shape number (1 linear, 2 exponential, 5 a numeric curvature) and
/// `curvature` is read only by shape 5. Returns 0, or -1 when refused —
/// including for a shape number no tempo curve has.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_shaped(
    h: *mut TempoMap,
    from_beats: f64,
    to_beats: f64,
    from_tempo: f64,
    to_tempo: f64,
    shape: u32,
    curvature: f64,
) -> i32 {
    let (Some(m), Some(shape)) = (unsafe { h.as_mut() }, Shape::from_parts(shape, curvature))
    else {
        return -1;
    };
    match m.shaped(from_beats, to_beats, from_tempo, to_tempo, shape) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Writes a whole tempo envelope from beat `at`: `n` segments, so `tempos`
/// holds `n + 1` values and `extents`, `shapes` and `curvatures` hold `n` each.
/// `seconds` reads the extents as wall clock rather than as beats. Returns 0,
/// or -1 when refused — and a refused envelope writes nothing.
///
/// # Safety
/// `h` must be a live map handle, and the four arrays must hold the lengths
/// above.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_env(
    h: *mut TempoMap,
    at: f64,
    tempos: *const f64,
    extents: *const f64,
    shapes: *const u32,
    curvatures: *const f64,
    n: u32,
    seconds: i32,
) -> i32 {
    let Some(m) = (unsafe { h.as_mut() }) else {
        return -1;
    };
    let count = n as usize;
    if count == 0
        || tempos.is_null()
        || extents.is_null()
        || shapes.is_null()
        || curvatures.is_null()
    {
        return -1;
    }
    // SAFETY: the caller guarantees the lengths documented above.
    let (tempos, extents, shapes, curvatures) = unsafe {
        (
            std::slice::from_raw_parts(tempos, count + 1),
            std::slice::from_raw_parts(extents, count),
            std::slice::from_raw_parts(shapes, count),
            std::slice::from_raw_parts(curvatures, count),
        )
    };
    let mut resolved = Vec::with_capacity(count);
    for i in 0..count {
        match Shape::from_parts(shapes[i], curvatures[i]) {
            Some(shape) => resolved.push(shape),
            None => return -1,
        }
    }
    let unit = match seconds != 0 {
        true => Extent::Seconds,
        false => Extent::Beats,
    };
    match m.write_env(at, tempos, extents, &resolved, unit) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Drops every breakpoint at or after beat `b` (never the first).
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_truncate_from(h: *mut TempoMap, b: f64) {
    if let Some(m) = unsafe { h.as_mut() } {
        m.truncate_from(b);
    }
}

/// How many segments the map holds (always at least 1). 0 for a null handle.
///
/// # Safety
/// `h` must be a live map handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_len(h: *const TempoMap) -> u32 {
    unsafe { h.as_ref() }.map_or(0, |m| m.len() as u32)
}

/// Writes segment `i` into `out` as `[beats, secs, tempo, curve, end_beats,
/// end_tempo]` — `curve` is 0 for a constant tempo and 1 for a ramp, whose two
/// trailing fields are 0.0 when it is not one. Returns 0, or -1 when the index
/// is out of range.
///
/// # Safety
/// `h` must be a live map handle and `out` must point to 7 writable `f64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_segment(
    h: *const TempoMap,
    i: u32,
    out: *mut f64,
) -> i32 {
    let Some(m) = (unsafe { h.as_ref() }) else {
        return -1;
    };
    let Some(seg) = m.segments().get(i as usize) else {
        return -1;
    };
    if out.is_null() {
        return -1;
    }
    let (shape, end_beats, end_tempo, curvature) = match seg.curve {
        Curve::Step => (0.0, 0.0, 0.0, 0.0),
        Curve::Shaped {
            shape,
            end_beats,
            end_tempo,
        } => (
            f64::from(shape.number()),
            end_beats,
            end_tempo,
            shape.curvature(),
        ),
    };
    // SAFETY: caller guarantees `out` has room for 7 f64s.
    let dst = unsafe { std::slice::from_raw_parts_mut(out, 7) };
    dst.copy_from_slice(&[
        seg.beats, seg.secs, seg.tempo, shape, end_beats, end_tempo, curvature,
    ]);
    0
}

/// The last segment's affine triple, written into `out` as `[base_beats,
/// base_seconds, tempo]` — what a clock caches so reading *now* stays three
/// float operations with no search.
///
/// # Safety
/// `h` must be a live map handle and `out` must point to 3 writable `f64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_tempomap_last(h: *const TempoMap, out: *mut f64) -> i32 {
    let Some(m) = (unsafe { h.as_ref() }) else {
        return -1;
    };
    if out.is_null() {
        return -1;
    }
    let last = m.last();
    // SAFETY: caller guarantees `out` has room for 3 f64s.
    let dst = unsafe { std::slice::from_raw_parts_mut(out, 3) };
    dst.copy_from_slice(&[last.beats, last.secs, last.tempo]);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handle_answers_what_the_map_does() {
        let h = clausters_tempomap_new(1.0);
        unsafe {
            assert_eq!(clausters_tempomap_push(h, 2.0, 2.0), 0);
            assert_eq!(clausters_tempomap_secs_at(h, 8.0), 5.0);
            assert_eq!(clausters_tempomap_secs_at(h, 1.0), 1.0);
            assert_eq!(clausters_tempomap_beats_at(h, 5.0), 8.0);
            assert_eq!(clausters_tempomap_span_secs(h, 2.0, 4.0), 1.0);
            assert_eq!(clausters_tempomap_len(h), 2);
            // A backwards breakpoint is refused and changes nothing.
            assert_eq!(clausters_tempomap_push(h, 1.0, 4.0), -1);
            assert_eq!(clausters_tempomap_len(h), 2);
            let mut last = [0.0; 3];
            assert_eq!(clausters_tempomap_last(h, last.as_mut_ptr()), 0);
            assert_eq!(last, [2.0, 2.0, 2.0]);
            let mut seg = [0.0; 7];
            assert_eq!(clausters_tempomap_segment(h, 0, seg.as_mut_ptr()), 0);
            assert_eq!(seg[..4], [0.0, 0.0, 1.0, 0.0]);
            assert_eq!(clausters_tempomap_segment(h, 9, seg.as_mut_ptr()), -1);
            let c = clausters_tempomap_clone(h);
            assert_eq!(clausters_tempomap_push(c, 16.0, 1.0), 0);
            assert_eq!(clausters_tempomap_len(h), 2); // the copy is independent
            clausters_tempomap_free(c);
            clausters_tempomap_free(h);
        }
    }

    #[test]
    fn a_ramp_crosses_and_reports_its_shape() {
        let h = clausters_tempomap_new(1.0);
        unsafe {
            assert_eq!(clausters_tempomap_ramp(h, 0.0, 4.0, 1.0, 2.0), 0);
            let secs = clausters_tempomap_secs_at(h, 4.0);
            assert!((secs - (2.0f64).ln() / 0.25).abs() < 1e-12);
            assert!((clausters_tempomap_tempo_at(h, 2.0) - 1.5).abs() < 1e-12);
            let mut seg = [0.0; 7];
            assert_eq!(clausters_tempomap_segment(h, 0, seg.as_mut_ptr()), 0);
            assert_eq!((seg[3], seg[4], seg[5], seg[6]), (1.0, 4.0, 2.0, 0.0));
            clausters_tempomap_free(h);
        }
    }

    #[test]
    fn a_shape_and_an_envelope_cross_as_seven_numbers() {
        let h = clausters_tempomap_new(1.0);
        unsafe {
            // A curvature is the seventh number, and the only one that reads it.
            assert_eq!(clausters_tempomap_shaped(h, 0.0, 8.0, 1.0, 4.0, 5, -4.0), 0);
            let mut seg = [0.0; 7];
            assert_eq!(clausters_tempomap_segment(h, 0, seg.as_mut_ptr()), 0);
            assert_eq!((seg[3], seg[4], seg[5], seg[6]), (5.0, 8.0, 4.0, -4.0));
            // A shape number no tempo curve has is refused, not silently linear.
            assert_eq!(
                clausters_tempomap_shaped(h, 9.0, 10.0, 4.0, 1.0, 3, 0.0),
                -1
            );
            clausters_tempomap_free(h);

            // An envelope of three segments, extents in seconds.
            let h = clausters_tempomap_new(1.0);
            let tempos = [1.0, 2.0, 2.0, 0.5];
            let extents = [3.0, 4.0, 2.0];
            let shapes = [1u32, 1, 2];
            let curvatures = [0.0, 0.0, 0.0];
            assert_eq!(
                clausters_tempomap_env(
                    h,
                    0.0,
                    tempos.as_ptr(),
                    extents.as_ptr(),
                    shapes.as_ptr(),
                    curvatures.as_ptr(),
                    3,
                    1,
                ),
                0
            );
            assert_eq!(clausters_tempomap_len(h), 4);
            let mut last = [0.0; 3];
            assert_eq!(clausters_tempomap_last(h, last.as_mut_ptr()), 0);
            // Nine seconds of extents land the envelope's end on second nine.
            assert!((last[1] - 9.0).abs() < 1e-9);
            assert!((last[2] - 0.5).abs() < 1e-12);
            clausters_tempomap_free(h);
        }
    }

    #[test]
    fn an_anchored_map_is_the_clocks_own_expression() {
        let h = clausters_tempomap_anchored(2.0, 3.0, 1.5);
        unsafe {
            for b in [0.0, 3.0, 10.0] {
                assert_eq!(
                    clausters_tempomap_secs_at(h, b),
                    crate::time::clausters_core_beats_to_secs(2.0, 3.0, 1.5, b)
                );
            }
            clausters_tempomap_free(h);
        }
        assert!(clausters_tempomap_anchored(0.0, 0.0, 0.0).is_null());
    }
}
