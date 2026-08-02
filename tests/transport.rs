//! The server transport: a governed subtree frozen and resumed sample-exactly.

use std::sync::Arc as SegArc;
use std::sync::atomic::Ordering;

use clausters::dsp::{Limits, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair, engine_pair_full};
use clausters::server::ipc::Segment;

// The group-binding tests below build a real synth to make a group's output
// legible, which needs the `synth` def family (UGenSynth/synthdef::compile).
// The transport-clock tests above them need neither, so only this half of
// the file is gated — gating the whole file would silently drop the clock
// tests' only coverage in a build without the def family.
#[cfg(feature = "synth")]
use std::sync::Arc;

#[cfg(feature = "synth")]
use clausters::clausters_core::rng::SEED_STRIDE;
#[cfg(feature = "synth")]
use clausters::node::{AddAction, Group, MAX_GROUP_CHILDREN, Place, ROOT_NODE_ID};
#[cfg(feature = "synth")]
use clausters::server::engine::EngineHandle;
#[cfg(feature = "synth")]
use clausters::synthdef::instance::UGenSynth;
#[cfg(feature = "synth")]
use clausters::synthdef::{SynthDef, SynthDefSpec, compile};

/// A synth that keeps writing `level` to `bus` every block, used to make a
/// group's output legible without a def file: `ReplaceOut` at a constant bus
/// with a constant value, the same pattern `tests/engine.rs` uses for its
/// silencer def. `level` is control index 0 rather than a literal, so a test
/// can move it later with `Cmd::SetControl` — which is how the scheduling
/// tests below tell whether a bundle has fired.
#[cfg(feature = "synth")]
fn constant_def(bus: i32, level: f32) -> Arc<SynthDef> {
    let json = format!(
        r#"{{
            "name": "constant",
            "controls": [{{"name": "level", "default": {level}}}],
            "ugens": [
                {{"kind": "ReplaceOut", "inputs": [{{"const": {bus}.0}}, {{"control": 0}}]}}
            ]
        }}"#
    );
    let spec: SynthDefSpec = serde_json::from_str(&json).unwrap();
    Arc::new(compile(spec).unwrap())
}

/// A seeded noise generator: the case that makes a pause interesting, since it
/// has no material to index into. Its position *is* its internal state, so a
/// resume either continues the stream or restarts it, audibly.
#[cfg(feature = "synth")]
fn noise_def() -> Arc<SynthDef> {
    let json = r#"{
        "name": "noise",
        "controls": [{"name": "amp", "default": 0.3}],
        "ugens": [
            {"kind": "WhiteNoise", "inputs": []},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 0}]},
            {"kind": "ReplaceOut", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    let spec: SynthDefSpec = serde_json::from_str(json).unwrap();
    Arc::new(compile(spec).unwrap())
}

/// A group holding one seeded noise synth, for the pause-transparency proof.
#[cfg(feature = "synth")]
fn add_noise_synth_in_new_group(handle: &mut EngineHandle, group_id: i32, _bus: i32) {
    handle
        .send(Cmd::AddGroup {
            id: group_id,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::with_capacity(MAX_GROUP_CHILDREN),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: group_id + 1,
            target: group_id,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(noise_def(), 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
}

/// Builds a new group under the root and, inside it, a single synth that
/// writes `level` to `bus` every block — the constant-output helper the
/// transport tests use to tell a governed subtree's output from a live one's.
#[cfg(feature = "synth")]
fn add_constant_synth_in_new_group(handle: &mut EngineHandle, group_id: i32, bus: i32, level: f32) {
    handle
        .send(Cmd::AddGroup {
            id: group_id,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::with_capacity(MAX_GROUP_CHILDREN),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: group_id + 1,
            target: group_id,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(
                constant_def(bus, level),
                48_000.0,
                SEED_STRIDE,
            )),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
}

/// Render `blocks` blocks of silence, to advance both clocks.
fn run_blocks(engine: &mut clausters::server::engine::Engine, blocks: usize) {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..blocks {
        engine.process_block(&mut out);
    }
}

#[test]
fn transport_clock_tracks_the_device_clock_while_rolling() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 10);

    let device = handle.current_samples();
    let transport = handle.current_transport_samples();
    assert_eq!(device, (BLOCK_SIZE * 10) as u64);
    assert_eq!(transport, device, "rolling, the two advance together");
}

#[test]
fn transport_clock_freezes_while_stopped_and_the_device_clock_does_not() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    let transport_at_stop = handle.current_transport_samples();

    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 6);

    assert_eq!(
        handle.current_transport_samples(),
        transport_at_stop,
        "stopped, the transport clock holds"
    );
    assert_eq!(
        handle.current_samples(),
        (BLOCK_SIZE * 10) as u64,
        "the device clock never stops"
    );
}

#[test]
fn resuming_continues_rather_than_restarting() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 6);
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);

    // 8 rolled blocks in total; the 6 frozen ones are not in the piece.
    assert_eq!(handle.current_transport_samples(), (BLOCK_SIZE * 8) as u64);
}

#[test]
fn a_transport_that_never_rolled_stays_at_zero() {
    let (mut engine, handle) = engine_pair(48_000.0, 2);
    run_blocks(&mut engine, 10);
    assert_eq!(handle.current_transport_samples(), 0);
    assert_eq!(handle.current_samples(), (BLOCK_SIZE * 10) as u64);
}

#[test]
fn frozen_time_is_counted_to_the_sample_not_to_the_block() {
    // A stop and a resume that both land *inside* a block, at different
    // offsets. Crediting a whole block of frozen time whenever the transport
    // happens to be stopped at the block boundary loses (stop offset - resume
    // offset) samples every cycle, and the error accumulates without bound —
    // so ten cycles here, and the assertion is the exact sample count.
    const STOP_AT: u64 = 32;
    const RESUME_AT: u64 = 16;
    const CYCLES: u64 = 10;
    const PERIOD: u64 = 6; // blocks between one stop and the next
    const BLOCKS: u64 = CYCLES * PERIOD + 2;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();

    let block = BLOCK_SIZE as u64;
    let mut frozen = 0;
    for cycle in 0..CYCLES {
        let stop = (cycle * PERIOD + 2) * block + STOP_AT;
        let resume = (cycle * PERIOD + 5) * block + RESUME_AT;
        frozen += resume - stop;
        handle
            .send(Cmd::Schedule {
                time: stop,
                cmds: vec![Cmd::TransportRun { rolling: false }],
            })
            .ok()
            .unwrap();
        handle
            .send(Cmd::Schedule {
                time: resume,
                cmds: vec![Cmd::TransportRun { rolling: true }],
            })
            .ok()
            .unwrap();
    }

    run_blocks(&mut engine, BLOCKS as usize);

    assert_eq!(handle.current_samples(), BLOCKS * block);
    assert_eq!(
        handle.current_transport_samples(),
        BLOCKS * block - frozen,
        "the transport clock counts exactly the samples it rolled"
    );
}

#[cfg(feature = "synth")]
#[test]
fn stopping_freezes_the_governed_subtree_and_leaves_the_rest_alone() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);

    // Group 100 is governed; group 200 is not. Each holds one synth writing a
    // constant to its own audio bus (bus 0 governed, bus 1 live).
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.5);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.25);
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert!(
        (out[0] - 0.5).abs() < 1e-6,
        "governed synth sounds while rolling"
    );
    assert!((out[1] - 0.25).abs() < 1e-6, "live synth sounds");

    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();
    engine.process_block(&mut out);
    assert_eq!(out[0], 0.0, "governed subtree is silent while stopped");
    assert!((out[1] - 0.25).abs() < 1e-6, "the live synth is untouched");

    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    engine.process_block(&mut out);
    assert!((out[0] - 0.5).abs() < 1e-6, "and it comes back on resume");
}

#[cfg(feature = "synth")]
#[test]
fn unbinding_while_stopped_unfreezes_the_group() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.5);
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert_eq!(out[0], 0.0);

    // Unbinding must not leave a frozen ownerless subtree behind.
    handle.send(Cmd::TransportGroup { id: -1 }).ok().unwrap();
    engine.process_block(&mut out);
    assert!(
        (out[0] - 0.5).abs() < 1e-6,
        "unbinding thaws what it governed"
    );
}

#[cfg(feature = "synth")]
#[test]
fn binding_while_already_stopped_freezes_immediately() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.5);

    // No TransportRun at all: the transport is stopped from the start, and
    // binding must freeze the group right away, before it ever rolls.
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert_eq!(
        out[0], 0.0,
        "binding a group while stopped freezes it immediately"
    );
}

#[cfg(feature = "synth")]
#[test]
fn rebinding_thaws_the_previous_group_before_taking_the_new_one() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.5);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.25);

    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert_eq!(out[0], 0.0, "group 100 is frozen while bound and stopped");
    assert!(
        (out[1] - 0.25).abs() < 1e-6,
        "group 200 is not governed yet"
    );

    // Rebind to group 200: this must thaw 100 before governing 200.
    handle.send(Cmd::TransportGroup { id: 200 }).ok().unwrap();
    engine.process_block(&mut out);
    assert!(
        (out[0] - 0.5).abs() < 1e-6,
        "rebinding thaws the previously governed group"
    );
    assert_eq!(out[1], 0.0, "the newly governed group freezes in turn");
}

#[cfg(feature = "synth")]
#[test]
fn a_bundle_to_a_governed_node_waits_out_the_pause() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.0);
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();

    // At transport sample 2 blocks in, raise the governed synth's level.
    handle
        .send(Cmd::Schedule {
            time: (BLOCK_SIZE * 2) as u64,
            cmds: vec![Cmd::SetControl {
                id: 101,
                index: 0,
                value: 0.5,
            }],
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out); // block 0
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();
    for _ in 0..5 {
        engine.process_block(&mut out); // 5 frozen blocks
    }
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    engine.process_block(&mut out); // block 1 of the piece
    assert_eq!(out[0], 0.0, "not due yet: only two blocks of the piece ran");
    engine.process_block(&mut out); // block 2 of the piece: due
    assert!((out[0] - 0.5).abs() < 1e-6, "fires at its transport sample");
}

#[cfg(feature = "synth")]
#[test]
fn a_bundle_to_a_live_node_fires_during_the_pause() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.0);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.0);
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();

    handle
        .send(Cmd::Schedule {
            time: (BLOCK_SIZE * 2) as u64,
            cmds: vec![Cmd::SetControl {
                id: 201,
                index: 0,
                value: 0.25,
            }],
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..3 {
        engine.process_block(&mut out);
    }
    assert!(
        (out[1] - 0.25).abs() < 1e-6,
        "a live node's bundle obeys the device clock and does not wait"
    );
}

#[cfg(feature = "synth")]
#[test]
fn moving_a_live_node_into_the_governed_group_waits_out_the_pause() {
    // A move is classified by *either* end. Splicing a node into a frozen
    // subtree is the same structural edit as creating one there, and both must
    // wait for the resume -- otherwise `/node_before` and `/node_add` answer
    // the same question differently.
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.0);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.25);
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();

    // Synth 201 is live and sounding on bus 1. Move it under the frozen group.
    handle
        .send(Cmd::Schedule {
            time: (BLOCK_SIZE * 2) as u64,
            cmds: vec![Cmd::MoveNode {
                id: 201,
                target: 100,
                place: Place::Head,
            }],
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..4 {
        engine.process_block(&mut out);
    }
    assert!(
        (out[1] - 0.25).abs() < 1e-6,
        "the move waited: 201 is still live in its own group, still sounding"
    );

    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    for _ in 0..4 {
        engine.process_block(&mut out);
    }
    assert!(
        (out[1] - 0.25).abs() < 1e-6,
        "rolling, 201 sounds wherever it lives"
    );

    // The move landed on the resume, so 201 is governed now: stopping again
    // must silence it. That is what proves the move happened at all -- a move
    // does not change which bus a synth writes, only who governs it.
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();
    engine.process_block(&mut out);
    assert_eq!(out[1], 0.0, "201 is inside the frozen group now");
}

#[cfg(feature = "synth")]
#[test]
fn a_mixed_bundle_goes_whole_to_the_transport_queue() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.0);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.0);
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();

    // One message inside, one outside: a bundle is atomic, so both wait.
    handle
        .send(Cmd::Schedule {
            time: BLOCK_SIZE as u64,
            cmds: vec![
                Cmd::SetControl {
                    id: 101,
                    index: 0,
                    value: 0.5,
                },
                Cmd::SetControl {
                    id: 201,
                    index: 0,
                    value: 0.25,
                },
            ],
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..4 {
        engine.process_block(&mut out);
    }
    assert_eq!(out[1], 0.0, "the live half waits with the governed half");

    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    for _ in 0..2 {
        engine.process_block(&mut out);
    }
    assert!(
        (out[1] - 0.25).abs() < 1e-6,
        "and both land together on resume"
    );
}

#[cfg(feature = "synth")]
#[test]
fn an_ungoverned_server_never_enters_the_two_queue_path() {
    // The transport queue stays empty when no group is bound, so scheduling
    // behaves exactly as it did before the transport existed.
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.0);
    handle
        .send(Cmd::Schedule {
            time: (BLOCK_SIZE * 2) as u64,
            cmds: vec![Cmd::SetControl {
                id: 201,
                index: 0,
                value: 0.25,
            }],
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..3 {
        engine.process_block(&mut out);
    }
    assert!((out[1] - 0.25).abs() < 1e-6);
    assert!(
        engine.transport_queue_is_empty(),
        "nothing was ever governed"
    );
}

#[test]
fn the_segment_publishes_the_transport_clock() {
    // A local peer reads the piece's position with a load, the way it already
    // reads the device clock -- and the two must differ while stopped, which is
    // the whole reason the second counter is in the header.
    let segment = Segment::in_memory();
    let (mut engine, mut handle) = engine_pair_full(
        48_000.0,
        2,
        0,
        Some(SegArc::clone(&segment)),
        NUM_AUDIO_BUSES,
        NUM_CONTROL_BUSES,
        Limits::default(),
    );
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    assert_eq!(
        segment.transport_clock().load(Ordering::Acquire),
        (BLOCK_SIZE * 4) as u64
    );

    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    assert_eq!(
        segment.transport_clock().load(Ordering::Acquire),
        (BLOCK_SIZE * 4) as u64,
        "the transport clock holds while the device clock runs on"
    );
    assert_eq!(
        segment.clock().load(Ordering::Acquire),
        (BLOCK_SIZE * 8) as u64
    );
}

/// Renders `blocks` blocks of a small governed piece, optionally freezing the
/// transport for the device-sample span `pause`. Returns the interleaved
/// output. The piece is a seeded noise generator plus a scheduled control
/// change, so both the stochastic and the message paths are exercised.
#[cfg(feature = "synth")]
fn render_piece(blocks: usize, pause: Option<(u64, u64)>) -> Vec<f32> {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_noise_synth_in_new_group(&mut handle, 100, 0);
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();

    // Two governed control changes, on the transport axis: they must land at
    // the same *piece* time in both renders, however long the pause was.
    for (beat, amp) in [(7u64, 0.4f32), (23, 0.15)] {
        handle
            .send(Cmd::Schedule {
                time: beat * BLOCK_SIZE as u64 + 11,
                cmds: vec![Cmd::SetControl {
                    id: 101,
                    index: 0,
                    value: amp,
                }],
            })
            .ok()
            .unwrap();
    }
    if let Some((start, end)) = pause {
        handle
            .send(Cmd::Schedule {
                time: start,
                cmds: vec![Cmd::TransportRun { rolling: false }],
            })
            .ok()
            .unwrap();
        handle
            .send(Cmd::Schedule {
                time: end,
                cmds: vec![Cmd::TransportRun { rolling: true }],
            })
            .ok()
            .unwrap();
    }

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut all = Vec::with_capacity(blocks * BLOCK_SIZE * 2);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        all.extend_from_slice(&out);
    }
    all
}

/// Freezing and resuming must be **transparent** to the piece: cut the frozen
/// span out of a paused render and it is the unpaused render, sample for
/// sample.
///
/// This one equality proves three things at once — the subtree's internal state
/// survived the freeze, the transport clock stopped exactly when the DSP did,
/// and the transport queue neither lost nor advanced an event. Over a
/// seeded-noise def it also proves the stochastic process **continued** rather
/// than restarted, which is the case no DAW transport protocol covers: a piece
/// that generates its own material has no index to seek to, so continuing is
/// the only thing a pause can mean.
///
/// The pause span is deliberately **not** block-aligned. An aligned span makes
/// this assertion blind to the defect it exists to catch: frozen time credited
/// a block at a time shows zero error exactly when both ends sit on a boundary.
#[cfg(feature = "synth")]
#[test]
fn a_pause_is_transparent_to_the_rendered_piece() {
    let block = BLOCK_SIZE as u64;
    let pause_start = 10 * block + 37;
    let pause_end = 27 * block + 5;
    let frozen = (pause_end - pause_start) as usize;

    let straight = render_piece(40, None);
    let paused = render_piece(58, Some((pause_start, pause_end)));

    // Cut the frozen span out of the paused render. Two channels, so a frame
    // index scales by the channel count.
    let cut_start = pause_start as usize * 2;
    let cut_end = cut_start + frozen * 2;
    let mut spliced = paused[..cut_start].to_vec();
    spliced.extend_from_slice(&paused[cut_end..]);
    spliced.truncate(straight.len());

    assert_eq!(spliced.len(), straight.len());
    for (i, (a, b)) in spliced.iter().zip(straight.iter()).enumerate() {
        assert_eq!(
            a,
            b,
            "sample {i} diverged after the splice (frame {})",
            i / 2
        );
    }
}

/// What a frozen subtree leaves on the buses it was writing.
///
/// Both answers already fall out of how the engine runs a block; this pins them
/// as guarantees rather than accidents, because a client mapping a control to a
/// governed synth depends on them.
///
/// An **audio** bus is a signal: it is cleared every block and nobody refills
/// it, so it goes silent, and a live effect reading it decays naturally. A
/// **control** bus is a value: control buses are not cleared, so it holds what
/// it last had. That asymmetry is the point — a control bus falling to zero
/// would make every parameter mapped to it jump on every pause.
#[cfg(feature = "synth")]
#[test]
fn a_frozen_subtree_silences_its_audio_bus_and_holds_its_control_bus() {
    let json = r#"{
        "name": "seam",
        "controls": [{"name": "level", "default": 0.5}],
        "ugens": [
            {"kind": "ReplaceOut", "inputs": [{"const": 0.0}, {"control": 0}]},
            {"kind": "OutCtl", "inputs": [{"const": 3.0}, {"control": 0}]}
        ]
    }"#;
    let spec: SynthDefSpec = serde_json::from_str(json).unwrap();
    let def = Arc::new(compile(spec).unwrap());

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::AddGroup {
            id: 100,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::with_capacity(MAX_GROUP_CHILDREN),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: 101,
            target: 100,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle.send(Cmd::TransportGroup { id: 100 }).ok().unwrap();
    handle
        .send(Cmd::TransportRun { rolling: true })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert!(
        (out[0] - 0.5).abs() < 1e-6,
        "rolling: the audio bus carries it"
    );
    assert!(
        (handle.control_buses().get(3) - 0.5).abs() < 1e-6,
        "rolling: the control bus carries it too"
    );

    handle
        .send(Cmd::TransportRun { rolling: false })
        .ok()
        .unwrap();
    engine.process_block(&mut out);

    assert_eq!(
        out[0], 0.0,
        "an audio bus is a signal: with nobody writing it, it is silence"
    );
    assert!(
        (handle.control_buses().get(3) - 0.5).abs() < 1e-6,
        "a control bus is a value: it holds, so no mapped parameter jumps to 0"
    );
}
