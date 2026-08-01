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
//!
//! The **spectral** section measures the frequency-domain family, where the
//! interesting number is not throughput but the *peak block*: an FFT chain
//! concentrates all its work on the block where the hop closes, a sawtooth
//! load that averages (and EMA load meters) underreport.

use std::sync::Arc;
use std::time::Instant;

use clausters::clausters_core::rng::SEED_STRIDE;
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

    bench_sine_vs_wavetable();

    bench_pan();

    bench_fused();

    bench_spectral();

    #[cfg(feature = "faust")]
    {
        bench_ugen_vs_faust();
        bench_gain_overhead();
    }
    // M13: parallel groups. Independent chains on disjoint buses become one
    // stage that fans out across the worker pool; the speedup column is the
    // whole point of /group_parallel.
    let chains = 8usize;
    let voices = 125usize;
    println!(
        "\nparallel group (/group_parallel): {chains} subgroups x {voices} sines, disjoint buses:"
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

/// `Sine` (f64 phase accumulation + `sin()` per sample) against the table
/// readers `Osc` (linear interpolation) and `OscN` (no interpolation) on a
/// sine wavetable of scsynth's size (8192 samples = 4096 points) — the
/// measurement behind keeping `Sine` transcendental: if the table were much
/// faster at high voice counts, a table-based sine would earn a place.
/// Same graph shape for all three (osc · 0.001 → Out 0, freq at control 0).
fn bench_sine_vs_wavetable() {
    use clausters::dsp::buffer::Buffer;
    use clausters::dsp::wavetable::{GenCommand, GenFlags};

    let table = Arc::new(
        GenCommand::Sine1 {
            flags: GenFlags {
                normalize: true,
                wavetable: true,
                clear: true,
            },
            amps: vec![1.0],
        }
        .apply(&Buffer::zeroed(8192, 1, SAMPLE_RATE)),
    );

    let def = |name: &str, kind: &str| -> Arc<clausters::synthdef::SynthDef> {
        let inputs = if kind == "Sine" {
            serde_json::json!([{"control": 0}])
        } else {
            serde_json::json!([{"const": 0.0}, {"control": 0}, {"const": 0.0}])
        };
        Arc::new(
            compile(
                serde_json::from_value(serde_json::json!({
                    "name": name,
                    "controls": [{"name": "freq", "default": 440.0}],
                    "ugens": [
                        {"kind": kind, "inputs": inputs},
                        {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.001}]},
                        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                    ]
                }))
                .unwrap(),
            )
            .expect("def compiles"),
        )
    };
    let defs = [
        ("Sine", def("cmp_sine", "Sine")),
        ("Osc", def("cmp_osc", "Osc")),
        ("OscN", def("cmp_oscn", "OscN")),
    ];

    println!("\nSine (f64 phase + sin) vs wavetable Osc/OscN (8192-sample table), xRT:");
    println!(
        "  {:>6}  {:>11}  {:>11}  {:>11}",
        "synths", "Sine", "Osc", "OscN"
    );
    for &n in VOICE_COUNTS {
        let mut cols = Vec::new();
        for (_, d) in &defs {
            let d = Arc::clone(d);
            let t = Arc::clone(&table);
            let blocks = bench_with(
                n,
                move |_| {
                    Box::new(UGenSynth::new(
                        Arc::clone(&d),
                        SAMPLE_RATE as f32,
                        SEED_STRIDE,
                    ))
                },
                {
                    let t = Arc::clone(&t);
                    move |engine, handle, out| {
                        send_cmd(
                            engine,
                            handle,
                            out,
                            Cmd::SetBuffer {
                                index: 0,
                                buffer: Some(Arc::clone(&t)),
                            },
                        );
                    }
                },
            );
            cols.push(blocks * BLOCK_SIZE as f64 / SAMPLE_RATE);
        }
        println!(
            "  {n:>6}  {:>10.1}x  {:>10.1}x  {:>10.1}x",
            cols[0], cols[1], cols[2]
        );
    }
}

/// What the pan family's one deliberate deviation costs (U7). Every equal-power
/// row computes its gain pair from a polynomial rather than a table, **once per
/// block** when the position is a scalar and **per sample** when it is audio
/// rate — because interpolating the two gains across the block, the way a
/// filter coefficient is interpolated here, would leave a 3 dB hole in the
/// middle of every block a fast sweep crosses.
///
/// The two rows are the same graph (`Sine → Pan2 → 2× Out`) with the position
/// wired to a constant and to an `LFTri`, so the difference between them is
/// exactly the per-sample path: 64 polynomial evaluations a block instead of
/// one. The claim being measured is that the second is affordable at all —
/// which is most of the reason the law is ten flops and not a `sin()` call.
fn bench_pan() {
    let def = |name: &str, moving: bool| -> Arc<clausters::synthdef::SynthDef> {
        let pos = if moving {
            serde_json::json!({"ugen": 1})
        } else {
            serde_json::json!({"const": 0.3})
        };
        Arc::new(
            compile(
                serde_json::from_value(serde_json::json!({
                    "name": name,
                    "controls": [{"name": "freq", "default": 440.0}],
                    "ugens": [
                        {"kind": "Sine", "inputs": [{"control": 0}]},
                        {"kind": "LFTri", "inputs": [{"const": 3.0}, {"const": 0.0}]},
                        {"kind": "Pan2", "inputs": [
                            {"ugen": 0}, pos, {"const": 0.05}, {"const": 0.0}]},
                        {"kind": "Pan2", "inputs": [
                            {"ugen": 0}, pos, {"const": 0.05}, {"const": 1.0}]},
                        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]},
                        {"kind": "Out", "inputs": [{"const": 1.0}, {"ugen": 3}]}
                    ]
                }))
                .unwrap(),
            )
            .expect("pan def compiles"),
        )
    };
    let fixed = def("cmp_pan_fixed", false);
    let moving = def("cmp_pan_moving", true);

    println!("\npan position, block-rate vs per-sample (Sine -> Pan2 -> 2x Out):");
    println!(
        "  {:>6}  {:>13}  {:>13}  {:>14}",
        "synths", "scalar xRT", "ar pos xRT", "per-sample cost"
    );
    for &n in VOICE_COUNTS {
        let f = Arc::clone(&fixed);
        let a = bench(n, move |_| {
            Box::new(UGenSynth::new(
                Arc::clone(&f),
                SAMPLE_RATE as f32,
                SEED_STRIDE,
            ))
        });
        let m = Arc::clone(&moving);
        let b = bench(n, move |_| {
            Box::new(UGenSynth::new(
                Arc::clone(&m),
                SAMPLE_RATE as f32,
                SEED_STRIDE,
            ))
        });
        let a_xrt = a * BLOCK_SIZE as f64 / SAMPLE_RATE;
        let b_xrt = b * BLOCK_SIZE as f64 / SAMPLE_RATE;
        println!("  {n:>6}  {a_xrt:>11.1}x  {b_xrt:>11.1}x  {:>12.2}x", a / b);
    }
    println!(
        "  (the moving row also pays for its own LFTri, so the ratio is an\n\
         \x20  upper bound on what evaluating the law per sample costs.)"
    );
}

/// The fused arithmetic rows (`MulAdd`, `Sum3`, `Sum4`) against the unfused
/// graphs they replace — the measurement behind offering them at all, since a
/// client can always write the operators out longhand.
///
/// Fusing saves two things at once and the columns cannot separate them: the
/// extra `dyn` dispatch and intermediate wire buffer per operator dropped, and
/// one pass over the block instead of two or three. Both rows share their
/// source (the same `Sine` wire read two, three or four times), so the ratio
/// isolates the arithmetic rather than the sources feeding it.
///
/// The two shapes are deliberately the extremes of the broadcast rule: the
/// `MulAdd` row folds `sig * k + k`, where two of three inputs are constants,
/// and the `Sum4` row sums four signals with no constant at all.
///
/// Read the ratio in the **middle** of the sweep. By 1000 voices the graph is
/// near real time and the ratio swings from 0.90x to 1.09x between rounds — the
/// measurement is competing with the scheduler, not reporting the fold.
fn bench_fused() {
    let def = |name: &str, ugens: serde_json::Value| -> Arc<clausters::synthdef::SynthDef> {
        Arc::new(
            compile(
                serde_json::from_value(serde_json::json!({
                    "name": name,
                    "controls": [{"name": "freq", "default": 440.0}],
                    "ugens": ugens,
                }))
                .unwrap(),
            )
            .expect("fused def compiles"),
        )
    };
    let sine = serde_json::json!({"kind": "Sine", "inputs": [{"control": 0}]});

    // a*b + c in one UGen, against the Mul -> Add pair with a wire between.
    let mul_add = def(
        "cmp_muladd_fused",
        serde_json::json!([
            sine,
            {"kind": "MulAdd", "inputs": [{"ugen": 0}, {"const": 0.5}, {"const": 0.1}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]),
    );
    let mul_then_add = def(
        "cmp_muladd_unfused",
        serde_json::json!([
            sine,
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
            {"kind": "Add", "inputs": [{"ugen": 1}, {"const": 0.1}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
        ]),
    );
    // Four signals summed in one UGen, against the three Adds it folds.
    let sum4 = def(
        "cmp_sum4_fused",
        serde_json::json!([
            sine,
            {"kind": "Sum4", "inputs": [
                {"ugen": 0}, {"ugen": 0}, {"ugen": 0}, {"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]),
    );
    let three_adds = def(
        "cmp_sum4_unfused",
        serde_json::json!([
            sine,
            {"kind": "Add", "inputs": [{"ugen": 0}, {"ugen": 0}]},
            {"kind": "Add", "inputs": [{"ugen": 1}, {"ugen": 0}]},
            {"kind": "Add", "inputs": [{"ugen": 2}, {"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
        ]),
    );

    println!("\nfused arithmetic vs the unfused graph it folds (shared Sine source):");
    println!(
        "  {:>6}  {:>12}  {:>12}  {:>10}  {:>12}  {:>10}  {:>6}",
        "synths", "MulAdd xRT", "Mul+Add xRT", "fused", "Sum4 xRT", "3x Add xRT", "fused"
    );
    for &n in VOICE_COUNTS {
        let run = |d: &Arc<clausters::synthdef::SynthDef>| {
            let d = Arc::clone(d);
            bench(n, move |_| {
                Box::new(UGenSynth::new(
                    Arc::clone(&d),
                    SAMPLE_RATE as f32,
                    SEED_STRIDE,
                ))
            }) * BLOCK_SIZE as f64
                / SAMPLE_RATE
        };
        let (f_ma, u_ma) = (run(&mul_add), run(&mul_then_add));
        let (f_s4, u_s4) = (run(&sum4), run(&three_adds));
        println!(
            "  {n:>6}  {f_ma:>11.1}x  {u_ma:>11.1}x  {:>9.2}x  {f_s4:>11.1}x  {u_s4:>9.1}x  {:>5.2}x",
            f_ma / u_ma,
            f_s4 / u_s4
        );
    }
}

/// The spectral (`fr`) family, in three views:
///
/// 1. **Raw transforms** — one `rfft`/`irfft` call per supported size against
///    the 64-frame block budget. This is the whole per-hop cost of an
///    `FFT`/`IFFT` bookend pair (the `PV_*` in between are linear scans).
/// 2. **Partitioned-convolution MAC** — the frequency-domain delay-line inner
///    loop a future partitioned convolver runs per hop (`P` complex bin-wise
///    multiply–accumulates). Uniformly partitioned, all of it lands on the hop
///    block unless the implementation spreads the partitions across the hop's
///    blocks — this row is the spike that spreading would flatten.
/// 3. **A full chain through the engine** — `Sine → FFT → PV_MagAbove → IFFT
///    → Out` per voice. The xRT column is the average story; the peak-block
///    column is the real-time one: every voice is added on the same block, so
///    all hops land on the same block — the aligned worst case (hop-phase
///    staggering at instantiation is the lever that would spread it).
fn bench_spectral() {
    use clausters_core::fft;

    let budget_us = BLOCK_SIZE as f64 / SAMPLE_RATE * 1e6;
    println!(
        "\nspectral transforms (per call; one {BLOCK_SIZE}-frame block @ {SAMPLE_RATE} Hz = {budget_us:.0} us):"
    );
    println!(
        "  {:>6}  {:>10}  {:>10}  {:>15}",
        "n", "rfft", "irfft", "pair, % block"
    );
    for &n in fft::SUPPORTED_SIZES {
        let input: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut frame = vec![0.0f32; n];
        let mut time = vec![0.0f32; n];
        let fwd = time_per_call(|| fft::rfft_into(std::hint::black_box(&input), &mut frame));
        let inv = time_per_call(|| fft::irfft_into(std::hint::black_box(&frame), &mut time));
        println!(
            "  {n:>6}  {fwd:>8.2} us  {inv:>8.2} us  {:>14.1}%",
            (fwd + inv) / budget_us * 100.0
        );
    }

    println!("  partitioned-convolution spectral MAC per hop (uniform FDL):");
    for &(n, parts, label) in &[(4096usize, 47usize, "2 s IR"), (4096, 12, "0.5 s IR")] {
        let us = time_conv_mac(n, parts);
        println!(
            "    n={n} x {parts} partitions ({label} @ 48k): {us:>6.1} us = {:.1}% of block",
            us / budget_us * 100.0
        );
    }

    let def = Arc::new(
        compile(
            serde_json::from_value(serde_json::json!({
                "name": "cmp_spectral",
                "controls": [{"name": "freq", "default": 440.0}],
                "ugens": [
                    {"kind": "Sine", "inputs": [{"control": 0}]},
                    {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
                    {"kind": "PV_MagAbove", "inputs": [{"ugen": 1}, {"const": 0.0}]},
                    {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
                ]
            }))
            .unwrap(),
        )
        .expect("spectral def compiles"),
    );

    // Node ids drive the S11 hop-phase stagger, so the id spacing selects the
    // scenario: consecutive ids spread their hops (the default behavior),
    // while ids congruent modulo blocks-per-hop (512-sample hop / 64 = 8) all
    // hop on the same block — the pre-S11 aligned worst case, kept measurable
    // on purpose.
    let run = |n: usize, id_step: i32| {
        let (mut engine, mut handle) = engine_pair(SAMPLE_RATE as f32, 2);
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];
        for i in 0..n {
            let mut synth: Box<dyn SynthNode> = Box::new(UGenSynth::new(
                Arc::clone(&def),
                SAMPLE_RATE as f32,
                SEED_STRIDE,
            ));
            synth.set_control(0, 50.0 + i as f32);
            let cmd = Cmd::AddSynth {
                id: 1000 + i as i32 * id_step,
                target: 0,
                action: AddAction::Tail,
                synth,
                usage: Default::default(),
            };
            send_cmd(&mut engine, &mut handle, &mut out, cmd);
        }
        engine.process_block(&mut out);
        handle.collect_garbage();
        measure_peak(&mut engine, &mut out)
    };

    println!(
        "\nspectral chain (Sine → FFT 1024 → PV_MagAbove → IFFT → Out), aligned vs S11-staggered hops:"
    );
    println!(
        "  {:>6}  {:>11}  {:>12}  {:>14}  {:>14}",
        "synths", "xRT", "avg block", "peak aligned", "peak staggered"
    );
    for &n in VOICE_COUNTS {
        let (_, _, peak_aligned) = run(n, 8); // ids ≡ 0 (mod 8): one hop block
        let (blocks_per_sec, avg_us, peak_stag) = run(n, 1); // consecutive ids
        let xrt = blocks_per_sec * BLOCK_SIZE as f64 / SAMPLE_RATE;
        println!(
            "  {n:>6}  {xrt:>10.1}x  {avg_us:>9.1} us  {peak_aligned:>11.1} us  {peak_stag:>11.1} us"
        );
    }
    println!(
        "  (peak = the worst single block; aligned, every chain transforms on the same\n\
         \x20  hop block, staggered (S11, id-derived) the spikes spread. The budget is\n\
         \x20  {budget_us:.0} us per block — and the hard deadline is the audio callback,\n\
         \x20  which further amortizes when it covers more than one block.)"
    );

    bench_conv(budget_us);
}

/// The M28 partitioned convolver: one voice convolving white noise with a
/// 2 s impulse response (94 partitions of 1024 at fft 2048). The point is the
/// peak-vs-average gap: the FDL MACs are spread across the hop's blocks, so
/// the hop block only adds the input FFT/IFFT pair — without the spreading,
/// all ~94 MACs (hundreds of us, see the MAC row above) would land on it.
fn bench_conv(budget_us: f64) {
    use clausters::dsp::buffer::Buffer;
    use clausters::dsp::conv::layout;
    use clausters::dsp::wavetable::GenCommand;

    let fft_size = 2048usize;
    let part = fft_size / 2;
    let ir_frames = 2 * SAMPLE_RATE as usize; // 2 s
    let parts = ir_frames.div_ceil(part);
    let ir: Vec<f32> = (0..ir_frames)
        .map(|k| ((k as f32 * 0.37).sin()) * (-(k as f32) / 24000.0).exp() * 0.05)
        .collect();
    let prepared = GenCommand::PreparePartConv {
        src: Arc::new(Buffer::new(ir, 1, ir_frames, SAMPLE_RATE)),
        fft_size,
    }
    .apply(&Buffer::zeroed(
        layout::frames(fft_size, parts),
        1,
        SAMPLE_RATE,
    ));

    let def = Arc::new(
        compile(
            serde_json::from_value(serde_json::json!({
                "name": "cmp_conv",
                "controls": [{"name": "freq", "default": 440.0}],
                "ugens": [
                    {"kind": "WhiteNoise", "inputs": []},
                    {"kind": "Conv", "inputs": [{"ugen": 0}, {"const": 0.0}],
                     "fft_size": fft_size, "partitions": parts},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ]
            }))
            .unwrap(),
        )
        .expect("conv def compiles"),
    );

    let (mut engine, mut handle) = engine_pair(SAMPLE_RATE as f32, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    send_cmd(
        &mut engine,
        &mut handle,
        &mut out,
        Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(prepared)),
        },
    );
    send_cmd(
        &mut engine,
        &mut handle,
        &mut out,
        Cmd::AddSynth {
            id: 1000,
            target: 0,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(def, SAMPLE_RATE as f32, SEED_STRIDE)),
            usage: Default::default(),
        },
    );
    engine.process_block(&mut out);
    handle.collect_garbage();

    // Per-phase profile of the hop period, min-filtered: the minimum over
    // many periods strips OS scheduling noise from each block phase, leaving
    // the deterministic per-block cost — flat spread share everywhere, plus
    // the FFT/IFFT pair on the hop phase. A raw single max would mostly
    // measure preemption blips.
    let phases = part / BLOCK_SIZE;
    for _ in 0..(100 * phases) {
        engine.process_block(&mut out); // warmup, and settle the hop phase
    }
    let mut phase_min = vec![f64::INFINITY; phases];
    let mut sum = 0.0f64;
    let periods = 400usize;
    for _ in 0..periods {
        for slot in phase_min.iter_mut() {
            let t = Instant::now();
            engine.process_block(&mut out);
            let dt = t.elapsed().as_secs_f64();
            sum += dt;
            if dt < *slot {
                *slot = dt;
            }
        }
    }
    let avg_us = sum / (periods * phases) as f64 * 1e6;
    let peak_us = phase_min.iter().fold(0.0f64, |a, &b| a.max(b)) * 1e6;
    let flat_us = phase_min
        .iter()
        .fold(f64::INFINITY, |a, &b| a.min(b))
        .max(0.0)
        * 1e6;
    println!(
        "\npartitioned convolution (Conv, 2 s IR, {parts} partitions of {part}, MACs spread):"
    );
    println!(
        "  1 voice: avg block {avg_us:>6.1} us | steady phase {flat_us:>6.1} us | hop phase \
         {peak_us:>6.1} us (budget {budget_us:.0} us)"
    );
    println!(
        "  (per-phase minima over {periods} hop periods, so OS noise is filtered out:\n\
         \x20  the spread MAC share is the steady phase, and the hop phase adds only the\n\
         \x20  input FFT/IFFT pair — compare the un-spread MAC row above.)"
    );
}

/// Times one call of `f` in a warmed-up loop, in microseconds.
fn time_per_call(mut f: impl FnMut() -> bool) -> f64 {
    for _ in 0..1000 {
        assert!(f());
    }
    let iters = 20000usize;
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    start.elapsed().as_secs_f64() / iters as f64 * 1e6
}

/// One hop of a uniformly partitioned convolver's inner loop: accumulate
/// `parts` complex bin-wise products of packed size-`n` frames (the
/// [`fft::rfft_into`] layout: `[dc, nyquist, re, im, …]`) into one frame.
fn time_conv_mac(n: usize, parts: usize) -> f64 {
    let frames: Vec<Vec<f32>> = (0..parts)
        .map(|p| (0..n).map(|i| ((i + p) as f32 * 0.001).sin()).collect())
        .collect();
    let kernel = frames.clone();
    let mut acc = vec![0.0f32; n];
    let half = n / 2;
    let iters = 2000usize;
    let start = Instant::now();
    for _ in 0..iters {
        acc.iter_mut().for_each(|v| *v = 0.0);
        for p in 0..parts {
            let (a, b) = (
                std::hint::black_box(&frames[p]),
                std::hint::black_box(&kernel[p]),
            );
            acc[0] += a[0] * b[0]; // DC
            acc[1] += a[1] * b[1]; // Nyquist
            for k in 1..half {
                let (ar, ai) = (a[2 * k], a[2 * k + 1]);
                let (br, bi) = (b[2 * k], b[2 * k + 1]);
                acc[2 * k] += ar * br - ai * bi;
                acc[2 * k + 1] += ar * bi + ai * br;
            }
        }
        std::hint::black_box(&acc);
    }
    start.elapsed().as_secs_f64() / iters as f64 * 1e6
}

/// Like [`measure`], but also times every block individually: returns
/// (blocks/s, average block time in us, worst single block in us). The peak
/// is the number that matters for a sawtooth (hop-concentrated) load.
fn measure_peak(engine: &mut Engine, out: &mut [f32]) -> (f64, f64, f64) {
    for _ in 0..100 {
        engine.process_block(out); // warmup
    }
    let start = Instant::now();
    let mut blocks = 0u64;
    let mut peak = 0.0f64;
    loop {
        for _ in 0..64 {
            let t = Instant::now();
            engine.process_block(out);
            let dt = t.elapsed().as_secs_f64();
            if dt > peak {
                peak = dt;
            }
        }
        blocks += 64;
        if start.elapsed().as_secs_f64() >= MEASURE_SECS {
            break;
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    assert!(out.iter().all(|s| s.is_finite()));
    (
        blocks as f64 / elapsed,
        elapsed / blocks as f64 * 1e6,
        peak * 1e6,
    )
}

fn make_default_synth() -> Box<dyn SynthNode> {
    static DEF: std::sync::OnceLock<Arc<clausters::synthdef::SynthDef>> =
        std::sync::OnceLock::new();
    let def = DEF.get_or_init(|| Arc::new(compile(default_spec()).expect("default def compiles")));
    Box::new(UGenSynth::new(
        Arc::clone(def),
        SAMPLE_RATE as f32,
        SEED_STRIDE,
    ))
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
        let ugen = bench(n, move |_| {
            Box::new(UGenSynth::new(
                Arc::clone(&ud),
                SAMPLE_RATE as f32,
                SEED_STRIDE,
            ))
        });
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
            Box::new(UGenSynth::new(
                Arc::clone(&ug),
                SAMPLE_RATE as f32,
                SEED_STRIDE,
            ))
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
            synth: Box::new(UGenSynth::new(
                Arc::clone(src_def),
                SAMPLE_RATE as f32,
                SEED_STRIDE,
            )),
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
/// summing into that chain's private bus — the layout where /group_parallel
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
            let mut synth = Box::new(UGenSynth::new(
                Arc::clone(&def),
                SAMPLE_RATE as f32,
                SEED_STRIDE,
            ));
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
fn bench(n: usize, make: impl FnMut(usize) -> Box<dyn SynthNode>) -> f64 {
    bench_with(n, make, |_, _, _| {})
}

/// `bench` with a setup hook run before the synths are added (e.g. to plug a
/// wavetable buffer into the engine).
fn bench_with(
    n: usize,
    mut make: impl FnMut(usize) -> Box<dyn SynthNode>,
    setup: impl FnOnce(&mut Engine, &mut EngineHandle, &mut [f32]),
) -> f64 {
    let (mut engine, mut handle) = engine_pair(SAMPLE_RATE as f32, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    setup(&mut engine, &mut handle, &mut out);
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
