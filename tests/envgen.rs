//! `EnvGen`: segment-based envelopes with SC shape curves, gate-driven sustain
//! at the release node, and `doneAction` freeing, plus U4's `Line`/`XLine` —
//! which are that same segment engine with the header filled in — and the
//! node-control set. The engine renders offline; the envelope's output goes to
//! bus 0 so `render` can read it back.
//!
//! These drive a whole `Engine` rather than a bare synth, because half of what
//! is under test here is a *done action*: freeing a node, pausing it, resuming
//! it with `/n_run`. That is engine behavior, not signal.
//!
//! Rule 5, the block split, is not here: it is the same test for every row and
//! runs from the shared table (`tests/subjects.rs`), which covers the two ramps.

#![cfg(feature = "synth")]

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
    Box::new(UGenSynth::new(Arc::new(compile(spec).unwrap()), SR))
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
fn gate_already_closed_at_the_first_block_releases_and_frees() {
    // The stuck-voice race: a note-on (/s_new, gate 1) and its note-off
    // (/n_set gate 0) applied in the same command drain, before the node's
    // first block. The envelope never sees a gate edge — it must count the
    // gate found closed at birth as a release, play the release segment out
    // silently and let the done action free the node, instead of playing the
    // full envelope and sustaining forever on a closed gate.
    let spec = envgen_spec(
        0.0,
        2.0, // freeSelf
        2.0,
        -1.0,
        &[
            [1.0, secs(64), 1.0, 0.0],
            [0.5, secs(64), 1.0, 0.0],
            [0.0, secs(64), 1.0, 0.0],
        ],
    );
    let (mut engine, mut handle) = spawn(spec);
    // The note-off lands in the same drain as the add, before any block runs.
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 0,
            value: 0.0,
        })
        .ok()
        .unwrap();
    // Born released from level 0: the release segment (64 samples) is silent.
    let out = render(&mut engine, 1);
    for (i, s) in out.iter().enumerate() {
        assert!(s.abs() < 1e-6, "born-released sample {i}: {s} != 0.0");
    }
    // The segment completes in block 2 (freed at its end); block 3 observes
    // the now-empty tree in the published counter.
    render(&mut engine, 2);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        0,
        "the never-heard voice freed itself"
    );
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

// ---- Line / XLine: the one-segment envelopes (U4) ----

/// A `Line` (or `XLine`) written to bus 0, at the given rate. `done_action` is
/// an input like `EnvGen`'s.
fn line_spec(kind: &str, rate: &str, start: f64, end: f64, dur: f64, done_action: f64) -> Value {
    json!({
        "name": "line",
        "ugens": [
            {"kind": kind, "rate": rate, "inputs": [
                {"const": start}, {"const": end},
                {"const": dur}, {"const": done_action}
            ]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    })
}

#[test]
fn line_ramps_over_its_duration_then_holds_the_end() {
    // The same assertion as the linear EnvGen segment above, which is the
    // point: this *is* that segment, with the header filled in.
    let (mut engine, _handle) = spawn(line_spec("Line", "ar", 0.0, 1.0, secs(64), 0.0));
    let out = render(&mut engine, 2);
    for (i, s) in out[..BLOCK_SIZE].iter().enumerate() {
        let want = i as f32 / 64.0;
        assert!((*s - want).abs() < 1e-6, "ramp sample {i}: {s} != {want}");
    }
    for s in &out[BLOCK_SIZE..] {
        assert!((*s - 1.0).abs() < 1e-6, "hold: {s} != 1.0");
    }
}

#[test]
fn xline_moves_by_a_constant_ratio() {
    // What makes the exponential one worth a separate name: equal *ratios*
    // per sample, so it reads as a straight line to the ear driving a pitch.
    let (mut engine, _handle) = spawn(line_spec("XLine", "ar", 0.01, 1.0, secs(64), 0.0));
    let out = render(&mut engine, 1);
    assert!((out[0] - 0.01).abs() < 1e-6, "start: {}", out[0]);
    let ratio = 100f32.powf(1.0 / 64.0);
    for i in 1..BLOCK_SIZE - 1 {
        let r = out[i + 1] / out[i];
        assert!((r - ratio).abs() < 1e-4, "ratio at {i}: {r} != {ratio}");
    }
}

#[test]
fn a_control_rate_line_takes_the_same_wall_clock_time() {
    // The ramp lasts four blocks. At kr it advances one step per block, and
    // "one step" has to mean a whole block of time — the test that fails by a
    // factor of BLOCK_SIZE if a UGen reads the engine's rate instead of its
    // own (see the calculation-rate note in docs/decisions.md).
    let dur = secs(4 * BLOCK_SIZE);
    let (mut engine, _handle) = spawn(line_spec("Line", "kr", 0.0, 1.0, dur, 0.0));
    let out = render(&mut engine, 6);
    // Block b (0-based) holds the value at its start: b/4.
    for b in 0..4 {
        let want = b as f32 / 4.0;
        let got = out[b * BLOCK_SIZE];
        assert!((got - want).abs() < 1e-6, "kr block {b}: {got} != {want}");
    }
    for b in 4..6 {
        let got = out[b * BLOCK_SIZE];
        assert!(
            (got - 1.0).abs() < 1e-6,
            "kr block {b} should hold 1.0: {got}"
        );
    }
}

#[test]
fn a_ten_second_ramp_does_not_drift_from_its_closed_form() {
    // Rule 4 for the ramps. They accumulate — one addition (or one
    // multiplication) per sample, which is what makes them cheap enough for
    // audio rate — so the question a long ramp asks is whether the running sum
    // stays on the closed form `start + t·(end − start)`. It does, because the
    // accumulator is `f64`: over 480 000 samples the drift is around 1e-13,
    // while the same loop in `f32` would be visibly short of its target by now.
    //
    // The landing is not left to the accumulation at all: when the counter runs
    // out the level is assigned `end`, so the hold is exact rather than close.
    // The tolerance below is therefore a few ulps of the range, not a fraction
    // of it.
    let secs10 = 10.0f64;
    let n = (SR * secs10 as f32) as usize;
    let blocks = n / BLOCK_SIZE;

    let (mut engine, _handle) = spawn(line_spec("Line", "ar", 0.0, 1.0, secs10, 0.0));
    let out = render(&mut engine, blocks + 2);
    for i in [1usize, n / 3, n / 2, n - 2] {
        let want = i as f32 / n as f32;
        assert!(
            (out[i] - want).abs() < 1e-6,
            "Line at sample {i} of {n}: {} != {want}",
            out[i]
        );
    }
    // And it arrives, exactly, rather than stopping just short.
    for s in &out[n..] {
        assert_eq!(*s, 1.0, "a finished Line holds its target exactly");
    }

    // XLine the same way, against its own closed form `start * (end/start)^t`
    // — the absolute claim, where the short test asserts only that consecutive
    // samples keep a constant ratio. Both are needed: a ramp with the right
    // ratio everywhere can still have started from the wrong place.
    let (start, end) = (0.01f64, 1.0f64);
    let (mut engine, _handle) = spawn(line_spec("XLine", "ar", start, end, secs10, 0.0));
    let out = render(&mut engine, blocks + 2);
    for i in [1usize, n / 3, n / 2, n - 2] {
        let want = (start * (end / start).powf(i as f64 / n as f64)) as f32;
        assert!(
            (out[i] / want - 1.0).abs() < 1e-5,
            "XLine at sample {i} of {n}: {} != {want}",
            out[i]
        );
    }
    for s in &out[n..] {
        assert!(
            (*s - end as f32).abs() < 1e-6,
            "a finished XLine holds its target: {s}"
        );
    }
}

#[test]
fn a_ramp_reads_its_geometry_once_and_ignores_it_afterwards() {
    // scsynth's semantics, and the price of the cheap inner loop: the step is
    // derived on the first sample, so `end` and `dur` are init-rate. A control
    // that moves them mid-flight moves nothing — the ramp still lands where it
    // was aimed when it was born. (`done_action` is the one input still read
    // every block; it addresses the node, not the ramp.)
    let spec = json!({
        "name": "line_mod",
        "controls": [{"name": "end", "default": 1.0}],
        "ugens": [
            {"kind": "Line", "inputs": [
                {"const": 0.0}, {"control": 0}, {"const": secs(4 * BLOCK_SIZE)},
                {"const": 0.0}
            ]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    });
    let (mut engine, mut handle) = spawn(spec);
    render(&mut engine, 1);
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 0,
            value: 5.0,
        })
        .ok()
        .unwrap();
    let out = render(&mut engine, 5);
    // Blocks 1..4 of the ramp, then the hold — all on the original geometry.
    for b in 0..3 {
        let want = (b + 1) as f32 / 4.0;
        let got = out[b * BLOCK_SIZE];
        assert!((got - want).abs() < 1e-6, "block {b}: {got} != {want}");
    }
    for s in &out[3 * BLOCK_SIZE..] {
        assert_eq!(*s, 1.0, "holds the end it was born with, not the new one");
    }
}

#[test]
fn line_carries_the_whole_done_action_set() {
    // doneAction 2 (freeSelf) through the same path EnvGen uses — the reason
    // Line is built on the segment engine rather than beside it.
    let (mut engine, handle) = spawn(line_spec("Line", "ar", 0.0, 1.0, secs(64), 2.0));
    render(&mut engine, 1);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        1,
        "alive during the ramp"
    );
    render(&mut engine, 2);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        0,
        "freed when the ramp ended"
    );
}

// ---- FreeSelf / PauseSelf / Done / FreeSelfWhenDone (U4) ----

/// A node-control UGen fed by control 0, its output written to bus 0 so the
/// pass-through is observable.
fn nodectl_spec(kind: &str) -> Value {
    json!({
        "name": "ctl",
        "controls": [{"name": "trig", "default": 0.0}],
        "ugens": [
            {"kind": kind, "rate": "ar", "inputs": [{"control": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    })
}

fn set(handle: &mut EngineHandle, value: f32) {
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 0,
            value,
        })
        .ok()
        .unwrap();
}

#[test]
fn free_self_passes_its_input_through_until_it_goes_positive() {
    let (mut engine, mut handle) = spawn(nodectl_spec("FreeSelf"));
    set(&mut handle, -0.25);
    let out = render(&mut engine, 2);
    assert!(
        out.iter().all(|s| (*s + 0.25).abs() < 1e-6),
        "an input that is not yet positive passes through untouched"
    );
    assert_eq!(handle.counters().synths.load(Ordering::Relaxed), 1);

    set(&mut handle, 1.0);
    render(&mut engine, 2);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        0,
        "freed once the input went positive"
    );
}

#[test]
fn pause_self_does_not_latch_so_n_run_really_resumes() {
    // The property the implementation is shaped around: the action is reported
    // for the block just processed, never remembered. A latched one would
    // re-pause the instant /n_run 1 resumed the node, making the command
    // useless and PauseSelf a one-way door.
    let (mut engine, mut handle) = spawn(nodectl_spec("PauseSelf"));
    set(&mut handle, 1.0);
    let out = render(&mut engine, 2);
    assert!(
        out[..BLOCK_SIZE].iter().all(|s| (*s - 1.0).abs() < 1e-6),
        "the pausing block still runs and still passes through"
    );
    assert!(
        out[BLOCK_SIZE..].iter().all(|s| s.abs() < 1e-6),
        "paused from the next block on"
    );
    assert_eq!(handle.counters().synths.load(Ordering::Relaxed), 1, "alive");

    // Drop the input first, then resume: it must stay running.
    set(&mut handle, 0.0);
    handle
        .send(Cmd::RunNode {
            id: 1000,
            run: true,
        })
        .ok()
        .unwrap();
    let out = render(&mut engine, 2);
    assert!(
        out.iter().all(|s| s.abs() < 1e-6),
        "resumed and passing through its now-zero input"
    );
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        1,
        "still alive, still running"
    );

    // Raise it again and it pauses again — a gate, not a one-shot.
    set(&mut handle, 1.0);
    let out = render(&mut engine, 3);
    assert!(
        out[BLOCK_SIZE..].iter().all(|s| s.abs() < 1e-6),
        "re-paused while the input is up"
    );
}

/// A ramp with `doneAction` 0 (it frees nothing itself) watched by `kind`.
fn watcher_spec(kind: &str, dur: f64) -> Value {
    json!({
        "name": "watch",
        "ugens": [
            {"kind": "Line", "inputs": [
                {"const": 0.0}, {"const": 1.0}, {"const": dur}, {"const": 0.0}
            ]},
            {"kind": kind, "rate": "ar", "inputs": [{"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    })
}

#[test]
fn done_reports_the_flag_of_the_ugen_it_watches() {
    // The flag is not the watched signal: this ramp ends at 1.0 and Done also
    // reads 1.0, so the test uses the *timing* to tell them apart — the ramp
    // holds 1.0 only from the sample after it lands, while Done is 0 for the
    // whole first block and 1 from the second.
    let (mut engine, _handle) = spawn(watcher_spec("Done", secs(64)));
    let out = render(&mut engine, 3);
    assert!(
        out[..BLOCK_SIZE].iter().all(|s| s.abs() < 1e-6),
        "not done while the ramp is running"
    );
    assert!(
        out[BLOCK_SIZE..].iter().all(|s| (*s - 1.0).abs() < 1e-6),
        "done from the block the ramp finished in"
    );
}

#[test]
fn free_self_when_done_frees_on_a_ramp_that_frees_nothing() {
    // The idiom it exists for: the envelope's own doneAction is 0 because
    // something else in the graph still needs it, and the freeing is a
    // separate decision.
    let (mut engine, handle) = spawn(watcher_spec("FreeSelfWhenDone", secs(64)));
    let out = render(&mut engine, 1);
    assert!(
        out.iter().any(|s| *s > 0.4),
        "it passes the watched ramp through"
    );
    assert_eq!(handle.counters().synths.load(Ordering::Relaxed), 1);
    render(&mut engine, 2);
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        0,
        "freed once the ramp finished"
    );
}

#[test]
fn a_watcher_rejects_a_source_that_never_finishes() {
    // Reading a wire that has no done flag would be zero for the node's whole
    // life with nothing to see, so the compiler names it instead.
    let spec = json!({
        "name": "bad",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Done", "rate": "ar", "inputs": [{"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    });
    let err = compile(serde_json::from_value::<SynthDefSpec>(spec).unwrap()).unwrap_err();
    assert!(err.contains("Sine has no done flag"), "{err}");
}

#[test]
fn a_watcher_rejects_a_constant_source() {
    let spec = json!({
        "name": "bad",
        "ugens": [
            {"kind": "FreeSelfWhenDone", "rate": "ar", "inputs": [{"const": 1.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    });
    let err = compile(serde_json::from_value::<SynthDefSpec>(spec).unwrap()).unwrap_err();
    assert!(err.contains("must be another UGen"), "{err}");
}
