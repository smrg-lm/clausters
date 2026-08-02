//! What the transport costs per block, and whether that cost follows traffic.
//!
//! Three questions, three sections:
//!
//! 1. **Idle cost.** With no group bound the transport governs nothing, and the
//!    block must cost what it costs a server that has no transport at all:
//!
//!    ```sh
//!    cargo run --release --example bench_transport
//!    ```
//!
//! 2. **Governed cost.** With a group bound, is the block-cut loop's second
//!    queue measurable when nothing is scheduled? And what does a *stopped*
//!    transport cost (it should be cheaper: the subtree is skipped).
//!
//! 3. **Traffic dependence.** Routing a bundle walks each targeted node up its
//!    parent chain, on the audio thread, once per enqueue. That is the only
//!    part of the design whose cost scales with what clients send, so it is
//!    swept over both bundle count and tree size.
//!
//! Only `process_block` is timed; building the tree happens before the loop.

use std::sync::Arc;
use std::time::Instant;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::{Limits, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES};
use clausters::node::{AddAction, Group, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair_full};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{compile, default_spec};

const SAMPLE_RATE: f64 = 48000.0;
/// Minimum wall time per measurement.
const MEASURE_SECS: f64 = 0.4;
/// The block budget at 48 kHz: 64 frames must be computed in this much wall
/// time or the callback is late.
const BLOCK_BUDGET_NS: f64 = BLOCK_SIZE as f64 / SAMPLE_RATE * 1e9;
/// Largest tree swept below. The default group child capacity is 512, so a
/// bigger tree would be silently truncated (the engine rejects the adds) and
/// every "2048-voice" number would really be a 512-voice one -- raise the
/// limits at boot instead, the way `--max-graph-children` does.
const MAX_VOICES: usize = 4096;

/// An engine sized for the sweep rather than for the defaults.
fn boot() -> (Engine, EngineHandle) {
    engine_pair_full(
        SAMPLE_RATE as f32,
        2,
        0,
        None,
        NUM_AUDIO_BUSES,
        NUM_CONTROL_BUSES,
        Limits {
            max_nodes: MAX_VOICES * 2,
            max_group_children: MAX_VOICES,
            ..Limits::default()
        },
    )
}

fn make_synth() -> Box<dyn clausters::node::SynthNode> {
    let def = Arc::new(compile(default_spec()).unwrap());
    Box::new(UGenSynth::new(def, SAMPLE_RATE as f32, SEED_STRIDE))
}

/// Builds a tree of `voices` synths inside one group under the root, and
/// returns that group's id. The engine is stepped every so often because the
/// command FIFO is bounded: a big tree does not fit in it at once.
fn build(engine: &mut Engine, handle: &mut EngineHandle, voices: usize) -> i32 {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let group = 1000;
    handle
        .send(Cmd::AddGroup {
            id: group,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::with_capacity(MAX_VOICES),
        })
        .ok()
        .unwrap();
    for i in 0..voices {
        if i % 256 == 0 {
            engine.process_block(&mut out);
        }
        handle
            .send(Cmd::AddSynth {
                id: group + 1 + i as i32,
                target: group,
                action: AddAction::Tail,
                synth: make_synth(),
                usage: Default::default(),
            })
            .ok()
            .unwrap();
    }
    engine.process_block(&mut out);
    group
}

/// Times `process_block` over at least `MEASURE_SECS` of wall time, returning
/// nanoseconds per block.
fn time_blocks(engine: &mut Engine, out: &mut [f32]) -> f64 {
    // Warm up: the first block drains every queued command and touches the
    // pages the loop will reuse.
    for _ in 0..64 {
        engine.process_block(out);
    }
    let mut blocks = 0u64;
    let start = Instant::now();
    loop {
        for _ in 0..256 {
            engine.process_block(out);
            blocks += 1;
        }
        if start.elapsed().as_secs_f64() >= MEASURE_SECS {
            break;
        }
    }
    start.elapsed().as_nanos() as f64 / blocks as f64
}

fn report(label: &str, ns: f64, baseline: Option<f64>) {
    let pct = ns / BLOCK_BUDGET_NS * 100.0;
    match baseline {
        Some(base) => {
            let delta = (ns - base) / base * 100.0;
            println!("  {label:<38} {ns:8.1} ns/block  {pct:5.2}% budget  {delta:+6.2}%");
        }
        None => println!("  {label:<38} {ns:8.1} ns/block  {pct:5.2}% budget"),
    }
}

fn main() {
    println!(
        "transport cost — {SAMPLE_RATE} Hz, blocks of {BLOCK_SIZE} frames \
         (budget {BLOCK_BUDGET_NS:.0} ns)"
    );

    // ---- 1 & 2: the per-block cost with nothing scheduled ----
    for &voices in &[0usize, 64, 512] {
        println!("\n{voices} voices, no scheduled traffic:");
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];

        let (mut engine, mut handle) = boot();
        let _group = build(&mut engine, &mut handle, voices);
        let idle = time_blocks(&mut engine, &mut out);
        report("ungoverned (no group bound)", idle, None);

        {
            let (mut engine, mut handle) = boot();
            let group = build(&mut engine, &mut handle, voices);
            handle.send(Cmd::TransportGroup { id: group }).ok().unwrap();
            handle
                .send(Cmd::TransportRun { rolling: true })
                .ok()
                .unwrap();
            let rolling = time_blocks(&mut engine, &mut out);
            report("governed, rolling", rolling, Some(idle));

            let (mut engine, mut handle) = boot();
            let group = build(&mut engine, &mut handle, voices);
            handle.send(Cmd::TransportGroup { id: group }).ok().unwrap();
            handle
                .send(Cmd::TransportRun { rolling: false })
                .ok()
                .unwrap();
            let stopped = time_blocks(&mut engine, &mut out);
            report("governed, stopped (subtree frozen)", stopped, Some(idle));
        }
    }

    // ---- 3: does the cost follow the traffic? ----
    //
    // Routing happens once per enqueued bundle, so the load that matters is
    // bundles *arriving*, not bundles pending. Each measured block therefore
    // enqueues a fresh batch before processing.
    println!("\ntraffic dependence — routing runs once per arriving bundle:");
    for &voices in &[64usize, 512, 2048] {
        println!("  {voices}-voice tree:");
        for &per_block in &[0usize, 8, 64] {
            let mut out = vec![0.0f32; BLOCK_SIZE * 2];

            let ungoverned = {
                let (mut engine, mut handle) = boot();
                let group = build(&mut engine, &mut handle, voices);
                time_with_traffic(&mut engine, &mut handle, &mut out, per_block, group, voices)
            };
            let governed = {
                let (mut engine, mut handle) = boot();
                let group = build(&mut engine, &mut handle, voices);
                handle.send(Cmd::TransportGroup { id: group }).ok().unwrap();
                handle
                    .send(Cmd::TransportRun { rolling: true })
                    .ok()
                    .unwrap();
                time_with_traffic(&mut engine, &mut handle, &mut out, per_block, group, voices)
            };

            println!(
                "    {per_block:>3} bundles/block   ungoverned {ungoverned:9.1} ns   \
                 governed {governed:9.1} ns   {:+6.2}%",
                (governed - ungoverned) / ungoverned * 100.0
            );
        }
    }
}

/// Times blocks while `per_block` bundles arrive before each one. The bundles
/// target the **deepest** node in the tree, the worst case for a classifier
/// that resolves an id by scanning and then walks up its parents.
fn time_with_traffic(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    out: &mut [f32],
    per_block: usize,
    group: i32,
    voices: usize,
) -> f64 {
    let deepest = group + voices as i32;
    let enqueue = |handle: &mut EngineHandle, at: u64| {
        for i in 0..per_block {
            let _ = handle.send(Cmd::Schedule {
                time: at + 10_000_000 + i as u64,
                cmds: vec![Cmd::SetControl {
                    id: deepest,
                    index: 0,
                    value: 440.0,
                }],
            });
        }
    };
    for _ in 0..64 {
        enqueue(handle, 0);
        engine.process_block(out);
    }
    let mut blocks = 0u64;
    let start = Instant::now();
    loop {
        for _ in 0..256 {
            enqueue(handle, blocks);
            engine.process_block(out);
            blocks += 1;
        }
        if start.elapsed().as_secs_f64() >= MEASURE_SECS {
            break;
        }
    }
    start.elapsed().as_nanos() as f64 / blocks as f64
}
