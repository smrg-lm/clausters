//! The filter core (U2).
//!
//! The two-pole rows are asserted against the **analytic transfer function of
//! the structure they implement**, evaluated in `f64` — never against a stored
//! buffer and never against scsynth's numbers. The trapezoidal state-variable
//! filter is the bilinear transform of the analog two-pole prototype, so its
//! magnitude at a frequency `f` is the prototype evaluated at the *pre-warped*
//! ratio
//!
//! ```text
//! W = tan(pi*f/sr) / tan(pi*fc/sr)
//! ```
//!
//! with `|H_lp| = 1 / sqrt((1 - W^2)^2 + (k*W)^2)` and the bandpass, highpass
//! and notch numerators `W`, `W^2` and `|1 - W^2|` over the same denominator.
//! That expression is the whole specification; if the filter matches it at
//! twenty points across the band, it is the filter it claims to be.
//!
//! The one-pole family is asserted the same way, against its own closed form —
//! an FIR for `OneZero`, a one-pole gain for `Integrator` — and additionally
//! sample by sample against its difference equation, which a frequency response
//! cannot see a one-sample state error through.
//!
//! Rule 5, the block split, is not here: it is the same test for every row and
//! runs from the shared table over all twelve at once (`tests/subjects.rs`).

#![cfg(feature = "synth")]

#[path = "common/bench.rs"]
mod bench;
#[path = "common/signal.rs"]
mod signal;

use std::f64::consts::PI;
use std::sync::Arc;

use bench::{SR, render_with_input};
use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use signal::*;

/// Analysis window for every gain measurement, and the render it is taken from.
/// The window starts at a whole multiple of itself, so a frequency that is a
/// whole number of periods in `WIN` is also a whole number of periods in the
/// slice actually analysed.
const WIN: usize = 8192;
const RENDER: usize = 32_768;

/// Snaps a frequency so `WIN` samples hold a whole number of its periods —
/// *coherent sampling*, which is what makes the single-bin DFT in
/// `response_at` exact rather than leaky. Without it a gain reads a tenth of a
/// dB off and every tolerance below becomes a guess.
fn snap(f: f32) -> f32 {
    let bin = SR / WIN as f32;
    (f / bin).round().max(1.0) * bin
}

fn sine(freq: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (std::f32::consts::TAU * freq * i as f32 / SR).sin())
        .collect()
}

/// The analytic magnitude of the analog two-pole prototype at the pre-warped
/// ratio `w`, for each tap.
fn analytic(tap: &str, f: f32, fc: f32, k: f64) -> f64 {
    let g = |x: f32| (PI * x as f64 / SR as f64).tan();
    let w = g(f) / g(fc);
    let denom = ((1.0 - w * w).powi(2) + (k * w).powi(2)).sqrt();
    match tap {
        "lp" => 1.0 / denom,
        "hp" => w * w / denom,
        // The normalized bandpass both `BPF` and `Resonz` promise: unity at the
        // centre, hence the leading `k`.
        "bp" => k * w / denom,
        "notch" => (1.0 - w * w).abs() / denom,
        _ => unreachable!(),
    }
}

/// Drives the filter with a sine at each frequency and returns the measured
/// gain, discarding a generous transient first.
fn measured_response(ugen_json: &str, freqs: &[f32]) -> Vec<f32> {
    freqs
        .iter()
        .map(|&f| {
            let x = sine(f, RENDER);
            let y = render_with_input(ugen_json, &x);
            // Analyse the last window only: three windows in, any two-pole
            // transient is long gone even at a Q high enough to ring.
            let from = RENDER - WIN;
            let (gain, _) = response_at(&x[from..], &y[from..], f, SR);
            gain
        })
        .collect()
}

/// Twenty points spread logarithmically across the audible band.
fn sweep() -> Vec<f32> {
    (0..20)
        .map(|i| snap(30.0 * 2.0f32.powf(i as f32 * 9.0 / 19.0)))
        .collect()
}

fn assert_matches_analytic(kind: &str, tap: &str, fc: f32, rq: Option<f32>, tol_db: f64) {
    let json = match rq {
        None => format!(r#"{{"kind": "{kind}", "inputs": [{{"ugen": 0}}, {{"const": {fc}}}]}}"#),
        Some(q) => format!(
            r#"{{"kind": "{kind}", "inputs": [{{"ugen": 0}}, {{"const": {fc}}}, {{"const": {q}}}]}}"#
        ),
    };
    let k = rq.map(|q| q as f64).unwrap_or(std::f64::consts::SQRT_2);
    let freqs = sweep();
    let got = measured_response(&json, &freqs);
    for (f, g) in freqs.iter().zip(&got) {
        let want = analytic(tap, *f, fc, k);
        // Compare in dB: a filter is specified in dB and a relative tolerance
        // would be meaningless deep in a stopband.
        let (got_db, want_db) = (20.0 * (*g as f64).log10(), 20.0 * want.log10());
        // A notch measured exactly at its centre has an analytic null, which no
        // finite arithmetic reaches and no tolerance in dB can express. Assert
        // the only meaningful thing there: that it really is a null.
        if want_db < -80.0 {
            assert!(
                got_db < -80.0,
                "{kind} at {f:.0} Hz (fc {fc}) should null, measured {got_db:.1} dB"
            );
            continue;
        }
        assert!(
            (got_db - want_db).abs() < tol_db,
            "{kind} at {f:.0} Hz (fc {fc}): {got_db:.2} dB, analytic {want_db:.2} dB"
        );
    }
}

// ---- the two-pole rows against their transfer function ----

#[test]
fn butterworth_rows_match_the_analytic_response() {
    // 0.25 dB over a 9-octave sweep, at three cutoffs an octave-and-a-half
    // apart. The residual is the measurement's, not the filter's: the analysis
    // window is not coherent with every sweep frequency.
    for fc in [snap(100.0), snap(1000.0), snap(6000.0)] {
        assert_matches_analytic("LPF", "lp", fc, None, 0.25);
        assert_matches_analytic("HPF", "hp", fc, None, 0.25);
    }
}

#[test]
fn resonant_rows_match_the_analytic_response_across_q() {
    for &rq in &[1.0f32, 0.5, 0.1] {
        let fc = snap(800.0);
        assert_matches_analytic("RLPF", "lp", fc, Some(rq), 0.1);
        assert_matches_analytic("RHPF", "hp", fc, Some(rq), 0.1);
        assert_matches_analytic("BPF", "bp", fc, Some(rq), 0.1);
        assert_matches_analytic("BRF", "notch", fc, Some(rq), 0.2);
        // Resonz gets its own sweep rather than riding on the one below. That
        // test says the two rows are the same implementation; this one says
        // the implementation is the right one, and either could fail alone.
        assert_matches_analytic("Resonz", "bp", fc, Some(rq), 0.1);
    }
}

#[test]
fn resonz_is_the_same_resonator_as_bpf() {
    // scsynth ships two historically distinct two-pole resonators with the same
    // parameterization and the same unity peak gain; here they are one
    // implementation under both names, and this is the assert that says so
    // rather than leaving a reader to wonder.
    let x = sine(800.0, 1 << 14);
    let bpf = render_with_input(
        r#"{"kind": "BPF", "inputs": [{"ugen": 0}, {"const": 800.0}, {"const": 0.3}]}"#,
        &x,
    );
    let resonz = render_with_input(
        r#"{"kind": "Resonz", "inputs": [{"ugen": 0}, {"const": 800.0}, {"const": 0.3}]}"#,
        &x,
    );
    assert_eq!(bpf, resonz);
}

#[test]
fn the_butterworth_pair_is_minus_three_db_at_its_cutoff() {
    // The one number everybody checks a filter by, stated on its own.
    for fc in [snap(80.0), snap(440.0), snap(3000.0)] {
        for (kind, tap) in [("LPF", "lp"), ("HPF", "hp")] {
            let json =
                format!(r#"{{"kind": "{kind}", "inputs": [{{"ugen": 0}}, {{"const": {fc}}}]}}"#);
            let g = measured_response(&json, &[fc])[0];
            let db = 20.0 * (g as f64).log10();
            assert!(
                (db + 3.0103).abs() < 0.1,
                "{kind} ({tap}) at its own cutoff {fc}: {db:.3} dB"
            );
        }
    }
}

#[test]
fn the_lowpass_stopband_falls_at_twelve_db_per_octave() {
    // Measured well below Nyquist on purpose. A *digital* two-pole is steeper
    // than 12 dB/octave near the top of the band, because the bilinear
    // transform warps the frequency axis: between 4 and 8 kHz at 48 kHz the
    // prototype is evaluated an octave and a tenth apart, and the section
    // measures -13.3 dB/octave there. That is the filter being right, not
    // wrong, so the textbook figure is checked where warping is negligible.
    let json = r#"{"kind": "LPF", "inputs": [{"ugen": 0}, {"const": 100.0}]}"#;
    let g = measured_response(json, &[snap(800.0), snap(1600.0)]);
    let slope = 20.0 * (g[1] as f64 / g[0] as f64).log10();
    assert!(
        (slope + 12.0).abs() < 0.2,
        "two-pole stopband slope {slope:.2} dB/octave"
    );
}

// ---- the properties the realization was chosen for ----

#[test]
fn a_low_cutoff_stays_accurate_over_a_long_run() {
    // The f64-state test. Ten seconds at fc = 20 Hz puts the poles right up
    // against z = 1, where f32 state truncation would show up first as a drifting
    // gain and then as noise. The passband gain must still be the analytic one.
    let n = (SR as usize) * 10;
    let f = snap(5.0);
    let x = sine(f, n);
    let y = render_with_input(
        r#"{"kind": "LPF", "inputs": [{"ugen": 0}, {"const": 20.0}]}"#,
        &x,
    );
    assert_finite(&y, "LPF at 20 Hz over 10 s");
    // Analyse a whole number of `WIN` blocks from the end, keeping coherence.
    let from = n - n % WIN - WIN;
    let (gain, _) = response_at(&x[from..from + WIN], &y[from..from + WIN], f, SR);
    let want = analytic("lp", f, 20.0, std::f64::consts::SQRT_2);
    let err_db = 20.0 * ((gain as f64) / want).log10();
    assert!(
        err_db.abs() < 0.1,
        "after 10 s at fc 20 Hz the gain is off by {err_db:.3} dB"
    );
}

#[test]
fn a_high_q_resonator_still_has_unity_peak_gain_after_ten_seconds() {
    // The other end of the long-run rule. `LPF` at 20 Hz puts the poles near
    // z = 1; a high-Q bandpass puts them near the unit circle at its centre
    // frequency, which is where a resonator's state is largest relative to
    // its input and where truncation would show first. Q = 50, driven at the
    // centre, where the normalized bandpass is unity by construction — so the
    // expected value is 1 exactly, with nothing to fit.
    let n = (SR as usize) * 10;
    let fc = snap(1000.0);
    let x = sine(fc, n);
    for kind in ["BPF", "Resonz"] {
        let y = render_with_input(
            &format!(
                r#"{{"kind": "{kind}", "inputs": [{{"ugen": 0}}, {{"const": {fc}}},
                    {{"const": 0.02}}]}}"#
            ),
            &x,
        );
        assert_finite(&y, &format!("{kind} at Q 50 over 10 s"));
        let from = n - n % WIN - WIN;
        let (gain, _) = response_at(&x[from..from + WIN], &y[from..from + WIN], fc, SR);
        let db = 20.0 * (gain as f64).log10();
        assert!(
            db.abs() < 0.1,
            "{kind} at Q 50 reads {db:.3} dB at its own centre after 10 s"
        );
    }
}

#[test]
fn the_leaky_integrator_still_matches_its_analytic_gain_after_ten_seconds() {
    // `Integrator` is the one-pole with no input normalization, so its state
    // is the largest of the family and its pole sits closest to z = 1: the
    // place to look for an accumulator losing bits. Its gain at w is
    // `1 / |1 - c e^-jw|`, closed form.
    let n = (SR as usize) * 10;
    let c = 0.9999f64;
    let f = snap(220.0);
    let x = sine(f, n);
    let y = render_with_input(
        &format!(r#"{{"kind": "Integrator", "inputs": [{{"ugen": 0}}, {{"const": {c}}}]}}"#),
        &x,
    );
    assert_finite(&y, "Integrator over 10 s");
    let from = n - n % WIN - WIN;
    let (gain, _) = response_at(&x[from..from + WIN], &y[from..from + WIN], f, SR);
    let w = std::f64::consts::TAU * f as f64 / SR as f64;
    let (re, im) = (1.0 - c * w.cos(), c * w.sin());
    let want = 1.0 / (re * re + im * im).sqrt();
    let err_db = 20.0 * ((gain as f64) / want).log10();
    assert!(
        err_db.abs() < 0.1,
        "after 10 s the Integrator's gain is off by {err_db:.3} dB \
         (measured {gain}, analytic {want})"
    );
}

#[test]
fn an_audio_rate_cutoff_sweep_stays_bounded() {
    // The reason the realization is trapezoidal integrators rather than a
    // direct-form section: a biquad whose coefficients are interpolated this
    // fast can leave its stable region. Sweep the cutoff from 20 Hz to 18 kHz
    // and back, at high resonance, driven by full-scale noise.
    let n = (SR as usize) * 2;
    let json = r#"{"name": "sweep", "ugens": [
        {"kind": "In", "inputs": [{"const": 1.0}]},
        {"kind": "LFTri", "inputs": [{"const": 40.0}, {"const": 0.0}]},
        {"kind": "MulAdd", "inputs": [{"ugen": 1}, {"const": 8990.0}, {"const": 9010.0}]},
        {"kind": "RLPF", "inputs": [{"ugen": 0}, {"ugen": 2}, {"const": 0.05}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}]}"#;
    let def = compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR, SEED_STRIDE);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut rng = clausters_core::rng::WhiteNoise::from_seed(0xC0FFEE);
    let mut worst = 0.0f32;
    let mut blocks = 0;
    while blocks * BLOCK_SIZE < n {
        buses.clear_audio();
        // SAFETY: single-threaded test, sole owner of bus 1.
        for s in unsafe { buses.audio_mut(1) }.iter_mut() {
            *s = rng.next_sample();
        }
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses: &buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
        };
        synth.process(&mut ctx);
        let out = buses.audio(0);
        assert_finite(out, "RLPF under an audio-rate cutoff sweep");
        worst = worst.max(peak(out));
        blocks += 1;
    }
    // A resonant lowpass boosts around its cutoff, so some gain is expected;
    // what must not happen is divergence. 1/rq = 20 is the analytic ceiling.
    assert!(worst < 40.0, "peak {worst} under an audio-rate sweep");
}

// ---- the mix row ----

#[test]
fn svf_tap_gains_reproduce_every_classic_response() {
    // (low, band, high) for each named response; the whole point of exposing
    // the mix is that these are reachable, and modulable, from one row.
    let fc = snap(800.0);
    let rq = 0.4f32;
    for (name, tap, low, band, high) in [
        ("lowpass", "lp", 1.0, 0.0, 0.0),
        ("highpass", "hp", 0.0, 0.0, 1.0),
        ("bandpass", "bp", 0.0, rq, 0.0),
        ("notch", "notch", 1.0, 0.0, 1.0),
    ] {
        let json = format!(
            r#"{{"kind": "Svf", "inputs": [{{"ugen": 0}}, {{"const": {fc}}},
                {{"const": {rq}}}, {{"const": {low}}}, {{"const": {band}}},
                {{"const": {high}}}]}}"#
        );
        let freqs = sweep();
        let got = measured_response(&json, &freqs);
        for (f, g) in freqs.iter().zip(&got) {
            let want = analytic(tap, *f, fc, rq as f64);
            let (a, b) = (20.0 * (*g as f64).log10(), 20.0 * want.log10());
            if b < -80.0 {
                assert!(
                    a < -80.0,
                    "Svf as {name} should null at {f:.0} Hz: {a:.1} dB"
                );
                continue;
            }
            assert!(
                (a - b).abs() < 0.1,
                "Svf as {name} at {f:.0} Hz: {a:.2} vs {b:.2} dB"
            );
        }
    }
}

#[test]
fn svf_allpass_mix_is_flat() {
    // (1, -rq, 1) is the allpass, and flatness *is* its definition — the
    // strongest assert available for a mix, because it cannot be satisfied by
    // accident.
    let (fc, rq) = (snap(700.0), 0.5f32);
    let json = format!(
        r#"{{"kind": "Svf", "inputs": [{{"ugen": 0}}, {{"const": {fc}}},
            {{"const": {rq}}}, {{"const": 1.0}}, {{"const": {}}},
            {{"const": 1.0}}]}}"#,
        -rq
    );
    let freqs = sweep();
    for (f, g) in freqs.iter().zip(&measured_response(&json, &freqs)) {
        let db = 20.0 * (*g as f64).log10();
        assert!(db.abs() < 0.02, "allpass at {f:.0} Hz: {db:.4} dB");
    }
}

#[test]
fn svf_mix_gains_are_modulable() {
    // A morph from lowpass to highpass driven by a control signal: the point of
    // the row. It must stay finite and actually change.
    let json = r#"{"name": "morph", "ugens": [
        {"kind": "In", "inputs": [{"const": 1.0}]},
        {"kind": "LFTri", "inputs": [{"const": 3.0}, {"const": 0.0}]},
        {"kind": "MulAdd", "inputs": [{"ugen": 1}, {"const": 0.5}, {"const": 0.5}]},
        {"kind": "BinaryOpUGen", "op": "sub",
         "inputs": [{"const": 1.0}, {"ugen": 2}]},
        {"kind": "Svf", "inputs": [{"ugen": 0}, {"const": 900.0}, {"const": 0.5},
            {"ugen": 3}, {"const": 0.0}, {"ugen": 2}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 4}]}]}"#;
    let def = compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR, SEED_STRIDE);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut rng = clausters_core::rng::WhiteNoise::from_seed(1234);
    let mut band_energy = Vec::new();
    for _ in 0..2000 {
        buses.clear_audio();
        // SAFETY: single-threaded test, sole owner of bus 1.
        for s in unsafe { buses.audio_mut(1) }.iter_mut() {
            *s = rng.next_sample();
        }
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses: &buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
        };
        synth.process(&mut ctx);
        assert_finite(buses.audio(0), "morphing Svf");
        band_energy.push(rms(buses.audio(0)));
    }
    // The response really sweeps, so the block RMS is not constant.
    let lo = band_energy.iter().cloned().fold(f32::MAX, f32::min);
    let hi = band_energy.iter().cloned().fold(0.0f32, f32::max);
    assert!(hi > lo * 1.2, "the morph did not change the response");
}

// ---- the one-pole family ----

#[test]
fn one_pole_matches_its_difference_equation() {
    // y[n] = (1-|c|) x[n] + c y[n-1], computed here directly.
    for c in [0.9f32, -0.9, 0.0, 0.5] {
        let x: Vec<f32> = (0..512).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let y = render_with_input(
            &format!(r#"{{"kind": "OnePole", "inputs": [{{"ugen": 0}}, {{"const": {c}}}]}}"#),
            &x,
        );
        let (mut want, cd) = (0.0f64, c as f64);
        for (i, &got) in y.iter().enumerate() {
            want = (1.0 - cd.abs()) * x[i] as f64 + cd * want;
            assert!(
                (got as f64 - want).abs() < 1e-6,
                "OnePole c={c} sample {i}: {got} vs {want}"
            );
        }
    }
}

#[test]
fn one_zero_matches_its_analytic_magnitude() {
    // The zero-only sibling, and the row that had no test of its own: an FIR,
    // `y[n] = (1-|c|) x[n] + c x[n-1]`, so its magnitude is exactly
    // `|(1-|c|) + c e^-jw|` with no approximation anywhere in the comparison.
    // A positive `c` is a lowpass with a null at Nyquist, a negative one a
    // highpass with a null at DC; both are checked, because the `(1-|c|)`
    // normalization is the easy thing to get wrong for the negative half.
    for c in [0.9f32, -0.9, 0.5, -0.5] {
        let json = format!(r#"{{"kind": "OneZero", "inputs": [{{"ugen": 0}}, {{"const": {c}}}]}}"#);
        let freqs = sweep();
        for (f, g) in freqs.iter().zip(&measured_response(&json, &freqs)) {
            let (c, w) = (c as f64, std::f64::consts::TAU * *f as f64 / SR as f64);
            let (re, im) = (1.0 - c.abs() + c * w.cos(), -c * w.sin());
            let want = (re * re + im * im).sqrt();
            let (got_db, want_db) = (20.0 * (*g as f64).log10(), 20.0 * want.log10());
            assert!(
                (got_db - want_db).abs() < 0.05,
                "OneZero c={c} at {f:.0} Hz: {got_db:.3} dB, analytic {want_db:.3} dB"
            );
        }
    }

    // And sample by sample against the difference equation, the way `OnePole`
    // is checked: the response above cannot see a one-sample state error.
    for c in [0.9f32, -0.9, 0.0, 0.5] {
        let x: Vec<f32> = (0..512).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let y = render_with_input(
            &format!(r#"{{"kind": "OneZero", "inputs": [{{"ugen": 0}}, {{"const": {c}}}]}}"#),
            &x,
        );
        let (mut x1, cd) = (0.0f64, c as f64);
        for (i, &got) in y.iter().enumerate() {
            let want = (1.0 - cd.abs()) * x[i] as f64 + cd * x1;
            x1 = x[i] as f64;
            assert!(
                (got as f64 - want).abs() < 1e-6,
                "OneZero c={c} sample {i}: {got} vs {want}"
            );
        }
    }
}

#[test]
fn leak_dc_removes_a_constant_and_keeps_the_signal() {
    // A coherent frequency and a whole-window slice, so the tone's own mean is
    // exactly zero and what `dc` reports is the leftover offset alone.
    let f = snap(300.0);
    let x: Vec<f32> = sine(f, RENDER).iter().map(|v| v + 0.5).collect();
    let y = render_with_input(
        r#"{"kind": "LeakDC", "inputs": [{"ugen": 0}, {"const": 0.995}]}"#,
        &x,
    );
    let (from, to) = (RENDER - WIN, RENDER);
    let residual = dc(&y[from..to]);
    assert!(residual.abs() < 1e-4, "residual DC {residual}");
    // The tone itself survives essentially untouched well above the corner.
    let (gain, _) = response_at(&x[from..to], &y[from..to], f, SR);
    assert!((gain - 1.0).abs() < 0.05, "LeakDC passband gain {gain}");
}

#[test]
fn integrator_accumulates_and_still_forgets() {
    // A unit step into a leaky integrator settles at 1/(1-c), the geometric sum.
    let c = 0.99f32;
    let x = vec![1.0f32; 1 << 13];
    let y = render_with_input(
        &format!(r#"{{"kind": "Integrator", "inputs": [{{"ugen": 0}}, {{"const": {c}}}]}}"#),
        &x,
    );
    assert_finite(&y, "Integrator");
    let settled = *y.last().unwrap() as f64;
    assert!(
        (settled - 1.0 / (1.0 - c as f64)).abs() < 0.5,
        "settled at {settled}, expected {}",
        1.0 / (1.0 - c as f64)
    );
}

#[test]
fn one_pole_family_stays_finite_at_an_unstable_coefficient() {
    for kind in ["OnePole", "OneZero", "LeakDC", "Integrator"] {
        for c in [1.0f32, -1.0, 5.0] {
            let x = vec![1.0f32; 4096];
            let y = render_with_input(
                &format!(r#"{{"kind": "{kind}", "inputs": [{{"ugen": 0}}, {{"const": {c}}}]}}"#),
                &x,
            );
            assert_finite(&y, &format!("{kind} at coef {c}"));
        }
    }
}
