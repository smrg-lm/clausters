//! The chaotic maps and Gendy held against the theory where scsynth's code
//! is no reference: the Henon map as it is written (scsynth's `HenonN` and
//! `HenonL` exchange its initial values) and its divergence, the
//! interpolated forms starting at the map's first value, a restart on a new
//! initial condition, and Gendy's ranges and Gendy3's pitch.
//! `sc_reference` holds the rest against scsynth.

#![cfg(feature = "synth")]

use crate::bench::{Run, render_def};
use serde_json::{Value, json};

fn c(v: f32) -> Value {
    json!({ "const": v })
}

fn render(ugens: Value, n: usize) -> Vec<f32> {
    let mut ugens = ugens.as_array().unwrap().clone();
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), {"ugen": last}]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    render_def(&def, &Run::new(n)).remove(0)
}

fn one(kind: &str, inputs: &[f32], n: usize) -> Vec<f32> {
    let inputs: Vec<Value> = inputs.iter().map(|&v| c(v)).collect();
    render(json!([{"kind": kind, "inputs": inputs}]), n)
}

/// At the full rate `HenonN` is the Henon map from `x0`, `x1`, one value a
/// sample, starting at `x1`.
#[test]
fn henon_is_the_henon_map() {
    // The parameters as the wire carries them, in f32: the map is chaotic.
    let (a, b, x0, x1) = (1.4f32 as f64, 0.3f32 as f64, 0.1f32 as f64, 0.2f32 as f64);
    let out = one(
        "HenonN",
        &[48_000.0, a as f32, b as f32, x0 as f32, x1 as f32],
        64,
    );
    let (mut p, mut q) = (x0, x1);
    for (k, &y) in out.iter().enumerate() {
        assert!((y as f64 - q).abs() < 1e-5, "x[{}]", k + 1);
        (p, q) = (q, 1.0 - a * q * q + b * p);
    }
}

/// A Henon map that diverges holds its last value instead of running off.
#[test]
fn a_diverged_henon_map_holds() {
    let out = one("HenonN", &[48_000.0, 2.0, 0.3, 1.0, 1.0], 256);
    assert!(out.iter().all(|x| x.abs() <= 1.5), "within the map's bound");
    let last = out[255];
    assert!(out[200..].iter().all(|&x| x == last), "and held");
}

/// The interpolated forms start at the map's first value: scsynth's cubic
/// forms start at 0, and its `LinCongL` at `xi` unscaled.
#[test]
fn the_interpolated_forms_start_at_the_first_value() {
    // A tenth of the rate: ten samples a step.
    let q = one("QuadC", &[4800.0, -1.0, 0.0, 0.75, 0.3], 8);
    assert!(q.iter().all(|&x| (x - 0.3).abs() < 1e-6), "QuadC: {q:?}");
    // xi 0.25 of a modulus of 1 is -0.5 scaled.
    let l = one("LinCongL", &[4800.0, 1.1, 0.13, 1.0, 0.25], 8);
    assert!(l.iter().all(|&x| (x + 0.5).abs() < 1e-6), "LinCongL: {l:?}");
}

/// A new initial condition restarts the map from it.
#[test]
fn a_new_initial_condition_restarts_the_map() {
    // xi 0.2 for half a second, then 0.7.
    let out = render(
        json!([
            {"kind": "LFPulse", "rate": "kr", "inputs": [c(1.0), c(0.5), c(0.5)]},
            {"kind": "MulAdd", "rate": "kr", "inputs": [{"ugen": 0}, c(0.5), c(0.2)]},
            {"kind": "QuadN", "inputs": [c(100.0), c(-1.0), c(0.0), c(0.75), {"ugen": 1}]}
        ]),
        64 * 380,
    );
    assert!((out[0] - 0.2).abs() < 1e-6);
    let at = out.iter().position(|&x| (x - 0.7).abs() < 1e-6);
    assert!(at.is_some(), "the map restarted at the new xi");
}

/// The Lorenz attractor stays bounded, and the standard map within +/-1.
#[test]
fn the_flows_stay_on_their_attractors() {
    let lorenz = one(
        "LorenzL",
        &[22_050.0, 10.0, 28.0, 2.667, 0.05, 0.1, 0.0, 0.0],
        48_000,
    );
    assert!(lorenz.iter().all(|x| x.abs() < 1.2), "x within +/-30");
    let standard = one("StandardN", &[48_000.0, 1.5, 0.5, 0.0], 48_000);
    assert!(standard.iter().all(|x| x.abs() <= 1.0));
}

/// Gendy's levels stay within +/-1 however hard they walk -- scsynth's
/// `Gendy2` reflects a level below -1 out of the range -- and `Gendy3` keeps
/// its pitch: with its walks still it repeats at its period, every cycle
/// starting from 0.
#[test]
fn gendy_stays_in_range_and_gendy3_keeps_its_pitch() {
    for kind in ["Gendy1", "Gendy2"] {
        let mut inputs = vec![0.0, 0.0, 1.0, 1.0, 200.0, 2000.0, 1.0, 1.0, 12.0, 12.0];
        if kind == "Gendy2" {
            inputs.extend([1.17, 0.31]);
        }
        let out = one(kind, &inputs, 48_000);
        assert!(out.iter().all(|x| x.abs() <= 1.0), "{kind} leaves +/-1");
    }
    let out = one(
        "Gendy3",
        &[0.0, 0.0, 1.0, 1.0, 480.0, 0.0, 0.0, 10.0, 10.0],
        4800,
    );
    for n in 100..out.len() - 100 {
        assert!((out[n + 100] - out[n]).abs() < 1e-5, "sample {n}");
    }
    let walking = one(
        "Gendy3",
        &[1.0, 1.0, 1.0, 1.0, 480.0, 1.0, 1.0, 10.0, 10.0],
        48_000,
    );
    assert!(walking.iter().all(|x| x.abs() <= 1.0));
    assert!(
        walking.iter().step_by(100).all(|x| x.abs() < 0.2),
        "each cycle starts near 0"
    );
}
