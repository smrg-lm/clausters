//! `EnvGen`: segment-based envelopes with SC shape curves, gate-driven sustain
//! at the release node, and `doneAction` freeing. The engine renders offline;
//! the envelope's output goes to bus 0 so `render` can read it back.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use clausters::node::{AddAction, Group, ROOT_NODE_ID, SynthNode};
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
fn envgen_spec(
    init: f64,
    done_action: f64,
    release_node: f64,
    loop_node: f64,
    segments: &[[f64; 4]],
) -> Value {
    let mut inputs = vec![
        json!({"control": 0}), // gate
        json!({"const": 1.0}), // levelScale
        json!({"const": 0.0}), // levelBias
        json!({"const": 1.0}), // timeScale
        json!({"const": done_action}),
        json!({"const": init}),
        json!({"const": segments.len() as f64}),
        json!({"const": release_node}),
        json!({"const": loop_node}),
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
    let (mut engine, _handle) = spawn(envgen_spec(
        0.0,
        0.0,
        -1.0,
        -1.0,
        &[[1.0, secs(64), 1.0, 0.0]],
    ));
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
    let (mut engine, _handle) = spawn(envgen_spec(
        0.01,
        0.0,
        -1.0,
        -1.0,
        &[[1.0, secs(64), 2.0, 0.0]],
    ));
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
        -1.0,
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
    let (mut engine, mut handle) = spawn(envgen_spec(
        0.0,
        2.0,
        -1.0,
        -1.0,
        &[[1.0, secs(64), 1.0, 0.0]],
    ));

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

#[test]
fn loop_node_cycles_while_gate_is_held_then_release_exits() {
    // levels 0 -> 1 -> 0 -> 0.3, each leg 64 samples. releaseNode = 2,
    // loopNode = 0: while the gate is held it cycles seg0 (0->1) and seg1
    // (1->0) with period 128; on release it plays seg2 (-> 0.3) and holds.
    let spec = envgen_spec(
        0.0,
        0.0,
        2.0,
        0.0,
        &[
            [1.0, secs(64), 1.0, 0.0],
            [0.0, secs(64), 1.0, 0.0],
            [0.3, secs(64), 1.0, 0.0],
        ],
    );
    let (mut engine, mut handle) = spawn(spec);

    // Four blocks held: the second 128-sample window repeats the first.
    let held = render(&mut engine, 4);
    for k in 0..2 * BLOCK_SIZE {
        assert!(
            (held[k] - held[2 * BLOCK_SIZE + k]).abs() < 1e-6,
            "loop period at {k}: {} != {}",
            held[k],
            held[2 * BLOCK_SIZE + k]
        );
    }
    // The window is a real cycle, not a stuck level: the peak of seg0 is 1.
    assert!(
        held[..BLOCK_SIZE].iter().any(|&s| s > 0.98),
        "attack reaches 1"
    );

    // Release exits the loop: seg2 ramps to 0.3, then it holds there.
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 0,
            value: 0.0,
        })
        .ok()
        .unwrap();
    let rel = render(&mut engine, 2);
    for s in &rel[BLOCK_SIZE..] {
        assert!(
            (*s - 0.3).abs() < 1e-6,
            "held at release target: {s} != 0.3"
        );
    }
}

#[test]
fn done_action_pause_self_stops_processing_but_keeps_the_node() {
    // doneAction = 1 (pauseSelf): when the segment ends the synth is paused —
    // skipped from then on (so its Out stops writing, bus 0 goes silent) but
    // never freed.
    let (mut engine, mut handle) = spawn(envgen_spec(
        0.0,
        1.0,
        -1.0,
        -1.0,
        &[[1.0, secs(64), 1.0, 0.0]],
    ));

    let out = render(&mut engine, 3);
    // Block 2: the segment has finished (value held at 1) and it still ran, so
    // the pause takes effect only from block 3.
    for s in &out[BLOCK_SIZE..2 * BLOCK_SIZE] {
        assert!((*s - 1.0).abs() < 1e-6, "last active output: {s} != 1.0");
    }
    // Block 3: paused, skipped — bus 0 is cleared and stays silent.
    for s in &out[2 * BLOCK_SIZE..] {
        assert!(s.abs() < 1e-6, "paused output must be silent: {s}");
    }
    // Paused, not freed: the node is still there and nothing hit the garbage.
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        1,
        "still alive"
    );
    assert_eq!(handle.collect_garbage(), 0, "nothing freed");
}

#[test]
fn done_action_free_group_frees_the_enclosing_group() {
    // A synth inside group 1 whose envelope ends with doneAction = 14
    // (freeGroup): the whole group (and the synth) is freed.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::new(),
        })
        .ok()
        .unwrap();
    let synth = synth_from(envgen_spec(
        0.0,
        14.0,
        -1.0,
        -1.0,
        &[[1.0, secs(64), 1.0, 0.0]],
    ));
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: 1,
            action: AddAction::Tail,
            synth,
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    render(&mut engine, 1);
    assert_eq!(handle.counters().synths.load(Ordering::Relaxed), 1);
    assert_eq!(
        handle.counters().groups.load(Ordering::Relaxed),
        2,
        "root + group 1"
    );

    // Block 2 ends the segment; the enclosing group is freed at its end.
    render(&mut engine, 2);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        0,
        "synth gone"
    );
    assert_eq!(
        handle.counters().groups.load(Ordering::Relaxed),
        1,
        "group 1 gone"
    );
    assert_eq!(
        handle.collect_garbage(),
        2,
        "group and synth left as garbage"
    );
}

// ---- S4: /n_run resume + the relative done actions through the real chain ----

/// A plain synth that sums a constant `dc` into bus 0 every block (no envelope,
/// no done action) — a marker to hear whether a node ran.
fn dc_spec(dc: f64) -> Value {
    json!({
        "name": "dc",
        "ugens": [{"kind": "Out", "inputs": [{"const": 0.0}, {"const": dc}]}]
    })
}

#[test]
fn n_run_resumes_a_paused_synth() {
    // pauseSelf (doneAction 1) parks the synth; /n_run 1 (RunNode run=true)
    // clears the pause so it runs again — PauseSelf is no longer terminal.
    let (mut engine, mut handle) = spawn(envgen_spec(
        0.0,
        1.0,
        -1.0,
        -1.0,
        &[[1.0, secs(64), 1.0, 0.0]],
    ));
    let out = render(&mut engine, 3);
    assert!(
        out[2 * BLOCK_SIZE..].iter().all(|s| s.abs() < 1e-6),
        "block 3 is paused/silent"
    );

    handle
        .send(Cmd::RunNode {
            id: 1000,
            run: true,
        })
        .ok()
        .unwrap();
    let out = render(&mut engine, 1);
    assert!(
        out.iter().any(|s| (*s - 1.0).abs() < 1e-6),
        "resumed: the held envelope is audible again"
    );
    assert_eq!(handle.counters().synths.load(Ordering::Relaxed), 1);
    assert_eq!(handle.collect_garbage(), 0, "resume frees nothing");
}

#[test]
fn done_action_free_self_and_next_frees_two_nodes() {
    // doneAction = 4 (freeSelfAndNext) drives the whole chain: float ->
    // DoneAction::from_i32 -> queue -> apply_done_action with next-sibling
    // resolution. The actor (1000) and its next sibling (1001) go; 1002 stays.
    let (mut engine, mut handle) = spawn(envgen_spec(
        0.0,
        4.0,
        -1.0,
        -1.0,
        &[[1.0, secs(64), 1.0, 0.0]],
    ));
    handle
        .send(add(1001, synth_from(dc_spec(0.0))))
        .ok()
        .unwrap();
    handle
        .send(add(1002, synth_from(dc_spec(0.0))))
        .ok()
        .unwrap();
    // The first block applies the queued adds; the actor's one-block segment has
    // not ended yet, so all three are present.
    render(&mut engine, 1);
    assert_eq!(handle.counters().synths.load(Ordering::Relaxed), 3);

    render(&mut engine, 2); // the segment ends and the action fires
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        1,
        "self + next freed, the third survives"
    );
}

#[test]
fn n_run_pauses_and_resumes_a_whole_group() {
    // /n_run on a group pauses its entire subtree (skipped, silent) and resumes
    // it. A DC synth inside the group is the marker.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::new(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: 1,
            action: AddAction::Tail,
            synth: synth_from(dc_spec(0.5)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    let out = render(&mut engine, 1);
    assert!(out.iter().any(|s| (*s - 0.5).abs() < 1e-6), "group runs");

    handle
        .send(Cmd::RunNode { id: 1, run: false })
        .ok()
        .unwrap();
    let out = render(&mut engine, 1);
    assert!(out.iter().all(|s| s.abs() < 1e-6), "paused group is silent");

    handle.send(Cmd::RunNode { id: 1, run: true }).ok().unwrap();
    let out = render(&mut engine, 1);
    assert!(out.iter().any(|s| (*s - 0.5).abs() < 1e-6), "resumed group");
    assert_eq!(handle.counters().synths.load(Ordering::Relaxed), 1);
}
