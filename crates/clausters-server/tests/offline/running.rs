//! The running statistics held against their definitions: a peak since a
//! trigger, a moving sum, a decaying peak, the zero-crossing frequency, and
//! the peak and RMS `SendPeakRMS` reports. `sc_reference` holds them
//! against scsynth.

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

fn render(ugens: Value, n: usize) -> Vec<f32> {
    let mut ugens = ugens.as_array().unwrap().clone();
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), {"ugen": last}]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    render_def(&def, &Run::new(n)).remove(0)
}

/// The input every statistic below reads: noise between -1 and 1.
fn noise() -> Value {
    json!({"kind": "WhiteNoise", "inputs": []})
}

/// `Peak` is the largest `|x|` since the last trigger, the triggering sample
/// included; `RunningSum` the sum of the last `n` samples.
#[test]
fn peak_and_running_sum_follow_their_definitions() {
    let n = 2048;
    let x = render(json!([noise()]), n);
    let peak = render(
        json!([noise(), {"kind": "Impulse", "inputs": [c(100.0), c(0.5)]},
            {"kind": "Peak", "inputs": [u(0), u(1)]}]),
        n,
    );
    let trig = render(
        json!([{"kind": "Impulse", "inputs": [c(100.0), c(0.5)]}]),
        n,
    );
    let mut level = x[0].abs();
    for i in 0..n {
        level = level.max(x[i].abs());
        assert_eq!(peak[i], level, "sample {i}");
        if trig[i] > 0.0 && (i == 0 || trig[i - 1] <= 0.0) {
            level = x[i].abs();
        }
    }
    let sum = render(
        json!([noise(), {"kind": "RunningSum", "inputs": [u(0), c(16.0)]}]),
        n,
    );
    for i in 16..n {
        let want: f32 = x[i - 15..=i].iter().sum();
        assert!((sum[i] - want).abs() < 1e-4, "sample {i}");
    }
}

/// A peak follower falls back by its decay a sample.
#[test]
fn peak_follower_decays_by_its_coefficient() {
    let out = render(
        json!([{"kind": "Impulse", "inputs": [c(0.0), c(0.0)]},
            {"kind": "PeakFollower", "inputs": [u(0), c(0.9)]}]),
        64,
    );
    for (k, &y) in out.iter().enumerate().take(20) {
        assert!((y - 0.9f32.powi(k as i32)).abs() < 1e-6, "sample {k}");
    }
}

/// `ZeroCrossing` measures a sine's frequency to well within a cent.
#[test]
fn zero_crossing_measures_a_frequency() {
    for f in [110.0f32, 1234.5, 4000.0] {
        let out = render(
            json!([{"kind": "Sine", "inputs": [c(f)]}, {"kind": "ZeroCrossing", "inputs": [u(0)]}]),
            9600,
        );
        let got = out[9599];
        assert!((got / f - 1.0).abs() < 1e-4, "{f}: {got}");
    }
}

/// `SendPeakRMS` reports a sine's peak and RMS.
#[test]
fn send_peak_rms_reports_peak_and_rms() {
    use clausters::dsp::registry::{BuildCtx, UGenConfig, lookup};
    use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
    let buses = Buses::new(ControlBuses::new(4), 4);
    let config = UGenConfig {
        inputs: 4,
        ..UGenConfig::default()
    };
    let mut ugen =
        (lookup("SendPeakRMS").unwrap().build)(&config, &BuildCtx::new(SR, BLOCK_SIZE, 1));
    // 10 replies a second: 4800 samples, a whole number of 480 Hz periods.
    let (rate, lag, id) = ([10.0f32], [0.0f32], [5.0f32]);
    let mut replies = Vec::new();
    for b in 0..150 {
        let x: Vec<f32> = (0..BLOCK_SIZE)
            .map(|i| 0.5 * (std::f32::consts::TAU * 480.0 * (b * BLOCK_SIZE + i) as f32 / SR).sin())
            .collect();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses: &buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
            transport: Default::default(),
        };
        let mut output = [0.0f32; BLOCK_SIZE];
        ugen.process(&mut ctx, &[&rate, &lag, &id, &x], &mut output);
        ugen.drain_replies(1, &mut |m| replies.push(m));
    }
    assert_eq!(replies.len(), 2);
    let text = format!("{:?}", replies[1]);
    let (peak, rms) = (replies[1].values()[0], replies[1].values()[1]);
    assert!((peak - 0.5).abs() < 1e-3, "{text}");
    assert!((rms - 0.5 / 2f32.sqrt()).abs() < 1e-3, "{text}");
}
