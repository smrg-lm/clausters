//! Panning, the stereo field and selection (U7).
//!
//! Every claim here is checked against the **closed form of the law actually
//! implemented**, never against a recorded buffer and never against scsynth's
//! output: the pan law is `sin`/`cos` of a quarter turn, the matrix rows are
//! two-by-two products, and both are known exactly. So the asserts are
//! measurements with published tolerances — the gain pair holds unit power to
//! `1e-5`, the polynomial tracks `f64::sin` to `2.6e-7` — plus the handful of
//! values that must be *exact* rather than close: a hard pan, an identity, a
//! quarter turn, a round trip through mid/side.
//!
//! **Every kind here is stateless** — a gain pair, a matrix row or an index,
//! computed from this sample's inputs and nothing carried over. So the two
//! tests the `audio-testing` rules ask of a stateful UGen do not apply the way
//! they do to a filter: there is no long run for numerical state to drift
//! through, and the block-split test below is a check that the *code* holds no
//! state rather than a check that its state survives a split. The rest of the
//! suite is the closed forms, plus one pass driving every row with inputs no
//! musician would write, since a wrap or a reciprocal is where a non-finite
//! sample would come from.

#![cfg(feature = "synth")]

use clausters::dsp::pan::{
    Pan, PanAz, PanKind, RotKind, Rotate, Select, SelectKind, quarter_sin, sin_cos_pi,
};
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx, UGen};

const SR: f32 = 48_000.0;

/// Renders one whole block of a UGen with the given input wires.
fn run(ugen: &mut dyn UGen, inputs: &[&[f32]]) -> Vec<f32> {
    run_len(ugen, inputs, BLOCK_SIZE)
}

/// Renders `n` samples — `n < BLOCK_SIZE` is the run a scheduled bundle leaves
/// when it splits a block.
fn run_len(ugen: &mut dyn UGen, inputs: &[&[f32]], n: usize) -> Vec<f32> {
    let buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = vec![0.0f32; n];
    let mut ctx = ProcessCtx {
        sample_rate: SR,
        full_sample_rate: SR,
        buses: &buses,
        buffers: &[],
        offset: 0,
        frames: n,
    };
    ugen.process(&mut ctx, inputs, &mut out);
    out
}

/// The first sample of a block — the whole answer when every wire is scalar.
fn run1(ugen: &mut dyn UGen, inputs: &[&[f32]]) -> f32 {
    run(ugen, inputs)[0]
}

/// Both channels of a two-channel row, by building it twice with the two
/// channel indices — which is what a def does.
fn stereo(kind: PanKind, inputs: &[&[f32]]) -> (f32, f32) {
    let mut left: Vec<&[f32]> = inputs.to_vec();
    let mut right: Vec<&[f32]> = inputs.to_vec();
    left.push(&[0.0]);
    right.push(&[1.0]);
    (
        run1(&mut Pan::new(kind), &left),
        run1(&mut Pan::new(kind), &right),
    )
}

/// Both channels of a matrix row.
fn matrix(kind: RotKind, inputs: &[&[f32]]) -> (f32, f32) {
    let mut left: Vec<&[f32]> = inputs.to_vec();
    let mut right: Vec<&[f32]> = inputs.to_vec();
    left.push(&[0.0]);
    right.push(&[1.0]);
    (
        run1(&mut Rotate::new(kind), &left),
        run1(&mut Rotate::new(kind), &right),
    )
}

/// One sample out of a selector over `sources`.
fn selected(kind: SelectKind, which: f32, sources: &[&[f32]]) -> f32 {
    let w = [which];
    let inputs: Vec<&[f32]> = std::iter::once(&w[..])
        .chain(sources.iter().copied())
        .collect();
    run1(&mut Select::new(kind), &inputs)
}

fn close(a: f32, b: f32, tol: f32, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: {a} vs {b}, off by {}",
        (a - b).abs()
    );
}

// ---- the law itself ----

/// The polynomial is the module's one approximation; this is its published
/// figure. scsynth's rounded 2049-entry table is off by up to 3.8e-4 — this is
/// three orders of magnitude closer, for no table and about ten flops.
///
/// The bound is well inside what the fifth Taylor term alone would give
/// (`3.5e-6`): forcing the coefficients to sum to one, which is what makes
/// `quarter_sin(1)` exact, also cancels most of the truncation error across
/// the range. The centre is not exact — it lands 5e-9 low — but nothing
/// depends on it being so, unlike the two ends.
#[test]
fn quarter_sine_tracks_the_real_one() {
    let mut worst = 0.0f64;
    for i in 0..=100_000 {
        let t = i as f64 / 100_000.0;
        let err = (quarter_sin(t) - (t * std::f64::consts::FRAC_PI_2).sin()).abs();
        worst = worst.max(err);
    }
    assert!(worst < 3e-7, "quarter sine worst error {worst}");
}

/// Both ends are exact, and that is not decoration: they are the gains of a
/// hard-panned source. The near side must be unity and the far side must be
/// silence, not -110 dB of it.
#[test]
fn the_ends_of_the_law_are_exact() {
    assert_eq!(quarter_sin(0.0), 0.0);
    assert_eq!(quarter_sin(1.0), 1.0);
}

/// A quadrant boundary is a sign flip or a swap, so the reduction must land on
/// it exactly: a half turn negates and a full turn is the identity.
#[test]
fn the_quadrants_are_exact() {
    for (p, want) in [
        (0.0, (0.0, 1.0)),
        (0.5, (1.0, 0.0)),
        (1.0, (0.0, -1.0)),
        (1.5, (-1.0, 0.0)),
        (2.0, (0.0, 1.0)),
        (-0.5, (-1.0, 0.0)),
        (-1.0, (0.0, -1.0)),
    ] {
        assert_eq!(sin_cos_pi(p), want, "sin_cos_pi({p})");
    }
}

#[test]
fn sin_cos_pi_tracks_the_real_ones_over_many_turns() {
    let mut worst = 0.0f64;
    for i in -20_000..=20_000 {
        let p = i as f64 / 1_000.0; // -20 .. 20 turns
        let (s, c) = sin_cos_pi(p);
        let angle = p * std::f64::consts::PI;
        worst = worst
            .max((s - angle.sin()).abs())
            .max((c - angle.cos()).abs());
    }
    assert!(worst < 3e-7, "sin_cos_pi worst error {worst}");
}

// ---- Pan2 / LinPan2 ----

#[test]
fn pan2_holds_unit_power_across_the_field() {
    let mut worst = 0.0f32;
    for i in 0..=2_000 {
        let pos = i as f32 / 1_000.0 - 1.0;
        let (l, r) = stereo(PanKind::Pan2, &[&[1.0], &[pos], &[1.0]]);
        worst = worst.max((l * l + r * r - 1.0).abs());
    }
    assert!(worst < 1e-5, "worst power deviation {worst}");
}

/// The gain pair is one function read from both ends, so this holds
/// **bit for bit** rather than to a tolerance.
#[test]
fn pan2_is_exactly_symmetric() {
    for i in 0..=1_000 {
        let pos = i as f32 / 1_000.0;
        let (l, r) = stereo(PanKind::Pan2, &[&[1.0], &[pos], &[1.0]]);
        let (ml, mr) = stereo(PanKind::Pan2, &[&[1.0], &[-pos], &[1.0]]);
        assert_eq!((l, r), (mr, ml), "pos {pos}");
    }
}

#[test]
fn pan2_hard_pans_exactly_and_centres_at_minus_three_db() {
    assert_eq!(
        stereo(PanKind::Pan2, &[&[1.0], &[-1.0], &[1.0]]),
        (1.0, 0.0)
    );
    assert_eq!(stereo(PanKind::Pan2, &[&[1.0], &[1.0], &[1.0]]), (0.0, 1.0));
    let (l, r) = stereo(PanKind::Pan2, &[&[1.0], &[0.0], &[1.0]]);
    close(l, std::f32::consts::FRAC_1_SQRT_2, 1e-6, "centre left");
    close(r, std::f32::consts::FRAC_1_SQRT_2, 1e-6, "centre right");
}

/// Out of range the position clamps rather than wrapping — a modulator that
/// overshoots stays hard panned instead of jumping to the other side.
#[test]
fn pan2_clamps_out_of_range() {
    assert_eq!(
        stereo(PanKind::Pan2, &[&[1.0], &[-4.0], &[1.0]]),
        (1.0, 0.0)
    );
    assert_eq!(stereo(PanKind::Pan2, &[&[1.0], &[9.0], &[1.0]]), (0.0, 1.0));
}

#[test]
fn pan2_level_scales_both_channels() {
    let (l, r) = stereo(PanKind::Pan2, &[&[1.0], &[0.3], &[0.25]]);
    let (fl, fr) = stereo(PanKind::Pan2, &[&[1.0], &[0.3], &[1.0]]);
    close(l, fl * 0.25, 1e-7, "left");
    close(r, fr * 0.25, 1e-7, "right");
}

/// The other law: the two gains sum to the level at every position, which is
/// what keeps a mono fold-down at one amplitude — at the price of a 3 dB dip
/// in the middle for anything summing by power.
#[test]
fn lin_pan2_holds_constant_amplitude() {
    for i in 0..=2_000 {
        let pos = i as f32 / 1_000.0 - 1.0;
        let (l, r) = stereo(PanKind::LinPan2, &[&[1.0], &[pos], &[1.0]]);
        close(l + r, 1.0, 1e-6, "amplitude sum");
    }
    assert_eq!(
        stereo(PanKind::LinPan2, &[&[1.0], &[0.0], &[1.0]]),
        (0.5, 0.5)
    );
}

// ---- Balance2 ----

/// Balance2 applies the *pan* law to a pair that is already stereo, so at the
/// centre both sides come back at 0.707. That is scsynth's behaviour and the
/// one thing about this row that surprises people: passing a stereo signal
/// through a centred Balance2 costs 3 dB.
#[test]
fn balance2_attenuates_by_three_db_at_the_centre() {
    let (l, r) = stereo(PanKind::Balance2, &[&[1.0], &[1.0], &[0.0], &[1.0]]);
    close(l, std::f32::consts::FRAC_1_SQRT_2, 1e-6, "left");
    close(r, std::f32::consts::FRAC_1_SQRT_2, 1e-6, "right");
}

#[test]
fn balance2_keeps_the_channels_apart() {
    // Hard left: the left input passes untouched, the right one is gone —
    // and it is the *right input* that is gone, not the right channel of a
    // mono source.
    assert_eq!(
        stereo(PanKind::Balance2, &[&[0.5], &[0.9], &[-1.0], &[1.0]]),
        (0.5, 0.0)
    );
    assert_eq!(
        stereo(PanKind::Balance2, &[&[0.5], &[0.9], &[1.0], &[1.0]]),
        (0.0, 0.9)
    );
}

// ---- the crossfades ----

#[test]
fn xfade2_is_equal_power_between_its_ends() {
    let a = run1(
        &mut Pan::new(PanKind::XFade2),
        &[&[1.0], &[0.0], &[-1.0], &[1.0]],
    );
    let b = run1(
        &mut Pan::new(PanKind::XFade2),
        &[&[0.0], &[1.0], &[1.0], &[1.0]],
    );
    assert_eq!((a, b), (1.0, 1.0));

    // Uncorrelated sources keep unit power across the fade; correlated ones
    // add up to +3 dB in the middle, which is the same fact seen from the
    // other side.
    for i in 0..=200 {
        let pan = i as f32 / 100.0 - 1.0;
        let ga = run1(
            &mut Pan::new(PanKind::XFade2),
            &[&[1.0], &[0.0], &[pan], &[1.0]],
        );
        let gb = run1(
            &mut Pan::new(PanKind::XFade2),
            &[&[0.0], &[1.0], &[pan], &[1.0]],
        );
        close(ga * ga + gb * gb, 1.0, 1e-5, "crossfade power");
    }
    let both = run1(
        &mut Pan::new(PanKind::XFade2),
        &[&[1.0], &[1.0], &[0.0], &[1.0]],
    );
    close(both, std::f32::consts::SQRT_2, 1e-6, "correlated centre");
}

#[test]
fn lin_xfade2_is_a_plain_interpolation() {
    for (pan, want) in [(-1.0, 1.0), (0.0, 0.5), (1.0, 0.0), (0.5, 0.25)] {
        let g = run1(
            &mut Pan::new(PanKind::LinXFade2),
            &[&[1.0], &[0.0], &[pan], &[1.0]],
        );
        close(g, want, 1e-6, "lin crossfade");
    }
}

// ---- the stereo-field matrix ----

#[test]
fn rotate2_at_rest_is_the_identity() {
    assert_eq!(
        matrix(RotKind::Rotate2, &[&[0.3], &[-0.7], &[0.0]]),
        (0.3, -0.7)
    );
}

/// A quarter turn *is* the mid/side basis change — the fact the whole
/// `MidSide` row exists to name.
#[test]
fn rotate2_at_a_quarter_turn_is_mid_side() {
    let (l, r) = (0.6f32, -0.2f32);
    let (x, y) = matrix(RotKind::Rotate2, &[&[l], &[r], &[0.25]]);
    let k = std::f32::consts::FRAC_1_SQRT_2;
    close(x, (l + r) * k, 1e-6, "mid");
    close(y, (r - l) * k, 1e-6, "side");
}

#[test]
fn rotate2_half_and_full_turns_are_exact() {
    let (x, y) = (0.3f32, -0.7f32);
    assert_eq!(matrix(RotKind::Rotate2, &[&[x], &[y], &[0.5]]), (y, -x));
    assert_eq!(matrix(RotKind::Rotate2, &[&[x], &[y], &[1.0]]), (-x, -y));
    assert_eq!(matrix(RotKind::Rotate2, &[&[x], &[y], &[2.0]]), (x, y));
}

/// A rotation moves the image without changing its size: the total power is
/// invariant at every angle. This is what "equal power rotation" means and
/// what separates it from width.
#[test]
fn rotate2_preserves_power_at_every_angle() {
    let (x, y) = (0.6f32, -0.35f32);
    let before = x * x + y * y;
    for i in 0..=400 {
        let pos = i as f32 / 100.0 - 2.0;
        let (a, b) = matrix(RotKind::Rotate2, &[&[x], &[y], &[pos]]);
        close(a * a + b * b, before, 1e-5, "rotated power");
    }
}

/// Normalized to `1/sqrt(2)` the matrix is an involution, so one row both
/// encodes and decodes. The round trip is exact to `f32`, not merely close.
#[test]
fn mid_side_is_its_own_inverse() {
    for (l, r) in [(0.6f32, -0.2f32), (1.0, 1.0), (-0.75, 0.75), (0.0, 0.3)] {
        let (m, s) = matrix(RotKind::MidSide, &[&[l], &[r]]);
        let (back_l, back_r) = matrix(RotKind::MidSide, &[&[m], &[s]]);
        close(back_l, l, 1e-6, "left round trip");
        close(back_r, r, 1e-6, "right round trip");
    }
}

#[test]
fn mid_side_puts_a_mono_signal_entirely_in_the_mid() {
    let (m, s) = matrix(RotKind::MidSide, &[&[0.4], &[0.4]]);
    close(m, 0.8 * std::f32::consts::FRAC_1_SQRT_2, 1e-6, "mid");
    assert_eq!(s, 0.0, "a mono pair has no side at all");
}

#[test]
fn stereo_width_spans_mono_to_wide() {
    let (l, r) = (0.8f32, 0.2f32);
    // Unity is the identity, exactly: the coefficients are 1 and 0, not
    // 0.99999 and 1e-8.
    assert_eq!(matrix(RotKind::Width, &[&[l], &[r], &[1.0]]), (l, r));
    // Zero collapses to the mid in both channels.
    let (a, b) = matrix(RotKind::Width, &[&[l], &[r], &[0.0]]);
    close(a, 0.5, 1e-6, "mono left");
    close(b, 0.5, 1e-6, "mono right");
    // Two is the textbook widening.
    let (a, b) = matrix(RotKind::Width, &[&[l], &[r], &[2.0]]);
    close(a, 1.5 * l - 0.5 * r, 1e-6, "wide left");
    close(b, 1.5 * r - 0.5 * l, 1e-6, "wide right");
    // Negative width swaps the sides, which is what scaling the side axis
    // past zero has to mean.
    let (a, b) = matrix(RotKind::Width, &[&[l], &[r], &[-1.0]]);
    close(a, r, 1e-6, "swapped left");
    close(b, l, 1e-6, "swapped right");
}

/// The composability claim the two rows are documented with: going through
/// `MidSide`, scaling the side and coming back is the same thing
/// `StereoWidth` does in one step.
#[test]
fn width_equals_the_mid_side_round_trip_with_a_scaled_side() {
    let (l, r) = (0.65f32, -0.15f32);
    for w in [0.0f32, 0.5, 1.0, 1.7, 3.0] {
        let (m, s) = matrix(RotKind::MidSide, &[&[l], &[r]]);
        let (a, b) = matrix(RotKind::MidSide, &[&[m], &[s * w]]);
        let (wa, wb) = matrix(RotKind::Width, &[&[l], &[r], &[w]]);
        close(a, wa, 1e-6, "left");
        close(b, wb, 1e-6, "right");
    }
}

// ---- PanAz ----

/// One instance per channel of a ring, each computing only its own gain.
fn ring(pos: f32, chans: usize, width: f32, orientation: f32) -> Vec<f32> {
    (0..chans)
        .map(|c| {
            run1(
                &mut PanAz,
                &[
                    &[1.0],
                    &[pos],
                    &[1.0],
                    &[width],
                    &[orientation],
                    &[chans as f32],
                    &[c as f32],
                ],
            )
        })
        .collect()
}

/// The default width of two makes neighbouring lobes a sine and a cosine of
/// the same angle, so the ring holds unit power wherever the source is —
/// including across the seam where the position wraps.
#[test]
fn pan_az_holds_unit_power_around_the_ring() {
    for chans in [2usize, 3, 4, 6, 8] {
        let mut worst = 0.0f32;
        for i in 0..=1_000 {
            let pos = i as f32 / 500.0 - 1.0;
            let power: f32 = ring(pos, chans, 2.0, 0.5).iter().map(|g| g * g).sum();
            worst = worst.max((power - 1.0).abs());
        }
        assert!(worst < 1e-5, "{chans} channels: worst deviation {worst}");
    }
}

/// With the ring's origin on channel 0, a source parked there is exactly unity
/// in it and exactly silent in the two channels a quarter of the ring away.
#[test]
fn pan_az_parks_a_source_on_a_channel() {
    let g = ring(0.0, 4, 2.0, 0.0);
    close(g[0], 1.0, 1e-6, "channel 0");
    assert_eq!(
        (g[1], g[3]),
        (0.0, 0.0),
        "the neighbours are outside the lobe"
    );
    assert_eq!(g[2], 0.0, "the opposite channel is silent");
    // Halfway between two channels both carry equal power.
    let g = ring(0.25, 4, 2.0, 0.0);
    close(g[0], std::f32::consts::FRAC_1_SQRT_2, 1e-6, "channel 0");
    close(g[1], std::f32::consts::FRAC_1_SQRT_2, 1e-6, "channel 1");
}

/// The position spans the whole ring over `[-1, 1]`, so the two ends are the
/// same place.
#[test]
fn pan_az_wraps_at_the_ends() {
    let a = ring(-1.0, 5, 2.0, 0.0);
    let b = ring(1.0, 5, 2.0, 0.0);
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        close(*x, *y, 1e-6, &format!("channel {i}"));
    }
}

/// A width below the channel spacing leaves gaps: between two speakers the
/// source is only in the nearer one, and at the exact midpoint of a
/// width-one ring of four it is in neither.
#[test]
fn pan_az_narrow_width_leaves_gaps() {
    let g = ring(0.25, 4, 1.0, 0.0);
    assert_eq!(g.iter().filter(|x| **x > 0.0).count(), 0, "{g:?}");
    let g = ring(0.1, 4, 1.0, 0.0);
    assert!(g[0] > 0.0 && g[1] == 0.0, "{g:?}");
}

/// Orientation rotates the ring itself. Half a channel is what an even ring
/// wants (scsynth's default), and it puts the origin between two speakers.
#[test]
fn pan_az_orientation_turns_the_ring() {
    let g = ring(0.0, 4, 2.0, 0.5);
    close(g[0], std::f32::consts::FRAC_1_SQRT_2, 1e-6, "channel 0");
    close(g[1], std::f32::consts::FRAC_1_SQRT_2, 1e-6, "channel 1");
    assert_eq!((g[2], g[3]), (0.0, 0.0));
}

// ---- Select / SelectX ----

#[test]
fn select_picks_by_a_truncated_index() {
    let sources: [&[f32]; 3] = [&[10.0], &[20.0], &[30.0]];
    for (which, want) in [
        (0.0, 10.0),
        (0.9, 10.0),
        (1.0, 20.0),
        (1.999, 20.0),
        (2.0, 30.0),
    ] {
        assert_eq!(
            selected(SelectKind::Pick, which, &sources),
            want,
            "which {which}"
        );
    }
}

/// Off either end the index holds the source at that end. scsynth clips
/// rather than wrapping, and a ported def must not change value.
#[test]
fn select_clamps_off_both_ends() {
    let sources: [&[f32]; 3] = [&[10.0], &[20.0], &[30.0]];
    for (which, want) in [(-5.0, 10.0), (-0.5, 10.0), (3.0, 30.0), (99.0, 30.0)] {
        assert_eq!(
            selected(SelectKind::Pick, which, &sources),
            want,
            "which {which}"
        );
    }
}

/// An audio-rate index switches per sample, not per block — the difference
/// between a selector and a block-rate gate.
#[test]
fn select_switches_within_a_block() {
    let which: Vec<f32> = (0..BLOCK_SIZE).map(|i| (i / 16) as f32).collect();
    let sources: [&[f32]; 3] = [&[10.0], &[20.0], &[30.0]];
    let inputs: Vec<&[f32]> = std::iter::once(&which[..])
        .chain(sources.iter().copied())
        .collect();
    let out = run(&mut Select::new(SelectKind::Pick), &inputs);
    assert_eq!(out[0], 10.0);
    assert_eq!(out[15], 10.0);
    assert_eq!(out[16], 20.0);
    assert_eq!(out[32], 30.0);
    assert_eq!(out[63], 30.0, "past the last source it holds it");
}

/// The crossfading selector is the pan law applied between two neighbours:
/// the same unit-power property, now along an array.
#[test]
fn select_x_crossfades_with_unit_power() {
    let mut worst = 0.0f32;
    for i in 0..=400 {
        let which = i as f32 / 100.0; // 0 .. 4 over five sources
        let mut power = 0.0;
        for source in 0..5usize {
            let ones: Vec<&[f32]> = (0..5)
                .map(|s| {
                    if s == source {
                        &[1.0f32][..]
                    } else {
                        &[0.0f32][..]
                    }
                })
                .collect();
            let g = selected(SelectKind::Cross, which, &ones);
            power += g * g;
        }
        worst = worst.max((power - 1.0).abs());
    }
    assert!(worst < 1e-5, "worst deviation {worst}");
}

/// A whole index lands on its source exactly — no residue of the neighbour.
#[test]
fn select_x_is_exact_on_whole_indices() {
    let sources: [&[f32]; 3] = [&[10.0], &[20.0], &[30.0]];
    for (which, want) in [(0.0, 10.0), (1.0, 20.0), (2.0, 30.0), (7.0, 30.0)] {
        assert_eq!(
            selected(SelectKind::Cross, which, &sources),
            want,
            "which {which}"
        );
    }
}

/// Halfway between two sources both arrive at 0.707 — the same 3 dB rise for
/// correlated material that `XFade2` has, because it is the same law.
#[test]
fn select_x_midpoint_is_the_equal_power_pair() {
    let sources: [&[f32]; 2] = [&[1.0], &[1.0]];
    close(
        selected(SelectKind::Cross, 0.5, &sources),
        std::f32::consts::SQRT_2,
        1e-6,
        "midpoint",
    );
}

// ---- the rate stance ----

/// The one place this track's block-rate rule is deliberately not applied: an
/// audio-rate position is evaluated **per sample**. Interpolating the two gains
/// across the block instead would put 0.5 where the law wants 0.707 — a 3 dB
/// hole in the middle of every block a fast pan sweeps.
#[test]
fn an_audio_rate_position_is_evaluated_per_sample() {
    // A full sweep inside one block: the worst case for interpolation.
    let pos: Vec<f32> = (0..BLOCK_SIZE)
        .map(|i| i as f32 / (BLOCK_SIZE - 1) as f32 * 2.0 - 1.0)
        .collect();
    let sig = vec![1.0f32; BLOCK_SIZE];
    let mut left = Pan::new(PanKind::Pan2);
    let out = run(&mut left, &[&sig, &pos, &[1.0], &[0.0]]);

    for (i, got) in out.iter().enumerate() {
        let t = (pos[i] as f64 * 0.5 + 0.5).clamp(0.0, 1.0);
        let want = quarter_sin(1.0 - t) as f32;
        close(*got, want, 1e-7, &format!("sample {i}"));
    }
    // And the midpoint really is the value a ramp would have missed.
    close(
        out[BLOCK_SIZE / 2],
        std::f32::consts::FRAC_1_SQRT_2,
        2e-2,
        "mid-block",
    );
}

/// A scalar position takes the once-per-block path; it must give the same
/// answer as the per-sample one, not a cheaper approximation of it.
#[test]
fn the_block_path_and_the_per_sample_path_agree() {
    for i in 0..=100 {
        let pos = i as f32 / 50.0 - 1.0;
        let scalar = stereo(PanKind::Pan2, &[&[1.0], &[pos], &[1.0]]);
        let block = vec![pos; BLOCK_SIZE];
        let sig = vec![1.0f32; BLOCK_SIZE];
        let l = run(
            &mut Pan::new(PanKind::Pan2),
            &[&sig, &block, &[1.0], &[0.0]],
        )[0];
        let r = run(
            &mut Pan::new(PanKind::Pan2),
            &[&sig, &block, &[1.0], &[1.0]],
        )[0];
        assert_eq!(scalar, (l, r), "pos {pos}");
    }
}

/// sclang builds `SelectX` out of two `Select`s and an `XFade2`, over an index
/// ping-pong (`which.round(2)`, `which.trunc(2) + 1`) and a folded pan. This
/// row is one state-free computation instead — so the equivalence is asserted
/// point by point rather than assumed from having copied the construction.
#[test]
fn select_x_agrees_with_the_sclang_construction() {
    /// sclang's `fold2(x, 1)`: fold into [-1, 1] rather than clip.
    fn fold2(x: f64) -> f64 {
        let y = (x + 1.0).rem_euclid(4.0);
        if y <= 2.0 { y - 1.0 } else { 3.0 - y }
    }
    let values = [11.0f64, 22.0, 33.0, 44.0];
    let sources: [&[f32]; 4] = [&[11.0], &[22.0], &[33.0], &[44.0]];
    let last = (values.len() - 1) as f64;

    for i in 0..=600 {
        let which = i as f64 / 200.0; // 0 .. 3, the whole index range
        // The sclang route: two clipped Selects and an equal-power crossfade.
        let a = values[((which / 2.0).round() * 2.0).clamp(0.0, last) as usize];
        let b = values[((which / 2.0).trunc() * 2.0 + 1.0).clamp(0.0, last) as usize];
        let t = (fold2(which * 2.0 - 1.0) * 0.5 + 0.5).clamp(0.0, 1.0);
        let want = a * quarter_sin(1.0 - t) + b * quarter_sin(t);

        let got = selected(SelectKind::Cross, which as f32, &sources);
        close(got, want as f32, 1e-5, &format!("which {which}"));
    }
}

/// Off the ends the two part company, on purpose. sclang's construction folds
/// the crossfade position while clipping the two picks, so a negative index
/// comes out as a **mix of the first two** sources and an index past the end as
/// the last source at 1.414 — 3 dB of gain from crossfading it with itself.
/// Here the index simply clamps, like `Select`'s.
#[test]
fn select_x_clamps_off_the_ends_rather_than_folding() {
    let sources: [&[f32]; 3] = [&[10.0], &[20.0], &[30.0]];
    for which in [-4.0f32, -1.0, -0.5, -0.001] {
        assert_eq!(
            selected(SelectKind::Cross, which, &sources),
            10.0,
            "which {which}"
        );
    }
    for which in [2.001f32, 3.5, 40.0] {
        assert_eq!(
            selected(SelectKind::Cross, which, &sources),
            30.0,
            "which {which}"
        );
    }
}

// ---- the two structural checks ----

/// Rendering a block whole and rendering it in two runs must give the same
/// samples. A synth's wires are sliced **run-relative** — every input and the
/// output start at index 0 of the current run, only bus reads carry the block
/// offset — so a UGen that indexes its inputs per sample is correct under a
/// split exactly when it holds no state across calls. This is what says so.
#[test]
fn a_split_block_renders_the_same_samples() {
    let half = BLOCK_SIZE / 2;
    let sig: Vec<f32> = (0..BLOCK_SIZE).map(|i| (i as f32 * 0.17).sin()).collect();
    let pos: Vec<f32> = (0..BLOCK_SIZE)
        .map(|i| i as f32 / (BLOCK_SIZE - 1) as f32 * 2.0 - 1.0)
        .collect();

    for chan in [0.0f32, 1.0] {
        let whole = run(&mut Pan::new(PanKind::Pan2), &[&sig, &pos, &[1.0], &[chan]]);
        let mut split = Vec::with_capacity(BLOCK_SIZE);
        for part in [0..half, half..BLOCK_SIZE] {
            split.extend(run_len(
                &mut Pan::new(PanKind::Pan2),
                &[&sig[part.clone()], &pos[part], &[1.0], &[chan]],
                half,
            ));
        }
        assert_eq!(whole, split, "channel {chan}");
    }

    // The same for the matrix and the selector, whose parameters are also read
    // per sample.
    let whole = run(
        &mut Rotate::new(RotKind::Rotate2),
        &[&sig, &pos, &pos, &[1.0]],
    );
    let mut split = Vec::with_capacity(BLOCK_SIZE);
    for part in [0..half, half..BLOCK_SIZE] {
        split.extend(run_len(
            &mut Rotate::new(RotKind::Rotate2),
            &[&sig[part.clone()], &pos[part.clone()], &pos[part], &[1.0]],
            half,
        ));
    }
    assert_eq!(whole, split, "Rotate2");
}

/// Every row, driven with inputs no musician would write: positions far out of
/// range, a ring with a zero and a negative width, an index past both ends, an
/// angle of a thousand turns. Nothing here may produce a NaN or an infinity —
/// one non-finite sample poisons every node downstream of it on the bus, and
/// the wrap in `PanAz` and the reciprocal behind its width are exactly where
/// one would come from.
#[test]
fn no_input_produces_a_non_finite_sample() {
    let wild: Vec<f32> = (0..BLOCK_SIZE)
        .map(|i| match i % 8 {
            0 => 0.0,
            1 => -1e30,
            2 => 1e30,
            3 => -400.0,
            4 => 400.0,
            5 => 1e-30,
            6 => -1.0,
            _ => 1.0,
        })
        .collect();
    let sig = vec![0.5f32; BLOCK_SIZE];
    let check = |what: &str, out: Vec<f32>| {
        for (i, s) in out.iter().enumerate() {
            assert!(s.is_finite(), "{what}: sample {i} is {s}");
        }
    };

    for kind in [
        PanKind::Pan2,
        PanKind::LinPan2,
        PanKind::Balance2,
        PanKind::XFade2,
        PanKind::LinXFade2,
    ] {
        let mut inputs: Vec<&[f32]> = vec![&sig];
        if !matches!(kind, PanKind::Pan2 | PanKind::LinPan2) {
            inputs.push(&sig);
        }
        inputs.push(&wild); // position
        inputs.push(&wild); // level
        inputs.push(&[1.0]); // channel
        check(&format!("{kind:?}"), run(&mut Pan::new(kind), &inputs));
    }

    for kind in [RotKind::Rotate2, RotKind::MidSide, RotKind::Width] {
        let mut inputs: Vec<&[f32]> = vec![&sig, &sig];
        if kind != RotKind::MidSide {
            inputs.push(&wild);
        }
        inputs.push(&[1.0]);
        check(&format!("{kind:?}"), run(&mut Rotate::new(kind), &inputs));
    }

    // The ring: a zero width would divide by zero, a negative one would invert
    // the wrap, and a huge position must still land somewhere on the ring.
    for width in [&[0.0f32][..], &[-2.0][..], &[1e30][..], &wild] {
        for chan in 0..4u32 {
            check(
                &format!("PanAz width {width:?} chan {chan}"),
                run(
                    &mut PanAz,
                    &[
                        &sig,
                        &wild,
                        &[1.0],
                        width,
                        &wild,
                        &[4.0],
                        &[chan as f32][..],
                    ],
                ),
            );
        }
    }

    let sources: [&[f32]; 3] = [&sig, &sig, &sig];
    for kind in [SelectKind::Pick, SelectKind::Cross] {
        let inputs: Vec<&[f32]> = std::iter::once(&wild[..])
            .chain(sources.iter().copied())
            .collect();
        check(&format!("{kind:?}"), run(&mut Select::new(kind), &inputs));
    }
}
