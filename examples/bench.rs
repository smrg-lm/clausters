//! Graph throughput benchmark (M7): processes blocks offline as fast as
//! possible and reports the real-time headroom at 48 kHz. The interesting
//! number is the "x real time" column: how many copies of that graph fit in
//! one audio callback budget. Run it in release mode:
//!
//! ```sh
//! cargo run --release --example bench
//! cargo run --release --example bench --features faust
//! ```

use std::sync::Arc;
use std::time::Instant;

use clausters::node::{AddAction, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{compile, default_spec};

const SAMPLE_RATE: f64 = 48000.0;
/// Minimum wall time per measurement.
const MEASURE_SECS: f64 = 0.5;

fn main() {
    println!(
        "graph benchmark — {SAMPLE_RATE} Hz, blocks of {BLOCK_SIZE} frames, release-mode wall clock"
    );
    println!("\ndefault def (SinOsc · amp → 2× Out):");
    for &n in &[1usize, 32, 128, 512, 1000] {
        report(n, bench(n, |_| make_default_synth()));
    }

    #[cfg(feature = "faust")]
    {
        use clausters::faust::compiler::{CompilePayload, compile};
        use clausters::faust::synth::FaustSynth;

        let src = r#"import("stdfaust.lib");
                     freq = hslider("freq", 440, 20, 20000, 0.01);
                     process = os.osc(freq) * 0.1;"#;
        let def = Arc::new(
            compile("bench", &CompilePayload::Source(src.into())).expect("faust def compiles"),
        );
        println!("\nfaust def (os.osc(freq) · 0.1, JIT-compiled):");
        for &n in &[1usize, 32, 128, 512, 1000] {
            let def = Arc::clone(&def);
            report(
                n,
                bench(n, move |_| {
                    Box::new(
                        FaustSynth::new(Arc::clone(&def), SAMPLE_RATE as f32)
                            .expect("faust instance"),
                    )
                }),
            );
        }
    }
}

fn make_default_synth() -> Box<dyn SynthNode> {
    static DEF: std::sync::OnceLock<Arc<clausters::synthdef::SynthDef>> = std::sync::OnceLock::new();
    let def = DEF.get_or_init(|| Arc::new(compile(default_spec()).expect("default def compiles")));
    Box::new(UGenSynth::new(Arc::clone(def)))
}

fn report(n: usize, blocks_per_sec: f64) {
    let xrt = blocks_per_sec * BLOCK_SIZE as f64 / SAMPLE_RATE;
    println!(
        "  {n:5} synths: {blocks_per_sec:>12.0} blocks/s = {xrt:>8.1}x real time ({:>7.1} synth·xRT)",
        xrt * n as f64
    );
}

/// Builds an engine with `n` synths (detuned via control 0) and measures
/// block throughput on this thread, exactly as the audio callback would run.
fn bench(n: usize, mut make: impl FnMut(usize) -> Box<dyn SynthNode>) -> f64 {
    let (mut engine, mut handle) = engine_pair(SAMPLE_RATE as f32, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for i in 0..n {
        let mut synth = make(i);
        synth.set_control(0, 50.0 + i as f32); // spread the frequencies
        add(&mut engine, &mut handle, &mut out, i as i32, synth);
    }
    engine.process_block(&mut out); // plug in the last batch
    handle.collect_garbage();

    for _ in 0..100 {
        engine.process_block(&mut out); // warmup
    }
    let start = Instant::now();
    let mut blocks = 0u64;
    loop {
        for _ in 0..256 {
            engine.process_block(&mut out);
        }
        blocks += 256;
        if start.elapsed().as_secs_f64() >= MEASURE_SECS {
            break;
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    handle.collect_garbage();
    assert!(out.iter().all(|s| s.is_finite()));
    blocks as f64 / elapsed
}

fn add(
    engine: &mut Engine,
    handle: &mut EngineHandle,
    out: &mut [f32],
    i: i32,
    synth: Box<dyn SynthNode>,
) {
    let mut cmd = Cmd::AddSynth {
        id: 1000 + i,
        target: 0,
        action: AddAction::Tail,
        synth,
    };
    // The command FIFO holds 1024 entries: drain it by processing a block
    // whenever it fills up.
    while let Err(back) = handle.send(cmd) {
        engine.process_block(out);
        handle.collect_garbage();
        cmd = back;
    }
}
