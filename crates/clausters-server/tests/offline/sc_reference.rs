//! The server's UGens held against SuperCollider's own code.
//!
//! `tests/sc_reference/vectors.json` holds small graphs and what scsynth's
//! plugins computed for them, run through a harness that builds the part of
//! scsynth a plugin sees (`tests/sc_reference/harness.cpp`, driven by
//! `gen.py`, which says how to regenerate the file from a checkout). This
//! module renders each graph through the server and compares the probed wire
//! sample by sample, so a port that drifted from scsynth -- an off-by-one in a
//! trigger, a coefficient computed at the wrong rate, a phase that starts a
//! sample late -- fails here with the case's name and the first sample that
//! differs.
//!
//! A case may allow a difference, and must say why: a UGen realized
//! differently on purpose (`Sine` is not a wavetable) is held to what the
//! decision keeps rather than to scsynth's samples, and its `why` names the
//! decision. Nothing here needs SuperCollider to run.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::dsp::buffer::Buffer;
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

const VECTORS: &str = include_str!("../sc_reference/vectors.json");

/// The default tolerance: the difference two correct computations of the same
/// formula may show after a few hundred samples of state, one of them in
/// `f32` and the other in `f64`, or the two in another order.
const DEFAULT_TOL: f64 = 1e-5;

/// The server's spec for a case's graph: its UGens with scsynth's spellings
/// removed, and an `Out` reading the probed one.
fn spec(case: &Value) -> (SynthDefSpec, bool) {
    let mut ugens: Vec<Value> = case["ugens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            let mut u = u.clone();
            u.as_object_mut().unwrap().remove("sc");
            u
        })
        .collect();
    let probe = case
        .get("probe")
        .and_then(Value::as_u64)
        .unwrap_or(ugens.len() as u64 - 1);
    let audio = ugens[probe as usize]["rate"] == "ar";
    ugens.push(json!({"kind": "Out", "rate": "ar",
        "inputs": [{"const": 0.0}, {"ugen": probe}]}));
    let def = json!({"name": case["name"], "ugens": ugens});
    (serde_json::from_value(def).unwrap(), audio)
}

/// The probed wire, as scsynth's harness reports it: every sample of an
/// audio-rate wire, one value per block of any other.
fn render(case: &Value) -> Result<Vec<f32>, String> {
    let (spec, audio) = spec(case);
    let def = clausters::synthdef::compile(spec).map_err(|e| format!("compile: {e}"))?;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    for b in case["buffers"].as_array().into_iter().flatten() {
        let data: Vec<f32> = b["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        let channels = b["channels"].as_u64().unwrap() as usize;
        let frames = b["frames"].as_u64().unwrap() as usize;
        let sr = b.get("sr").and_then(Value::as_f64).unwrap_or(SR as f64);
        let mut samples = vec![0.0; channels * frames];
        let n = data.len().min(samples.len());
        samples[..n].copy_from_slice(&data[..n]);
        handle
            .send(Cmd::SetBuffer {
                index: b["num"].as_u64().unwrap() as usize,
                buffer: Some(Arc::new(Buffer::new(samples, channels, frames, sr))),
            })
            .ok()
            .unwrap();
    }
    let seed = case.get("seed").and_then(Value::as_u64).unwrap_or(0);
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(Arc::new(def), SR, seed)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    let mut got = Vec::new();
    for _ in 0..case["blocks"].as_u64().unwrap() {
        engine.process_block(&mut out);
        handle.collect_garbage();
        let left = out.iter().step_by(CHANNELS).copied();
        if audio {
            got.extend(left);
        } else {
            got.push(out[0]);
        }
    }
    Ok(got)
}

/// Where `got` first leaves `want` by more than `tol`, and the largest
/// difference anywhere.
fn compare(got: &[f32], want: &[f64], tol: f64) -> Option<String> {
    if got.len() != want.len() {
        return Some(format!("{} values, scsynth has {}", got.len(), want.len()));
    }
    let mut first = None;
    let mut worst = 0.0f64;
    for (i, (&g, &w)) in got.iter().zip(want).enumerate() {
        let g = g as f64;
        let err = if (g.is_nan() && w.is_nan()) || g == w {
            0.0
        } else {
            (g - w).abs() / w.abs().max(1.0)
        };
        let err = if err.is_nan() { f64::INFINITY } else { err };
        worst = worst.max(err);
        if err > tol && first.is_none() {
            first = Some(i);
        }
    }
    first.map(|i| {
        let from = i.saturating_sub(2);
        let to = (i + 3).min(got.len());
        format!(
            "first at {i}: got {:?}, scsynth {:?}; worst {worst:.3e} (tol {tol:.1e})",
            &got[from..to],
            &want[from..to]
        )
    })
}

#[test]
fn every_ugen_computes_what_scsynth_computes() {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    let mut failures = Vec::new();
    let cases = doc["cases"].as_array().unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let tol = case
            .get("tol")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_TOL);
        assert!(
            tol <= 1e-4 || case.get("why").is_some(),
            "{name}: a tolerance of {tol} needs a `why`"
        );
        let want: Vec<f64> = case["expect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| match v {
                // JSON has no NaN or infinity: gen.py writes their names.
                Value::String(s) => s.parse().unwrap(),
                v => v.as_f64().unwrap(),
            })
            .collect();
        // A random source's stream starts one sample's draws later in scsynth,
        // whose constructor runs a sample of the calculation: `shift` says by
        // how many samples the server's output leads.
        let shift = case.get("shift").and_then(Value::as_u64).unwrap_or(0) as usize;
        assert!(
            shift == 0 || case.get("why").is_some(),
            "{name}: a shift needs a `why`"
        );
        // A transient of scsynth's own at the start (a first block its
        // constructor's state leaves different): `skip` says how many of the
        // first values are not compared.
        let skip = case.get("skip").and_then(Value::as_u64).unwrap_or(0) as usize;
        assert!(
            skip == 0 || case.get("why").is_some(),
            "{name}: a skip needs a `why`"
        );
        match render(case) {
            Err(e) => failures.push(format!("{name}: {e}")),
            Ok(got) => {
                let (got, want) = (&got[shift..], &want[..want.len() - shift]);
                let (got, want) = (&got[skip..], &want[skip..]);
                if let Some(why) = compare(got, want, tol) {
                    failures.push(format!("{name}: {why}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from scsynth:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
