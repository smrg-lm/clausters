//! `OscUGens.cpp`'s oscillators held against the theory, where scsynth's own
//! output is no reference: `Formant`'s restart (scsynth's overflows), what
//! `Blip`'s and `Klank`'s spectra and decays are by definition, and the
//! recurrences against the sine they compute. `sc_reference` holds the rest
//! against scsynth.

#![cfg(feature = "synth")]

use crate::bench::{Run, render_def_with_done};
use crate::signal::{amplitude_at, coherent_freq};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;

fn c(v: f32) -> Value {
    json!({ "const": v })
}

/// Renders `n` samples of a graph whose last UGen goes to bus 0, and the done
/// action reported after each block.
fn render(ugens: Value, n: usize) -> (Vec<f32>, Vec<u8>) {
    let mut ugens = ugens.as_array().unwrap().clone();
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), {"ugen": last}]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    let (mut out, done) = render_def_with_done(&def, &Run::new(n));
    (out.remove(0), done)
}

/// `Blip` is `numharm` harmonics of equal amplitude `1/numharm`, and nothing
/// above them.
#[test]
fn blip_is_its_harmonics_and_nothing_else() {
    let n = 1 << 15;
    let f = coherent_freq(300.0, SR, n);
    let (out, _) = render(json!([{"kind": "Blip", "inputs": [c(f), c(8.0)]}]), n);
    for k in 1..=8 {
        let a = amplitude_at(&out, f * k as f32, SR);
        assert!((a - 1.0 / 8.0).abs() < 1e-3, "harmonic {k}: {a}");
    }
    for k in 9..=12 {
        assert!(
            amplitude_at(&out, f * k as f32, SR) < 1e-4,
            "harmonic {k} is not there"
        );
    }
}

/// `Formant` restarts every burst at the fundamental's crossing, so its
/// output repeats at the fundamental's period whatever the formant's
/// frequency -- which scsynth's, whose restart overflows, does not.
#[test]
fn formant_repeats_at_the_fundamental() {
    // 1 kHz: 48 samples a period, exactly.
    let (out, _) = render(
        json!([{"kind": "Formant", "inputs": [c(1000.0), c(3170.0), c(1500.0)]}]),
        4800,
    );
    for i in 96..out.len() - 48 {
        assert!((out[i + 48] - out[i]).abs() < 1e-4, "sample {i}");
    }
    // The burst is the window over the carrier: no larger than the window's
    // peak of 2.
    assert!(out.iter().all(|x| x.abs() <= 2.0));
}

/// A `Klank` resonator rings at its frequency and falls 60 dB over its ring
/// time.
#[test]
fn klank_rings_its_time_to_minus_sixty_db() {
    let time = 0.05;
    let n = (SR * 0.12) as usize;
    let (out, _) = render(
        json!([
            {"kind": "Impulse", "inputs": [c(0.0), c(0.0)]},
            {"kind": "Klank", "inputs": [{"ugen": 0}, c(1.0), c(0.0), c(1.0),
                c(1000.0), c(1.0), c(time)]}
        ]),
        n,
    );
    let peak = |from: usize| {
        out[from..from + 96]
            .iter()
            .map(|x| x.abs())
            .fold(0.0, f32::max)
    };
    let at = (time * SR) as usize;
    // Measured from 1 ms in, so over the ring time less that millisecond.
    let drop = 20.0 * (peak(at) / peak(48)).log10();
    let want = -60.0 * (at - 48) as f32 / at as f32;
    assert!((drop - want).abs() < 0.5, "fell {drop} dB, {want} expected");
    // A period of 48 samples: the crossings are a kilohertz apart.
    let crossings = out[48..at]
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count() as f32;
    let hz = crossings / ((at - 48) as f32 / SR);
    assert!((hz - 1000.0).abs() < 25.0, "rings at {hz} Hz");
}

/// `FSinOsc`'s recurrence is the sine it computes, to rounding, over a long
/// run; `SinOscFB` without feedback is the sine too.
#[test]
fn the_recurrences_are_the_sine() {
    let n = 48_000;
    let f = 441.3;
    let (fsin, _) = render(json!([{"kind": "FSinOsc", "inputs": [c(f), c(0.25)]}]), n);
    let (fb, _) = render(json!([{"kind": "SinOscFB", "inputs": [c(f), c(0.0)]}]), n);
    for (i, (&a, &b)) in fsin.iter().zip(&fb).enumerate() {
        let t = i as f64 * f as f64 / SR as f64;
        let want = (std::f64::consts::TAU * t).sin();
        let cos = (std::f64::consts::TAU * (t + 0.25)).sin();
        assert!((a as f64 - cos).abs() < 1e-5, "FSinOsc at {i}");
        assert!((b as f64 - want).abs() < 1e-6, "SinOscFB at {i}");
    }
}

/// `PSinGrain` is one grain under a parabola peaking at its amplitude, and
/// frees its synth when the grain ends.
#[test]
fn psin_grain_is_one_grain_and_frees_its_synth() {
    let dur = 0.01;
    let samples = (dur * SR) as usize;
    let (out, done) = render(
        json!([{"kind": "PSinGrain", "inputs": [c(2000.0), c(dur), c(0.5)]}]),
        samples + 256,
    );
    let peak = out.iter().map(|x| x.abs()).fold(0.0, f32::max);
    assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
    assert!(
        out[samples..].iter().all(|&x| x == 0.0),
        "silent after the grain"
    );
    let block = samples / 64;
    assert!(
        done[..block].iter().all(|&d| d == 0),
        "alive during the grain"
    );
    assert_eq!(*done.last().unwrap(), 2, "frees its synth after");
}
