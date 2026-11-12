//! **What a bus costs**, which is what decides how many a server boots with.
//!
//! Buses are a configured resource (`--audio-buses`, `--control-buses`) and
//! the count is a partition: half the space is private to GraphDef instances,
//! so a multitrack's tracks come out of the same number a patch's scopes do. Raising
//! it is not free -- every audio bus has a write marker checked each block,
//! and written buses are cleared on the next block -- so the default is a measurement and
//! not a taste.
//!
//! Ignored by default, because it is a measurement and not an assertion:
//! `cargo test --release --test bus_scale -- --ignored --nocapture`.

use clausters::server::engine::{BLOCK_SIZE, engine_pair_full};
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCKS: usize = 20_000;

/// The mean nanoseconds one empty `process_block` takes with `buses` audio
/// buses -- including the write-marker checks, with nothing else running.
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
        "\nEvery audio bus checks a write marker on the audio thread each \
         block; only written buses clear samples. The tracks column is the private half divided by what a \
         track spends, which is the number the default is chosen for."
    );
}

#[test]
fn untouched_audio_buses_stay_silent_and_written_buses_clear_once() {
    use clausters::dsp::{Buses, ControlBuses};
    let mut buses = Buses::new(ControlBuses::new(8), 128);
    assert!((0..128).all(|bus| !buses.audio_written(bus)));
    std::thread::scope(|scope| {
        for bus in [0, 17, 127] {
            let shared = &buses;
            scope.spawn(move || {
                // SAFETY: each thread has exclusive access to its bus.
                unsafe { shared.audio_mut(bus) }.fill(0.25);
            });
        }
    });
    for bus in 0..128 {
        let written = [0, 17, 127].contains(&bus);
        assert_eq!(buses.audio_written(bus), written);
        assert!(
            buses
                .audio(bus)
                .iter()
                .all(|sample| *sample == if written { 0.25 } else { 0.0 })
        );
    }
    for _ in 0..2 {
        buses.clear_audio();
        assert!((0..128).all(|bus| !buses.audio_written(bus)));
        assert!((0..128).all(|bus| buses.audio(bus).iter().all(|sample| *sample == 0.0)));
    }
    // Clamped writes and ReplaceOut go through the same activity tracking.
    // SAFETY: this thread exclusively owns every bus.
    (unsafe { buses.audio_replace(1000) })[0] = 0.5;
    assert!(buses.audio_written(127));
    buses.clear_audio();
    assert_eq!(buses.audio(127)[0], 0.0);
}
