//! `EnvGen`: segment-based envelopes with SC shape curves, gate-driven sustain
//! at the release node, and `doneAction` freeing. The engine renders offline;
//! the envelope's output goes to bus 0 so `render` can read it back.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

/// Duration, in seconds, of `n` samples at the test sample rate.
fn secs(n: usize) -> f64 {
    n as f64 / SR as f64
}

/// Builds an `EnvGen` synth whose output is written to bus 0. `gate` is a
/// control (index 0) so tests can release it with `SetControl`. `segments` is a
/// flat list of `[target, duration_secs, shape, curve]`.
fn envgen_spec(init: f64, done_action: f64, release_node: f64, segments: &[[f64; 4]]) -> Value {
    let mut inputs = vec![
        json!({"control": 0}), // gate
        json!({"const": 1.0}), // levelScale
        json!({"const": 0.0}), // levelBias
        json!({"const": 1.0}), // timeScale
        json!({"const": done_action}),
        json!({"const": init}),
        json!({"const": segments.len() as f64}),
        json!({"const": release_node}),
        json!({"const": -1.0}), // loopNode (unused)
    ];
    for s in segments {
        for v in s {
            inputs.push(json!({ "const": v }));
        }
    }
    json!({
        "name": "env",
        "controls": [{"name": "gate", "default": 1.0}],
        "ugens": [
            {"kind": "EnvGen", "inputs": inputs},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    })
}

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

fn spawn(spec: Value) -> (Engine, EngineHandle) {
    let (engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add(1000, synth_from(spec))).ok().unwrap();
    (engine, handle)
}

#[test]
fn linear_segment_ramps_then_holds_the_target() {
    // One 64-sample segment from 0 to 1; no release node, no done action.
    let (mut engine, _handle) = spawn(envgen_spec(0.0, 0.0, -1.0, &[[1.0, secs(64), 1.0, 0.0]]));
    let out = render(&mut engine, 2);
    for (i, s) in out[..BLOCK_SIZE].iter().enumerate() {
        // frac = i/64, value = frac.
        let want = i as f32 / 64.0;
        assert!((*s - want).abs() < 1e-6, "ramp sample {i}: {s} != {want}");
    }
    // The segment lands exactly on its target and holds it.
    for s in &out[BLOCK_SIZE..] {
        assert!((*s - 1.0).abs() < 1e-6, "hold: {s} != 1.0");
    }
}

#[test]
fn exponential_segment_multiplies_by_a_constant_ratio() {
    // 0.01 -> 1.0 exponentially over 64 samples: each sample is the previous
    // times a fixed ratio (100^(1/64)).
    let (mut engine, _handle) = spawn(envgen_spec(0.01, 0.0, -1.0, &[[1.0, secs(64), 2.0, 0.0]]));
    let out = render(&mut engine, 1);
    assert!((out[0] - 0.01).abs() < 1e-6, "start: {}", out[0]);
    let ratio = 100f32.powf(1.0 / 64.0);
    for i in 1..BLOCK_SIZE - 1 {
        let r = out[i + 1] / out[i];
        assert!((r - ratio).abs() < 1e-4, "ratio at {i}: {r} != {ratio}");
    }
}

#[test]
fn gate_sustains_at_the_release_node_then_releases() {
    // ADSR: levels 0 -> 1 -> 0.5 (sustain) -> 0, each leg 64 samples,
    // releaseNode = 2. doneAction 0 so it holds at 0 instead of freeing.
    let spec = envgen_spec(
        0.0,
        0.0,
        2.0,
        &[
            [1.0, secs(64), 1.0, 0.0],
            [0.5, secs(64), 1.0, 0.0],
            [0.0, secs(64), 1.0, 0.0],
        ],
    );
    let (mut engine, mut handle) = spawn(spec);

    // Four blocks with the gate open: attack, decay, then sustain.
    let held = render(&mut engine, 4);
    for (i, s) in held[..BLOCK_SIZE].iter().enumerate() {
        assert!((*s - i as f32 / 64.0).abs() < 1e-6, "attack {i}");
    }
    // Blocks 2 and 3 sustain at 0.5 no matter how long the gate stays open.
    for s in &held[2 * BLOCK_SIZE..4 * BLOCK_SIZE] {
        assert!((*s - 0.5).abs() < 1e-6, "sustain: {s} != 0.5");
    }

    // Release: the gate falls, the release segment plays 0.5 -> 0.
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 0,
            value: 0.0,
        })
        .ok()
        .unwrap();
    let rel = render(&mut engine, 2);
    for (i, s) in rel[..BLOCK_SIZE].iter().enumerate() {
        let want = 0.5 - 0.5 * (i as f32 / 64.0);
        assert!((*s - want).abs() < 1e-6, "release {i}: {s} != {want}");
    }
    // After the release segment ends it rests at 0.
    for s in &rel[BLOCK_SIZE..] {
        assert!(s.abs() < 1e-6, "rest: {s} != 0.0");
    }
}

#[test]
fn done_action_free_self_frees_the_node() {
    // A one-shot envelope with doneAction = 2 (freeSelf): when the segment
    // ends the engine frees the node.
    let (mut engine, mut handle) = spawn(envgen_spec(0.0, 2.0, -1.0, &[[1.0, secs(64), 1.0, 0.0]]));

    render(&mut engine, 1);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        1,
        "alive during segment"
    );

    // Block 2 completes the segment (freed at its end); block 3 observes the
    // now-empty tree in the published counter.
    render(&mut engine, 2);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        0,
        "freed after segment"
    );
    assert_eq!(
        handle.collect_garbage(),
        1,
        "the freed synth left through the garbage FIFO"
    );
}
