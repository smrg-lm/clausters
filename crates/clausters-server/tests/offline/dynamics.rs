//! The dynamics, `Median`, `MoogFF` and the analytic signal held against
//! the theory: what a follower's time means (and `Amplitude` at control
//! rate over audio, whose times scsynth stretches by a block), a limiter's
//! ceiling and delay, a compressor's steady gain, the ladder's response at
//! its cutoff, the quadrature of `Hilbert` and the shift of `FreqShift`.
//! `sc_reference` holds the rest against scsynth.

#![cfg(feature = "synth")]

use crate::bench::{Run, render_def};
use crate::signal::{amplitude_at, coherent_freq};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;

fn c(v: f32) -> Value {
    json!({ "const": v })
}

fn u(i: usize) -> Value {
    json!({ "ugen": i })
}

/// `n` samples of the graph's last UGen (at control rate, the value of
/// each block, repeated across it).
fn render(ugens: Value, n: usize) -> Vec<f32> {
    let mut ugens = ugens.as_array().unwrap().clone();
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), {"ugen": last}]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    render_def(&def, &Run::new(n)).remove(0)
}

/// A step from 0 to 1 half a second in.
fn step() -> Value {
    json!({"kind": "LFPulse", "inputs": [c(1.0), c(0.5), c(0.5)]})
}

/// A follower comes within a tenth of a step in its attack time, at control
/// rate over an audio input as at audio rate.
#[test]
fn amplitude_follows_in_its_time() {
    let at = 24_000;
    for rate in ["ar", "kr"] {
        let out = render(
            json!([step(), {"kind": "Amplitude", "rate": rate, "inputs": [u(0), c(0.01), c(0.01)]}]),
            at + 1200,
        );
        // Within a block of the attack time, as a control-rate value is.
        let early = out[at + 480 - 64];
        let late = out[at + 480 + 64];
        assert!(early < 0.9 && late > 0.9, "{rate}: {early} then {late}");
    }
}

/// A limiter's output never passes its level, and is its input `2 dur`
/// late.
#[test]
fn limiter_holds_its_ceiling_two_windows_late() {
    let out = render(
        json!([
            {"kind": "WhiteNoise", "inputs": []},
            {"kind": "LFNoise0", "inputs": [c(30.0)]},
            {"kind": "MulAdd", "inputs": [u(1), c(1.5), c(1.6)]},
            {"kind": "Mul", "inputs": [u(0), u(2)]},
            {"kind": "Limiter", "inputs": [u(3), c(0.5), c(0.005)]}
        ]),
        48_000,
    );
    let (worst, at) =
        out.iter().enumerate().fold(
            (0.0f32, 0),
            |(m, k), (i, x)| if x.abs() > m { (x.abs(), i) } else { (m, k) },
        );
    assert!(worst <= 0.5 + 1e-6, "the ceiling: {worst} at {at}");
    let late = 2 * 240;
    assert!(
        out[..late].iter().all(|&x| x == 0.0),
        "two windows of silence first"
    );
    assert!(
        out[late..].iter().any(|x| x.abs() > 0.45),
        "and the level reached"
    );
}

/// The normalizer brings a quiet signal's peaks to its level.
#[test]
fn normalizer_reaches_its_level() {
    let out = render(
        json!([
            {"kind": "Sine", "inputs": [c(997.0)]},
            {"kind": "Mul", "inputs": [u(0), c(0.05)]},
            {"kind": "Normalizer", "inputs": [u(1), c(0.8), c(0.01)]}
        ]),
        9600,
    );
    let peak = out[2400..].iter().map(|x| x.abs()).fold(0.0, f32::max);
    assert!((peak - 0.8).abs() < 0.01, "{peak}");
}

/// A compressor's steady gain is `(env / thresh)^(slope - 1)`.
#[test]
fn compander_settles_on_its_gain() {
    let n = 1 << 14;
    let f = coherent_freq(500.0, SR, n);
    let out = render(
        json!([
            {"kind": "Sine", "inputs": [c(f)]},
            {"kind": "Compander", "inputs": [u(0), u(0), c(0.5), c(1.0), c(0.5), c(0.001), c(1.0)]}
        ]),
        2 * n,
    );
    // The control's envelope sits at the sine's peak, 1: a gain of
    // (1 / 0.5)^-0.5.
    let a = amplitude_at(&out[n..], f, SR);
    assert!((a - 0.5f32.sqrt()).abs() < 0.01, "{a}");
}

/// Four one-poles, each 3 dB down at the cutoff: the ladder without
/// resonance is 12 dB down there, and its feedback lifts it exactly as the
/// ladder's transfer function says.
#[test]
fn moog_ff_is_twelve_db_down_at_its_cutoff() {
    let n = 1 << 14;
    let f = coherent_freq(1000.0, SR, n);
    let level = |gain: f32| {
        let out = render(
            json!([
                {"kind": "Sine", "inputs": [c(f)]},
                {"kind": "MoogFF", "inputs": [u(0), c(f), c(gain), c(0.0)]}
            ]),
            2 * n,
        );
        20.0 * amplitude_at(&out[n..], f, SR).log10()
    };
    assert!((level(0.0) + 12.04).abs() < 0.1, "{}", level(0.0));
    // Each pole is 1 / (1 + j) at the cutoff, so the four are -1/4 and the
    // feedback k gives -1/4 / (1 - k/4): 0 dB at k = 3, +6 dB at 3.5.
    assert!(level(3.0).abs() < 0.1, "{}", level(3.0));
    assert!((level(3.5) - 6.02).abs() < 0.1, "{}", level(3.5));
}

/// `Hilbert`'s two outputs are in quadrature: for a sine their magnitude is
/// its amplitude at every sample; `FreqShift` moves a sine by its shift.
#[test]
fn hilbert_is_in_quadrature_and_freq_shift_shifts() {
    let n = 1 << 14;
    let f = coherent_freq(1000.0, SR, n);
    let part = |chan: f32| {
        render(
            json!([{"kind": "Sine", "inputs": [c(f)]}, {"kind": "Hilbert", "inputs": [u(0), c(chan)]}]),
            2 * n,
        )
    };
    let (re, im) = (part(0.0), part(1.0));
    for i in n..2 * n {
        let m = (re[i] * re[i] + im[i] * im[i]).sqrt();
        assert!((m - 1.0).abs() < 0.02, "magnitude {m} at {i}");
    }
    let shift = coherent_freq(200.0, SR, n);
    let out = render(
        json!([{"kind": "Sine", "inputs": [c(f)]}, {"kind": "FreqShift", "inputs": [u(0), c(shift), c(0.0)]}]),
        2 * n,
    );
    let up = amplitude_at(&out[n..], f + shift, SR);
    let image = amplitude_at(&out[n..], f - shift, SR);
    assert!(
        up > 0.98 && image < 0.02,
        "{up} at the shift, {image} at its image"
    );
}

/// A median of three removes a one-sample spike and keeps a step.
#[test]
fn median_removes_a_spike_and_keeps_an_edge() {
    let out = render(
        json!([
            {"kind": "Impulse", "inputs": [c(100.0), c(0.3)]},
            {"kind": "LFPulse", "inputs": [c(50.0), c(0.75), c(0.5)]},
            {"kind": "Add", "inputs": [u(0), u(1)]},
            {"kind": "Median", "inputs": [c(3.0), u(2)]}
        ]),
        2048,
    );
    let pulse = render(
        json!([{"kind": "LFPulse", "inputs": [c(50.0), c(0.75), c(0.5)]}]),
        2048,
    );
    // The step arrives a sample late: the median of three is centred.
    for i in 1..2048 {
        assert_eq!(out[i], pulse[i - 1], "sample {i}");
    }
}
