//! S7: live hardware input. The device input path can't run in the sandbox, so
//! these exercise the engine seam directly: a ring feeds interleaved input
//! frames into the input buses (`channels..channels + input_channels`), and an
//! `In` UGen reading that bus proves the round-trip end to end.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};

const SR: f32 = 48_000.0;

/// A synth that copies input bus `in_bus` straight to output bus 0.
fn passthru_from(in_bus: f32) -> Box<dyn SynthNode> {
    let spec: SynthDefSpec = serde_json::from_value(serde_json::json!({
        "name": "passthru",
        "ugens": [
            {"kind": "In", "inputs": [{"const": in_bus}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }))
    .unwrap();
    Box::new(UGenSynth::new(Arc::new(compile(spec).unwrap())))
}

/// One input channel pushed through the ring reaches `In` and comes back out on
/// output channel 0, sample for sample, in the same block (no added latency).
#[test]
fn hardware_input_reaches_in_ugen() {
    // 2 output channels -> input channel 0 lives on audio bus 2.
    let (mut engine, mut handle) = engine_pair(SR, 2);
    let mut tx = engine.input_ring(1, BLOCK_SIZE * 8);

    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: passthru_from(2.0),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    // A distinct value per frame so a wrong offset would be obvious.
    let input: Vec<f32> = (0..BLOCK_SIZE).map(|i| (i as f32 + 1.0) / 1000.0).collect();
    for &s in &input {
        assert!(tx.push(s).is_ok());
    }

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);

    for (f, &want) in input.iter().enumerate() {
        assert!(
            (out[f * 2] - want).abs() < 1e-6,
            "frame {f}: output {} != input {want}",
            out[f * 2]
        );
    }
}

/// With no input stream attached, the input buses read as silence — `In` on an
/// input bus produces nothing rather than stale or garbage samples.
#[test]
fn no_input_reads_silence() {
    let (mut engine, mut handle) = engine_pair(SR, 2);
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: passthru_from(2.0),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert!(out.iter().all(|&s| s == 0.0), "no input -> silence");
}

/// An input underrun (ring emptier than a block) is not a stall: the missing
/// tail reads as silence and the engine keeps processing.
#[test]
fn input_underrun_degrades_to_silence() {
    let (mut engine, mut handle) = engine_pair(SR, 2);
    let mut tx = engine.input_ring(1, BLOCK_SIZE * 8);
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: passthru_from(2.0),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    // Only half a block available.
    for i in 0..BLOCK_SIZE / 2 {
        tx.push(0.5 + i as f32).ok();
    }
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out); // must not panic or block
    // The tail (no samples pushed) is silent.
    assert_eq!(out[(BLOCK_SIZE - 1) * 2], 0.0);
}
