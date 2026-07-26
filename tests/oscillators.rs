//! The phase family (U1): `Saw`, `Pulse`, the `LF*` shapes and `Phasor`.
//!
//! The asserts are measurements, per the rules in the `audio-testing` skill:
//! frequency from the rendered signal, amplitude from its extremes, and — for
//! the two band-limited kinds — an **alias SNR** compared against the naive
//! (non-band-limited) waveform of the same shape at the same fundamental. The
//! baseline is regenerated inside the test rather than hardcoded, so the claim
//! is "PolyBLEP beats naive by this much here", not "this number was true once".
//!
//! Which kinds those are is the one thing to keep straight while reading: only
//! `Saw` and `Pulse` are band-limited. `LFSaw`, `LFPulse`, `LFTri` **and
//! `VarSaw`** are all one `Lf` core and are naive on purpose — modulation
//! sources with exact corners — so an alias figure is reported for them and
//! never asserted. What they are held to is their shape and their frequency.
//!
//! Rule 5, the block split, is not here: it is the same test for every row and
//! runs from the shared table over all seven at once (`tests/subjects.rs`).

#![cfg(feature = "synth")]

#[path = "common/bench.rs"]
mod bench;
#[path = "common/signal.rs"]
mod signal;

use bench::{SR, render};
use signal::*;

/// The analysis window every spectral assert here uses; `coherent_freq` and
/// `alias_snr_db` are both tied to it.
const N: usize = 4096;

fn saw(freq: f32, n: usize) -> Vec<f32> {
    render(
        &format!(r#"{{"kind": "Saw", "inputs": [{{"const": {freq}}}]}}"#),
        n,
    )
}

fn pulse(freq: f32, width: f32, n: usize) -> Vec<f32> {
    render(
        &format!(r#"{{"kind": "Pulse", "inputs": [{{"const": {freq}}}, {{"const": {width}}}]}}"#),
        n,
    )
}

/// `LFSaw`/`LFTri` have no duty cycle and therefore only two inputs; the helper
/// follows the same rule the registry declares.
fn lf(kind: &str, freq: f32, iphase: f32, width: f32, n: usize) -> Vec<f32> {
    let w = match kind {
        "LFPulse" | "VarSaw" => format!(", {{\"const\": {width}}}"),
        _ => String::new(),
    };
    render(
        &format!(
            r#"{{"kind": "{kind}", "inputs": [{{"const": {freq}}},
                 {{"const": {iphase}}}{w}]}}"#
        ),
        n,
    )
}

/// The same waveform without band limiting — the baseline each PolyBLEP claim
/// is measured against, generated here so the comparison is like for like.
fn naive_saw(freq: f32, n: usize) -> Vec<f32> {
    let dt = freq as f64 / SR as f64;
    let mut p = 0.5f64;
    (0..n)
        .map(|_| {
            let v = 2.0 * p - 1.0;
            p += dt;
            if p >= 1.0 {
                p -= 1.0;
            }
            v as f32
        })
        .collect()
}

fn naive_pulse(freq: f32, width: f64, n: usize) -> Vec<f32> {
    let dt = freq as f64 / SR as f64;
    let mut p = 0.0f64;
    (0..n)
        .map(|_| {
            let v = if p < width { 1.0f32 } else { -1.0 };
            p += dt;
            if p >= 1.0 {
                p -= 1.0;
            }
            v
        })
        .collect()
}

/// Fundamental frequency from the rendered signal, by locating the strongest
/// harmonic of the analysis grid — independent of whatever we asked for.
fn measured_f0(x: &[f32]) -> f32 {
    let n = x.len();
    let mut mags = vec![0.0f32; n / 2];
    assert!(clausters_core::fft::rfft_magnitudes_into(x, &mut mags));
    let (b, _) = mags
        .iter()
        .enumerate()
        .skip(2)
        .fold(
            (0usize, 0.0f32),
            |acc, (i, &m)| {
                if m > acc.1 { (i, m) } else { acc }
            },
        );
    b as f32 * SR / n as f32
}

// ---- Saw ----

#[test]
fn saw_has_the_right_frequency_amplitude_and_no_dc() {
    let f = coherent_freq(220.0, SR, N);
    let x = saw(f, N);
    assert_finite(&x, "Saw");
    assert!((measured_f0(&x) - f).abs() < SR / N as f32);
    // A full-range ramp: the extremes approach +/-1 without exceeding it by
    // more than the BLEP's own overshoot at the corner.
    assert!(peak(&x) <= 1.05, "peak {}", peak(&x));
    assert!(peak(&x) > 0.95, "peak {}", peak(&x));
    // RMS of a unit ramp is 1/sqrt(3).
    assert!(
        (rms(&x) - 1.0 / 3.0f32.sqrt()).abs() < 0.02,
        "rms {}",
        rms(&x)
    );
    // scsynth's leaky integrator leaves a residual offset here; an accumulator
    // has none, and this is the assert that says so.
    assert!(dc(&x).abs() < 2e-3, "dc {}", dc(&x));
}

#[test]
fn saw_starts_at_zero_rather_than_at_the_bottom_of_its_range() {
    // A saw beginning at -1 injects a step into every voice it starts.
    let x = saw(110.0, 8);
    assert!(x[0].abs() < 1e-6, "first sample {}", x[0]);
}

#[test]
fn saw_aliases_far_less_than_the_naive_ramp() {
    // Both an absolute floor and the gain over the naive ramp, each set a few
    // dB below what `report_the_measured_alias_figures` prints — so the test
    // catches a regression without pretending to a precision it cannot hold
    // across libm versions. The floor falls with the fundamental because a
    // fourth-order PolyBLEP is still quasi-band-limited: its residual grows
    // with the fundamental. At 105 Hz the figure is within ~2.5 dB of the
    // harness's own floor (a pure tone measures 99.2 dB), so that bound is
    // really "as clean as this analysis can see".
    for (target, min_abs_db, min_gain_db) in [
        (105.0, 90.0, 55.0),
        (996.0, 38.0, 22.0),
        (3996.0, 34.0, 25.0),
    ] {
        let f = coherent_freq(target, SR, N);
        let ours = alias_snr_db(&saw(f, N), f, SR);
        let base = alias_snr_db(&naive_saw(f, N), f, SR);
        assert!(
            ours >= min_abs_db,
            "Saw at {f:.1} Hz: {ours:.1} dB, wanted at least {min_abs_db}"
        );
        assert!(
            ours - base >= min_gain_db,
            "Saw at {f:.1} Hz: {ours:.1} dB vs naive {base:.1} dB \
             (gain {:.1} dB, wanted at least {min_gain_db})",
            ours - base
        );
    }
}

#[test]
fn saw_survives_a_negative_frequency() {
    // Running the phase backwards must flip the discontinuity's correction, not
    // disable it: a reversed saw is still band-limited.
    let f = coherent_freq(996.0, SR, N);
    let x = saw(-f, N);
    assert_finite(&x, "reversed Saw");
    let ours = alias_snr_db(&x, f, SR);
    let base = alias_snr_db(&naive_saw(f, N), f, SR);
    assert!(ours > base + 22.0, "reversed saw {ours:.1} vs {base:.1} dB");
}

// ---- Pulse ----

#[test]
fn pulse_is_bipolar_and_its_width_sets_the_duty_cycle() {
    let f = coherent_freq(220.0, SR, N);
    for (width, want_dc) in [(0.5f32, 0.0f64), (0.25, -0.5), (0.75, 0.5)] {
        let x = pulse(f, width, N);
        assert_finite(&x, "Pulse");
        assert!(peak(&x) <= 1.05);
        // Mean of a +/-1 square of duty w is 2w - 1.
        assert!(
            (dc(&x) as f64 - want_dc).abs() < 0.02,
            "width {width}: dc {} wanted {want_dc}",
            dc(&x)
        );
    }
}

#[test]
fn pulse_aliases_far_less_than_the_naive_square() {
    for (target, min_abs_db, min_gain_db) in [
        (105.0, 92.0, 55.0),
        (996.0, 38.0, 22.0),
        (3996.0, 33.0, 25.0),
    ] {
        let f = coherent_freq(target, SR, N);
        let ours = alias_snr_db(&pulse(f, 0.5, N), f, SR);
        let base = alias_snr_db(&naive_pulse(f, 0.5, N), f, SR);
        assert!(
            ours >= min_abs_db,
            "Pulse at {f:.1} Hz: {ours:.1} dB, wanted at least {min_abs_db}"
        );
        assert!(
            ours - base >= min_gain_db,
            "Pulse at {f:.1} Hz: {ours:.1} dB vs naive {base:.1} dB"
        );
    }
}

#[test]
fn pulse_degenerate_widths_stay_finite() {
    for w in [0.0f32, 1.0, -1.0, 2.0] {
        let x = pulse(440.0, w, 512);
        assert_finite(&x, "Pulse at a degenerate width");
        assert!(peak(&x) <= 1.05, "width {w}: peak {}", peak(&x));
    }
}

// ---- the LF family ----

#[test]
fn lf_shapes_have_their_documented_ranges_and_starting_points() {
    let n = 4096;
    let f = SR / n as f32 * 7.0; // seven whole cycles in the window

    let s = lf("LFSaw", f, 0.0, 0.5, n);
    assert!(s[0].abs() < 1e-6, "LFSaw starts at {}", s[0]);
    assert!(s[1] > s[0], "LFSaw must rise");
    assert!(peak(&s) <= 1.0 + 1e-6);

    let t = lf("LFTri", f, 0.0, 0.5, n);
    assert!(t[0].abs() < 1e-6, "LFTri starts at {}", t[0]);
    assert!(t[1] > t[0], "LFTri must rise");
    assert!(peak(&t) <= 1.0 + 1e-6);
    assert!(dc(&t).abs() < 1e-3, "LFTri dc {}", dc(&t));

    // LFPulse is scsynth's gate range, [0, 1] — not bipolar like Pulse.
    let p = lf("LFPulse", f, 0.0, 0.25, n);
    assert!(p.iter().all(|&v| (0.0..=1.0).contains(&v)));
    assert!((dc(&p) - 0.25).abs() < 0.01, "LFPulse duty {}", dc(&p));

    // VarSaw at width 0.5 is a triangle; near 1 it approaches a rising ramp.
    let v = lf("VarSaw", f, 0.0, 0.5, n);
    assert!(peak(&v) <= 1.0 + 1e-6);
    assert!(dc(&v).abs() < 1e-2, "VarSaw dc {}", dc(&v));
}

#[test]
fn var_saw_sweeps_from_a_falling_ramp_through_a_triangle_to_a_rising_one() {
    // The whole point of the row, and what distinguishes it from `LFTri`: the
    // duty cycle moves the peak. Asserted against the shape's closed form —
    // rising over the first `width` of the cycle, falling over the rest, so
    // the peak sits at exactly `width` and the value at any phase is known.
    let n = 4800;
    let f = SR / n as f32; // exactly one cycle in the window, so phase == i/n
    for width in [0.1f32, 0.25, 0.5, 0.75, 0.9] {
        let x = lf("VarSaw", f, 0.0, width, n);
        assert_finite(&x, "VarSaw");
        assert!(peak(&x) <= 1.0 + 1e-6, "width {width}: peak {}", peak(&x));

        // It starts at the bottom and reaches the top where the duty says.
        assert!(
            (x[0] + 1.0).abs() < 1e-3,
            "width {width}: starts at {}",
            x[0]
        );
        let (top, _) =
            x.iter().enumerate().fold(
                (0usize, f32::MIN),
                |a, (i, &v)| if v > a.1 { (i, v) } else { a },
            );
        let want_top = (width * n as f32).round() as usize;
        assert!(
            top.abs_diff(want_top) <= 1,
            "width {width}: peaks at sample {top}, the duty point is {want_top}"
        );

        // And every sample is on one of the two straight lines.
        for (i, &v) in x.iter().enumerate() {
            let t = i as f32 / n as f32;
            let want = if t < width {
                2.0 * t / width - 1.0
            } else {
                1.0 - 2.0 * (t - width) / (1.0 - width)
            };
            assert!(
                (v - want).abs() < 2e-3,
                "width {width} at phase {t:.4}: {v} wanted {want}"
            );
        }
    }

    // Both limits degenerate to a plain ramp rather than dividing by zero.
    for width in [0.0f32, 1.0] {
        let x = lf("VarSaw", 220.0, 0.0, width, 512);
        assert_finite(&x, "VarSaw at a degenerate width");
        assert!(peak(&x) <= 1.0 + 1e-6, "width {width}: peak {}", peak(&x));
    }
}

#[test]
fn every_lf_shape_runs_at_the_frequency_it_was_given() {
    // The shapes were asserted for range and starting point but never for
    // pitch, which is the one thing a modulation source is chosen for: an
    // `LFSaw` at 3 Hz that runs at 3.7 slews everything downstream. Measured
    // over the *whole* window rather than a cycle, so a per-block rounding of
    // the increment would show up as an accumulated error.
    for target in [3.0f32, 220.0, 3000.0] {
        // The estimator resolves each crossing to a sample, so its relative
        // error is about one sample over the span it measures: the window has
        // to be long in *samples* for a high frequency and long in *cycles*
        // for a low one. Four cycles or a third of a second, whichever is
        // more, satisfies both across the range.
        let n = ((4.0 * SR / target) as usize).max(16384);
        // A whole number of cycles in it, so the count is exact at both ends.
        let cycles = (target * n as f32 / SR).round().max(1.0);
        let f = cycles * SR / n as f32;
        for kind in ["LFSaw", "LFTri", "LFPulse", "VarSaw"] {
            let x = lf(kind, f, 0.0, 0.5, n);
            assert_finite(&x, kind);
            // LFPulse is unipolar, so it has no zero crossing to count: shift
            // it to the bipolar range the estimator expects.
            let x: Vec<f32> = if kind == "LFPulse" {
                x.iter().map(|v| v - 0.5).collect()
            } else {
                x
            };
            let got = zero_crossing_freq(&x, SR);
            assert!(
                (got - f).abs() < f * 1e-3,
                "{kind} at {f:.3} Hz measures {got:.3} Hz"
            );
        }
    }
}

#[test]
fn lf_initial_phase_is_in_cycles_and_applies_only_once() {
    let n = 1024;
    let f = SR / n as f32; // exactly one cycle in the window
    // A quarter cycle into a triangle that starts at 0 and peaks at a quarter.
    let t = lf("LFTri", f, 0.25, 0.5, n);
    assert!((t[0] - 1.0).abs() < 1e-3, "iphase 0.25 gives {}", t[0]);
    // Half a cycle into a rising LFSaw is its wrap point: -1.
    let s = lf("LFSaw", f, 0.5, 0.5, n);
    assert!((s[0] + 1.0).abs() < 1e-3, "iphase 0.5 gives {}", s[0]);
    // A phase outside [0, 1) wraps rather than misbehaving.
    let s2 = lf("LFSaw", f, 2.5, 0.5, n);
    assert!((s2[0] - s[0]).abs() < 1e-3);
}

// ---- Phasor ----

#[test]
fn phasor_ramps_by_rate_per_sample_and_wraps_at_its_end() {
    // rate 1 per sample over [0, 10): the classic buffer index.
    let x = render(
        r#"{"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 1.0},
            {"const": 0.0}, {"const": 10.0}, {"const": 0.0}]}"#,
        25,
    );
    let want: Vec<f32> = (0..25).map(|i| (i % 10) as f32).collect();
    assert_eq!(x, want);
}

#[test]
fn phasor_starts_at_start_and_resets_on_a_rising_trigger() {
    // Start is 5, so the first sample is 5 and not 0.
    let x = render(
        r#"{"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 1.0},
            {"const": 5.0}, {"const": 8.0}, {"const": 0.0}]}"#,
        6,
    );
    assert_eq!(x, vec![5.0, 6.0, 7.0, 5.0, 6.0, 7.0]);

    // A trigger from an Impulse resets to reset_pos on its exact sample. The
    // first Impulse sample is always 1, so the reset lands on sample 0.
    let x = render(
        r#"{"kind": "Phasor", "inputs": [{"const": 1.0}, {"const": 1.0},
            {"const": 0.0}, {"const": 100.0}, {"const": 42.0}]}"#,
        3,
    );
    assert_eq!(x, vec![42.0, 43.0, 44.0]);
}

#[test]
fn phasor_with_a_zero_range_holds_instead_of_dividing_by_zero() {
    let x = render(
        r#"{"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 1.0},
            {"const": 3.0}, {"const": 3.0}, {"const": 0.0}]}"#,
        4,
    );
    assert_finite(&x, "Phasor with a zero range");
}

// ---- the long run ----
//
// Rule 4, and the one place the family's `f64` accumulator has to earn itself.
// It earns itself in `Phasor` and **not** in the wrapped phase: measuring both
// is what pins down which claim is true. See `f64_is_for_the_position_not_the_
// phase` for the figures.

#[test]
fn a_read_position_high_in_a_long_file_still_advances() {
    // This is the test the `f64` position exists for, and it is `Phasor`'s
    // real job: an index into a buffer, a rate of 1 per sample. Start it at
    // 2^24 -- eight minutes into a 48 kHz file, an ordinary place to be -- and
    // an `f32` position **stops dead**, because there the spacing between
    // representable values is 2 and `pos += 1.0` rounds back to where it was.
    // Not a drift, a stall: measured, an `f32` accumulator advances 0 frames
    // in ten seconds where this one advances 479 999.
    let secs = 10.0f64;
    let n = (SR as f64 * secs) as usize;
    let start = (1u32 << 24) as f64; // 16 777 216, exact in f32 as well
    let x = render(
        r#"{"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 1.0},
            {"const": 16777216.0}, {"const": 33554432.0}, {"const": 0.0}]}"#,
        n,
    );
    assert_finite(&x, "Phasor high in a long file");
    assert_eq!(x[0] as f64, start, "it must begin where it was told");
    let advanced = x[n - 1] as f64 - start;
    let want = (n - 1) as f64;
    // The output is `f32`, and up there its resolution is 2 frames -- so the
    // *reported* position is coarse even though the accumulator is not. That
    // is the tolerance, and it is 5 orders of magnitude away from a stall.
    assert!(
        (advanced - want).abs() <= 2.0,
        "after {secs} s the position advanced {advanced} frames, not {want}"
    );

    // The wrapped-phase rows have no such magnitude to lose, so what a long
    // run asks of them is only that the pitch at the end of a note is the
    // pitch it started at.
    let f = 55.0f32;
    let y = saw(f, n);
    assert_finite(&y, "Saw over ten seconds");
    let first = zero_crossing_freq(&y[..SR as usize], SR);
    let final_second = zero_crossing_freq(&y[n - SR as usize..], SR);
    assert!(
        (final_second - f).abs() < 0.01,
        "Saw at {f} Hz reads {final_second:.4} Hz in its tenth second \
         (it read {first:.4} Hz in its first)"
    );
}

/// Not an assert — the measurement behind the claim above, kept because it
/// corrects an intuition the module doc used to state.
/// `cargo test --test oscillators -- --nocapture f64_is_for`
#[test]
fn f64_is_for_the_position_not_the_phase() {
    // The same accumulator, stepped in each precision, for ten seconds.
    let n = (SR as f64 * 10.0) as usize;

    // A phase wrapped into [0, 1) never grows, so its `f32` rounding error is
    // ~1 ulp of 1.0 per step and random-walks nowhere: the two precisions
    // measure the same pitch. `f32` would have been enough here.
    let mut p32 = 0.0f32;
    let mut p64 = 0.0f64;
    let (dt32, dt64) = (55.0f32 / SR, 55.0f64 / SR as f64);
    let (mut w32, mut w64) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for _ in 0..n {
        w32.push(2.0 * p32 - 1.0);
        w64.push((2.0 * p64 - 1.0) as f32);
        p32 += dt32;
        if p32 >= 1.0 {
            p32 -= 1.0;
        }
        p64 += dt64;
        if p64 >= 1.0 {
            p64 -= 1.0;
        }
    }
    let tail = n - SR as usize;
    println!(
        "        wrapped phase, tenth second: f32 {:.4} Hz, f64 {:.4} Hz",
        zero_crossing_freq(&w32[tail..], SR),
        zero_crossing_freq(&w64[tail..], SR)
    );

    // An unwrapped position does grow, and above 2^24 an `f32` one stops.
    let start = (1u32 << 24) as f64;
    let (mut q32, mut q64) = (start as f32, start);
    for _ in 1..n {
        q32 += 1.0;
        q64 += 1.0;
    }
    println!(
        "        position from 2^24, ten seconds: f32 advanced {:.0}, \
         f64 advanced {:.0} (of {})",
        q32 as f64 - start,
        q64 - start,
        n - 1
    );
}

/// Not an assert — the measured figures the docs quote.
/// `cargo test --test oscillators -- --nocapture report`
#[test]
fn report_the_measured_alias_figures() {
    println!("        fundamental   Saw      naive     Pulse    naive");
    for target in [105.0, 996.0, 3996.0] {
        let f = coherent_freq(target, SR, N);
        println!(
            "        {f:9.1} Hz  {:6.1}  {:8.1}  {:8.1}  {:7.1}",
            alias_snr_db(&saw(f, N), f, SR),
            alias_snr_db(&naive_saw(f, N), f, SR),
            alias_snr_db(&pulse(f, 0.5, N), f, SR),
            alias_snr_db(&naive_pulse(f, 0.5, N), f, SR),
        );
    }
    // The `Lf` core, for contrast: these are naive by design, and the number
    // is what "do not listen to a modulation source" costs. `LFTri` and
    // `VarSaw` are continuous — only their slope jumps — so they alias far
    // less than the two with a step in them, which is why a triangle is the
    // one LF shape that is sometimes audible.
    println!("\n        the Lf core (not band-limited, on purpose)");
    println!("        fundamental  LFSaw   LFPulse    LFTri   VarSaw");
    for target in [105.0, 996.0, 3996.0] {
        let f = coherent_freq(target, SR, N);
        println!(
            "        {f:9.1} Hz  {:6.1}  {:8.1}  {:7.1}  {:7.1}",
            alias_snr_db(&lf("LFSaw", f, 0.0, 0.5, N), f, SR),
            alias_snr_db(&lf("LFPulse", f, 0.0, 0.5, N), f, SR),
            alias_snr_db(&lf("LFTri", f, 0.0, 0.5, N), f, SR),
            alias_snr_db(&lf("VarSaw", f, 0.0, 0.5, N), f, SR),
        );
    }
}
