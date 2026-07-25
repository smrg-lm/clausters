//! The measurement harness's own tests (U0).
//!
//! Every helper in `tests/common/signal.rs` is driven here with a signal whose
//! answer is known in closed form, so a broken *measurement* fails in this
//! suite rather than silently passing — or failing — a UGen elsewhere.

#[path = "common/signal.rs"]
mod signal;

use signal::*;

const SR: f32 = 48_000.0;
const N: usize = 4096;

fn sine(freq: f32, amp: f32, n: usize, phase: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = std::f32::consts::TAU * freq * i as f32 / SR + phase;
            amp * t.sin()
        })
        .collect()
}

#[test]
fn rms_and_peak_match_a_sine_of_known_amplitude() {
    let f = coherent_freq(1000.0, SR, N);
    let x = sine(f, 0.5, N, 0.0);
    assert!((peak(&x) - 0.5).abs() < 1e-3);
    assert!((rms(&x) - 0.5 / 2.0f32.sqrt()).abs() < 1e-4);
    assert!(dc(&x).abs() < 1e-5);
    assert_finite(&x, "sine");
}

#[test]
fn amplitude_and_phase_recover_a_known_component() {
    let f = coherent_freq(997.0, SR, N);
    // Two components; the measurement must isolate each.
    let g = coherent_freq(3011.0, SR, N);
    let a = sine(f, 0.4, N, 0.0);
    let b = sine(g, 0.1, N, 0.0);
    let x: Vec<f32> = a.iter().zip(&b).map(|(p, q)| p + q).collect();
    assert!((amplitude_at(&x, f, SR) - 0.4).abs() < 1e-4);
    assert!((amplitude_at(&x, g, SR) - 0.1).abs() < 1e-4);
    // A sine is -90 deg relative to the cosine basis of the DFT.
    let phase = phase_at(&x, f, SR);
    assert!(
        (phase + std::f32::consts::FRAC_PI_2).abs() < 1e-3,
        "{phase}"
    );
}

#[test]
fn response_at_reports_a_known_gain_and_delay() {
    let f = coherent_freq(500.0, SR, N);
    let input = sine(f, 1.0, N, 0.0);
    // A pure gain and a pure phase shift, applied analytically.
    let shift = 0.7f32;
    let output: Vec<f32> = (0..N)
        .map(|i| {
            let t = std::f32::consts::TAU * f * i as f32 / SR;
            0.25 * (t + shift).sin()
        })
        .collect();
    let (gain, phase) = response_at(&input, &output, f, SR);
    assert!((gain - 0.25).abs() < 1e-4, "gain {gain}");
    assert!((phase - shift).abs() < 1e-3, "phase {phase}");
}

#[test]
fn coherent_freq_lands_on_an_odd_whole_number_of_periods() {
    for target in [100.0, 440.0, 1000.0, 4000.0] {
        let f = coherent_freq(target, SR, N);
        let k = f as f64 * N as f64 / SR as f64;
        assert!((k - k.round()).abs() < 1e-6, "{f} is not bin-aligned");
        assert_eq!(k.round() as i64 % 2, 1, "{f} is not on an odd bin");
        // Snapping must stay near what was asked for.
        assert!((f - target).abs() < 2.0 * SR / N as f32);
    }
}

#[test]
fn alias_snr_is_huge_for_a_pure_tone_and_finite_for_a_naive_saw() {
    // A single coherent sine has no non-harmonic energy at all: the floor
    // is the arithmetic, not the signal.
    let f = coherent_freq(1000.0, SR, N);
    let pure = sine(f, 0.5, N, 0.0);
    let clean = alias_snr_db(&pure, f, SR);
    assert!(clean > 80.0, "a pure tone measured only {clean} dB");

    // A naive (non-band-limited) saw at a high fundamental aliases badly,
    // and the measurement has to *see* that.
    let f = coherent_freq(4000.0, SR, N);
    let dt = f as f64 / SR as f64;
    let mut phase = 0.0f64;
    let naive: Vec<f32> = (0..N)
        .map(|_| {
            let v = 2.0 * phase - 1.0;
            phase += dt;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            v as f32
        })
        .collect();
    let dirty = alias_snr_db(&naive, f, SR);
    assert!(
        (0.0..30.0).contains(&dirty),
        "a naive saw at 4 kHz should be visibly dirty, measured {dirty} dB"
    );
    assert!(clean > dirty + 50.0);
}

#[test]
fn spectral_slope_reads_zero_for_white_and_minus_three_for_pink() {
    // A deterministic white source, then the standard one-pole pinking
    // filter, whose response is -3 dB/octave over the fitted band.
    let mut rng = clausters_core::rng::WhiteNoise::from_seed(0x5EED_1234);
    let white: Vec<f32> = (0..1 << 17).map(|_| rng.next_sample()).collect();
    let s = spectral_slope_db_per_octave(&white, SR, 100.0, 12_800.0);
    assert!(s.abs() < 0.3, "white noise slope {s} dB/oct");

    // Voss-style approximation is U6's job; here a cascade of one-poles
    // shaped to -3 dB/oct is enough to prove the *estimator* reads it.
    let pink = pinken(&white, SR);
    let s = spectral_slope_db_per_octave(&pink, SR, 100.0, 12_800.0);
    assert!((s + 3.01).abs() < 0.5, "pink noise slope {s} dB/oct");
}

/// A textbook −3 dB/octave shaper: three one-pole/one-zero sections spaced
/// a decade apart (Robert Bristow-Johnson's coefficients), accurate to
/// about ±0.3 dB from 10 Hz to 20 kHz at 44.1–48 kHz.
fn pinken(x: &[f32], _sr: f32) -> Vec<f32> {
    let (mut b0, mut b1, mut b2) = (0.0f64, 0.0f64, 0.0f64);
    x.iter()
        .map(|&v| {
            let w = v as f64;
            b0 = 0.99765 * b0 + w * 0.0990460;
            b1 = 0.96300 * b1 + w * 0.2965164;
            b2 = 0.57000 * b2 + w * 1.0526913;
            ((b0 + b1 + b2 + w * 0.1848) * 0.2) as f32
        })
        .collect()
}

#[test]
fn group_delay_recovers_an_integer_and_a_fractional_shift() {
    let f = coherent_freq(300.0, SR, 2048);
    let x = sine(f, 1.0, 2048, 0.0);
    // Integer delay: exact.
    let mut y = vec![0.0f32; 2048];
    y[17..].copy_from_slice(&x[..2048 - 17]);
    let d = group_delay_samples(&x[..1024], &y, 64);
    assert!((d - 17.0).abs() < 0.05, "integer delay measured {d}");

    // Fractional delay, produced analytically by shifting the phase.
    let frac = 3.4f32;
    let y: Vec<f32> = (0..2048)
        .map(|i| {
            let t = std::f32::consts::TAU * f * (i as f32 - frac) / SR;
            t.sin()
        })
        .collect();
    let d = group_delay_samples(&x[..1024], &y, 64);
    assert!((d - frac).abs() < 0.15, "fractional delay measured {d}");
}

/// Not an assert — a printed record of what the harness actually measures, so
/// the numbers the U-track docs quote come from a run rather than from memory.
/// Read it with `cargo test --test signal -- --nocapture report`.
#[test]
fn report_the_measured_figures() {
    let f = coherent_freq(1000.0, SR, N);
    println!("pure tone at {f:.2} Hz: alias SNR {:.1} dB", {
        let x = sine(f, 0.5, N, 0.0);
        alias_snr_db(&x, f, SR)
    });
    for target in [100.0, 1000.0, 4000.0] {
        let f = coherent_freq(target, SR, N);
        let dt = f as f64 / SR as f64;
        let mut phase = 0.0f64;
        let naive: Vec<f32> = (0..N)
            .map(|_| {
                let v = 2.0 * phase - 1.0;
                phase += dt;
                if phase >= 1.0 {
                    phase -= 1.0;
                }
                v as f32
            })
            .collect();
        println!(
            "naive saw at {f:8.2} Hz: alias SNR {:6.1} dB",
            alias_snr_db(&naive, f, SR)
        );
    }
}
