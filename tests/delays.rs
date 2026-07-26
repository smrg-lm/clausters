//! The delay core (U3): `DelayN/L/C`, `CombN/L/C`, `AllpassN/L/C`.
//!
//! Each family is asserted by the property that *defines* it rather than by a
//! stored buffer: a pure delay places an impulse at an exact frame, a
//! fractional one has the group delay it was asked for, a comb's envelope
//! follows the decay time it was given, and an allpass is flat. That last one
//! is the strongest assert in the track — flatness cannot be satisfied by
//! accident, so a filter that passes it really is an allpass.
//!
//! Rule 5, the block split, is not here: it is the same test for every row and
//! runs from the shared table over all nine at once (`tests/subjects.rs`).

#![cfg(feature = "synth")]

#[path = "common/bench.rs"]
mod bench;
#[path = "common/signal.rs"]
mod signal;

use std::sync::Arc;

use bench::{SR, render_with_input};
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use signal::*;

const WIN: usize = 8192;
const RENDER: usize = 32_768;

/// Snaps a frequency to a whole number of periods per `WIN` samples, so the
/// single-bin measurements are exact (see the `audio-testing` skill).
fn snap(f: f32) -> f32 {
    let bin = SR / WIN as f32;
    (f / bin).round().max(1.0) * bin
}

fn impulse(n: usize) -> Vec<f32> {
    let mut x = vec![0.0f32; n];
    x[0] = 1.0;
    x
}

fn sine(freq: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (std::f32::consts::TAU * freq * i as f32 / SR).sin())
        .collect()
}

fn delay_json(kind: &str, delay: f32, max: f32) -> String {
    format!(
        r#"{{"kind": "{kind}", "inputs": [{{"ugen": 0}}, {{"const": {delay}}}],
            "max_delay": {max}}}"#
    )
}

fn feedback_json(kind: &str, delay: f32, decay: f32, max: f32) -> String {
    format!(
        r#"{{"kind": "{kind}", "inputs": [{{"ugen": 0}}, {{"const": {delay}}},
            {{"const": {decay}}}], "max_delay": {max}}}"#
    )
}

// ---- pure delay ----

#[test]
fn delay_n_places_an_impulse_on_an_exact_frame() {
    // A whole number of frames: the delay is exact and the output is the input
    // shifted, sample for sample.
    for frames in [1usize, 17, 480, 4801] {
        let secs = frames as f32 / SR;
        let y = render_with_input(&delay_json("DelayN", secs, 0.2), &impulse(8192));
        let hit = y.iter().position(|&v| v != 0.0);
        assert_eq!(
            hit,
            Some(frames),
            "DelayN of {frames} frames landed at {hit:?}"
        );
        assert!((y[frames] - 1.0).abs() < 1e-6, "amplitude {}", y[frames]);
        // And nothing else anywhere.
        let others: f32 = y
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != frames)
            .map(|(_, v)| v.abs())
            .sum();
        assert!(others < 1e-6, "a pure delay left {others} elsewhere");
    }
}

#[test]
fn a_zero_delay_passes_the_signal_through() {
    // The line is written before it is read precisely so this works rather than
    // returning whatever is at the far end of the buffer.
    let x = sine(snap(500.0), 4096);
    let y = render_with_input(&delay_json("DelayN", 0.0, 0.2), &x);
    for (i, (a, b)) in x.iter().zip(&y).enumerate() {
        assert!((a - b).abs() < 1e-6, "sample {i}: {a} vs {b}");
    }
}

#[test]
fn fractional_delays_have_the_group_delay_they_were_asked_for() {
    // The whole reason `L` and `C` exist. A delay of 100.5 frames cannot be
    // expressed by `N`, which rounds; the interpolating forms must land within
    // a fraction of a sample.
    let target = 100.5f32;
    let secs = target / SR;
    let x = sine(snap(400.0), RENDER);
    for (kind, tol) in [("DelayL", 0.05f32), ("DelayC", 0.05)] {
        let y = render_with_input(&delay_json(kind, secs, 0.2), &x);
        let d = group_delay_samples(&x[..WIN], &y, 200);
        assert!(
            (d - target).abs() < tol,
            "{kind} asked for {target} frames, measured {d}"
        );
    }
    // `N` rounds to a whole frame, which is its documented contract.
    let y = render_with_input(&delay_json("DelayN", secs, 0.2), &x);
    let d = group_delay_samples(&x[..WIN], &y, 200);
    assert!((d - d.round()).abs() < 0.05, "DelayN gave a fractional {d}");
}

#[test]
fn cubic_interpolation_beats_linear_on_a_fractional_delay() {
    // Both land at the right time; the difference is what they do to the
    // spectrum. Linear interpolation is a lowpass whose loss grows with
    // frequency, so measure the gain of a high tone through a half-sample delay.
    let secs = 100.5 / SR;
    let f = snap(9000.0);
    let x = sine(f, RENDER);
    let mut gains = Vec::new();
    for kind in ["DelayL", "DelayC"] {
        let y = render_with_input(&delay_json(kind, secs, 0.2), &x);
        let from = RENDER - WIN;
        let (g, _) = response_at(&x[from..], &y[from..], f, SR);
        gains.push(20.0 * (g as f64).log10());
    }
    // Measured at 9 kHz through a half-sample delay: linear loses about 1.6 dB,
    // cubic about 0.36 dB. Neither is transparent at three quarters of the way
    // to Nyquist - four-point interpolation is not a brick wall - but the
    // ordering and the size of the gap are the reason to pay for `C`.
    assert!(
        gains[0] < -1.0,
        "linear should lose level: {:.2} dB",
        gains[0]
    );
    assert!(
        gains[1] > gains[0] + 1.0,
        "cubic {:.2} dB should clearly beat linear {:.2} dB",
        gains[1],
        gains[0]
    );
    assert!(
        gains[1].abs() < 0.6,
        "cubic lost more than expected: {:.2} dB",
        gains[1]
    );
}

// ---- comb ----

#[test]
fn comb_decays_by_sixty_db_over_its_decay_time() {
    // `decaytime` is the time for the echo train to fall to 1/1000. The count
    // starts at the **first** echo, which is the direct path and comes back at
    // full amplitude: y[D] = 1, y[2D] = g, y[3D] = g^2. So the envelope is
    // 10^(-3(t - delay)/decay), not 10^(-3t/decay).
    let delay_frames = 480usize; // 10 ms
    let delay = delay_frames as f32 / SR;
    let decay = 0.5f32;
    let n = (SR as usize) / 2 + 9600;
    let y = render_with_input(&feedback_json("CombN", delay, decay, 0.05), &impulse(n));
    for echo in [1usize, 10, 25, 48] {
        let idx = echo * delay_frames;
        let t = idx as f64 / SR as f64;
        let want = 10f64.powf(-3.0 * (t - delay as f64) / decay as f64);
        let got = y[idx].abs() as f64;
        assert!(
            (got / want - 1.0).abs() < 0.02,
            "echo {echo} at {t:.4}s: {got:.6} vs {want:.6}"
        );
    }
    // One decay time after the first echo it really is 60 dB down.
    let idx = (((delay + decay) * SR) as usize / delay_frames) * delay_frames;
    let at_decay = y[idx].abs();
    assert!(
        (at_decay / 1e-3 - 1.0).abs() < 0.1,
        "{at_decay} one decay time in, expected about 0.001"
    );
}

#[test]
fn a_negative_decay_time_inverts_alternate_echoes() {
    // The first echo is the direct path and is never inverted; the sign
    // alternates from the second, which is the first to have gone round the
    // loop.
    let delay = 480.0 / SR;
    let y = render_with_input(&feedback_json("CombN", delay, -0.5, 0.05), &impulse(4096));
    assert!(y[480] > 0.0, "first echo {}", y[480]);
    assert!(y[960] < 0.0, "second echo {}", y[960]);
    assert!(y[1440] > 0.0, "third echo {}", y[1440]);
    assert!(y[1920] < 0.0, "fourth echo {}", y[1920]);
}

#[test]
fn a_zero_decay_time_leaves_a_single_echo() {
    let delay = 480.0 / SR;
    let y = render_with_input(&feedback_json("CombN", delay, 0.0, 0.05), &impulse(4096));
    assert!((y[480] - 1.0).abs() < 1e-6);
    assert!(y[960].abs() < 1e-6, "a zero decay must not repeat");
}

// ---- allpass ----

#[test]
fn allpass_magnitude_is_flat() {
    // *The* test for this family: an allpass changes phase and nothing else, so
    // a flat magnitude is not evidence of correctness, it is the definition.
    //
    // The render has to outlast the ring, or what is measured is the tail of
    // the onset transient rather than the steady state - three decay times puts
    // the transient 180 dB down.
    for kind in ["AllpassN", "AllpassL", "AllpassC"] {
        for decay in [0.2f32, 0.5, 1.0] {
            let json = feedback_json(kind, 480.0 / SR, decay, 0.05);
            let need = (3.0 * decay * SR) as usize + WIN;
            let render = need.div_ceil(WIN) * WIN;
            for f in [snap(120.0), snap(700.0), snap(2500.0), snap(9000.0)] {
                let x = sine(f, render);
                let y = render_with_input(&json, &x);
                let from = render - WIN;
                let (g, _) = response_at(&x[from..], &y[from..], f, SR);
                let db = 20.0 * (g as f64).log10();
                assert!(
                    db.abs() < 0.02,
                    "{kind} (decay {decay}) at {f:.0} Hz: {db:.4} dB"
                );
            }
        }
    }
}

#[test]
fn allpass_still_shifts_phase() {
    // Flat, but not a wire: the phase must actually move, or the "allpass" is a
    // pass-through and the previous test proves nothing.
    let json = feedback_json("AllpassN", 480.0 / SR, 1.0, 0.05);
    let f = snap(700.0);
    let x = sine(f, RENDER);
    let y = render_with_input(&json, &x);
    let from = RENDER - WIN;
    let (_, phase) = response_at(&x[from..], &y[from..], f, SR);
    assert!(
        phase.abs() > 0.1,
        "phase shift {phase} is suspiciously small"
    );
}

// ---- the long run ----
//
// Rule 4, pointed where this family actually accumulates. There is no running
// position here to lose precision in — the read offset is recomputed from the
// delay time every sample, so nothing counts upward. What recirculates is the
// **signal**: a feedback form stores its own output back into an `f32` line, so
// a long decay is a long chain of round trips each quantized once. `delay.rs`
// says `f32` is enough because the line holds signal rather than filter state;
// for the comb and the allpass that reasoning is loose, since what they store
// *is* recursive. These are the tests that settle it.

#[test]
fn an_allpass_ringing_for_ten_seconds_is_still_flat() {
    // Flatness is the allpass's definition and it survives nothing by accident,
    // so it is also the sharpest thing to re-check after thousands of round
    // trips: a 10 ms line over 10 s recirculates a thousand times, each one
    // rounding to `f32`. Same tolerance as the short test — 0.02 dB — because
    // the claim is that the property does not degrade, not that it degrades
    // slowly.
    let n = (SR as usize) * 10;
    for f in [snap(120.0), snap(700.0), snap(2500.0)] {
        let x = sine(f, n);
        let y = render_with_input(&feedback_json("AllpassC", 480.0 / SR, 4.0, 0.05), &x);
        assert_finite(&y, "AllpassC over ten seconds");
        let from = n - n % WIN - WIN;
        let (g, _) = response_at(&x[from..from + WIN], &y[from..from + WIN], f, SR);
        let db = 20.0 * (g as f64).log10();
        assert!(
            db.abs() < 0.02,
            "AllpassC at {f:.0} Hz reads {db:.4} dB after ten seconds of ringing"
        );
    }
}

#[test]
fn a_comb_with_a_long_decay_still_follows_its_envelope_at_ten_seconds() {
    // A thousand round trips in, the echo train must still be where
    // 10^(-3(t-delay)/decay) says. A per-round-trip error would compound
    // geometrically and show here as a level that has drifted off the curve —
    // exactly what a short render cannot see.
    let delay_frames = 480usize;
    let (delay, decay) = (delay_frames as f32 / SR, 30.0f32);
    let n = (SR as usize) * 10;
    let y = render_with_input(&feedback_json("CombN", delay, decay, 0.05), &impulse(n));
    assert_finite(&y, "CombN over ten seconds");
    for echo in [900usize, 950, 999] {
        let idx = echo * delay_frames;
        let t = idx as f64 / SR as f64;
        let want = 10f64.powf(-3.0 * (t - delay as f64) / decay as f64);
        let got = y[idx].abs() as f64;
        assert!(
            (got / want - 1.0).abs() < 0.01,
            "echo {echo} at {t:.3}s: {got:.6} vs {want:.6}"
        );
    }
}

#[test]
fn a_comb_driven_for_ten_seconds_stays_under_its_analytic_ceiling() {
    // The other half: not an impulse dying away but a full-scale signal going
    // in for the whole run, so the loop is charged rather than draining. The
    // steady-state gain of a comb at one of its peaks is 1/(1-g), which is the
    // ceiling — the test is that it converges to it instead of walking past it.
    let delay = 480.0 / SR;
    let decay = 2.0f32;
    let g = 10f64.powf(-3.0 * delay as f64 / decay as f64);
    let ceiling = 1.0 / (1.0 - g);
    let n = (SR as usize) * 10;
    let x: Vec<f32> = {
        let mut rng = clausters_core::rng::WhiteNoise::from_seed(0xDECA1);
        (0..n).map(|_| rng.next_sample()).collect()
    };
    let y = render_with_input(&feedback_json("CombL", delay, decay, 0.05), &x);
    assert_finite(&y, "CombL charged for ten seconds");
    let worst = peak(&y) as f64;
    assert!(
        worst < ceiling,
        "peak {worst:.2} after ten seconds, past the analytic ceiling {ceiling:.2}"
    );
    // And it really did charge: a loop that quietly lost its feedback would
    // also pass the bound above.
    assert!(
        worst > 3.0,
        "peak {worst:.2} -- the comb does not seem to be resonating at all"
    );
}

// ---- bounds and modulation ----

#[test]
fn a_delay_longer_than_the_line_is_clamped_not_wrapped() {
    // Asking for more than `max_delay` must saturate at the longest the line
    // can serve, not fold back to a short delay.
    let y = render_with_input(&delay_json("DelayN", 1.0, 0.01), &impulse(4096));
    let hit = y.iter().position(|&v| v != 0.0).unwrap();
    let max_frames = (0.01 * SR) as usize;
    assert!(
        hit >= max_frames && hit <= max_frames + 4,
        "clamped to {hit} frames, line holds about {max_frames}"
    );
}

#[test]
fn a_modulated_delay_time_stays_finite_and_bounded() {
    // A chorus: an LFO sweeping the delay time, through the interpolating and
    // the feedback forms alike.
    let json = r#"{"name": "chorus", "ugens": [
        {"kind": "In", "inputs": [{"const": 1.0}]},
        {"kind": "LFTri", "inputs": [{"const": 3.0}, {"const": 0.0}]},
        {"kind": "MulAdd", "inputs": [{"ugen": 1}, {"const": 0.004}, {"const": 0.006}]},
        {"kind": "AllpassC", "inputs": [{"ugen": 0}, {"ugen": 2}, {"const": 2.0}],
         "max_delay": 0.02},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}]}"#;
    let def = compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut rng = clausters_core::rng::WhiteNoise::from_seed(99);
    let mut worst = 0.0f32;
    for _ in 0..3000 {
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
        assert_finite(buses.audio(0), "AllpassC under a modulated delay time");
        worst = worst.max(peak(buses.audio(0)));
    }
    assert!(worst < 8.0, "peak {worst} under a modulated delay");
}

#[test]
fn a_delay_with_no_max_delay_field_still_builds() {
    // The field defaults rather than failing the def, like `fft_size` does.
    let y = render_with_input(
        r#"{"kind": "DelayN", "inputs": [{"ugen": 0}, {"const": 0.01}]}"#,
        &impulse(4096),
    );
    assert_eq!(y.iter().position(|&v| v != 0.0), Some((0.01 * SR) as usize));
}
