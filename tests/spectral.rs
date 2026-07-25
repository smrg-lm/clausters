//! S8 tests: the frequency-domain (`fr`) chain — an `FFT`→`IFFT` round trip
//! reconstructs a tone, and a `PV_*` filter attenuates a band — driven through
//! the real engine (`process_block`), plus a `/u_cmd` window swap.

#![cfg(feature = "synth")]

use std::sync::Arc;

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
    UGenSynth::new(Arc::new(clausters::synthdef::compile(spec).unwrap()), SR)
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

/// A pure tone through `FFT` and straight back out `IFFT` (Hann, 50% hop)
/// reconstructs the tone: unity gain (the overlap-add is window-normalized) and
/// the same frequency, delayed by the transform latency.
#[test]
fn fft_ifft_round_trip_reconstructs_a_tone() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    // Sine(440) -> FFT(512, 0.5, Hann) -> IFFT -> Out.
    let synth = spec_synth(json!({
        "name": "roundtrip",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}],
             "fft_size": 512, "hop": 0.5, "wintype": 0},
            {"kind": "IFFT", "inputs": [{"ugen": 1}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
        ]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();

    let sig = render_channel(&mut engine, 300); // ~19200 samples
    // Skip the startup latency (window + FIFO fill), then compare energy.
    let out_rms = rms(&sig, 4000, 18000);
    // A unit-amplitude sine has RMS 1/sqrt(2) ~ 0.707; unity reconstruction
    // keeps it (within window/FFT tolerance).
    assert!(
        (out_rms - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.08,
        "round-trip RMS {out_rms}, expected ~0.707"
    );
}

/// A high tone through `FFT` -> `PV_BrickWall` (low pass) -> `IFFT` is removed,
/// while the same chain without the filter passes it — the PV filter attenuates
/// its band.
#[test]
fn pv_brickwall_attenuates_a_high_tone() {
    let tone = 9000.0f32;

    // Reference: round trip, no filter — the tone survives.
    let (mut e_ref, mut h_ref) = engine_pair(SR, CHANNELS);
    h_ref
        .send(add_synth(
            1,
            spec_synth(json!({
                "name": "pass",
                "ugens": [
                    {"kind": "Sine", "inputs": [{"const": tone}]},
                    {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
                    {"kind": "IFFT", "inputs": [{"ugen": 1}]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
                ]
            })),
        ))
        .ok()
        .unwrap();
    let pass = render_channel(&mut e_ref, 300);
    let pass_rms = rms(&pass, 6000, 18000);

    // Filtered: a strong low-pass brick wall zeroes the tone's bin.
    let (mut e_bw, mut h_bw) = engine_pair(SR, CHANNELS);
    h_bw.send(add_synth(
        1,
        spec_synth(json!({
            "name": "brickwall",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": tone}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
                {"kind": "PV_BrickWall", "inputs": [{"ugen": 1}, {"const": 0.85}]},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        })),
    ))
    .ok()
    .unwrap();
    let filtered = render_channel(&mut e_bw, 300);
    let filtered_rms = rms(&filtered, 6000, 18000);

    assert!(
        pass_rms > 0.3,
        "unfiltered tone should survive, RMS {pass_rms}"
    );
    assert!(
        filtered_rms < 0.1 * pass_rms,
        "brick wall should attenuate the tone: {filtered_rms} vs {pass_rms}"
    );
}

/// `PV_MagAbove` with a high threshold gates out a quiet tone (its bins fall
/// below the threshold), while a threshold of 0 passes it.
#[test]
fn pv_magabove_gates_below_threshold() {
    let build = |thresh: f32| {
        json!({
            "name": "magabove",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 3000.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 1024},
                {"kind": "PV_MagAbove", "inputs": [{"ugen": 1}, {"const": thresh}]},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        })
    };
    let (mut e_pass, mut h_pass) = engine_pair(SR, CHANNELS);
    h_pass
        .send(add_synth(1, spec_synth(build(0.0))))
        .ok()
        .unwrap();
    let pass_rms = rms(&render_channel(&mut e_pass, 300), 6000, 18000);

    // A huge threshold zeroes every bin -> silence.
    let (mut e_gate, mut h_gate) = engine_pair(SR, CHANNELS);
    h_gate
        .send(add_synth(1, spec_synth(build(1.0e9))))
        .ok()
        .unwrap();
    let gate_rms = rms(&render_channel(&mut e_gate, 300), 6000, 18000);

    assert!(
        pass_rms > 0.3,
        "threshold 0 passes the tone, RMS {pass_rms}"
    );
    assert!(
        gate_rms < 1e-3,
        "a huge threshold gates everything, RMS {gate_rms}"
    );
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

/// `/u_cmd <ugen> window <wintype>` swaps an `FFT`'s analysis window live (the
/// first consumer of the S6 typed per-UGen command surface). Here we drive the
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

/// S11 hop-phase stagger: the node id shifts *when* a chain's first frame
/// fires (a deterministic sub-hop, block-quantized offset), without touching
/// the reconstruction itself. Two identical passthrough chains under different
/// node ids start `stagger` samples apart but agree sample-for-sample in the
/// steady state — the analysis grid shifts, the content timing does not.
#[test]
fn hop_stagger_shifts_only_the_first_frame() {
    // FFT(512, 50% hop) at BLOCK_SIZE 64: 4 blocks per hop. Node id 4 ≡ 0
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
    // empty), so the first nonzero sample marks the first fire — it moves by
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

// ---- M27: the curated PV set ----

/// `PV_MagClip` limits loud bins to the threshold but is transparent when the
/// threshold clears every magnitude: same def, huge vs tiny threshold.
#[test]
fn pv_magclip_limits_loud_bins() {
    let build = |thresh: f32| {
        spec_synth(json!({
            "name": "clip",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "PV_MagClip", "inputs": [{"ugen": 1}, {"const": thresh}]},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }))
    };
    let render = |thresh: f32| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        handle.send(add_synth(1, build(thresh))).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    let clipped = render(1.0); // bin magnitudes of a unit sine are way above 1
    let open = render(1.0e9); // clears everything: transparent
    let passthrough = {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({
            "name": "pass",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "IFFT", "inputs": [{"ugen": 1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
            ]
        }));
        handle.send(add_synth(1, synth)).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    assert_eq!(open, passthrough, "an over-threshold clip is transparent");
    let (r_clip, r_open) = (rms(&clipped, 6000, 19000), rms(&open, 6000, 19000));
    assert!(
        r_clip < r_open * 0.5,
        "clipping attenuates: {r_clip} vs {r_open}"
    );
    assert!(r_clip > 1e-4, "clipped tone still sounds");
}

/// `PV_Add` (the two-chain combiner): summing the spectra of two tones carries
/// both — the combined power matches the two individual renders' power sum.
#[test]
fn pv_add_combines_two_chains() {
    let one = |freq: f32| {
        spec_synth(json!({
            "name": "single",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": freq}]},
                {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.2}]},
                {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}], "fft_size": 512},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }))
    };
    let both = spec_synth(json!({
        "name": "added",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.2}]},
            {"kind": "FFT", "inputs": [{"ugen": 1}, {"const": 1.0}], "fft_size": 512},
            {"kind": "Sine", "inputs": [{"const": 3000.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 3}, {"const": 0.2}]},
            {"kind": "FFT", "inputs": [{"ugen": 4}, {"const": 1.0}], "fft_size": 512},
            {"kind": "PV_Add", "inputs": [{"ugen": 2}, {"ugen": 5}]},
            {"kind": "IFFT", "inputs": [{"ugen": 6}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 7}]}
        ]
    }));
    let render = |s: UGenSynth| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        handle.send(add_synth(1, s)).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    let a = rms(&render(one(440.0)), 6000, 19000);
    let b = rms(&render(one(3000.0)), 6000, 19000);
    let sum = rms(&render(both), 6000, 19000);
    let expect = (a * a + b * b).sqrt();
    assert!(
        (sum - expect).abs() < expect * 0.1,
        "combined power {sum} vs expected {expect}"
    );
}

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

/// `PV_MagFreeze`: un-frozen it is transparent; frozen from the first frame it
/// holds the initial (zero) magnitudes — silence.
#[test]
fn pv_magfreeze_holds_magnitudes() {
    let build = |freeze: f32| {
        spec_synth(json!({
            "name": "freeze",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "PV_MagFreeze", "inputs": [{"ugen": 1}, {"const": freeze}]},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }))
    };
    let render = |freeze: f32| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        handle.send(add_synth(1, build(freeze))).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    let open = render(0.0);
    assert!(rms(&open, 6000, 19000) > 0.5, "un-frozen passes the tone");
    let frozen = render(1.0);
    assert!(
        rms(&frozen, 6000, 19000) < 1e-4,
        "frozen-at-zero magnitudes stay silent"
    );
}

/// `PV_MagSmear`: zero neighbors is exactly transparent; a wide smear changes
/// the signal (the tone's energy spreads across bins).
#[test]
fn pv_magsmear_zero_is_transparent() {
    let build = |bins: f32| {
        spec_synth(json!({
            "name": "smear",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "PV_MagSmear", "inputs": [{"ugen": 1}, {"const": bins}]},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }))
    };
    let render = |bins: f32| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        handle.send(add_synth(1, build(bins))).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    let passthrough = {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({
            "name": "pass",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "IFFT", "inputs": [{"ugen": 1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
            ]
        }));
        handle.send(add_synth(1, synth)).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    assert_eq!(render(0.0), passthrough, "bins = 0 is transparent");
    let smeared = render(32.0);
    let diff: f32 = smeared
        .iter()
        .zip(&passthrough)
        .map(|(x, y)| (x - y).abs())
        .sum();
    assert!(diff > 1.0, "a wide smear changes the signal");
}

/// `PV_BinShift`: identity parameters are exactly transparent, and a +10-bin
/// shift moves a 440 Hz tone to ~440 + 10·(48000/512) ≈ 1377 Hz (measured by
/// zero crossings in the steady state).
#[test]
fn pv_binshift_moves_the_tone() {
    let build = |stretch: f32, shift: f32| {
        spec_synth(json!({
            "name": "binshift",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "PV_BinShift",
                 "inputs": [{"ugen": 1}, {"const": stretch}, {"const": shift}]},
                {"kind": "IFFT", "inputs": [{"ugen": 2}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }))
    };
    let render = |stretch: f32, shift: f32| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        handle
            .send(add_synth(1, build(stretch, shift)))
            .ok()
            .unwrap();
        render_channel(&mut engine, 300)
    };
    let passthrough = {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let synth = spec_synth(json!({
            "name": "pass",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
                {"kind": "IFFT", "inputs": [{"ugen": 1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
            ]
        }));
        handle.send(add_synth(1, synth)).ok().unwrap();
        render_channel(&mut engine, 300)
    };
    assert_eq!(
        render(1.0, 0.0),
        passthrough,
        "stretch 1, shift 0 is transparent"
    );

    let shifted = render(1.0, 10.0);
    let seg = &shifted[6000..19000];
    let crossings = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    let freq = crossings as f32 * SR / seg.len() as f32;
    assert!(
        (1250.0..1510.0).contains(&freq),
        "shifted tone at {freq} Hz, expected ~1377"
    );
}

/// M29 `PV_Kernel`: a bin-expression program reproducing a curated op renders
/// **sample-identically** to the built-in row — the mechanism's acceptance
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

/// The phase path: `phase + pi` flips every bin, so the reconstruction is the
/// negated round trip (within the polar round-trip tolerance — this path goes
/// through `atan2`/`cos`/`sin`, unlike the exact magnitude-scaling path).
#[test]
fn pv_kernel_phase_program_inverts_with_pi() {
    let render = |phase_expr: Option<Value>| {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        let mut kernel = json!({"kind": "PV_Kernel", "inputs": [{"ugen": 1}]});
        if let Some(e) = phase_expr {
            kernel["phase_expr"] = e;
        }
        let synth = spec_synth(json!({"name": "k", "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 512},
            kernel,
            {"kind": "IFFT", "inputs": [{"ugen": 2}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 3}]}
        ]}));
        handle.send(add_synth(1, synth)).ok().unwrap();
        render_channel(&mut engine, 200)
    };
    let plain = render(None);
    let flipped = render(Some(json!(["phase", std::f32::consts::PI, "add"])));
    let residue = rms(
        &plain
            .iter()
            .zip(&flipped)
            .map(|(a, b)| a + b)
            .collect::<Vec<_>>(),
        4000,
        12000,
    );
    let level = rms(&plain, 4000, 12000);
    assert!(level > 0.3, "the round trip should carry the tone");
    assert!(
        residue < 0.02 * level,
        "flipped render should negate the plain one: residue {residue} vs level {level}"
    );
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
