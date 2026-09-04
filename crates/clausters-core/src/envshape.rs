//! Envelope segment shapes: the interpolation curves of a breakpoint envelope.
//!
//! One segment of a breakpoint envelope runs from a start level `a` to a
//! target `b` over a duration, following one of the SuperCollider shape
//! curves; [`shape_value`] evaluates that interpolation at a normalized
//! position. It is the single source of truth for the segment math: the
//! server's `EnvGen` UGen plays envelopes through it and a client drawing or
//! editing an envelope (the GUI's breakpoint editor) evaluates the very same
//! function — what the editor draws is what the server plays, by construction.
//!
//! Allocation-free, so the audio thread calls it directly.

/// SuperCollider envelope shape numbers, the wire form a segment's `shape`
/// input carries. `CURVE` (the custom-curvature shape) is the only one that
/// reads the segment's `curve` value.
pub const SHAPE_STEP: i32 = 0;
pub const SHAPE_LINEAR: i32 = 1;
pub const SHAPE_EXPONENTIAL: i32 = 2;
pub const SHAPE_SINE: i32 = 3;
pub const SHAPE_WELCH: i32 = 4;
pub const SHAPE_CURVE: i32 = 5;
pub const SHAPE_SQUARED: i32 = 6;
pub const SHAPE_CUBED: i32 = 7;
pub const SHAPE_HOLD: i32 = 8;

/// SuperCollider envelope shape number, `t` in `[0, 1)` the position within the
/// segment, `a` the start level, `b` the target, `c` the curve value (only used
/// by the custom-curvature shape). Returns the interpolated level.
///
/// The endpoints hold: every shape yields `a` at `t == 0` and tends to `b` as
/// `t -> 1`; the exact target is committed by the caller when the segment
/// completes, so `t` never actually reaches 1.
#[inline]
pub fn shape_value(shape: i32, c: f32, a: f32, b: f32, t: f32) -> f32 {
    use core::f32::consts::{FRAC_PI_2, PI};
    match shape {
        // Step: jump to the target immediately and hold it for the duration.
        SHAPE_STEP => b,
        // Hold: stay at the start level; the jump to the target happens when
        // the segment completes.
        SHAPE_HOLD => a,
        // Exponential: equal ratios, which is exactly the map `warp` writes
        // between two levels — including the rule for the levels that have no
        // ratio (a zero endpoint, a sign change). That rule is the server's
        // `XLine`'s too, so it is read from one place rather than restated.
        SHAPE_EXPONENTIAL => crate::warp::exp_value(t, a, b),
        // Sine: equal-power ease in/out (half a cosine).
        SHAPE_SINE => a + (b - a) * (1.0 - (PI * t).cos()) * 0.5,
        // Welch: a quarter sine, concave for a rise and convex for a fall.
        SHAPE_WELCH => {
            if b >= a {
                a + (b - a) * (FRAC_PI_2 * t).sin()
            } else {
                b + (a - b) * (FRAC_PI_2 * (1.0 - t)).sin()
            }
        }
        // Custom curvature: `c` bends the segment (0 == linear, positive builds
        // slowly then fast, negative the reverse).
        SHAPE_CURVE => crate::warp::curve_value(t, a, b, c),
        // Squared / cubed: interpolate the square/cube root linearly, then raise
        // back. Squared clamps to non-negative levels (its root is real only
        // there); cubed uses the sign-preserving real cube root.
        SHAPE_SQUARED => {
            let ra = a.max(0.0).sqrt();
            let rb = b.max(0.0).sqrt();
            let s = ra + t * (rb - ra);
            s * s
        }
        SHAPE_CUBED => {
            let ra = a.cbrt();
            let rb = b.cbrt();
            let s = ra + t * (rb - ra);
            s * s * s
        }
        // Linear (1) and any unknown shape.
        _ => a + t * (b - a),
    }
}

/// The value axis a break-point curve is **drawn** against: the break-points'
/// own range with a tenth of headroom, and a flat curve still gets a band to be
/// dragged in.
///
/// It is the fresh answer, and on its own it is right exactly once — see
/// [`curve_axis`], which is what a view actually asks.
pub fn curve_range(values: &[f64]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &v in values {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if !lo.is_finite() || !hi.is_finite() {
        lo = 0.0;
        hi = 0.0;
    }
    let mut pad = (hi - lo) * 0.1;
    if pad == 0.0 {
        pad = hi.abs() * 0.1;
    }
    if pad == 0.0 {
        pad = 1.0;
    }
    (lo - pad, hi + pad)
}

/// The axis a view keeps for one curve: [`curve_range`] the first time, and
/// afterwards the axis it was **already** drawn against, widened only where the
/// data stopped fitting inside it.
///
/// Recomputing the range on every redraw is what makes an edit rescale the
/// picture — drag one point and every other one visibly moves — so the axis is
/// remembered per curve and only ever **grows**: never narrowed, so a point
/// dragged down and back up leaves the drawing where it was.
///
/// **One side at a time.** Only the end that stopped holding the data moves;
/// taking the union of the two padded ranges would drop the floor as well
/// whenever the ceiling grew, which is the same jump one step removed.
///
/// `kept` is the axis in hand, or `None` for a curve being drawn for the first
/// time. The same rule serves the time axis of a standalone editor and the
/// value axis of a clip's curve body, which is why it lives here rather than in
/// any one view: a rule with two implementations is how one curve comes to be
/// drawn two ways.
pub fn curve_axis(values: &[f64], kept: Option<(f64, f64)>) -> (f64, f64) {
    let (mut lo, mut hi) = curve_range(values);
    if let Some((klo, khi)) = kept {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for &v in values {
            min = min.min(v);
            max = max.max(v);
        }
        if !min.is_finite() {
            min = 0.0;
            max = 0.0;
        }
        if min >= klo {
            lo = klo;
        }
        if max <= khi {
            hi = khi;
        }
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPES: [i32; 9] = [
        SHAPE_STEP,
        SHAPE_LINEAR,
        SHAPE_EXPONENTIAL,
        SHAPE_SINE,
        SHAPE_WELCH,
        SHAPE_CURVE,
        SHAPE_SQUARED,
        SHAPE_CUBED,
        SHAPE_HOLD,
    ];

    #[test]
    fn endpoints_hold_for_every_shape() {
        for shape in SHAPES {
            // Step is the one exception at t == 0: it jumps to `b` at once.
            if shape != SHAPE_STEP {
                let start = shape_value(shape, 2.0, 0.25, 0.75, 0.0);
                assert!(
                    (start - 0.25).abs() < 1e-5,
                    "shape {shape} must start at `a`, got {start}"
                );
            }
            // Step jumps to `b` at once and hold stays at `a`; the others tend
            // to `b` as t -> 1.
            if shape != SHAPE_STEP && shape != SHAPE_HOLD {
                let end = shape_value(shape, 2.0, 0.25, 0.75, 0.9999);
                assert!(
                    (end - 0.75).abs() < 1e-2,
                    "shape {shape} must tend to `b`, got {end}"
                );
            }
        }
    }

    #[test]
    fn step_and_hold_are_the_two_constants() {
        assert_eq!(shape_value(SHAPE_STEP, 0.0, 0.2, 0.8, 0.5), 0.8);
        assert_eq!(shape_value(SHAPE_HOLD, 0.0, 0.2, 0.8, 0.5), 0.2);
    }

    #[test]
    fn linear_is_the_default_for_unknown_shapes() {
        assert_eq!(shape_value(SHAPE_LINEAR, 0.0, 0.0, 1.0, 0.25), 0.25);
        assert_eq!(shape_value(42, 0.0, 0.0, 1.0, 0.25), 0.25);
    }

    #[test]
    fn exponential_is_geometric_and_survives_zero_or_sign_change() {
        // 1 -> 4: the midpoint of a geometric run is 2.
        let mid = shape_value(SHAPE_EXPONENTIAL, 0.0, 1.0, 4.0, 0.5);
        assert!((mid - 2.0).abs() < 1e-5);
        // A zero endpoint is nudged rather than NaN.
        assert!(shape_value(SHAPE_EXPONENTIAL, 0.0, 0.0, 1.0, 0.5).is_finite());
        // A sign change falls back to linear.
        assert_eq!(shape_value(SHAPE_EXPONENTIAL, 0.0, -1.0, 1.0, 0.5), 0.0);
    }

    #[test]
    fn curve_zero_is_linear_and_the_sign_picks_the_bend() {
        assert_eq!(shape_value(SHAPE_CURVE, 0.0, 0.0, 1.0, 0.5), 0.5);
        // Positive curvature builds slowly then fast; negative the reverse.
        assert!(shape_value(SHAPE_CURVE, 4.0, 0.0, 1.0, 0.5) < 0.5);
        assert!(shape_value(SHAPE_CURVE, -4.0, 0.0, 1.0, 0.5) > 0.5);
    }

    #[test]
    fn welch_bends_by_direction_and_squared_clamps_negative() {
        // A rise is concave (above linear); a fall mirrors it.
        assert!(shape_value(SHAPE_WELCH, 0.0, 0.0, 1.0, 0.5) > 0.5);
        assert!(shape_value(SHAPE_WELCH, 0.0, 1.0, 0.0, 0.5) > 0.5);
        // Squared roots only non-negative levels.
        assert_eq!(shape_value(SHAPE_SQUARED, 0.0, -1.0, 1.0, 0.0), 0.0);
        // Cubed keeps the sign through the real cube root.
        let mid = shape_value(SHAPE_CUBED, 0.0, -1.0, 1.0, 0.5);
        assert!(mid.abs() < 1e-6);
    }

    #[test]
    fn a_curves_axis_is_its_range_with_a_tenth_of_headroom() {
        let (lo, hi) = curve_range(&[200.0, 4000.0, 800.0]);
        assert!((lo - (200.0 - 380.0)).abs() < 1e-9);
        assert!((hi - (4000.0 + 380.0)).abs() < 1e-9);
    }

    #[test]
    fn a_flat_curve_still_gets_a_band_to_be_dragged_in() {
        let (lo, hi) = curve_range(&[0.5, 0.5]);
        assert!(hi > lo);
        // And one flat at zero, where a proportional pad would be no pad.
        let (lo, hi) = curve_range(&[0.0, 0.0]);
        assert_eq!((lo, hi), (-1.0, 1.0));
        // No points at all is the same question with no answer in the data.
        let (lo, hi) = curve_range(&[]);
        assert_eq!((lo, hi), (-1.0, 1.0));
    }

    #[test]
    fn a_kept_axis_is_held_while_the_data_fits_inside_it() {
        let kept = curve_axis(&[0.0, 1.0], None);
        // Dragging a point down and back up must not move the drawing.
        assert_eq!(curve_axis(&[0.0, 0.5], Some(kept)), kept);
        assert_eq!(curve_axis(&[0.0, 1.0], Some(kept)), kept);
    }

    #[test]
    fn only_the_end_that_stopped_holding_the_data_moves() {
        let kept = curve_axis(&[0.0, 1.0], None);
        let (lo, hi) = curve_axis(&[0.0, 4.0], Some(kept));
        assert_eq!(lo, kept.0, "the floor still holds the data, so it stays");
        assert!(hi > kept.1, "the ceiling stopped holding it, so it grows");
    }
}
