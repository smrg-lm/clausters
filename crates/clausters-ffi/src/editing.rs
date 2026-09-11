//! The projections an editable structure owes its endpoints.
//!
//! The C half of [`clausters_editing`]. Sizes with a null `out` and fills with
//! a second call, like the rest of the JSON surface here — a projection is a
//! pure read, so a sizing pass changes nothing and can be repeated.

/// The props a break-point curve is drawn with: `{"points": [...], "min": ..,
/// "max": .., "duration": ..}` as JSON, for the `n` flat `t v shape curve`
/// values at `points`.
///
/// With `hold` non-zero, `kept_lo`/`kept_hi` are the value axis the view
/// already has and `held` the time span it already has; both are widened and
/// never narrowed, which is what keeps an edit from rescaling the picture under
/// the hand. `duration` is absent when the curve spans nothing.
///
/// Returns the byte count the answer needs, or 0 for a null `points` with a
/// non-zero `n`.
///
/// # Safety
/// `points` must be null or readable for `n` `f64`s, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_points_props(
    points: *const f64,
    n: usize,
    hold: i32,
    kept_lo: f64,
    kept_hi: f64,
    held: f64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if points.is_null() && n != 0 {
        return 0;
    }
    // SAFETY: caller guarantees `points` is readable for `n`.
    let slice = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(points, n) }
    };
    let kept = (hold != 0).then_some((kept_lo, kept_hi));
    let answer = clausters_editing::points::props_json(slice, kept, held);
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two-call shape: size with a null `out`, then fill.
    #[test]
    fn the_props_size_then_fill() {
        let points = [0.0f64, 0.5, 1.0, 0.0, 2.0, 1.0, 1.0, 0.0];
        let n = unsafe {
            clausters_editing_points_props(
                points.as_ptr(),
                points.len(),
                0,
                0.0,
                0.0,
                0.0,
                std::ptr::null_mut(),
                0,
            )
        };
        assert!(n > 0);
        let mut buf = vec![0u8; n];
        let wrote = unsafe {
            clausters_editing_points_props(
                points.as_ptr(),
                points.len(),
                0,
                0.0,
                0.0,
                0.0,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        assert_eq!(wrote, n);
        let answer: serde_json::Value = serde_json::from_slice(&buf).expect("JSON");
        assert_eq!(answer["duration"], serde_json::json!(2.0));
        assert_eq!(answer["points"].as_array().expect("points").len(), 8);
    }

    /// An axis in hand is read and only widened.
    #[test]
    fn an_axis_in_hand_crosses_the_boundary() {
        let points = [0.0f64, 0.0, 1.0, 0.0, 1.0, 0.5, 1.0, 0.0];
        let mut buf = vec![0u8; 256];
        let n = unsafe {
            clausters_editing_points_props(
                points.as_ptr(),
                points.len(),
                1,
                -4.0,
                4.0,
                9.0,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        let answer: serde_json::Value = serde_json::from_slice(&buf[..n]).expect("JSON");
        assert_eq!(answer["min"], serde_json::json!(-4.0));
        assert_eq!(answer["max"], serde_json::json!(4.0));
        assert_eq!(answer["duration"], serde_json::json!(9.0));
    }
}
