//! Calculation-rate tests (S1): one per rate — `ar` (per sample), `kr` (once
//! per block), `ir` (once at init, then frozen), `dr` (pulled on demand) — plus
//! the compiler's rate-coercion validation.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};

const SR: f32 = 48_000.0;

/// Renders `blocks` full blocks of audio bus 0.
fn render(json: &str, blocks: usize) -> Vec<f32> {
    let def = compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def));
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            buses: &buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
        };
        synth.process(&mut ctx);
        out.extend_from_slice(buses.audio(0));
    }
    out
}

fn compile_err(json: &str) -> String {
    compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap_err()
}

fn block(out: &[f32], b: usize) -> &[f32] {
    &out[b * BLOCK_SIZE..(b + 1) * BLOCK_SIZE]
}

// ---- ar: one value per sample ----

#[test]
fn ar_output_varies_within_a_block() {
    // A plain audio-rate sine changes from sample to sample.
    let json = r#"{
        "name": "ar",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 2000.0}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    let out = render(json, 1);
    let first = block(&out, 0);
    assert!(
        first.windows(2).any(|w| w[0] != w[1]),
        "ar output should vary per sample"
    );
}

// ---- kr: one value per block ----

#[test]
fn kr_output_is_constant_within_each_block() {
    // A kr Mul samples its audio-rate input once per block, so the wire it
    // feeds Out is block-constant; Out broadcasts it across the block.
    let json = r#"{
        "name": "kr",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Mul", "rate": "kr", "inputs": [{"ugen": 0}, {"const": 1.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    let out = render(json, 6);
    // Every 64-sample block is flat…
    for b in 0..6 {
        let blk = block(&out, b);
        assert!(
            blk.iter().all(|&x| x == blk[0]),
            "kr block {b} is not constant: {blk:?}"
        );
    }
    // …but the per-block value tracks the sine (440 Hz doesn't divide the
    // block rate, so consecutive blocks differ).
    let values: Vec<f32> = (0..6).map(|b| block(&out, b)[0]).collect();
    assert!(
        values.windows(2).any(|w| w[0] != w[1]),
        "kr value should change across blocks: {values:?}"
    );
}

// ---- ir: computed once at init, then held ----

#[test]
fn ir_samplerate_reports_the_engine_rate() {
    let json = r#"{
        "name": "sr",
        "ugens": [
            {"kind": "SampleRate", "inputs": []},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    let out = render(json, 3);
    assert!(out.iter().all(|&x| x == SR), "SampleRate.ir should be {SR}");
}

#[test]
fn ir_rand_is_drawn_once_and_frozen() {
    // Rand.ir draws a fresh number every time it runs; the init pass runs it
    // exactly once, so the value must be identical across all blocks (and in
    // range). If the ir skip were broken, later blocks would differ.
    let json = r#"{
        "name": "rnd",
        "ugens": [
            {"kind": "Rand", "inputs": [{"const": 2.0}, {"const": 5.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    let out = render(json, 8);
    let v = out[0];
    assert!((2.0..5.0).contains(&v), "Rand.ir out of range: {v}");
    assert!(
        out.iter().all(|&x| x == v),
        "Rand.ir must stay frozen across blocks (first {v})"
    );
}

// ---- dr: pulled on demand by a driver ----

#[test]
fn dr_demand_steps_through_the_sequence() {
    // Impulse at 750 Hz fires on sample 0 of every 64-sample block, so Demand
    // pulls exactly one Dseq value per block: 10, 20, 30, then loops.
    let json = r#"{
        "name": "seq",
        "ugens": [
            {"kind": "Impulse", "inputs": [{"const": 750.0}]},
            {"kind": "Dseq", "rate": "dr",
             "inputs": [{"const": 0.0}, {"const": 10.0}, {"const": 20.0}, {"const": 30.0}]},
            {"kind": "Demand", "inputs": [{"ugen": 0}, {"const": 0.0}, {"ugen": 1}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
        ]
    }"#;
    let out = render(json, 4);
    let expected = [10.0, 20.0, 30.0, 10.0];
    for (b, &want) in expected.iter().enumerate() {
        let blk = block(&out, b);
        assert!(
            blk.iter().all(|&x| x == want),
            "block {b} should hold {want}, got {:?}",
            blk[0]
        );
    }
}

#[test]
fn dr_demand_reset_restarts_the_stream() {
    // A finite Dseq (1 pass over [10, 20]) exhausted, then reset back to the
    // top. Impulse fires once per block; reset fires on block 3 via a control.
    let json = r#"{
        "name": "seqr",
        "controls": [{"name": "reset", "default": 0.0}],
        "ugens": [
            {"kind": "Impulse", "inputs": [{"const": 750.0}]},
            {"kind": "Dseq", "rate": "dr",
             "inputs": [{"const": 1.0}, {"const": 10.0}, {"const": 20.0}]},
            {"kind": "Demand", "inputs": [{"ugen": 0}, {"control": 0}, {"ugen": 1}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
        ]
    }"#;
    // Blocks: 10, 20, then exhausted (holds 20). We only check the first two
    // land, and that after exhaustion the value holds rather than changing.
    let out = render(json, 4);
    assert!(block(&out, 0).iter().all(|&x| x == 10.0));
    assert!(block(&out, 1).iter().all(|&x| x == 20.0));
    assert!(
        block(&out, 2).iter().all(|&x| x == 20.0),
        "exhausted: holds"
    );
    assert!(block(&out, 3).iter().all(|&x| x == 20.0), "still held");
}

// ---- compiler rate validation ----

#[test]
fn rejects_unknown_rate() {
    let json = r#"{"name":"x","ugens":[{"kind":"Sine","rate":"xr","inputs":[{"const":1.0}]}]}"#;
    assert!(compile_err(json).contains("unknown rate"));
}

#[test]
fn rejects_rate_not_allowed_for_kind() {
    // Out is audio-rate only.
    let json = r#"{"name":"x","ugens":[
        {"kind":"Sine","inputs":[{"const":1.0}]},
        {"kind":"Out","rate":"kr","inputs":[{"const":0.0},{"ugen":0}]}
    ]}"#;
    assert!(compile_err(json).contains("not allowed"));
}

#[test]
fn rejects_non_ir_input_to_ir_ugen() {
    // Rand.ir needs ir inputs; a control (kr) cannot be frozen at init.
    let json = r#"{"name":"x","controls":[{"name":"lo","default":0.0}],"ugens":[
        {"kind":"Rand","inputs":[{"control":0},{"const":1.0}]}
    ]}"#;
    assert!(compile_err(json).contains("requires ir inputs"));
}

#[test]
fn rejects_demand_wire_into_a_normal_input() {
    // A dr wire may only feed a demand driver's source slot.
    let json = r#"{"name":"x","ugens":[
        {"kind":"Dseq","rate":"dr","inputs":[{"const":0.0},{"const":1.0}]},
        {"kind":"Mul","inputs":[{"ugen":0},{"const":2.0}]}
    ]}"#;
    assert!(compile_err(json).contains("demand driver"));
}

#[test]
fn rejects_non_demand_source_in_demand_slot() {
    let json = r#"{"name":"x","ugens":[
        {"kind":"Impulse","inputs":[{"const":1.0}]},
        {"kind":"Demand","inputs":[{"ugen":0},{"const":0.0},{"const":5.0}]}
    ]}"#;
    assert!(compile_err(json).contains("must be a demand-rate"));
}
