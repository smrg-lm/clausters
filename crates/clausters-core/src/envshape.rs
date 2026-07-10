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
        // Exponential: needs same-sign, non-zero levels; a crossing through or
        // to zero is undefined, so nudge zeros to a tiny same-signed value and
        // fall back to linear across a sign change.
        SHAPE_EXPONENTIAL => {
            let a = if a.abs() < 1e-5 {
                1e-5_f32.copysign(a)
            } else {
                a
            };
            let b = if b.abs() < 1e-5 {
                1e-5_f32.copysign(b)
            } else {
                b
            };
            if a.signum() == b.signum() {
                a * (b / a).powf(t)
            } else {
                a + t * (b - a)
            }
        }
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
        SHAPE_CURVE => {
            if c.abs() < 0.001 {
                a + t * (b - a)
            } else {
                a + (b - a) * (1.0 - (t * c).exp()) / (1.0 - c.exp())
            }
        }
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
}
