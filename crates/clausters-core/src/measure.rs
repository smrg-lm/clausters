//! Signal measurements and stereo-field geometry shared by the server and the
//! GUI clients.
//!
//! These are general audio tools — not display code — so, by the rule that an
//! algorithm useful to more than one Clausters process lives once in the shared
//! core, they belong here rather than in any single client. Two land here, both
//! read by the GUI's phasescope and both useful well beyond it (a headless
//! Python capture, a future server analysis UGen, an electroacoustic-composition
//! sketch that plots or drives from a stereo field):
//!
//! - [`correlation`] — the stereo **correlation** (Pearson's r of the two
//!   channels), the phase-coherence number a goniometer annotates.
//! - [`lissajous_point`] / [`lissajous_into`] — the **Lissajous / goniometer**
//!   transform: a stereo `(L, R)` pair mapped to the 45°-rotated mid/side plane
//!   the classic goniometer draws. It is the shape an audio engineer or
//!   electroacoustic composer reads a stereo image from, so the geometry lives
//!   here once rather than only inside the GUI's drawing code.
//!
//! `no_std`-friendly and allocation-free; each function is a single pass over
//! the input slices.
//!
//! No FFI export yet: the phasescope computes these host-side (native and wasm
//! both link this crate directly), and no non-Rust client consumes them yet.
//! The export follows the concrete consumer, the way `peaks` grew one only when
//! the Python client needed to build the identical cache.

/// Pearson's correlation coefficient of two equal-length signals, in `[-1, 1]`.
///
/// This is the audio-engineering **stereo correlation** metric: `+1` when the
/// two channels are identical (a mono/in-phase signal), `0` when they are
/// decorrelated (a wide stereo field), `-1` when one is the negation of the
/// other (anti-phase — the mix cancels in mono). It is computed about each
/// channel's own mean, so a DC offset does not bias it.
///
/// Returns `None` when the inputs differ in length, are empty, or either
/// channel is constant over the window (a zero variance makes the coefficient
/// undefined — silence or pure DC, which the caller shows as "no reading"). The
/// result is clamped to `[-1, 1]` against rounding error.
pub fn correlation(x: &[f32], y: &[f32]) -> Option<f32> {
    if x.is_empty() || x.len() != y.len() {
        return None;
    }
    let n = x.len() as f64;
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    for (&a, &b) in x.iter().zip(y) {
        sx += a as f64;
        sy += b as f64;
    }
    let (mx, my) = (sx / n, sy / n);
    let (mut cov, mut vx, mut vy) = (0.0f64, 0.0f64, 0.0f64);
    for (&a, &b) in x.iter().zip(y) {
        let (dx, dy) = (a as f64 - mx, b as f64 - my);
        cov += dx * dy;
        vx += dx * dx;
        vy += dy * dy;
    }
    if vx <= 0.0 || vy <= 0.0 {
        return None; // a constant channel: correlation is undefined
    }
    Some(((cov / (vx * vy).sqrt()) as f32).clamp(-1.0, 1.0))
}

/// The Lissajous / goniometer coordinate of one stereo sample pair.
///
/// The audio-engineering goniometer plots the stereo signal as a Lissajous
/// figure rotated 45° into the **mid/side** plane, so a mono signal reads as a
/// vertical line and an anti-phase one as horizontal:
///
/// - `x` (horizontal) is the **side** component `(L − R) / √2` — the stereo
///   width;
/// - `y` (vertical) is the **mid** component `(L + R) / √2` — the mono sum.
///
/// The `1/√2` keeps the transform an isometry (a hard-panned channel reaches
/// the same distance from the origin as a centered one of equal level), so the
/// figure's shape is read directly. Returned as `[x, y]`.
pub fn lissajous_point(left: f32, right: f32) -> [f32; 2] {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    [(left - right) * INV_SQRT2, (left + right) * INV_SQRT2]
}

/// Maps a block of stereo pairs to their Lissajous / goniometer coordinates.
///
/// `left`, `right` and `out` must have the same length; `out[i]` receives
/// [`lissajous_point`]`(left[i], right[i])` (`[x, y]` = side, mid). Returns
/// `false`, leaving `out` untouched, on a length mismatch. Allocation-free — the
/// caller owns `out` — so a real-time or per-frame caller reuses one buffer.
pub fn lissajous_into(left: &[f32], right: &[f32], out: &mut [[f32; 2]]) -> bool {
    if left.len() != right.len() || out.len() != left.len() {
        return false;
    }
    for (o, (&l, &r)) in out.iter_mut().zip(left.iter().zip(right)) {
        *o = lissajous_point(l, r);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_maps_to_the_vertical_axis() {
        // Identical channels (mono): side is 0, the figure is a vertical line.
        let [x, y] = lissajous_point(0.7, 0.7);
        assert!(x.abs() < 1e-6, "mono has no side component");
        assert!((y - 0.7 * std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn anti_phase_maps_to_the_horizontal_axis() {
        let [x, y] = lissajous_point(0.7, -0.7);
        assert!(y.abs() < 1e-6, "anti-phase has no mid component");
        assert!((x - 0.7 * std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn a_hard_panned_channel_keeps_its_magnitude() {
        // The rotation is an isometry: |[x, y]| == |[L, R]|.
        let panned = lissajous_point(1.0, 0.0);
        let mag = (panned[0] * panned[0] + panned[1] * panned[1]).sqrt();
        assert!((mag - 1.0).abs() < 1e-6, "isometric, got {mag}");
    }

    #[test]
    fn lissajous_into_matches_the_pointwise_form() {
        let l = [0.1f32, -0.4, 0.9];
        let r = [0.2f32, 0.5, -0.3];
        let mut out = [[0.0f32; 2]; 3];
        assert!(lissajous_into(&l, &r, &mut out));
        for i in 0..3 {
            assert_eq!(out[i], lissajous_point(l[i], r[i]));
        }
        // A length mismatch is rejected, out untouched.
        let mut short = [[0.0f32; 2]; 2];
        assert!(!lissajous_into(&l, &r, &mut short));
    }

    #[test]
    fn identical_channels_are_perfectly_correlated() {
        let x: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin()).collect();
        let r = correlation(&x, &x).unwrap();
        assert!((r - 1.0).abs() < 1e-5, "mono reads +1, got {r}");
    }

    #[test]
    fn negated_channel_is_anti_correlated() {
        let x: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin()).collect();
        let neg: Vec<f32> = x.iter().map(|s| -s).collect();
        let r = correlation(&x, &neg).unwrap();
        assert!((r + 1.0).abs() < 1e-5, "anti-phase reads -1, got {r}");
    }

    #[test]
    fn orthogonal_channels_are_uncorrelated() {
        // A sine and a cosine of the same frequency over whole periods are
        // decorrelated: r ~ 0.
        let n = 400;
        let x: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * i as f32 / 100.0).sin())
            .collect();
        let y: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * i as f32 / 100.0).cos())
            .collect();
        let r = correlation(&x, &y).unwrap();
        assert!(r.abs() < 0.05, "quadrature reads ~0, got {r}");
    }

    #[test]
    fn a_dc_offset_does_not_bias_it() {
        let x: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin()).collect();
        let shifted: Vec<f32> = x.iter().map(|s| s + 5.0).collect();
        let r = correlation(&x, &shifted).unwrap();
        assert!((r - 1.0).abs() < 1e-5, "correlation is mean-centered");
    }

    #[test]
    fn degenerate_inputs_return_none() {
        assert_eq!(correlation(&[], &[]), None, "empty");
        assert_eq!(correlation(&[1.0, 2.0], &[1.0]), None, "length mismatch");
        assert_eq!(
            correlation(&[0.7, 0.7, 0.7], &[0.1, 0.2, 0.3]),
            None,
            "a constant channel has undefined correlation"
        );
    }
}
