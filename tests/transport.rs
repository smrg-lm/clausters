//! The server transport: a governed subtree frozen and resumed sample-exactly.

use std::sync::Arc as SegArc;
use std::sync::atomic::Ordering;

use clausters::dsp::{Limits, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair, engine_pair_full};
use clausters::server::ipc::Segment;

// The group-binding tests below build a real synth to make a group's output
// legible, which needs the `synth` def family (UGenSynth/synthdef::compile).
// The transport-clock tests above them need neither, so only this half of
// the file is gated -- gating the whole file would silently drop the clock
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
/// can move it later with `Cmd::SetControl` -- which is how the scheduling
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
/// has no samples to index into. Its position *is* its internal state, so a
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
/// writes `level` to `bus` every block -- the constant-output helper the
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

/// A synth whose output *is* the transport's position in the transport: the one
/// thing that proves the position reaches a graph at all.
#[cfg(feature = "synth")]
fn position_def() -> Arc<SynthDef> {
    let json = r#"{
        "name": "position",
        "controls": [{"name": "offset", "default": 0.0}],
        "ugens": [
            {"kind": "TransportPos", "inputs": [{"control": 0}]},
            {"kind": "ReplaceOut", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    let spec: SynthDefSpec = serde_json::from_str(json).unwrap();
    Arc::new(compile(spec).unwrap())
}

/// Renders one block and hands back bus 0, deinterleaved from the two-channel
/// output the transport tests render into.
#[cfg(feature = "synth")]
fn block_of_bus_0(engine: &mut clausters::server::engine::Engine) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    out.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 10);

    let device = handle.current_samples();
    let transport = handle.current_transport_samples(0);
    assert_eq!(device, (BLOCK_SIZE * 10) as u64);
    assert_eq!(transport, device, "rolling, the two advance together");
}

#[test]
fn transport_clock_freezes_while_stopped_and_the_device_clock_does_not() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    let transport_at_stop = handle.current_transport_samples(0);

    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 6);

    assert_eq!(
        handle.current_transport_samples(0),
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 6);
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);

    // 8 rolled blocks in total; the 6 frozen ones are not in the transport.
    assert_eq!(handle.current_transport_samples(0), (BLOCK_SIZE * 8) as u64);
}

#[test]
fn a_transport_that_never_rolled_stays_at_zero() {
    let (mut engine, handle) = engine_pair(48_000.0, 2);
    run_blocks(&mut engine, 10);
    assert_eq!(handle.current_transport_samples(0), 0);
    assert_eq!(handle.current_samples(), (BLOCK_SIZE * 10) as u64);
}

#[test]
fn the_position_advances_with_the_transport_and_holds_when_it_stops() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    assert_eq!(
        handle.current_transport_position(0),
        (BLOCK_SIZE * 4) as u64,
        "rolling from 0, the transport is where the transport clock is"
    );

    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 6);
    assert_eq!(
        handle.current_transport_position(0),
        (BLOCK_SIZE * 4) as u64,
        "stopped, the transport stays where it was"
    );
}

/// The distinction the two quantities exist for: a locate moves the transport and
/// leaves both clocks exactly where they are.
#[test]
fn a_locate_moves_the_position_and_neither_clock() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    let device = handle.current_samples();
    let transport = handle.current_transport_samples(0);

    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 1_000,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 1);

    assert_eq!(
        handle.current_transport_position(0),
        1_000 + BLOCK_SIZE as u64,
        "located, then one block of playing from there"
    );
    assert_eq!(
        handle.current_samples(),
        device + BLOCK_SIZE as u64,
        "the device clock only counted the block"
    );
    assert_eq!(
        handle.current_transport_samples(0),
        transport + BLOCK_SIZE as u64,
        "and so did the transport clock: a locate is not a jump in time"
    );
}

/// Locating while stopped is what an editor does before it presses play.
#[test]
fn a_locate_while_stopped_holds_until_the_transport_rolls() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 500,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 3);
    assert_eq!(handle.current_transport_position(0), 500);

    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 2);
    assert_eq!(
        handle.current_transport_position(0),
        500 + (BLOCK_SIZE * 2) as u64
    );
}

/// The wrap is cut to the sample, not to the block: a loop whose length is not
/// a multiple of the block still lands exactly on its start.
#[test]
fn the_position_wraps_inside_a_loop_to_the_sample() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    // 100 samples, deliberately not a multiple of the 64-sample block.
    handle
        .send(Cmd::TransportLoop {
            transport: 0,
            span: Some(10..110),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 10,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();

    // Four blocks is 256 samples over a 100-sample loop: two whole wraps and
    // 56 into the third pass.
    run_blocks(&mut engine, 4);
    assert_eq!(
        handle.current_transport_position(0),
        10 + (256 % 100),
        "the transport is 256 samples into a 100-sample loop starting at 10"
    );
}

/// A loop is half-open, so its end sample is never played and its length is
/// exactly `end - start`: after one full pass the position is back at the
/// start, not one past it.
#[test]
fn a_loop_of_one_block_returns_exactly_to_its_start() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let len = BLOCK_SIZE as u64;
    handle
        .send(Cmd::TransportLoop {
            transport: 0,
            span: Some(0..len),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 1);
    assert_eq!(handle.current_transport_position(0), 0);
    run_blocks(&mut engine, 1);
    assert_eq!(handle.current_transport_position(0), 0);
}

/// An inverted or empty span is not a loop; taking it would make the wrap
/// non-terminating, so it is dropped rather than trusted.
#[test]
fn an_empty_or_inverted_loop_is_not_a_loop() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportLoop {
            transport: 0,
            span: Some(100..100),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 2);
    assert_eq!(
        handle.current_transport_position(0),
        (BLOCK_SIZE * 2) as u64,
        "the position ran on as if nothing had been set"
    );
}

/// Turning a loop on does not move the transport: it keeps playing and wraps when
/// it first reaches the end.
#[test]
fn setting_a_loop_does_not_relocate_the_transport() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 1);
    handle
        .send(Cmd::TransportLoop {
            transport: 0,
            span: Some(0..1_000),
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 1);
    assert_eq!(
        handle.current_transport_position(0),
        (BLOCK_SIZE * 2) as u64,
        "still where it would have been"
    );
}

/// The claim the whole milestone rests on: a graph can read where the transport
/// is, sample by sample.
#[test]
#[cfg(feature = "synth")]
fn a_graph_reads_the_position_and_it_ramps_one_frame_per_sample() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::AddSynth {
            id: 100,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(position_def(), 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 1_000,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();

    let block = block_of_bus_0(&mut engine);
    let expected: Vec<f32> = (0..BLOCK_SIZE).map(|i| (1_000 + i) as f32).collect();
    assert_eq!(
        block, expected,
        "located at 1000, then one frame per sample"
    );

    let next = block_of_bus_0(&mut engine);
    assert_eq!(
        next[0],
        (1_000 + BLOCK_SIZE) as f32,
        "the second block continues where the first ended"
    );
}

/// A stopped transport holds the position rather than ramping it -- the case a
/// reader outside the governed group sees, since a governed one is frozen and
/// never runs at all.
#[test]
#[cfg(feature = "synth")]
fn a_stopped_transport_holds_the_position_a_graph_reads() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::AddSynth {
            id: 100,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(position_def(), 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 7,
        })
        .ok()
        .unwrap();

    let block = block_of_bus_0(&mut engine);
    assert!(
        block.iter().all(|s| *s == 7.0),
        "not rolling: the transport stands still and so does the signal"
    );
}

/// The `offset` input is what a clip uses to read its own samples from frame
/// 0, and the subtraction happens inside the UGen in f64.
#[test]
#[cfg(feature = "synth")]
fn the_offset_input_reads_a_clip_from_its_own_first_frame() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::AddSynth {
            id: 100,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(position_def(), 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetControl {
            id: 100,
            index: 0,
            value: 5_000.0,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 5_000,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();

    let block = block_of_bus_0(&mut engine);
    assert_eq!(
        block[0], 0.0,
        "the transport is at the clip's start, so the clip is at its own frame 0"
    );
    assert_eq!(block[BLOCK_SIZE - 1], (BLOCK_SIZE - 1) as f32);
}

/// A loop's seam, read from inside the graph: the wrap lands on a sample and
/// not on a block, so the signal steps straight from the loop's last frame to
/// its first.
#[test]
#[cfg(feature = "synth")]
fn a_graph_sees_the_loop_wrap_on_its_exact_sample() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::AddSynth {
            id: 100,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(position_def(), 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    // A 10-sample loop, so the wrap falls well inside the first block.
    handle
        .send(Cmd::TransportLoop {
            transport: 0,
            span: Some(0..10),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();

    let block = block_of_bus_0(&mut engine);
    let expected: Vec<f32> = (0..BLOCK_SIZE).map(|i| (i % 10) as f32).collect();
    assert_eq!(
        block, expected,
        "the position sawtooths through the loop, one sample at a time"
    );
}

/// **The end mark stops the transport on its exact sample** and locates it
/// to the return: read from inside a graph, the position ramps up to the
/// mark's last sample and then stands on the return, and the transport clock
/// counts exactly the samples that rolled.
#[test]
#[cfg(feature = "synth")]
fn the_transport_stops_on_its_end_mark_and_goes_back() {
    use clausters::server::engine::{EndMark, Garbage};

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::AddSynth {
            id: 100,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(position_def(), 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    // Not block-aligned: the stop falls inside the second block.
    handle
        .send(Cmd::TransportEnd {
            transport: 0,
            mark: Some(EndMark {
                end: 100,
                back: Some(10),
            }),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();

    let mut seen = block_of_bus_0(&mut engine);
    seen.extend(block_of_bus_0(&mut engine));
    seen.extend(block_of_bus_0(&mut engine));
    let expected: Vec<f32> = (0..3 * BLOCK_SIZE)
        .map(|i| if i < 100 { i as f32 } else { 10.0 })
        .collect();
    assert_eq!(seen, expected, "up to the mark, then on the return");
    assert_eq!(handle.current_transport_samples(0), 100, "stopped on it");
    assert_eq!(handle.current_transport_position(0), 10);
    let mut told = false;
    while let Some(g) = handle.pop_garbage() {
        told |= matches!(g, Garbage::TransportEnded { .. });
    }
    assert!(told, "and the network side is told the pass is over");
}

/// A loop wins over the mark: the position wraps before it can reach it.
#[test]
fn a_loop_wins_over_the_end_mark() {
    use clausters::server::engine::EndMark;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportLoop {
            transport: 0,
            span: Some(0..50),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportEnd {
            transport: 0,
            mark: Some(EndMark {
                end: 30,
                back: None,
            }),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    assert_eq!(
        handle.current_transport_samples(0),
        4 * BLOCK_SIZE as u64,
        "it never stopped"
    );
}

/// The transport never rolls past its mark: a play from past it stops at
/// once, and with no return the position rests where it was. Clearing the
/// mark lets it roll.
#[test]
fn a_play_from_past_the_mark_stops_at_once_and_a_cleared_mark_rolls() {
    use clausters::server::engine::EndMark;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportEnd {
            transport: 0,
            mark: Some(EndMark { end: 5, back: None }),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 20,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 2);
    assert_eq!(handle.current_transport_samples(0), 0, "no sample rolled");
    assert_eq!(handle.current_transport_position(0), 20);

    handle
        .send(Cmd::TransportEnd {
            transport: 0,
            mark: None,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 2);
    assert_eq!(
        handle.current_transport_position(0),
        20 + 2 * BLOCK_SIZE as u64
    );
}

#[test]
fn frozen_time_is_counted_to_the_sample_not_to_the_block() {
    // A stop and a resume that both land *inside* a block, at different
    // offsets. Crediting a whole block of frozen time whenever the transport
    // happens to be stopped at the block boundary loses (stop offset - resume
    // offset) samples every cycle, and the error accumulates without bound --
    // so ten cycles here, and the assertion is the exact sample count.
    const STOP_AT: u64 = 32;
    const RESUME_AT: u64 = 16;
    const CYCLES: u64 = 10;
    const PERIOD: u64 = 6; // blocks between one stop and the next
    const BLOCKS: u64 = CYCLES * PERIOD + 2;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
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
                cmds: vec![Cmd::TransportRun {
                    transport: 0,
                    rolling: false,
                }],
            })
            .ok()
            .unwrap();
        handle
            .send(Cmd::Schedule {
                time: resume,
                cmds: vec![Cmd::TransportRun {
                    transport: 0,
                    rolling: true,
                }],
            })
            .ok()
            .unwrap();
    }

    run_blocks(&mut engine, BLOCKS as usize);

    assert_eq!(handle.current_samples(), BLOCKS * block);
    assert_eq!(
        handle.current_transport_samples(0),
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
        .ok()
        .unwrap();
    engine.process_block(&mut out);
    assert_eq!(out[0], 0.0, "governed subtree is silent while stopped");
    assert!((out[1] - 0.25).abs() < 1e-6, "the live synth is untouched");

    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert_eq!(out[0], 0.0);

    // Unbinding must not leave a frozen ownerless subtree behind.
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: -1,
        })
        .ok()
        .unwrap();
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();

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

    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 200,
        })
        .ok()
        .unwrap();
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
        .ok()
        .unwrap();
    for _ in 0..5 {
        engine.process_block(&mut out); // 5 frozen blocks
    }
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    engine.process_block(&mut out); // block 1 of the transport
    assert_eq!(
        out[0], 0.0,
        "not due yet: only two blocks of the transport ran"
    );
    engine.process_block(&mut out); // block 2 of the transport: due
    assert!((out[0] - 0.5).abs() < 1e-6, "fires at its transport sample");
}

#[cfg(feature = "synth")]
#[test]
fn a_bundle_to_a_live_node_fires_during_the_pause() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.0);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.0);
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
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
        engine.transport_queue_is_empty(0),
        "nothing was ever governed"
    );
}

#[test]
fn the_segment_publishes_the_transport_clock() {
    // A local peer reads the transport's position with a load, the way it already
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    assert_eq!(
        segment.transport_clock(0).unwrap().load(Ordering::Acquire),
        (BLOCK_SIZE * 4) as u64
    );

    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 4);
    assert_eq!(
        segment.transport_clock(0).unwrap().load(Ordering::Acquire),
        (BLOCK_SIZE * 4) as u64,
        "the transport clock holds while the device clock runs on"
    );
    assert_eq!(
        segment.clock().load(Ordering::Acquire),
        (BLOCK_SIZE * 8) as u64
    );
}

/// The segment carries the position too, and a locate is what separates it
/// from the clock beside it: same header, same block, different answer.
#[test]
fn the_segment_publishes_the_position_a_locate_moved() {
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
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 9_000,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 2);

    assert_eq!(
        segment
            .transport_position(0)
            .unwrap()
            .load(Ordering::Acquire),
        9_000 + (BLOCK_SIZE * 2) as u64,
        "the transport is where it was located, plus what has played since"
    );
    assert_eq!(
        segment.transport_clock(0).unwrap().load(Ordering::Acquire),
        (BLOCK_SIZE * 2) as u64,
        "the clock beside it counted only the samples that elapsed"
    );
}

/// Renders `blocks` blocks of a small governed transport, optionally freezing the
/// transport for the device-sample span `pause`. Returns the interleaved
/// output. The transport is a seeded noise generator plus a scheduled control
/// change, so both the stochastic and the message paths are exercised.
#[cfg(feature = "synth")]
fn render_take(blocks: usize, pause: Option<(u64, u64)>) -> Vec<f32> {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_noise_synth_in_new_group(&mut handle, 100, 0);
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();

    // Two governed control changes, on the transport axis: they must land at
    // the same *transport* time in both renders, however long the pause was.
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
                cmds: vec![Cmd::TransportRun {
                    transport: 0,
                    rolling: false,
                }],
            })
            .ok()
            .unwrap();
        handle
            .send(Cmd::Schedule {
                time: end,
                cmds: vec![Cmd::TransportRun {
                    transport: 0,
                    rolling: true,
                }],
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

/// Freezing and resuming must be **transparent** to the transport: cut the frozen
/// span out of a paused render and it is the unpaused render, sample for
/// sample.
///
/// This one equality proves three things at once -- the subtree's internal state
/// survived the freeze, the transport clock stopped exactly when the DSP did,
/// and the transport queue neither lost nor advanced an event. Over a
/// seeded-noise def it also proves the stochastic process **continued** rather
/// than restarted, which is the case no DAW transport protocol covers: a transport
/// that generates its own samples has no index to seek to, so continuing is
/// the only thing a pause can mean.
///
/// The pause span is deliberately **not** block-aligned. An aligned span makes
/// this assertion blind to the defect it exists to catch: frozen time credited
/// a block at a time shows zero error exactly when both ends sit on a boundary.
#[cfg(feature = "synth")]
#[test]
fn a_pause_is_transparent_to_the_rendered_take() {
    let block = BLOCK_SIZE as u64;
    let pause_start = 10 * block + 37;
    let pause_end = 27 * block + 5;
    let frozen = (pause_end - pause_start) as usize;

    let straight = render_take(40, None);
    let paused = render_take(58, Some((pause_start, pause_end)));

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
/// it last had. That asymmetry is the point -- a control bus falling to zero
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
    handle
        .send(Cmd::TransportGroup {
            transport: 0,
            id: 100,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
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
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: false,
        })
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

// ---- several transports ----

/// Rolls or stops transport `transport`.
fn run(handle: &mut clausters::server::engine::EngineHandle, transport: usize, rolling: bool) {
    handle
        .send(Cmd::TransportRun { transport, rolling })
        .ok()
        .unwrap();
}

/// Two transports on one engine keep two clocks and two positions: rolling,
/// stopping and locating one moves nothing about the other.
#[test]
fn two_transports_roll_stop_and_locate_independently() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    assert_eq!(engine.transports(), clausters::dsp::DEFAULT_TRANSPORTS);
    run(&mut handle, 0, true);
    run_blocks(&mut engine, 3);
    assert_eq!(handle.current_transport_samples(0), (BLOCK_SIZE * 3) as u64);
    assert_eq!(
        handle.current_transport_samples(1),
        0,
        "transport 1 never rolled"
    );

    handle
        .send(Cmd::TransportLocate {
            transport: 1,
            position: 5_000,
        })
        .ok()
        .unwrap();
    run(&mut handle, 1, true);
    run(&mut handle, 0, false);
    run_blocks(&mut engine, 2);
    assert_eq!(
        handle.current_transport_position(1),
        5_000 + (BLOCK_SIZE * 2) as u64,
        "transport 1 rolls from where it was located"
    );
    assert_eq!(
        handle.current_transport_position(0),
        (BLOCK_SIZE * 3) as u64,
        "transport 0 holds where it stopped, and the locate did not reach it"
    );
    assert_eq!(handle.current_frozen_total(0), (BLOCK_SIZE * 2) as u64);
    assert_eq!(
        handle.current_frozen_total(1),
        (BLOCK_SIZE * 3) as u64,
        "each counts only the time it spent stopped"
    );
}

/// A stop scheduled inside a block freezes its own transport on that sample
/// and leaves the other rolling through it.
#[test]
fn a_stop_mid_block_freezes_its_own_transport_to_the_sample() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    run(&mut handle, 0, true);
    run(&mut handle, 1, true);
    handle
        .send(Cmd::Schedule {
            time: 10,
            cmds: vec![Cmd::TransportRun {
                transport: 1,
                rolling: false,
            }],
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 2);
    assert_eq!(handle.current_transport_samples(0), (BLOCK_SIZE * 2) as u64);
    assert_eq!(
        handle.current_transport_samples(1),
        10,
        "transport 1 stopped on sample 10"
    );
}

/// Each transport wraps inside its own loop, on its own sample.
#[test]
fn each_transport_wraps_inside_its_own_loop() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    for (transport, end) in [(0, 100u64), (1, 150)] {
        handle
            .send(Cmd::TransportLoop {
                transport,
                span: Some(0..end),
            })
            .ok()
            .unwrap();
        run(&mut handle, transport, true);
    }
    run_blocks(&mut engine, 4);
    let played = (BLOCK_SIZE * 4) as u64;
    assert_eq!(handle.current_transport_position(0), played % 100);
    assert_eq!(handle.current_transport_position(1), played % 150);
}

/// A command naming a transport the engine does not have is dropped, and
/// touches none of the ones it has.
#[test]
fn a_transport_past_the_table_is_ignored() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let past = engine.transports();
    run(&mut handle, past, true);
    handle
        .send(Cmd::TransportLocate {
            transport: past,
            position: 99,
        })
        .ok()
        .unwrap();
    run_blocks(&mut engine, 1);
    for transport in 0..past {
        assert_eq!(handle.current_transport_samples(transport), 0);
        assert_eq!(handle.current_transport_position(transport), 0);
    }
}

/// The segment publishes every transport, and says how many there are.
#[test]
fn the_segment_publishes_each_transport() {
    let segment = Segment::in_memory();
    let limits = Limits {
        transports: 3,
        ..Limits::default()
    };
    let (mut engine, mut handle) = engine_pair_full(
        48_000.0,
        2,
        0,
        Some(SegArc::clone(&segment)),
        NUM_AUDIO_BUSES,
        NUM_CONTROL_BUSES,
        limits,
    );
    assert_eq!(segment.transports(), 3);
    handle
        .send(Cmd::TransportLocate {
            transport: 2,
            position: 700,
        })
        .ok()
        .unwrap();
    run(&mut handle, 2, true);
    run_blocks(&mut engine, 2);
    let at = |i: usize| {
        (
            segment.transport_clock(i).unwrap().load(Ordering::Acquire),
            segment
                .transport_position(i)
                .unwrap()
                .load(Ordering::Acquire),
        )
    };
    assert_eq!(at(0), (0, 0));
    assert_eq!(at(1), (0, 0));
    assert_eq!(
        at(2),
        ((BLOCK_SIZE * 2) as u64, 700 + (BLOCK_SIZE * 2) as u64)
    );
}

/// A synth writing its transport's position to `bus`: what shows which
/// transport a node reads.
#[cfg(feature = "synth")]
fn position_def_to(bus: i32) -> Arc<SynthDef> {
    let json = format!(
        r#"{{
            "name": "position",
            "controls": [{{"name": "offset", "default": 0.0}}],
            "ugens": [
                {{"kind": "TransportPos", "inputs": [{{"control": 0}}]}},
                {{"kind": "ReplaceOut", "inputs": [{{"const": {bus}.0}}, {{"ugen": 0}}]}}
            ]
        }}"#
    );
    let spec: SynthDefSpec = serde_json::from_str(&json).unwrap();
    Arc::new(compile(spec).unwrap())
}

/// A group under the root holding one position synth writing to `bus`.
#[cfg(feature = "synth")]
fn add_position_synth_in_new_group(handle: &mut EngineHandle, group_id: i32, bus: i32) {
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
            synth: Box::new(UGenSynth::new(position_def_to(bus), 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
}

/// A node reads the transport governing it, and a node governed by none reads
/// transport 0 -- the one a server has always had. Stopping one transport
/// freezes its own group only.
#[cfg(feature = "synth")]
#[test]
fn a_node_reads_the_transport_that_governs_it() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 4);
    add_position_synth_in_new_group(&mut handle, 100, 0);
    add_position_synth_in_new_group(&mut handle, 200, 1);
    add_position_synth_in_new_group(&mut handle, 300, 2);
    for (transport, group, position) in [(0, 100, 1_000u64), (1, 200, 5_000)] {
        handle
            .send(Cmd::TransportGroup {
                transport,
                id: group,
            })
            .ok()
            .unwrap();
        handle
            .send(Cmd::TransportLocate {
                transport,
                position,
            })
            .ok()
            .unwrap();
        run(&mut handle, transport, true);
    }
    let mut out = vec![0.0f32; BLOCK_SIZE * 4];
    engine.process_block(&mut out);
    let bus =
        |out: &[f32], b: usize| -> Vec<f32> { out.iter().skip(b).step_by(4).copied().collect() };
    let ramp = |from: usize| -> Vec<f32> { (0..BLOCK_SIZE).map(|i| (from + i) as f32).collect() };
    assert_eq!(bus(&out, 0), ramp(1_000), "group 100 reads transport 0");
    assert_eq!(bus(&out, 1), ramp(5_000), "group 200 reads transport 1");
    assert_eq!(
        bus(&out, 2),
        ramp(1_000),
        "an ungoverned group reads transport 0"
    );

    run(&mut handle, 1, false);
    engine.process_block(&mut out);
    assert_eq!(
        bus(&out, 0),
        ramp(1_000 + BLOCK_SIZE),
        "transport 0 rolls on"
    );
    assert!(
        bus(&out, 1).iter().all(|s| *s == 0.0),
        "transport 1's group is frozen"
    );
}

/// A timed bundle waits on the queue of the transport governing its target,
/// and only that one: stopping transport 1 holds its bundle and leaves one
/// aimed at transport 0's group to fire.
#[cfg(feature = "synth")]
#[test]
fn a_bundle_waits_on_the_transport_that_governs_its_target() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.0);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.0);
    for (transport, group) in [(0, 100), (1, 200)] {
        handle
            .send(Cmd::TransportGroup {
                transport,
                id: group,
            })
            .ok()
            .unwrap();
        run(&mut handle, transport, true);
    }
    for id in [101, 201] {
        handle
            .send(Cmd::Schedule {
                time: (BLOCK_SIZE * 2) as u64,
                cmds: vec![Cmd::SetControl {
                    id,
                    index: 0,
                    value: 0.5,
                }],
            })
            .ok()
            .unwrap();
    }
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert!(!engine.transport_queue_is_empty(0));
    assert!(!engine.transport_queue_is_empty(1));
    run(&mut handle, 1, false);
    for _ in 0..3 {
        engine.process_block(&mut out);
    }
    assert!((out[0] - 0.5).abs() < 1e-6, "transport 0's bundle fired");
    assert!(
        !engine.transport_queue_is_empty(1),
        "transport 1's waits out its pause"
    );
    run(&mut handle, 1, true);
    engine.process_block(&mut out);
    engine.process_block(&mut out);
    assert!((out[1] - 0.5).abs() < 1e-6, "and fires on its own clock");
}

/// Binding a group to a transport takes it away from nothing else and tags
/// only it: rebinding transport 1 to another group thaws the first.
#[cfg(feature = "synth")]
#[test]
fn rebinding_one_transport_leaves_the_other_bound() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    add_constant_synth_in_new_group(&mut handle, 100, 0, 0.5);
    add_constant_synth_in_new_group(&mut handle, 200, 1, 0.25);
    for (transport, group) in [(0, 100), (1, 200)] {
        handle
            .send(Cmd::TransportGroup {
                transport,
                id: group,
            })
            .ok()
            .unwrap();
    }
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out);
    assert_eq!((out[0], out[1]), (0.0, 0.0), "both stopped, both frozen");
    handle
        .send(Cmd::TransportGroup {
            transport: 1,
            id: -1,
        })
        .ok()
        .unwrap();
    engine.process_block(&mut out);
    assert_eq!(out[0], 0.0, "transport 0 still holds its group");
    assert!((out[1] - 0.25).abs() < 1e-6, "unbinding thawed group 200");
}
