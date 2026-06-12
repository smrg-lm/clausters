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
use clausters::server::engine::{
    BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair, engine_pair_with_workers,
};
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
    // M13: parallel groups. Independent chains on disjoint buses become one
    // stage that fans out across the worker pool; the speedup column is the
    // whole point of /g_parallel.
    let chains = 8usize;
    let voices = 125usize;
    println!(
        "\nparallel group (/g_parallel): {chains} subgroups x {voices} sines, disjoint buses:"
    );
    let max_workers = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(0)
        .min(8);
    let base = bench_parallel(0, chains, voices);
    report_parallel(0, base, base);
    let mut counts: Vec<usize> = [1, 2, 3, max_workers]
        .into_iter()
        .filter(|&w| (1..=max_workers).contains(&w))
        .collect();
    counts.dedup();
    for w in counts {
        report_parallel(w, bench_parallel(w, chains, voices), base);
    }

}

fn make_default_synth() -> Box<dyn SynthNode> {
    static DEF: std::sync::OnceLock<Arc<clausters::synthdef::SynthDef>> = std::sync::OnceLock::new();
    let def = DEF.get_or_init(|| Arc::new(compile(default_spec()).expect("default def compiles")));
    Box::new(UGenSynth::new(Arc::clone(def)))
}


/// One parallel group with `chains` subgroups, each holding `voices` sines
/// summing into that chain's private bus — the layout where /g_parallel
/// shines: every subgroup is an independent unit of one big stage.
fn bench_parallel(workers: usize, chains: usize, voices: usize) -> f64 {
    use clausters::dsp::BusUsage;
    use clausters::node::Group;
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair_with_workers(SAMPLE_RATE as f32, 2, workers);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: 0,
            action: AddAction::Tail,
            group: Group::new(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetGroupParallel {
            id: 1,
            parallel: true,
        })
        .ok()
        .unwrap();

    for k in 0..chains {
        let gid = 10 + k as i32;
        handle
            .send(Cmd::AddGroup {
                id: gid,
                target: 1,
                action: AddAction::Tail,
                group: Group::new(),
            })
            .ok()
            .unwrap();
        let bus = 8.0 + k as f64;
        let spec: SynthDefSpec = serde_json::from_value(serde_json::json!({
            "name": format!("chain{k}"),
            "controls": [{"name": "freq", "default": 220.0}],
            "ugens": [
                {"kind": "SinOsc", "inputs": [{"control": 0}]},
                {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.001}]},
                {"kind": "Out", "inputs": [{"const": bus}, {"ugen": 1}]}
            ]
        }))
        .unwrap();
        let def = Arc::new(compile(spec).unwrap());
        let mut usage = BusUsage::default();
        usage.mark(bus as f32, false, true);
        for v in 0..voices {
            let mut synth = Box::new(UGenSynth::new(Arc::clone(&def)));
            synth.set_control(0, 50.0 + (k * voices + v) as f32);
            let mut cmd = Cmd::AddSynth {
                id: 1000 + (k * voices + v) as i32,
                target: gid,
                action: AddAction::Tail,
                synth,
                usage,
            };
            while let Err(back) = handle.send(cmd) {
                engine.process_block(&mut out);
                handle.collect_garbage();
                cmd = back;
            }
        }
    }
    engine.process_block(&mut out);
    handle.collect_garbage();

    for _ in 0..100 {
        engine.process_block(&mut out);
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
    assert!(out.iter().all(|s| s.is_finite()));
    blocks as f64 / elapsed
}

fn report_parallel(workers: usize, blocks_per_sec: f64, base: f64) {
    let xrt = blocks_per_sec * BLOCK_SIZE as f64 / SAMPLE_RATE;
    println!(
        "  {workers} workers: {blocks_per_sec:>12.0} blocks/s = {xrt:>8.1}x real time (speedup {:>4.2}x)",
        blocks_per_sec / base
    );
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
        usage: Default::default(),
    };
    // The command FIFO holds 1024 entries: drain it by processing a block
    // whenever it fills up.
    while let Err(back) = handle.send(cmd) {
        engine.process_block(out);
        handle.collect_garbage();
        cmd = back;
    }
}
