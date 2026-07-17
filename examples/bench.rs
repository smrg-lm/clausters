//! Graph throughput benchmark (M7): processes blocks offline as fast as
//! possible and reports the real-time headroom at 48 kHz. The interesting
//! number is the "x real time" column: how many copies of that graph fit in
//! one audio callback budget. Run it in release mode:
//!
//! ```sh
//! cargo run --release --example bench
//! cargo run --release --example bench --features faust
//! ```
//!
//! With `--features faust` it also runs two **apples-to-apples** UGen-vs-Faust
//! sections, both using `tests/faust_parity.rs` pairs the two engines compute
//! sample for sample, so the timing isolates per-synth audio-loop overhead —
//! the Rust UGen graph (boxed `dyn` dispatch per UGen, intermediate wire
//! buffers) against one Faust LLVM `compute` call. Only `process` is timed;
//! instantiation and JIT happen before the loop.
//!
//! - **sine** (`sin(2π·phasor)·0.2`): realistic, but our `Sine` works in f64
//!   while Faust `-single` is f32, so part of the gap is precision.
//! - **gain** (`·0.5` on a shared bus): bit-exact, no transcendental — the
//!   cleanest read of pure engine overhead.

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
/// Voice counts swept by every per-synth benchmark.
const VOICE_COUNTS: &[usize] = &[1, 32, 128, 512, 1000];

fn main() {
    println!(
        "graph benchmark — {SAMPLE_RATE} Hz, blocks of {BLOCK_SIZE} frames, release-mode wall clock"
    );
    println!("\ndefault def (Sine · amp → 2× Out):");
    for &n in VOICE_COUNTS {
        report(n, bench(n, |_| make_default_synth()));
    }

    #[cfg(feature = "faust")]
    {
        bench_ugen_vs_faust();
        bench_gain_overhead();
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
    static DEF: std::sync::OnceLock<Arc<clausters::synthdef::SynthDef>> =
        std::sync::OnceLock::new();
    let def = DEF.get_or_init(|| Arc::new(compile(default_spec()).expect("default def compiles")));
    Box::new(UGenSynth::new(Arc::clone(def)))
}

/// Head-to-head: the **same** DSP run by the two engines, so the only thing
/// the timing reflects is per-synth audio-loop overhead — UGen graph vs Faust
/// LLVM. The graph is `sin(2π·phasor(freq)) · 0.2 → one bus`, which is exactly
/// the parity pair from `tests/faust_parity.rs` (proven to agree sample for
/// sample): identical math, one output each, same frequency control (index 0,
/// swept by the harness), same `out` bus (0). Setup and JIT happen before the
/// timed loop, so only `process` is measured — what runs in the callback.
#[cfg(feature = "faust")]
fn bench_ugen_vs_faust() {
    use clausters::faust::compiler::{CompilePayload, compile as faust_compile};
    use clausters::faust::synth::FaustSynth;

    let ugen_def = Arc::new(
        compile(
            serde_json::from_value(serde_json::json!({
                "name": "cmp_usine",
                "controls": [{"name": "freq", "default": 440.0}],
                "ugens": [
                    {"kind": "Sine", "inputs": [{"control": 0}]},
                    {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.2}]},
                    {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ]
            }))
            .unwrap(),
        )
        .expect("ugen sine compiles"),
    );

    // Same recurrence as `Sine`: a wrapped phasor fed into `sin`, then ·0.2.
    // No `import` (keeps the def minimal), one hslider `freq` at control 0.
    let faust_src = format!(
        "freq = hslider(\"freq\", 440, 20, 20000, 0.01);\n\
         phasor = (+(freq/{sr}) : (_ <: _ - floor(_))) ~ _;\n\
         process = sin(6.283185307179586 * phasor) * 0.2;",
        sr = SAMPLE_RATE
    );
    let faust_def = Arc::new(
        faust_compile("cmp_fsine", &CompilePayload::Source(faust_src))
            .expect("faust sine compiles"),
    );

    println!("\nUGen vs Faust — identical DSP (sin(2π·phasor(freq)) · 0.2 → 1 bus), JIT excluded:");
    println!(
        "  {:>6}  {:>13}  {:>13}  {:>14}",
        "synths", "UGen xRT", "Faust xRT", "Faust slowdown"
    );
    for &n in VOICE_COUNTS {
        let ud = Arc::clone(&ugen_def);
        let ugen = bench(n, move |_| Box::new(UGenSynth::new(Arc::clone(&ud))));
        let fd = Arc::clone(&faust_def);
        let faust = bench(n, move |_| {
            Box::new(
                FaustSynth::new(
                    Arc::clone(&fd),
                    SAMPLE_RATE as f32,
                    &clausters::dsp::buffer::empty_pool(),
                )
                .expect("faust instance"),
            )
        });
        let u_xrt = ugen * BLOCK_SIZE as f64 / SAMPLE_RATE;
        let f_xrt = faust * BLOCK_SIZE as f64 / SAMPLE_RATE;
        println!(
            "  {n:>6}  {u_xrt:>11.1}x  {f_xrt:>11.1}x  {:>12.2}x",
            ugen / faust
        );
    }
    println!(
        "  (slowdown < 1.0 = Faust is faster. Caveat: Sine accumulates phase\n\
         \x20  and calls sin in f64; Faust -single does both in f32, which is cheaper,\n\
         \x20  so part of the gap is arithmetic precision, not engine overhead.)"
    );
}

/// Isolates **pure engine overhead**: a `· 0.5` gain stage on a shared input
/// bus, computed both ways. The two are bit-exact
/// (`tests/faust_parity.rs::gain_stages_are_bit_exact`) — one f32 multiply on
/// the same samples, no transcendental and no f64/f32 asymmetry — so the only
/// difference timed is how each engine moves a block through one synth: three
/// boxed `dyn` UGens with two intermediate wire buffers (`In · 0.5 → Out`)
/// against one Faust `compute` call (an in-copy, the multiply, an out-sum).
#[cfg(feature = "faust")]
fn bench_gain_overhead() {
    use clausters::faust::compiler::{CompilePayload, compile as faust_compile};
    use clausters::faust::synth::FaustSynth;

    let src_def = Arc::new(
        compile(
            serde_json::from_value(serde_json::json!({
                "name": "cmp_src",
                "ugens": [
                    {"kind": "Sine", "inputs": [{"const": 220.0}]},
                    {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.2}]},
                    {"kind": "Out",    "inputs": [{"const": 4.0}, {"ugen": 1}]}
                ]
            }))
            .unwrap(),
        )
        .expect("source compiles"),
    );
    let ugen_gain = Arc::new(
        compile(
            serde_json::from_value(serde_json::json!({
                "name": "cmp_ugain",
                "ugens": [
                    {"kind": "In",  "inputs": [{"const": 4.0}]},
                    {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ]
            }))
            .unwrap(),
        )
        .expect("ugen gain compiles"),
    );
    let faust_gain = Arc::new(
        faust_compile(
            "cmp_fgain",
            &CompilePayload::Source("process = _ * 0.5;".into()),
        )
        .expect("faust gain compiles"),
    );
    let in_idx = faust_gain.control_index("in").expect("in control");
    let out_idx = faust_gain.control_index("out").expect("out control");

    println!("\nUGen vs Faust — pure engine overhead (bit-exact · 0.5 gain, bus 4 → bus 0):");
    println!(
        "  {:>6}  {:>13}  {:>13}  {:>14}",
        "synths", "UGen xRT", "Faust xRT", "Faust slowdown"
    );
    for &n in VOICE_COUNTS {
        let ug = Arc::clone(&ugen_gain);
        let ugen = bench_chain(n, &src_def, move || {
            Box::new(UGenSynth::new(Arc::clone(&ug)))
        });
        let fg = Arc::clone(&faust_gain);
        let faust = bench_chain(n, &src_def, move || {
            let mut s = Box::new(
                FaustSynth::new(
                    Arc::clone(&fg),
                    SAMPLE_RATE as f32,
                    &clausters::dsp::buffer::empty_pool(),
                )
                .expect("instance"),
            );
            s.set_control(in_idx, 4.0);
            s.set_control(out_idx, 0.0);
            s
        });
        let u_xrt = ugen * BLOCK_SIZE as f64 / SAMPLE_RATE;
        let f_xrt = faust * BLOCK_SIZE as f64 / SAMPLE_RATE;
        println!(
            "  {n:>6}  {u_xrt:>11.1}x  {f_xrt:>11.1}x  {:>12.2}x",
            ugen / faust
        );
    }
    println!(
        "  (one shared source synth sits in both columns, so the high-n rows are\n\
         \x20  the cleanest read of the per-synth gain overhead.)"
    );
}

/// One source synth on bus 4, then `n` gain synths reading it into bus 0;
/// times only `process_block`. Sequential (0 workers), so add order is run
/// order: the source writes bus 4 before the gains read it.
#[cfg(feature = "faust")]
fn bench_chain(
    n: usize,
    src_def: &Arc<clausters::synthdef::SynthDef>,
    mut make_gain: impl FnMut() -> Box<dyn SynthNode>,
) -> f64 {
    let (mut engine, mut handle) = engine_pair(SAMPLE_RATE as f32, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    send_cmd(
        &mut engine,
        &mut handle,
        &mut out,
        Cmd::AddSynth {
            id: 1,
            target: 0,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(Arc::clone(src_def))),
            usage: Default::default(),
        },
    );
    for i in 0..n {
        send_cmd(
            &mut engine,
            &mut handle,
            &mut out,
            Cmd::AddSynth {
                id: 1000 + i as i32,
                target: 0,
                action: AddAction::Tail,
                synth: make_gain(),
                usage: Default::default(),
            },
        );
    }
    engine.process_block(&mut out);
    handle.collect_garbage();
    measure(&mut engine, &mut out)
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
                {"kind": "Sine", "inputs": [{"control": 0}]},
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
        let cmd = Cmd::AddSynth {
            id: 1000 + i as i32,
            target: 0,
            action: AddAction::Tail,
            synth,
            usage: Default::default(),
        };
        send_cmd(&mut engine, &mut handle, &mut out, cmd);
    }
    engine.process_block(&mut out); // plug in the last batch
    handle.collect_garbage();
    measure(&mut engine, &mut out)
}

/// Warmup, then time block throughput (blocks/s) — only `process_block`, the
/// work that runs in the audio callback. Shared by every benchmark.
fn measure(engine: &mut Engine, out: &mut [f32]) -> f64 {
    for _ in 0..100 {
        engine.process_block(out); // warmup
    }
    let start = Instant::now();
    let mut blocks = 0u64;
    loop {
        for _ in 0..256 {
            engine.process_block(out);
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

/// Sends one command, draining the FIFO (1024 entries) by processing a block
/// whenever it fills up.
fn send_cmd(engine: &mut Engine, handle: &mut EngineHandle, out: &mut [f32], mut cmd: Cmd) {
    while let Err(back) = handle.send(cmd) {
        engine.process_block(out);
        handle.collect_garbage();
        cmd = back;
    }
}
