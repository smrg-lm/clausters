//! The frequency-domain (`fr`) chain driven through the real engine
//! (`process_block`): a round trip is its input exactly, one window late, for
//! any window and hop, however the blocks are sliced, through a gate and
//! through a live window change; and the compiler holds a chain to its `FFT`.
//! What each `PV_*` does to a frame is held to a reference in
//! `tests/pv_reference.rs`.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::{UGenCmd, ugen_cmd_selector};
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

fn spec_synth(spec: Value) -> UGenSynth {
    let spec: SynthDefSpec = serde_json::from_value(spec).unwrap();
    UGenSynth::new(
        Arc::new(clausters::synthdef::compile(spec).unwrap()),
        SR,
        SEED_STRIDE,
    )
}

fn add_synth(id: i32, synth: UGenSynth) -> Cmd {
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth: Box::new(synth),
        usage: Default::default(),
    }
}

fn render_channel(engine: &mut Engine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        buf.extend(out.iter().step_by(CHANNELS).copied());
    }
    buf
}

/// RMS of `s[from..to]`, the steady-state measure past the chain's latency.
fn rms(s: &[f32], from: usize, to: usize) -> f32 {
    let seg = &s[from..to.min(s.len())];
    (seg.iter().map(|&x| x * x).sum::<f32>() / seg.len() as f32).sqrt()
}

/// The compiler rejects an unsupported FFT size and a chain UGen whose input 0
/// is not a spectral chain.
#[test]
fn compiler_validates_the_chain() {
    let bad_size: SynthDefSpec = serde_json::from_value(json!({
        "name": "badsize",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 777},
            {"kind": "IFFT", "inputs": [{"ugen": 1}]}
        ]
    }))
    .unwrap();
    assert!(clausters::synthdef::compile(bad_size).is_err());

    let bad_chain: SynthDefSpec = serde_json::from_value(json!({
        "name": "badchain",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            // IFFT fed a plain audio wire, not a spectral chain.
            {"kind": "IFFT", "inputs": [{"ugen": 0}]}
        ]
    }))
    .unwrap();
    assert!(clausters::synthdef::compile(bad_chain).is_err());
}

/// `/node_ugenCmd <ugen> window <wintype>` swaps an `FFT`'s analysis window live (the
/// first consumer of the typed per-UGen command surface). Here we drive the
/// UGen's `command` directly to confirm the selector wiring.
#[test]
fn u_cmd_swaps_the_fft_window() {
    use clausters::dsp::UGen;
    use clausters::dsp::registry::UGenConfig;
    use clausters::dsp::spectral::Fft;

    let mut fft = Fft::new(&UGenConfig {
        fft_size: Some(512),
        ..Default::default()
    });
    // Switching to a rectangular window must not panic and takes the selector.
    let cmd = UGenCmd {
        selector: ugen_cmd_selector("window"),
        args: {
            let mut a = [0.0f32; 8];
            a[0] = -1.0; // Window::Rectangular
            a
        },
        num_args: 1,
    };
    fft.command(&cmd);
    // An unrelated selector is ignored (no panic).
    fft.command(&UGenCmd {
        selector: ugen_cmd_selector("bogus"),
        args: [0.0; 8],
        num_args: 1,
    });
}

/// The hop-phase stagger: the node id shifts *when* a chain's first frame
/// fires (a deterministic sub-hop, block-quantized offset), without touching
/// the reconstruction itself. Two identical passthrough chains under different
/// node ids start `stagger` samples apart but agree sample-for-sample in the
/// steady state -- the analysis grid shifts, the content timing does not.
#[test]
fn hop_stagger_shifts_only_the_first_frame() {
    // FFT(512, 50% hop) at BLOCK_SIZE 64: 4 blocks per hop. Node id 4 == 0
    // (mod 4) keeps offset 0; node id 6 staggers by 2 blocks = 128 samples.
    let build = || {
        spec_synth(json!({
            "name": "stagger",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "IFFT", "inputs": [{"ugen": 1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
            ]
        }))
    };
    let render = |id: i32| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        handle.send(add_synth(id, build())).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    let aligned = render(4);
    let staggered = render(6);

    // Onset: before its first frame an `IFFT` emits exact zeros (the FIFO is
    // empty), so the first nonzero sample marks the first fire -- it moves by
    // exactly the 128-sample stagger.
    let onset = |s: &[f32]| s.iter().position(|&x| x != 0.0).unwrap();
    let shift = onset(&staggered) as i64 - onset(&aligned) as i64;
    assert_eq!(shift, 128, "onset shift");

    // Steady state: both chains carry the same latency (the stagger delays
    // the first fire, not the reconstruction), so past the startup the two
    // outputs are the same sine, sample-aligned.
    for i in 4000..12000 {
        assert!(
            (aligned[i] - staggered[i]).abs() < 1e-3,
            "steady-state mismatch at {i}: {} vs {}",
            aligned[i],
            staggered[i]
        );
    }

    // Determinism: the same node id renders bit-identically.
    let again = render(6);
    assert_eq!(staggered, again);
}

// ---- the curated PV set ----

/// The compiler validates a combiner's two chains: both inputs must be chains,
/// of equal window size, and distinct.
#[test]
fn compiler_validates_the_combiner() {
    let compile = |ugens: Value| {
        clausters::synthdef::compile(
            serde_json::from_value(json!({"name": "bad", "ugens": ugens})).unwrap(),
        )
    };
    // Input 1 is a plain audio wire, not a chain.
    assert!(
        compile(json!([
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
            {"kind": "PV_Add", "inputs": [{"ugen": 1}, {"ugen": 0}]},
            {"kind": "IFFT", "inputs": [{"ugen": 2}]}
        ]))
        .is_err()
    );
    // Window sizes differ.
    assert!(
        compile(json!([
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
            {"kind": "PV_Add", "inputs": [{"ugen": 1}, {"ugen": 2}]},
            {"kind": "IFFT", "inputs": [{"ugen": 3}]}
        ]))
        .is_err()
    );
    // The same chain on both sides.
    assert!(
        compile(json!([
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
            {"kind": "PV_Add", "inputs": [{"ugen": 1}, {"ugen": 1}]},
            {"kind": "IFFT", "inputs": [{"ugen": 2}]}
        ]))
        .is_err()
    );
}

/// `PV_Kernel`: a bin-expression program reproducing a curated op renders
/// **sample-identically** to the built-in row -- the mechanism's acceptance
/// test. Here `mag * (mag >= p0)` (a spectral gate) against `PV_MagAbove`.
#[test]
fn pv_kernel_reproduces_mag_above() {
    let thresh = 1.0f32;
    let render = |ugens: Value| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({"name": "k", "ugens": ugens}));
        handle.send(add_synth(1, synth)).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    let builtin = render(json!([
        {"kind": "Sine", "inputs": [{"const": 440.0}]},
        {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
        {"kind": "PV_MagAbove", "inputs": [{"ugen": 1}, {"const": thresh}]},
        {"kind": "IFFT", "inputs": [{"ugen": 2}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
    ]));
    let kernel = render(json!([
        {"kind": "Sine", "inputs": [{"const": 440.0}]},
        {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
        {"kind": "PV_Kernel", "inputs": [{"ugen": 1}, {"const": thresh}],
         "mag_expr": ["mag", "mag", "p0", "ge", "mul"]},
        {"kind": "IFFT", "inputs": [{"ugen": 2}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
    ]));
    assert!(
        rms(&builtin, 6000, 18000) > 0.3,
        "the gated tone should survive"
    );
    assert_eq!(
        builtin, kernel,
        "kernel gate must match PV_MagAbove exactly"
    );
}

/// A kernel low pass over the bin index (`mag * (bin < cutoff)`) matches
/// `PV_BrickWall` sample-for-sample (cutoff precomputed to the builtin's
/// rounding), and actually removes a high tone.
#[test]
fn pv_kernel_reproduces_brick_wall() {
    let wipe = 0.85f32;
    let cutoff = (513.0f32 * (1.0 - wipe)).round(); // PV_BrickWall's cutoff
    let render = |ugens: Value| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({"name": "k", "ugens": ugens}));
        handle.send(add_synth(1, synth)).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    let builtin = render(json!([
        {"kind": "Sine", "inputs": [{"const": 9000.0}]},
        {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
        {"kind": "PV_BrickWall", "inputs": [{"ugen": 1}, {"const": wipe}]},
        {"kind": "IFFT", "inputs": [{"ugen": 2}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
    ]));
    let kernel = render(json!([
        {"kind": "Sine", "inputs": [{"const": 9000.0}]},
        {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
        {"kind": "PV_Kernel", "inputs": [{"ugen": 1}],
         "mag_expr": ["mag", "bin", cutoff, "lt", "mul"]},
        {"kind": "IFFT", "inputs": [{"ugen": 2}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
    ]));
    assert!(
        rms(&builtin, 6000, 18000) < 0.05,
        "the brick wall should remove the tone"
    );
    assert_eq!(
        builtin, kernel,
        "kernel low pass must match PV_BrickWall exactly"
    );
}

/// A `PV_Kernel` with no expressions is the identity: the render equals the
/// bare `FFT`->`IFFT` round trip exactly.
#[test]
fn pv_kernel_identity_is_transparent() {
    let render = |with_kernel: bool| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let ugens = if with_kernel {
            json!([
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "PV_Kernel", "inputs": [{"ugen": 1}]},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ])
        } else {
            json!([
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "IFFT", "inputs": [{"ugen": 1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
            ])
        };
        let synth = spec_synth(json!({"name": "k", "ugens": ugens}));
        handle.send(add_synth(1, synth)).ok().unwrap();
        render_channel(&mut engine, 200)
    };
    assert_eq!(render(false), render(true));
}

/// The compiler rejects malformed kernel programs with a `/fail`-able error:
/// unknown words, stack underflow, a program netting two values, and a
/// parameter index past the UGen's inputs.
#[test]
fn compiler_validates_kernel_programs() {
    let compile = |kernel: Value| {
        let spec: SynthDefSpec = serde_json::from_value(json!({"name": "bad", "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
            kernel,
            {"kind": "IFFT", "inputs": [{"ugen": 2}]}
        ]}))
        .unwrap();
        clausters::synthdef::compile(spec)
    };
    let cases = [
        json!({"kind": "PV_Kernel", "inputs": [{"ugen": 1}], "mag_expr": ["bogus"]}),
        json!({"kind": "PV_Kernel", "inputs": [{"ugen": 1}], "mag_expr": ["mul"]}),
        json!({"kind": "PV_Kernel", "inputs": [{"ugen": 1}], "mag_expr": ["mag", "phase"]}),
        json!({"kind": "PV_Kernel", "inputs": [{"ugen": 1}], "mag_expr": ["p0"]}),
        json!({"kind": "PV_Kernel", "inputs": [{"ugen": 1}], "phase_expr": []}),
        // No chain input at all (the variadic guard).
        json!({"kind": "PV_Kernel", "inputs": []}),
    ];
    for (i, kernel) in cases.into_iter().enumerate() {
        assert!(compile(kernel).is_err(), "case {i} should fail to compile");
    }
    // The valid forms pass: p0 with one parameter input, both exprs given.
    let ok = json!({"kind": "PV_Kernel",
        "inputs": [{"ugen": 1}, {"const": 0.5}],
        "mag_expr": ["mag", "mag", "p0", "ge", "mul"],
        "phase_expr": ["phase"]});
    assert!(compile(ok).is_ok());
}

/// The input a tone test feeds its chain: `amp * sin(2 pi f n / SR)`.
fn tone(freq: f32, amp: f32, n: usize) -> f32 {
    amp * (std::f32::consts::TAU * freq * n as f32 / SR).sin()
}

/// Asserts `out` is `input` delayed by some lag up to `max_lag`, sample for
/// sample over `from..to`, and returns the lag. An unmodified chain is an
/// identity up to its latency, so nothing looser than rounding may separate
/// the two: a level error, a modulation or a misplaced frame all show here.
fn assert_delayed_copy(
    out: &[f32],
    input: impl Fn(usize) -> f32,
    max_lag: usize,
    from: usize,
    to: usize,
    tol: f32,
) -> usize {
    let err = |lag: usize| {
        (from..to)
            .map(|n| (out[n] - input(n - lag)).abs())
            .fold(0.0f32, f32::max)
    };
    let (lag, worst) = (0..=max_lag.min(from))
        .map(|lag| (lag, err(lag)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(
        worst < tol,
        "not a delayed copy: best lag {lag}, max error {worst}"
    );
    lag
}

/// An `FFT` -> `IFFT` round trip with a non-default window and hop is the
/// input, delayed.
#[test]
fn a_round_trip_is_exact_with_any_window_and_hop() {
    for (wintype, hop) in [
        (0, 0.5),
        (4, 0.25),
        (1, 0.5),
        (2, 0.5),
        (3, 0.25),
        (-1, 0.5),
    ] {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({
            "name": "roundtrip",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
                {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}],
                 "fft_size": 1024, "hop": hop, "wintype": wintype},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }));
        handle.send(add_synth(1, synth)).ok().unwrap();
        let sig = render_channel(&mut engine, 200);
        assert_delayed_copy(&sig, |n| tone(440.0, 0.5, n), 2048, 4096, 12000, 1e-4);
    }
}

/// A two-chain combiner resynthesizes with its chains' window and hop, not the
/// defaults: `PV_Add` of a tone and silence is the tone, delayed, whatever
/// window and hop the chains were analysed with.
#[test]
fn a_combiner_keeps_its_chains_window_and_hop() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let synth = spec_synth(json!({
        "name": "combined",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
            {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}],
             "fft_size": 1024, "hop": 0.25, "wintype": 4},
            {"kind": "FFT", "inputs": [{"const": 0.0}, {"const": 1.0}],
             "fft_size": 1024, "hop": 0.25, "wintype": 4},
            {"kind": "PV_Add", "inputs": [{"ugen": 2}, {"ugen": 3}]},
            {"kind": "IFFT", "inputs": [{"ugen": 4}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 5}]}
        ]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();
    let sig = render_channel(&mut engine, 200);
    assert_delayed_copy(&sig, |n| tone(440.0, 0.5, n), 2048, 4096, 12000, 1e-4);
}

/// The compiler holds every UGen on a chain to its `FFT`'s hop and window: a
/// combiner over two chains that differ in either, and a filter or `IFFT` that
/// names another, are errors rather than a resynthesis at the wrong level.
#[test]
fn compiler_holds_a_chain_to_its_ffts_hop_and_window() {
    let compile = |ugens: Value| {
        clausters::synthdef::compile(
            serde_json::from_value(json!({"name": "bad", "ugens": ugens})).unwrap(),
        )
    };
    let pair = |b: Value| {
        compile(json!([
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
            b,
            {"kind": "PV_Add", "inputs": [{"ugen": 1}, {"ugen": 2}]},
            {"kind": "IFFT", "inputs": [{"ugen": 3}]}
        ]))
    };
    // Hops differ; windows differ; both the same (the defaults, spelled out).
    assert!(
        pair(
            json!({"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}],
                    "fft_size": 512, "hop": 0.25})
        )
        .is_err()
    );
    assert!(
        pair(
            json!({"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}],
                    "fft_size": 512, "wintype": 1})
        )
        .is_err()
    );
    assert!(
        pair(
            json!({"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}],
                    "fft_size": 512, "hop": 0.5, "wintype": 0})
        )
        .is_ok()
    );
    // An IFFT naming a window or hop other than its chain's.
    let sink = |extra: Value| {
        let mut ifft = json!({"kind": "IFFT", "inputs": [{"ugen": 1}]});
        for (k, v) in extra.as_object().unwrap() {
            ifft[k] = v.clone();
        }
        compile(json!([
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}],
             "fft_size": 512, "hop": 0.25, "wintype": 4},
            ifft
        ]))
    };
    assert!(sink(json!({"wintype": 0})).is_err());
    assert!(sink(json!({"hop": 0.5})).is_err());
    assert!(sink(json!({"wintype": 4, "hop": 0.25})).is_ok());
}

/// A round trip is exact however the engine slices its blocks, and its latency
/// is exactly one window. Any timed bundle -- for any node -- cuts the block
/// for every synth, and a synth may start mid-block: the chain has to frame
/// its input every `hop` samples regardless, or the overlap-add misplaces
/// frames and scales them wrongly. A hop that is not a whole number of blocks
/// (0.3 of 1024 is 307 samples) is framed just as exactly.
#[test]
fn a_round_trip_is_exact_across_split_blocks_and_any_hop() {
    const START: u64 = 13;
    for hop in [0.5, 0.3, 0.25, 1.0 / 16.0] {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({
            "name": "roundtrip",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
                {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}],
                 "fft_size": 1024, "hop": hop},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }));
        handle
            .send(Cmd::Schedule {
                time: START,
                cmds: vec![add_synth(1, synth)],
            })
            .ok()
            .unwrap();
        // Empty bundles at an offset that walks through every block phase.
        for k in 0..150u64 {
            handle
                .send(Cmd::Schedule {
                    time: 200 + 97 * k,
                    cmds: vec![],
                })
                .ok()
                .unwrap();
        }
        let sig = render_channel(&mut engine, 250);
        let lag = assert_delayed_copy(
            &sig,
            |n| {
                n.checked_sub(START as usize)
                    .map_or(0.0, |local| tone(440.0, 0.5, local))
            },
            2048 + START as usize,
            4096,
            15000,
            1e-4,
        );
        assert_eq!(lag, 1024, "hop {hop}: latency is one window");
    }
}

/// A hop is a fraction of the window in (0, 1] and at least one block long;
/// anything else fails the def instead of running at a hop other than the one
/// asked for.
#[test]
fn compiler_rejects_a_hop_out_of_range() {
    let compile = |fft_size: usize, hop: f32| {
        clausters::synthdef::compile(
            serde_json::from_value(json!({"name": "hop", "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}],
                 "fft_size": fft_size, "hop": hop},
                {"kind": "IFFT", "inputs": [{"ugen": 1}]}
            ]}))
            .unwrap(),
        )
    };
    assert!(compile(256, 0.125).is_err(), "32 samples, under a block");
    assert!(compile(256, 0.25).is_ok(), "64 samples, one block");
    assert!(compile(1024, 0.0).is_err());
    assert!(compile(1024, -0.5).is_err());
    assert!(compile(1024, 1.5).is_err());
    assert!(compile(1024, 1.0).is_ok());
    assert!(compile(1024, 0.3).is_ok(), "307 samples, not a whole block");
}

/// `active` gates what the chain analyses: the output is the gated input,
/// exactly, one window late -- through the switch off and the switch back on
/// alike. The gate moves at the exact sample of a timed `/node_set`.
#[test]
fn active_gates_the_input_exactly_on_both_edges() {
    const OFF: u64 = 9000;
    const ON: u64 = 15_037;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let synth = spec_synth(json!({
        "name": "gated",
        "controls": [{"name": "active", "default": 1.0}],
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
            {"kind": "FFT", "inputs": [{"ugen": 1}, {"control": 0}],
             "fft_size": 1024, "hop": 0.25},
            {"kind": "IFFT", "inputs": [{"ugen": 2}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
        ]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();
    for (time, value) in [(OFF, 0.0), (ON, 1.0)] {
        handle
            .send(Cmd::Schedule {
                time,
                cmds: vec![Cmd::SetControl {
                    id: 1,
                    index: 0,
                    value,
                }],
            })
            .ok()
            .unwrap();
    }
    let sig = render_channel(&mut engine, 400);
    let gated = |n: usize| {
        let on = !(OFF as usize..ON as usize).contains(&n);
        if on { tone(440.0, 0.5, n) } else { 0.0 }
    };
    let lag = assert_delayed_copy(&sig, gated, 1024, 4096, 24_000, 1e-4);
    assert_eq!(lag, 1024);
}

/// A chain gated off lends a combiner silence, not the last frame it took:
/// `PV_Add` of a tone and a second tone whose chain is switched off is the
/// first tone plus the second one gated, exactly.
#[test]
fn a_gated_chain_lends_a_combiner_silence() {
    const OFF: u64 = 9000;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let synth = spec_synth(json!({
        "name": "gated_b",
        "controls": [{"name": "active", "default": 1.0}],
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.25}]},
            {"kind": "Sine", "inputs": [{"const": 700.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 2}, {"const": 0.25}]},
            {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}], "fft_size": 1024},
            {"kind": "FFT", "inputs": [{"ugen": 3}, {"control": 0}], "fft_size": 1024},
            {"kind": "PV_Add", "inputs": [{"ugen": 4}, {"ugen": 5}]},
            {"kind": "IFFT", "inputs": [{"ugen": 6}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 7}]}
        ]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();
    handle
        .send(Cmd::Schedule {
            time: OFF,
            cmds: vec![Cmd::SetControl {
                id: 1,
                index: 0,
                value: 0.0,
            }],
        })
        .ok()
        .unwrap();
    let sig = render_channel(&mut engine, 400);
    let sum = |n: usize| {
        let b = if n < OFF as usize {
            tone(700.0, 0.25, n)
        } else {
            0.0
        };
        tone(440.0, 0.25, n) + b
    };
    assert_delayed_copy(&sig, sum, 1024, 4096, 24_000, 1e-4);
}

/// A `window` command for UGen `ugen` of node 1, timed at `time`.
fn window_at(time: u64, ugen: u32, wintype: f32) -> Cmd {
    let mut args = [0.0f32; 8];
    args[0] = wintype;
    Cmd::Schedule {
        time,
        cmds: vec![Cmd::UGenCommand {
            id: 1,
            ugen_index: ugen,
            command: UGenCmd {
                selector: ugen_cmd_selector("window"),
                args,
                num_args: 1,
            },
        }],
    }
}

/// A live window change is the chain's: sent to the `FFT` alone, the `IFFT`
/// follows it frame by frame, and the samples whose frames straddle the
/// change -- some analysed with the old window, some with the new -- are
/// normalized by what those frames actually used. The round trip stays the
/// input, exactly, through the change. Two changes closer than a window apart
/// are too: the second waits until the first is a window old.
#[test]
fn a_live_window_change_keeps_the_round_trip_exact() {
    for changes in [
        vec![(9000u64, 4.0f32)],
        vec![(9000, 4.0), (9300, 1.0), (16_001, -1.0)],
    ] {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({
            "name": "change",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
                {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}],
                 "fft_size": 1024, "hop": 0.25},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }));
        handle.send(add_synth(1, synth)).ok().unwrap();
        for &(time, wintype) in &changes {
            handle.send(window_at(time, 2, wintype)).ok().unwrap();
        }
        let sig = render_channel(&mut engine, 450);
        let lag = assert_delayed_copy(&sig, |n| tone(440.0, 0.5, n), 1024, 4096, 27_000, 1e-4);
        assert_eq!(lag, 1024, "changes {changes:?}");
    }
}

/// A combiner whose two chains are analysed with different windows refuses:
/// silence, and one fault naming both windows. Changed together, in one
/// bundle, the two chains stay one window and the sum stays exact.
#[test]
fn a_combiner_refuses_chains_whose_windows_differ() {
    let build = || {
        spec_synth(json!({
            "name": "pair",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.25}]},
                {"kind": "Sine", "inputs": [{"const": 700.0}]},
                {"kind": "Mul", "inputs": [{"ugen": 2}, {"const": 0.25}]},
                {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}], "fft_size": 1024},
                {"kind": "FFT", "inputs": [{"ugen": 3}, {"const": 1.0}], "fft_size": 1024},
                {"kind": "PV_Add", "inputs": [{"ugen": 4}, {"ugen": 5}]},
                {"kind": "IFFT", "inputs": [{"ugen": 6}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 7}]}
            ]
        }))
    };
    let sum = |n: usize| tone(440.0, 0.25, n) + tone(700.0, 0.25, n);

    // Both chains change together: still the exact sum.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add_synth(1, build())).ok().unwrap();
    handle.send(window_at(9000, 4, 1.0)).ok().unwrap();
    handle.send(window_at(9000, 5, 1.0)).ok().unwrap();
    let sig = render_channel(&mut engine, 400);
    assert_delayed_copy(&sig, sum, 1024, 4096, 24_000, 1e-4);
    assert!(std::iter::from_fn(|| handle.pop_reply()).next().is_none());

    // Only chain A changes: from the change on, silence and one fault.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle.send(add_synth(1, build())).ok().unwrap();
    handle.send(window_at(9000, 4, 1.0)).ok().unwrap();
    let sig = render_channel(&mut engine, 400);
    assert_delayed_copy(&sig, sum, 1024, 4096, 8000, 1e-4);
    assert!(
        sig[9000 + 1024 + 1024..].iter().all(|&x| x == 0.0),
        "refused: silence once no frame from before the change is left"
    );
    let faults: Vec<_> = std::iter::from_fn(|| handle.pop_reply()).collect();
    assert_eq!(faults.len(), 1, "reported once");
    assert_eq!(faults[0].kind, clausters::dsp::ReplyKind::Fault);
    assert_eq!(faults[0].name(), "PV_Add");
    let text = clausters::dsp::describe_fault(&faults[0]);
    assert!(text.contains("Sine") && text.contains("Hann"), "{text}");
}
