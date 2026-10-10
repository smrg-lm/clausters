//! The axis a break-point curve is drawn against, and a segment's level.

/// One envelope segment's level: shape `shape` (SuperCollider's numbers),
/// curvature `c`, from `a` to `b`, at the position `t` in `[0, 1)` --
/// `clausters_core::envshape::shape_value`, what `EnvGen` plays and what a
/// pattern following a curve (`pseg`) reads.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_shape_value(shape: i32, c: f32, a: f32, b: f32, t: f32) -> f32 {
    clausters_core::envshape::shape_value(shape, c, a, b, t)
}

/// The value axis for the `n` curve values at `values`, written to `out[0]`
/// (low) and `out[1]` (high). With `hold` non-zero, `out` is read **first** as
/// the axis already in hand and is only widened where the data stopped fitting
/// inside it. Returns 0, or -1 on a null pointer.
///
/// Why a view asks rather than computing the range itself: recomputed on every
/// redraw the range makes an edit rescale the picture, so dragging one point
/// visibly moves every other one. The held form is the answer, and it is one
/// answer for the standalone curve editor, the clip body that draws the same
/// curve, and both clients.
///
/// # Safety
/// `values` must be readable for `n` `f64`s and `out` readable and writable
/// for 2.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_curve_axis(
    values: *const f64,
    n: usize,
    hold: i32,
    out: *mut f64,
) -> i32 {
    if out.is_null() || (values.is_null() && n != 0) {
        return -1;
    }
    // SAFETY: caller guarantees `values` is readable for `n` and `out` for 2.
    let slice = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(values, n) }
    };
    // SAFETY: caller contract.
    let kept = if hold != 0 {
        Some(unsafe { (*out, *out.add(1)) })
    } else {
        None
    };
    let (lo, hi) = clausters_core::envshape::curve_axis(slice, kept);
    // SAFETY: caller contract.
    unsafe {
        *out = lo;
        *out.add(1) = hi;
    }
    0
}
