//! The analysis UGens held to their definitions, where scsynth's code is
//! not what they define (`sc_reference` holds the rest against scsynth):
//! a measure of a frame through any chain, the detectors on clicks and on a
//! steady tone, the beat trackers on a click train, the key of a chord and
//! the pitch of a tone.

#![cfg(feature = "synth")]

use crate::bench::{Run, render_def};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;

fn c(v: f32) -> Value {
    json!({ "const": v })
}

fn u(i: usize) -> Value {
    json!({ "ugen": i })
}

/// `n` samples of the graph's last UGen (at control rate, each slice's value
/// repeated across it).
fn render(ugens: Vec<Value>, n: usize) -> Vec<f32> {
    let mut ugens = ugens;
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), u(last)]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    render_def(&def, &Run::new(n)).remove(0)
}

fn sine(f: f32) -> Value {
    json!({"kind": "Sine", "inputs": [c(f)]})
}

fn add(a: usize, b: usize) -> Value {
    json!({"kind": "BinaryOpUGen", "op": "add", "inputs": [u(a), u(b)]})
}

fn mul(a: usize, k: f32) -> Value {
    json!({"kind": "BinaryOpUGen", "op": "mul", "inputs": [u(a), c(k)]})
}

/// An `FFT` of input `x`, `n` points at half a frame's hop.
fn fft(x: usize, n: usize) -> Value {
    json!({"kind": "FFT", "rate": "kr", "inputs": [u(x), c(1.0)], "fft_size": n, "hop": 0.5})
}

/// The measure `kind` of a tone through a chain of `n` points, at its end.
fn measure_of(tone: Vec<Value>, n: usize, kind: &str, extra: &[f32]) -> f32 {
    let mut g = tone;
    let x = g.len() - 1;
    g.push(fft(x, n));
    let mut inputs = vec![u(x + 1)];
    inputs.extend(extra.iter().map(|&v| c(v)));
    g.push(json!({"kind": kind, "rate": "kr", "inputs": inputs}));
    *render(g, 24_000).last().unwrap()
}

#[test]
fn a_tone_has_its_centroid_and_its_percentiles() {
    for n in [1024, 4096] {
        let cent = measure_of(vec![sine(1500.0)], n, "SpecCentroid", &[]);
        assert!((cent - 1500.0).abs() < 60.0, "{n}: centroid {cent}");
        let median = measure_of(vec![sine(1500.0)], n, "SpecPcile", &[0.5, 1.0, 0.0]);
        assert!(
            (median - 1500.0).abs() < SR / n as f32,
            "{n}: median {median}"
        );
        let bin = measure_of(vec![sine(1500.0)], n, "SpecPcile", &[0.5, 0.0, 1.0]);
        assert!(
            (bin - 1500.0 * n as f32 / SR).abs() <= 1.0,
            "{n}: bin {bin}"
        );
    }
}

/// DC counts in the centroid: a tone over an offset is lower than the tone.
#[test]
fn dc_pulls_the_centroid_down() {
    let tone = measure_of(vec![sine(3000.0)], 1024, "SpecCentroid", &[]);
    let offset = measure_of(
        vec![
            sine(3000.0),
            json!({"kind": "DC", "rate": "ar", "inputs": [c(1.0)]}),
            add(0, 1),
        ],
        1024,
        "SpecCentroid",
        &[],
    );
    assert!(offset < tone * 0.8, "{offset} against {tone}");
}

#[test]
fn noise_is_flat_and_a_tone_is_not() {
    let noise = measure_of(
        vec![json!({"kind": "WhiteNoise"})],
        1024,
        "SpecFlatness",
        &[],
    );
    let tone = measure_of(vec![sine(1000.0)], 1024, "SpecFlatness", &[]);
    assert!(noise > 0.5 && tone < 0.1, "noise {noise}, tone {tone}");
}

/// A tone reads the same loudness through chains of any size and window,
/// louder for a louder tone, and silence reads nothing.
#[test]
fn loudness_follows_the_tone_not_the_chain() {
    let at = |n: usize, amp: f32| {
        measure_of(vec![sine(1000.0), mul(0, amp)], n, "Loudness", &[0.25, 1.0])
    };
    let (a, b) = (at(1024, 0.5), at(4096, 0.5));
    assert!(
        (a / b - 1.0).abs() < 0.1,
        "1024 points {a} sones, 4096 points {b}"
    );
    assert!(at(1024, 0.05) < a * 0.5);
    assert_eq!(at(1024, 0.0), 0.0);
}

/// The MFCC of band-limited noise, averaged over a second, does not depend
/// on the chain's size: the bands are at their frequencies and the power at
/// one calibration. (A pure tone's would: its leakage into the far bands
/// falls with the resolution.)
#[test]
fn mfcc_is_the_same_through_two_sizes() {
    let mean = |n: usize, chan: f32| {
        let g = vec![
            json!({"kind": "WhiteNoise"}),
            json!({"kind": "LPF", "inputs": [u(0), c(3000.0)]}),
            fft(1, n),
            json!({"kind": "MFCC", "rate": "kr", "inputs": [u(2), c(13.0), c(chan)]}),
        ];
        let out = render(g, 96_000);
        out[48_000..].iter().sum::<f32>() / 48_000.0
    };
    for chan in [0.0, 1.0, 3.0] {
        let (a, b) = (mean(1024, chan), mean(2048, chan));
        assert!((a - b).abs() < 0.03, "coefficient {chan}: {a} and {b}");
    }
}

/// A C major chord is in C major, an A minor one in A minor.
#[test]
fn a_chord_has_its_key() {
    let chord = |fs: [f32; 3]| vec![sine(fs[0]), sine(fs[1]), add(0, 1), sine(fs[2]), add(2, 3)];
    let key = |fs| {
        let mut g = chord(fs);
        g.push(json!({"kind": "FFT", "rate": "kr", "inputs": [u(4), c(1.0)], "fft_size": 4096, "hop": 0.5}));
        g.push(json!({"kind": "KeyTrack", "rate": "kr", "inputs": [u(5), c(2.0), c(0.5)]}));
        *render(g, 96_000).last().unwrap()
    };
    assert_eq!(key([261.63, 329.63, 392.0]), 0.0);
    assert_eq!(key([220.0, 261.63, 329.63]), 21.0);
}

/// Clicks four times a second over a tone, through a detector: about one
/// detection a click.
fn detections(kind: &str, inputs: &[f32]) -> usize {
    let mut g = vec![
        json!({"kind": "Impulse", "inputs": [c(4.0), c(0.5)]}),
        sine(440.0),
        mul(1, 0.05),
        add(0, 2),
        fft(3, 512),
    ];
    let rate = if kind.starts_with("PV_") { "ar" } else { "kr" };
    let mut ins = vec![u(4)];
    ins.extend(inputs.iter().map(|&v| c(v)));
    g.push(json!({"kind": kind, "rate": rate, "inputs": ins}));
    let out = render(g, 96_000);
    // Rising edges of the trigger.
    out.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
}

#[test]
fn the_detectors_find_the_clicks() {
    // Eight clicks in two seconds.
    for odf in [0.0, 2.0, 3.0, 4.0, 6.0] {
        let n = detections("Onsets", &[0.5, odf]);
        assert!((7..=9).contains(&n), "odftype {odf}: {n} onsets");
    }
    let n = detections("PV_HainsworthFoote", &[1.0, 0.0, 0.3, 0.04]);
    assert!((7..=9).contains(&n), "HainsworthFoote: {n}");
    let n = detections("PV_JensenAndersen", &[0.25, 0.25, 0.25, 0.25, 0.02, 0.04]);
    assert!((7..=9).contains(&n), "JensenAndersen: {n}");
}

/// The complex-domain detection function of a steady tone falls to
/// nothing: each bin is where its last two frames predict.
#[test]
fn a_steady_tone_is_predicted() {
    let mut g = vec![sine(1000.0), fft(0, 1024)];
    g.push(json!({"kind": "Onsets", "rate": "kr",
        "inputs": [u(1), c(0.5), c(2.0), c(1.0), c(0.1), c(10.0), c(11.0), c(0.0), c(1.0)]}));
    let out = render(g, 48_000);
    let late = out[40_000..].iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    assert!(late < 0.05, "{late}");
}

/// A click train at `bps` beats a second.
fn clicks(bps: f32) -> Value {
    json!({"kind": "Impulse", "inputs": [c(bps), c(0.0)]})
}

#[test]
fn beat_track_finds_the_tempo() {
    for bps in [2.0, 2.5] {
        let g = vec![
            clicks(bps),
            fft(0, 1024),
            json!({"kind": "BeatTrack", "rate": "kr", "inputs": [u(1), c(0.0), c(3.0)]}),
        ];
        let out = render(g, 48_000 * 20);
        let tempo = *out.last().unwrap();
        assert!((tempo - bps).abs() < 0.05, "{bps}: {tempo}");
    }
}

#[test]
fn beat_track2_finds_another_tempo() {
    let g = vec![
        json!({"kind": "Impulse", "rate": "kr", "inputs": [c(2.5), c(0.0)]}),
        json!({"kind": "OutCtl", "inputs": [c(0.0), u(0)]}),
        json!({"kind": "BeatTrack2", "rate": "kr",
            "inputs": [c(0.0), c(1.0), c(2.0), c(0.02), c(0.0), c(-2.5), c(3.0)]}),
    ];
    let out = render(g, 48_000 * 4);
    assert!(
        (out.last().unwrap() - 2.5).abs() < 1e-4,
        "{}",
        out.last().unwrap()
    );
}

#[test]
fn pitch_follows_a_tone_and_not_silence() {
    for f in [110.0, 440.0, 1234.0] {
        let g = vec![
            sine(f),
            json!({"kind": "Pitch", "rate": "kr", "inputs": [u(0)]}),
        ];
        let out = render(g, 9600);
        let got = *out.last().unwrap();
        assert!((got / f - 1.0).abs() < 0.003, "{f}: {got}");
    }
    let has = |amp: f32| {
        let g = vec![
            sine(440.0),
            mul(0, amp),
            json!({"kind": "Pitch", "rate": "kr", "inputs": [u(1), c(440.0), c(60.0), c(4000.0),
                c(100.0), c(16.0), c(1.0), c(0.01), c(0.5), c(1.0), c(0.0), c(1.0)]}),
        ];
        *render(g, 9600).last().unwrap()
    };
    assert_eq!(has(1.0), 1.0);
    assert_eq!(has(0.001), 0.0);
}

/// At control rate, the input is taken once a block.
#[test]
fn pitch_reads_a_control_signal_a_block_at_a_time() {
    // 5 Hz at a control rate of 750 Hz.
    let g = vec![
        json!({"kind": "Sine", "rate": "kr", "inputs": [c(5.0)]}),
        json!({"kind": "Pitch", "rate": "kr", "inputs": [u(0), c(440.0), c(2.0), c(100.0), c(10.0)]}),
    ];
    let got = *render(g, 48_000 * 2).last().unwrap();
    assert!((got - 5.0).abs() < 0.05, "{got}");
}
