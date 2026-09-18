//! **What a bus costs**, which is what decides how many a server boots with.
//!
//! Buses are a configured resource (`--audio-buses`, `--control-buses`) and
//! the count is a partition: half the space is private to GraphDef instances,
//! so a multitrack's tracks come out of the same number a patch's scopes do. Raising
//! it is not free -- every audio bus is a block of samples that is **cleared
//! every block**, on the audio thread -- so the default is a measurement and
//! not a taste.
//!
//! Ignored by default, because it is a measurement and not an assertion:
//! `cargo test --release --test bus_scale -- --ignored --nocapture`.

use clausters::server::engine::{BLOCK_SIZE, engine_pair_full};
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCKS: usize = 20_000;

/// The mean nanoseconds one empty `process_block` takes with `buses` audio
/// buses -- which is the clear, since nothing else is running.
fn idle_block(buses: usize) -> f64 {
    let (mut engine, _handle) = engine_pair_full(
        SR,
        2,
        0,
        None,
        buses,
        16_384,
        clausters::dsp::Limits::default(),
    );
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
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
fn what_an_audio_bus_costs_a_block() {
    let budget = BLOCK_SIZE as f64 / SR as f64 * 1e9;
    println!("one block's budget at {SR} Hz: {budget:.0} ns");
    println!(
        "\n{:>7}  {:>10}  {:>11}  {:>9}  {:>10}",
        "buses", "memory", "idle block", "of budget", "tracks"
    );
    for buses in [128usize, 256, 512, 1024, 4096, 16_384] {
        let idle = idle_block(buses);
        let bytes = buses * BLOCK_SIZE * 4;
        // A track spends four private buses (its mix and its post, stereo) and
        // a mono clip one more, so this is what the private half is worth in
        // the unit a person cares about.
        let tracks = clausters_core::registry::graph_audio_reserved(buses) / 4;
        println!(
            "{buses:>7}  {:>7} KB  {idle:>8.0} ns  {:>8.2} %  {tracks:>10}",
            bytes / 1024,
            idle / budget * 100.0
        );
    }
    println!(
        "\nEvery audio bus is a block of samples cleared on the audio thread \
         every block, so the count is paid whether or not anything is patched \
         through it. The tracks column is the private half divided by what a \
         track spends, which is the number the default is chosen for."
    );
}
