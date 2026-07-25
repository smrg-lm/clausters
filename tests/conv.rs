//! M28 tests: the partitioned convolver — a golden comparison against direct
//! time-domain convolution, the reported intrinsic latency, and a kernel swap
//! crossfade — driven through the real engine, plus the `prepare_partconv`
//! buffer layout.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::dsp::buffer::Buffer;
use clausters::dsp::conv::layout;
use clausters::dsp::wavetable::GenCommand;
use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
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

fn set_buffer(engine: &mut Engine, handle: &mut EngineHandle, index: usize, buf: Buffer) {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    handle
        .send(Cmd::SetBuffer {
            index,
            buffer: Some(Arc::new(buf)),
        })
        .ok()
        .unwrap();
    engine.process_block(&mut out);
    handle.collect_garbage();
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

fn rms(s: &[f32], from: usize, to: usize) -> f32 {
    let seg = &s[from..to.min(s.len())];
    (seg.iter().map(|&x| x * x).sum::<f32>() / seg.len() as f32).sqrt()
}

/// Deterministic pseudo-random samples in `[-1, 1]` (a plain LCG, seeded).
fn lcg_samples(n: usize, mut state: u32) -> Vec<f32> {
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1 << 23) as f32 - 1.0
        })
        .collect()
}

/// Prepares `ir` (a mono impulse response) into the kernel layout the `Conv`
/// UGen reads, exactly as `/b_gen prepare_partconv` does.
fn prepare(ir: &[f32], fft_size: usize) -> Buffer {
    let parts = ir.len().div_ceil(fft_size / 2);
    let target = Buffer::zeroed(layout::frames(fft_size, parts), 1, SR as f64);
    GenCommand::PreparePartConv {
        src: Arc::new(Buffer::new(ir.to_vec(), 1, ir.len(), SR as f64)),
        fft_size,
    }
    .apply(&target)
}

/// The golden test: a known signal (from a buffer, via `PlayBuf`) through a
/// multi-partition kernel matches direct time-domain convolution, delayed by
/// exactly the reported latency (`L` samples).
#[test]
fn conv_matches_direct_convolution() {
    let fft_size = 512usize; // L = 256
    let latency = fft_size / 2;
    let sig = lcg_samples(4096, 12345);
    // A 700-tap IR spanning 3 partitions, scaled small so the f32 transform
    // roundoff stays well under the tolerance.
    let ir: Vec<f32> = lcg_samples(700, 999)
        .iter()
        .map(|x| x * 0.05)
        .collect::<Vec<_>>();

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    set_buffer(
        &mut engine,
        &mut handle,
        0,
        Buffer::new(sig.clone(), 1, sig.len(), SR as f64),
    );
    set_buffer(&mut engine, &mut handle, 1, prepare(&ir, fft_size));

    let synth = spec_synth(json!({
        "name": "convolve",
        "ugens": [
            {"kind": "PlayBuf",
             "inputs": [{"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0}]},
            {"kind": "Conv", "inputs": [{"ugen": 0}, {"const": 1.0}],
             "fft_size": 512, "partitions": 4},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();
    let got = render_channel(&mut engine, 48); // 3072 samples

    // Direct convolution in f64, the reference.
    let expect: Vec<f32> = (0..2500)
        .map(|t| {
            let mut acc = 0.0f64;
            for (j, &h) in ir.iter().enumerate() {
                if t >= j {
                    acc += h as f64 * sig[t - j] as f64;
                }
            }
            acc as f32
        })
        .collect();

    let mut max_err = 0.0f32;
    for (t, &e) in expect.iter().enumerate() {
        let err = (got[latency + t] - e).abs();
        max_err = max_err.max(err);
    }
    assert!(
        max_err < 5e-3,
        "partitioned vs direct convolution: max error {max_err}"
    );
    // And it is not vacuous: the reference has real energy.
    assert!(rms(&expect, 0, 2500) > 0.1);
}

/// The convolver is the first UGen with intrinsic latency: the synth reports
/// its partition length through `SynthNode::latency`; an ordinary def
/// reports 0.
#[test]
fn conv_reports_its_latency() {
    let conv = spec_synth(json!({
        "name": "lat",
        "ugens": [
            {"kind": "WhiteNoise", "inputs": []},
            {"kind": "Conv", "inputs": [{"ugen": 0}, {"const": 1.0}], "fft_size": 2048},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }));
    assert_eq!(conv.latency(), 1024);

    let plain = spec_synth(json!({
        "name": "nolat",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }));
    assert_eq!(plain.latency(), 0);
}

/// A kernel swap: moving the `kernel` input to another prepared buffer takes
/// effect (a unit delta kernel vs a half-gain one), the output stays finite
/// throughout, and the transition crossfades within one partition.
#[test]
fn kernel_swap_crossfades() {
    let fft_size = 512usize;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    set_buffer(&mut engine, &mut handle, 1, prepare(&[1.0], fft_size));
    set_buffer(&mut engine, &mut handle, 2, prepare(&[0.5], fft_size));

    let synth = spec_synth(json!({
        "name": "swap",
        "controls": [{"name": "kern", "default": 1.0}],
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 330.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.4}]},
            {"kind": "Conv", "inputs": [{"ugen": 1}, {"control": 0}], "fft_size": 512},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}
        ]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();

    let before = render_channel(&mut engine, 100); // 6400 samples, kernel 1
    handle
        .send(Cmd::SetControl {
            id: 1,
            index: 0,
            value: 2.0,
        })
        .ok()
        .unwrap();
    let after = render_channel(&mut engine, 100); // kernel 2 (half gain)

    let r_before = rms(&before, 2000, 6400);
    let r_after = rms(&after, 2000, 6400);
    let expected = 0.4 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (r_before - expected).abs() < 0.02,
        "delta kernel is a passthrough: {r_before} vs {expected}"
    );
    assert!(
        (r_after - expected * 0.5).abs() < 0.02,
        "half-gain kernel halves the level: {r_after}"
    );
    for (i, x) in before.iter().chain(after.iter()).enumerate() {
        assert!(x.is_finite(), "non-finite sample at {i}");
    }
}

/// `prepare_partconv` writes the documented layout: `[L, P]`, then packed
/// spectra — a delta IR's first partition transforms to an all-ones spectrum.
#[test]
fn prepare_partconv_layout() {
    let prepared = prepare(&[1.0, 0.0, 0.0], 256);
    let data = prepared.data();
    assert_eq!(data[0], 128.0, "partition length");
    assert_eq!(data[1], 1.0, "partition count");
    let spectrum = &data[layout::HEADER..layout::HEADER + 256];
    // rfft of a delta: every real slot 1 (dc, nyquist, all re), every im 0.
    assert!((spectrum[0] - 1.0).abs() < 1e-5, "dc");
    assert!((spectrum[1] - 1.0).abs() < 1e-5, "nyquist");
    for b in 1..128 {
        assert!((spectrum[2 * b] - 1.0).abs() < 1e-4, "re at bin {b}");
        assert!(spectrum[2 * b + 1].abs() < 1e-4, "im at bin {b}");
    }
}
