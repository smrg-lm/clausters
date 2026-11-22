//! `LFUGens.cpp`'s rows held against the theory, where scsynth's own output
//! is no reference: what its priming shortens (a vibrato's delay and onset),
//! what it computes wrongly on purpose here (`IEnvGen`'s curvature and hold,
//! `Unwrap`'s start, `Linen`'s short gate), and the shapes' relation to the
//! sine they approximate. `sc_reference` holds the rest against scsynth.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;

/// Renders `blocks` blocks of a graph whose last UGen is written to bus 0,
/// one value per sample at audio rate, one per block at control rate.
fn render(ugens: Value, blocks: usize) -> Vec<f32> {
    let mut ugens = ugens.as_array().unwrap().clone();
    let probe = ugens.len() - 1;
    let audio = ugens[probe].get("rate").is_none_or(|r| r == "ar");
    ugens.push(json!({"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": probe}]}));
    let spec: SynthDefSpec = serde_json::from_value(json!({"name": "t", "ugens": ugens})).unwrap();
    let def = compile(spec).unwrap();
    let (mut engine, mut handle) = engine_pair(SR, 2);
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(Arc::new(def), SR, 1)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut got = Vec::new();
    for _ in 0..blocks {
        engine.process_block(&mut out);
        handle.collect_garbage();
        if audio {
            got.extend(out.iter().step_by(2));
        } else {
            got.push(out[0]);
        }
    }
    got
}

fn c(v: f32) -> Value {
    json!({ "const": v })
}

/// The vibrato passes `freq` untouched through its delay, then fades the
/// deviation in over the onset, linearly, to the full depth.
#[test]
fn vibrato_waits_its_delay_then_fades_in() {
    let (delay, onset) = (48usize, 96usize);
    let out = render(
        json!([
            {"kind": "DC", "inputs": [c(440.0)]},
            {"kind": "Vibrato", "inputs": [{"ugen": 0}, c(500.0), c(0.1),
                c(delay as f32 / SR), c(onset as f32 / SR), c(0.0), c(0.0), c(0.0), c(0.0)]}
        ]),
        8,
    );
    assert!(
        out[..delay].iter().all(|&x| x == 440.0),
        "the delay passes freq"
    );
    // During the onset the deviation is the parabola scaled by the onset's
    // level, (k + 1) / (onset + 1) at its k-th sample.
    let period = 4.0 * 500.0 / SR as f64;
    for k in 0..onset {
        let phase = -1.0 + period * k as f64;
        let w = if phase < 1.0 {
            1.0 - phase * phase
        } else {
            let z = phase - 2.0;
            z * z - 1.0
        };
        let level = (k + 1) as f64 / (onset + 1) as f64;
        let want = 440.0 * (1.0 + level * 0.1 * w);
        assert!(
            (out[delay + k] as f64 - want).abs() < 1e-3,
            "onset sample {k}"
        );
    }
    let peak = out[delay + onset..]
        .iter()
        .map(|x| (x - 440.0).abs())
        .fold(0.0, f32::max);
    assert!(
        (peak - 44.0).abs() < 0.1,
        "the full depth after the onset: {peak}"
    );
}

/// A gate shorter than the rise releases from where the level got to, as an
/// envelope's release node does, rather than finishing the rise.
#[test]
fn linen_releases_a_gate_shorter_than_its_rise() {
    // Open for 5 ms of a 100 ms rise, then a 20 ms release.
    let out = render(
        json!([
            {"kind": "LFPulse", "rate": "kr", "inputs": [c(2.0), c(0.0), c(0.01)]},
            {"kind": "Linen", "rate": "kr", "inputs": [{"ugen": 0}, c(0.1), c(1.0), c(0.02), c(0.0)]}
        ]),
        200,
    );
    let peak = out.iter().copied().fold(0.0, f32::max);
    assert!(
        peak > 0.0 && peak < 0.1,
        "it stopped rising when the gate closed: {peak}"
    );
    assert!(
        out[40..].iter().all(|&x| x.abs() < 1e-6),
        "and released to 0"
    );
}

/// `IEnvGen`'s held segment is its start level wherever the index is, and a
/// segment bends by the curvature given, not by its integer part.
#[test]
fn ienvgen_holds_its_start_and_bends_by_the_curvature_given() {
    let ienv = |index: f32, segs: &[[f32; 4]]| {
        let total: f32 = segs.iter().map(|s| s[0]).sum();
        let mut inputs = vec![c(index), c(0.0), c(0.0), c(segs.len() as f32), c(total)];
        for s in segs {
            inputs.extend(s.iter().map(|&v| c(v)));
        }
        render(json!([{"kind": "IEnvGen", "inputs": inputs}]), 1)[0]
    };
    // A hold from 0.2 to 0.8: 0.2 throughout, whatever was read before.
    let hold = [
        [0.1, 1.0, 0.0, 0.2],
        [0.3, 8.0, 0.0, 0.8],
        [0.1, 1.0, 0.0, 0.0],
    ];
    assert_eq!(ienv(0.25, &hold), 0.2);
    // Curvature 2.5 at the middle of a 0 -> 1 segment.
    let got = ienv(0.1, &[[0.2, 5.0, 2.5, 1.0]]);
    let want = (1.0 - (0.5f64 * 2.5).exp()) / (1.0 - 2.5f64.exp());
    assert!((got as f64 - want).abs() < 1e-6, "{got} vs {want}");
}

/// `Unwrap` starts inside its range: a start above it comes down by whole
/// ranges, one below it goes up.
#[test]
fn unwrap_brings_its_start_into_the_range() {
    for (x, want) in [(1.5, 0.5), (-0.25, 0.75), (3.0, 0.0), (0.5, 0.5)] {
        let out = render(
            json!([
                {"kind": "DC", "inputs": [c(x)]},
                {"kind": "Unwrap", "inputs": [{"ugen": 0}, c(0.0), c(1.0)]}
            ]),
            1,
        );
        assert!(
            (out[0] - want).abs() < 1e-6,
            "{x} -> {} (want {want})",
            out[0]
        );
    }
}

/// Hard sync: the slave restarts with every master cycle, so the output
/// repeats at the master's period whatever the slave's frequency.
#[test]
fn sync_saw_repeats_at_the_master() {
    let out = render(
        json!([{"kind": "SyncSaw", "inputs": [c(1000.0), c(3170.0)]}]),
        4,
    );
    for n in 48..out.len() - 48 {
        assert!((out[n + 48] - out[n]).abs() < 1e-4, "sample {n}");
    }
}

/// `LFCub` and `LFPar` approximate a sine and a cosine: the right period,
/// starting where the two start, within their polynomials' error (0.020
/// and 0.056 of full scale, at their worst).
#[test]
fn lf_cub_and_par_approximate_the_sine() {
    let f = 750.0;
    for (kind, start_phase, worst) in [("LFCub", 0.0, 0.021), ("LFPar", 0.25, 0.057)] {
        let out = render(json!([{"kind": kind, "inputs": [c(f), c(0.0)]}]), 2);
        let mut err = 0.0f64;
        for (n, &y) in out.iter().enumerate() {
            let t = n as f64 * f as f64 / SR as f64 + start_phase;
            err = err.max((y as f64 - (std::f64::consts::TAU * t).sin()).abs());
        }
        assert!(err < worst, "{kind}: {err}");
    }
}

/// `T2A` puts the trigger at the offset it is given, once per rising edge.
#[test]
fn t2a_places_the_trigger_at_its_offset() {
    let out = render(
        json!([
            {"kind": "DC", "rate": "kr", "inputs": [c(0.6)]},
            {"kind": "T2A", "inputs": [{"ugen": 0}, c(9.0)]}
        ]),
        2,
    );
    let hits: Vec<(usize, f32)> = out
        .iter()
        .copied()
        .enumerate()
        .filter(|&(_, x)| x != 0.0)
        .collect();
    assert_eq!(hits, vec![(9, 0.6)]);
}
