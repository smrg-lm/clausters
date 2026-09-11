//! **What a join costs a whole engine**, which is the only number that decides
//! anything.
//!
//! `tests/buffer_stitch.rs` measures one read against one read, and a
//! microbenchmark of nothing but the load exaggerates every added operation.
//! This measures what a piece actually is: many `PlayBuf` readers running in
//! one engine, once over plain buffers and once over joins of the same samples,
//! timed by `process_block` — the budget the audio thread is actually spending.
//!
//! Ignored by default, because it is a measurement and not an assertion:
//! `cargo test --release --test stitch_load -- --ignored --nocapture`.

#![cfg(feature = "synth")]

use std::sync::Arc;
use std::time::Instant;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::buffer::Buffer;
use clausters::dsp::stitch::{PartSpec, Stitch};
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};

const SR: f64 = 48_000.0;
const FRAMES: usize = 48_000;
const BLOCKS: usize = 4_000;

/// One `PlayBuf` per reader, each on its own buffer index, all summed to bus 0.
fn player(index: f32) -> SynthDefSpec {
    serde_json::from_str(&format!(
        r#"{{
            "name": "player",
            "ugens": [
                {{"kind": "PlayBuf", "inputs": [
                    {{"const": {index}.0}}, {{"const": 0.0}}, {{"const": 1.0}},
                    {{"const": 1.0}}, {{"const": 0.0}}, {{"const": 0.0}}, {{"const": 0.0}}
                ]}},
                {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}
            ]
        }}"#
    ))
    .unwrap()
}

fn take() -> Arc<Buffer> {
    Arc::new(Buffer::new(
        (0..FRAMES)
            .map(|i| ((i % 971) as f32 / 971.0) - 0.5)
            .collect(),
        1,
        FRAMES,
        SR,
    ))
}

/// The same samples as a join of `parts` pieces, each fading into the next —
/// which is what a comping pass cut by hand looks like.
fn joined(src: &Arc<Buffer>, parts: usize) -> Arc<Buffer> {
    joined_with(src, parts, 64)
}

fn joined_with(src: &Arc<Buffer>, parts: usize, fade: usize) -> Arc<Buffer> {
    let span = FRAMES / parts;
    let specs = (0..parts)
        .map(|i| PartSpec {
            src: Arc::clone(src),
            src_index: 0,
            src_start: i * span,
            frames: span,
            fade_in: fade,
            fade_out: fade,
            map: vec![0],
        })
        .collect();
    Arc::new(Buffer::stitched(
        Stitch::new(specs, 1, SR).expect("built"),
        1,
        SR,
    ))
}

/// Runs `readers` players over the buffers `make` hands out, and answers the
/// mean nanoseconds one `process_block` took.
fn run(readers: usize, make: impl Fn(usize) -> Arc<Buffer>) -> f64 {
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for i in 0..readers {
        handle
            .send(Cmd::SetBuffer {
                index: i,
                buffer: Some(make(i)),
            })
            .ok()
            .unwrap();
        let def = Arc::new(compile(player(i as f32)).unwrap());
        handle
            .send(Cmd::AddSynth {
                id: 1000 + i as i32,
                target: ROOT_NODE_ID,
                action: AddAction::Tail,
                synth: Box::new(UGenSynth::new(def, SR as f32, SEED_STRIDE)),
                usage: Default::default(),
            })
            .ok()
            .unwrap();
    }
    // Warm up: the commands are applied on the first blocks.
    for _ in 0..64 {
        engine.process_block(&mut out);
    }
    let start = Instant::now();
    for _ in 0..BLOCKS {
        engine.process_block(&mut out);
    }
    start.elapsed().as_secs_f64() / BLOCKS as f64 * 1e9
}

#[test]
#[ignore]
fn what_a_join_costs_a_whole_engine() {
    let budget = BLOCK_SIZE as f64 / SR * 1e9;
    println!("one block's budget at {SR} Hz: {budget:.0} ns\n");
    println!(
        "{:>7}  {:>11}  {:>11}  {:>11}  {:>11}",
        "readers", "plain", "join x8", "x8 no fade", "join x256"
    );
    for readers in [1usize, 8, 32, 128] {
        let src = take();
        let plain = run(readers, |_| Arc::clone(&src));
        let few = run(readers, |_| joined(&src, 8));
        let bare = run(readers, |_| joined_with(&src, 8, 0));
        let many = run(readers, |_| joined(&src, 256));
        println!(
            "{readers:>7}  {:>8.0} ns  {:>8.0} ns  {:>8.0} ns  {:>8.0} ns",
            plain, few, bare, many
        );
        println!(
            "         {:>8.2} %  {:>8.2} %  {:>8.2} %  {:>8.2} %   of one block",
            plain / budget * 100.0,
            few / budget * 100.0,
            bare / budget * 100.0,
            many / budget * 100.0
        );
        println!(
            "         {:>8}    {:>+7.0}%    {:>+7.0}%    {:>+7.0}%   over plain",
            "",
            (few / plain - 1.0) * 100.0,
            (bare / plain - 1.0) * 100.0,
            (many / plain - 1.0) * 100.0
        );
    }
    println!(
        "\nA reader holds the run it is inside (`Buffer::run_at`), so the lookup is \
         once a block and not once a sample.\nWhat is left over a plain buffer is \
         mostly the crossfade -- which is audio somebody asked for -- and how many \
         parts a join has still barely matters, except that a shorter part spends \
         more of itself inside a fade."
    );
}
