//! S8 tests: the frequency-domain (`fr`) chain — an `FFT`→`IFFT` round trip
//! reconstructs a tone, and a `PV_*` filter attenuates a band — driven through
//! the real engine (`process_block`), plus a `/u_cmd` window swap.

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
    UGenSynth::new(Arc::new(clausters::synthdef::compile(spec).unwrap()))
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
    // SinOsc(440) -> FFT(512, 0.5, Hann) -> IFFT -> Out.
    let synth = spec_synth(json!({
        "name": "roundtrip",
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 440.0}]},
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
                    {"kind": "SinOsc", "inputs": [{"const": tone}]},
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
                {"kind": "SinOsc", "inputs": [{"const": tone}]},
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
                {"kind": "SinOsc", "inputs": [{"const": 3000.0}]},
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
            {"kind": "SinOsc", "inputs": [{"const": 440.0}]},
            {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 777},
            {"kind": "IFFT", "inputs": [{"ugen": 1}]}
        ]
    }))
    .unwrap();
    assert!(clausters::synthdef::compile(bad_size).is_err());

    let bad_chain: SynthDefSpec = serde_json::from_value(json!({
        "name": "badchain",
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 440.0}]},
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
