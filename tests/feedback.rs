//! `LocalIn`/`LocalOut` feedback: synth-private feedback buses with exactly
//! one control block (64 samples) of delay. DC signals make the delay visible
//! to the sample; the engine renders offline, no audio device.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

fn synth_from(spec_json: Value) -> Box<dyn SynthNode> {
    let spec: SynthDefSpec = serde_json::from_value(spec_json).unwrap();
    Box::new(UGenSynth::new(Arc::new(compile(spec).unwrap())))
}

fn add(id: i32, synth: Box<dyn SynthNode>) -> Cmd {
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth,
        usage: Default::default(),
    }
}

/// Renders `blocks` blocks and returns channel 0.
fn render(engine: &mut Engine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        buf.extend(out.iter().step_by(CHANNELS).copied());
    }
    buf
}

#[test]
fn local_feedback_delays_by_exactly_one_block() {
    // LocalOut writes a constant 1.0 into channel 0 every block; LocalIn reads
    // channel 0 and goes to bus 0. LocalIn sees the *previous* block's write,
    // so the output is silent for the first block, then 1.0 forever.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add(
            1000,
            synth_from(json!({
                "name": "fb",
                "ugens": [
                    {"kind": "LocalIn",  "inputs": [{"const": 0.0}]},
                    {"kind": "Out",      "inputs": [{"const": 0.0}, {"ugen": 0}]},
                    {"kind": "LocalOut", "inputs": [{"const": 0.0}, {"const": 1.0}]}
                ]
            })),
        ))
        .ok()
        .unwrap();

    let left = render(&mut engine, 4);
    for (i, s) in left.iter().enumerate() {
        let expected = if i < BLOCK_SIZE { 0.0 } else { 1.0 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn feedback_loop_accumulates_block_by_block() {
    // A block-rate integrator: out = LocalIn; LocalOut writes LocalIn + 1.
    // With one block of delay, block k carries the value k.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add(
            1000,
            synth_from(json!({
                "name": "accum",
                "ugens": [
                    {"kind": "LocalIn",  "inputs": [{"const": 0.0}]},
                    {"kind": "Add",      "inputs": [{"ugen": 0}, {"const": 1.0}]},
                    {"kind": "Out",      "inputs": [{"const": 0.0}, {"ugen": 0}]},
                    {"kind": "LocalOut", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ]
            })),
        ))
        .ok()
        .unwrap();

    let blocks = 6;
    let left = render(&mut engine, blocks);
    for (i, s) in left.iter().enumerate() {
        let block = (i / BLOCK_SIZE) as f32;
        assert_eq!(*s, block, "sample {i} (block {block})");
    }
}

#[test]
fn two_independent_feedback_channels() {
    // Channels 0 and 1 must not cross-talk: each carries its own constant,
    // both delayed one block.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add(
            1000,
            synth_from(json!({
                "name": "fb2",
                "ugens": [
                    {"kind": "LocalIn",  "inputs": [{"const": 0.0}]},
                    {"kind": "LocalIn",  "inputs": [{"const": 1.0}]},
                    {"kind": "Add",      "inputs": [{"ugen": 0}, {"ugen": 1}]},
                    {"kind": "Out",      "inputs": [{"const": 0.0}, {"ugen": 2}]},
                    {"kind": "LocalOut", "inputs": [{"const": 0.0}, {"const": 0.25}]},
                    {"kind": "LocalOut", "inputs": [{"const": 1.0}, {"const": 0.5}]}
                ]
            })),
        ))
        .ok()
        .unwrap();

    let left = render(&mut engine, 3);
    for (i, s) in left.iter().enumerate() {
        let expected = if i < BLOCK_SIZE { 0.0 } else { 0.75 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

#[test]
fn feedback_survives_a_mid_block_schedule_split() {
    // A scheduled command splits the block at sample 100 (block 1, offset 36).
    // The feedback buffer is written/read per slice, so the one-block delay
    // still holds: the LocalOut=1.0 def stays silent for block 0 then 1.0.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add(
            1000,
            synth_from(json!({
                "name": "fb",
                "ugens": [
                    {"kind": "LocalIn",  "inputs": [{"const": 0.0}]},
                    {"kind": "Out",      "inputs": [{"const": 0.0}, {"ugen": 0}]},
                    {"kind": "LocalOut", "inputs": [{"const": 0.0}, {"const": 1.0}]}
                ]
            })),
        ))
        .ok()
        .unwrap();
    // A no-op timed bundle at sample 100 forces a split of block 1.
    handle
        .send(Cmd::Schedule {
            time: 100,
            cmds: vec![],
        })
        .ok()
        .unwrap();

    let left = render(&mut engine, 3);
    for (i, s) in left.iter().enumerate() {
        let expected = if i < BLOCK_SIZE { 0.0 } else { 1.0 };
        assert_eq!(*s, expected, "sample {i}");
    }
}

// ---- compile-time validation ----

#[test]
fn rejects_localout_before_its_localin() {
    let spec: SynthDefSpec = serde_json::from_value(json!({
        "name": "bad",
        "ugens": [
            {"kind": "LocalOut", "inputs": [{"const": 0.0}, {"const": 1.0}]},
            {"kind": "LocalIn",  "inputs": [{"const": 0.0}]},
            {"kind": "Out",      "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }))
    .unwrap();
    let err = compile(spec).unwrap_err();
    assert!(err.contains("LocalIn must precede LocalOut"), "got: {err}");
}

#[test]
fn rejects_non_constant_feedback_channel() {
    let spec: SynthDefSpec = serde_json::from_value(json!({
        "name": "bad",
        "controls": [{"name": "ch", "default": 0.0}],
        "ugens": [
            {"kind": "LocalIn", "inputs": [{"control": 0}]},
            {"kind": "Out",     "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }))
    .unwrap();
    let err = compile(spec).unwrap_err();
    assert!(
        err.contains("must be a non-negative constant"),
        "got: {err}"
    );
}
