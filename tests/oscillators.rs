//! The phase family (U1): `Saw`, `Pulse`, the `LF*` shapes and `Phasor`.
//!
//! The asserts are measurements, per the rules in the `audio-testing` skill:
//! frequency from the rendered signal, amplitude from its extremes, and — for
//! the two band-limited kinds — an **alias SNR** compared against the naive
//! (non-band-limited) waveform of the same shape at the same fundamental. The
//! baseline is regenerated inside the test rather than hardcoded, so the claim
//! is "PolyBLEP beats naive by this much here", not "this number was true once".

#![cfg(feature = "synth")]

#[path = "common/signal.rs"]
mod signal;

use std::sync::Arc;

use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use signal::*;

const SR: f32 = 48_000.0;
/// The analysis window every spectral assert here uses; `coherent_freq` and
/// `alias_snr_db` are both tied to it.
const N: usize = 4096;

/// Renders `n` samples of a one-UGen def written straight to bus 0.
fn render(ugen: &str, n: usize) -> Vec<f32> {
    let json = format!(
        r#"{{"name": "o", "ugens": [{ugen},
            {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}]}}"#
    );
    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses: &buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
        };
        synth.process(&mut ctx);
        out.extend_from_slice(buses.audio(0));
    }
    out.truncate(n);
    out
}

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

// ---- block splitting ----

#[test]
fn phase_ugens_are_identical_across_a_split_block() {
    // A scheduled bundle splits a block at an arbitrary sample; a stateful UGen
    // must not notice. Render the same def whole and in two halves.
    for ugen in [
        r#"{"kind": "Saw", "inputs": [{"const": 333.0}]}"#,
        r#"{"kind": "Pulse", "inputs": [{"const": 333.0}, {"const": 0.3}]}"#,
        r#"{"kind": "LFTri", "inputs": [{"const": 333.0}, {"const": 0.0}]}"#,
        r#"{"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 0.25},
            {"const": 0.0}, {"const": 7.0}, {"const": 0.0}]}"#,
    ] {
        let whole = render(ugen, BLOCK_SIZE * 4);
        let split = render_split(ugen, BLOCK_SIZE * 4, 21);
        assert_eq!(whole, split, "split render differs for {ugen}");
    }
}

/// Renders the same def but cutting every block at `at`, the way a timed bundle
/// does.
fn render_split(ugen: &str, n: usize, at: usize) -> Vec<f32> {
    let json = format!(
        r#"{{"name": "o", "ugens": [{ugen},
            {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}]}}"#
    );
    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        buses.clear_audio();
        for (offset, frames) in [(0, at), (at, BLOCK_SIZE - at)] {
            let mut ctx = ProcessCtx {
                sample_rate: SR,
                full_sample_rate: SR,
                buses: &buses,
                buffers: &[],
                offset,
                frames,
            };
            synth.process(&mut ctx);
        }
        out.extend_from_slice(buses.audio(0));
    }
    out.truncate(n);
    out
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
}
