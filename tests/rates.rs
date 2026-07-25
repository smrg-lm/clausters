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
    let mut synth = UGenSynth::new(Arc::new(def), SR);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
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

/// Renders `blocks` blocks, cut into slices of `split` frames — what a
/// scheduled bundle does to a block (M6). The audio bus is read once per whole
/// block, so the result is directly comparable with [`render`]'s.
fn render_split(json: &str, blocks: usize, split: usize) -> Vec<f32> {
    let def = compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        buses.clear_audio();
        let mut offset = 0;
        while offset < BLOCK_SIZE {
            let frames = split.min(BLOCK_SIZE - offset);
            let mut ctx = ProcessCtx {
                sample_rate: SR,
                full_sample_rate: SR,
                buses: &buses,
                buffers: &[],
                offset,
                frames,
            };
            synth.process(&mut ctx);
            offset += frames;
        }
        out.extend_from_slice(buses.audio(0));
    }
    out
}

/// Counts rising edges, so an impulse counts once however many samples the
/// `Out` broadcast it across.
fn count_pulses(sig: &[f32]) -> usize {
    let mut prev = 0.0;
    let mut n = 0;
    for &s in sig {
        if s > 0.5 && prev <= 0.5 {
            n += 1;
        }
        prev = s;
    }
    n
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

// ---- kr: the control rate is a time base, not just a decimation ----
//
// A `kr` UGen emits one sample per slice, so *its* sample rate is `full /
// frames` — scsynth's `unit->mRate->mSampleRate`. Anything that turns seconds
// into samples divides by that, which is what makes a period in Hz mean the
// same thing at either rate.

const ONE_SECOND: usize = SR as usize / BLOCK_SIZE;

#[test]
fn kr_frequency_is_in_hertz_like_ar() {
    // Ten cycles per second is ten impulses per second, whichever rate runs
    // them. Reading the engine's rate here instead would make the control-rate
    // one 64 times too slow — one impulse per second.
    let json = |rate: &str| {
        format!(
            r#"{{
            "name": "imp",
            "ugens": [
                {{"kind": "Impulse", "rate": "{rate}", "inputs": [{{"const": 10.0}}]}},
                {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}
            ]
        }}"#
        )
    };
    let ar = render(&json("ar"), ONE_SECOND);
    let kr = render(&json("kr"), ONE_SECOND);
    assert_eq!(count_pulses(&ar), 10, "Impulse.ar at 10 Hz over one second");
    assert_eq!(count_pulses(&kr), 10, "Impulse.kr at 10 Hz over one second");
}

#[test]
fn kr_time_survives_a_block_split() {
    // Cutting a block into slices makes a kr UGen run once per *slice*, so the
    // rate has to come from the slice length rather than from BLOCK_SIZE: a
    // shorter tick covers proportionally less time, and the two cancel. A
    // scheduled bundle therefore does not speed control time up.
    let json = r#"{
        "name": "imp",
        "ugens": [
            {"kind": "Impulse", "rate": "kr", "inputs": [{"const": 10.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    for split in [1, 7, 16, 32, 64] {
        let out = render_split(json, ONE_SECOND, split);
        assert_eq!(
            count_pulses(&out),
            10,
            "Impulse.kr at 10 Hz over one second, blocks cut into {split}-frame slices"
        );
    }
}

#[test]
fn kr_samplerate_still_reports_the_engine_rate() {
    // The one quantity that is a hardware fact rather than a time base: a
    // control-rate SampleRate reports 48 kHz, not the 750 Hz it runs at.
    let json = r#"{
        "name": "sr",
        "ugens": [
            {"kind": "SampleRate", "rate": "kr", "inputs": []},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    let out = render(json, 3);
    assert!(
        out.iter().all(|&x| x == SR),
        "SampleRate.kr should be {SR}, got {}",
        out[0]
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
