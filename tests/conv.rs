//! The partitioned convolver -- a golden comparison against direct
//! time-domain convolution, the reported intrinsic latency, and a kernel swap
//! crossfade -- driven through the real engine, plus the `prepare_partconv`
//! buffer layout.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::buffer::Buffer;
use clausters::dsp::conv::{fault, layout};
use clausters::dsp::wavetable::GenCommand;
use clausters::dsp::{ReplyKind, describe_fault};
use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use clausters::server::render::{RenderConfig, Score, render_to_vec};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use rosc::{OscMessage, OscType};
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
/// UGen reads, exactly as `/buffer_gen prepare_partconv` does.
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
             "inputs": [{"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                        {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]},
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
/// spectra -- a delta IR's first partition transforms to an all-ones spectrum.
#[test]
fn prepare_partconv_layout() {
    let prepared = prepare(&[1.0, 0.0, 0.0], 256);
    let data = prepared.to_vec();
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

/// A synth convolving a 440 Hz tone with the kernel in buffer 1, on a `Conv`
/// of `fft_size` holding `partitions`.
fn tone_through(fft_size: usize, partitions: usize) -> UGenSynth {
    spec_synth(json!({
        "name": "held",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Conv", "inputs": [{"ugen": 0}, {"const": 1.0}],
             "fft_size": fft_size, "partitions": partitions},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }))
}

/// Every reply the engine has queued.
fn replies(handle: &mut EngineHandle) -> Vec<clausters::dsp::ReplyMsg> {
    std::iter::from_fn(|| handle.pop_reply()).collect()
}

/// A kernel longer than the `Conv` holds is refused, not cut to fit: the
/// output is silence, and the fault is reported once with the buffer, the
/// kernel's partitions and the capacity.
#[test]
fn a_kernel_longer_than_the_conv_plays_silence_and_says_so() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    // 700 taps at L = 256: three partitions, into a Conv that holds two.
    let ir: Vec<f32> = lcg_samples(700, 7).iter().map(|x| x * 0.05).collect();
    set_buffer(&mut engine, &mut handle, 1, prepare(&ir, 512));
    handle
        .send(add_synth(1, tone_through(512, 2)))
        .ok()
        .unwrap();

    let out = render_channel(&mut engine, 40);
    assert!(out.iter().all(|&x| x == 0.0), "a refused kernel is silence");
    let faults = replies(&mut handle);
    assert_eq!(faults.len(), 1, "reported once");
    let f = &faults[0];
    assert_eq!(
        (f.kind, f.id, f.node_id),
        (ReplyKind::Fault, fault::TOO_LONG, 1)
    );
    assert_eq!(f.values(), &[1.0, 3.0, 2.0]);
    assert!(describe_fault(f).contains("has 3 partitions and this Conv holds 2"));

    render_channel(&mut engine, 40);
    assert!(
        replies(&mut handle).is_empty(),
        "and not again while it stays"
    );
}

/// A kernel prepared at another FFT size is refused the same way.
#[test]
fn a_kernel_of_another_partition_size_plays_silence_and_says_so() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    set_buffer(&mut engine, &mut handle, 1, prepare(&[1.0], 256));
    handle
        .send(add_synth(1, tone_through(512, 4)))
        .ok()
        .unwrap();

    let out = render_channel(&mut engine, 40);
    assert!(out.iter().all(|&x| x == 0.0));
    let faults = replies(&mut handle);
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].id, fault::PARTITION);
    assert_eq!(faults[0].values(), &[1.0, 128.0, 256.0]);
}

/// A kernel that fits exactly -- as many partitions as the `Conv` holds --
/// is used whole, with no fault.
#[test]
fn a_kernel_that_fits_exactly_is_used() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let ir: Vec<f32> = lcg_samples(700, 7).iter().map(|x| x * 0.05).collect();
    set_buffer(&mut engine, &mut handle, 1, prepare(&ir, 512));
    handle
        .send(add_synth(1, tone_through(512, 3)))
        .ok()
        .unwrap();
    let out = render_channel(&mut engine, 40);
    assert!(rms(&out, 1000, 2560) > 0.01);
    assert!(replies(&mut handle).is_empty());
}

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

/// An offline render with an IR in buffer 0 (`ir_frames` of it), a kernel
/// buffer of `kernel_frames` prepared from it at 512, and a tone through a
/// `Conv` holding `partitions`.
fn render_conv(ir_frames: i32, kernel_frames: i32, partitions: usize) -> Result<(), String> {
    render_conv_from(
        msg(
            "/buffer_alloc",
            vec![OscType::Int(0), OscType::Int(ir_frames), OscType::Int(1)],
        ),
        kernel_frames,
        partitions,
    )
}

/// [`render_conv`] with the IR loaded into buffer 0 by `load`.
fn render_conv_from(load: OscMessage, kernel_frames: i32, partitions: usize) -> Result<(), String> {
    let def = serde_json::to_vec(
        &serde_json::from_value::<SynthDefSpec>(json!({
            "name": "c",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "Conv", "inputs": [{"ugen": 0}, {"const": 1.0}],
                 "fft_size": 512, "partitions": partitions},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let events = vec![
        (
            0.0,
            vec![
                load,
                msg(
                    "/buffer_alloc",
                    vec![
                        OscType::Int(1),
                        OscType::Int(kernel_frames),
                        OscType::Int(1),
                    ],
                ),
                msg(
                    "/buffer_gen",
                    vec![
                        OscType::Int(1),
                        OscType::String("prepare_partconv".into()),
                        OscType::Int(512),
                        OscType::Int(0),
                    ],
                ),
                msg(
                    "/def_send",
                    vec![OscType::String("synth".into()), OscType::Blob(def)],
                ),
                msg(
                    "/synth_new",
                    vec![
                        OscType::String("c".into()),
                        OscType::Int(100),
                        OscType::Int(0),
                        OscType::Int(0),
                    ],
                ),
            ],
        ),
        (0.1, vec![msg("/node_free", vec![OscType::Int(100)])]),
    ];
    let cfg = RenderConfig {
        sample_rate: SR as f64,
        channels: 1,
        ..RenderConfig::default()
    };
    render_to_vec(&Score::new(events)?, &cfg).map(|_| ())
}

/// Offline, a refused kernel fails the render with the reason -- a file of
/// silence with no error would be a wrong result nobody is told about -- and
/// `prepare_partconv` refuses a buffer too small for the whole response
/// rather than preparing part of it.
#[test]
fn offline_a_kernel_that_cannot_be_used_whole_fails_the_render() {
    // 1000 frames at L = 256: four partitions.
    let whole = layout::frames(512, 4) as i32;
    assert!(render_conv(1000, whole, 4).is_ok());

    let err = render_conv(1000, whole, 2).unwrap_err();
    assert!(
        err.contains("has 4 partitions and this Conv holds 2"),
        "{err}"
    );

    let err = render_conv(1000, whole - 1, 4).unwrap_err();
    assert!(err.contains("need a 2050-sample buffer"), "{err}");

    // 256 partitions is the most a Conv holds; one frame more is refused.
    let err = render_conv(256 * 256 + 1, layout::frames(512, 257) as i32, 4).unwrap_err();
    assert!(err.contains("over the 256 a Conv holds"), "{err}");
}

/// A mono 16-bit WAV of `frames` of a decaying click at `rate`, in the
/// system temp directory.
fn ir_wav(rate: u32, frames: usize) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "clausters-conv-ir-{rate}-{}.wav",
        std::process::id()
    ));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for k in 0..frames {
        w.write_sample((8000.0 * (-(k as f32) / 100.0).exp()) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
    path
}

/// An impulse response recorded at another rate is refused: a partition is a
/// span of samples, so used as if it matched it would convolve as a response
/// stretched in time and shifted in frequency. The server converts no rates.
#[test]
fn prepare_partconv_refuses_an_impulse_response_at_another_rate() {
    let whole = layout::frames(512, 4) as i32;
    let read = |path: &std::path::Path| {
        msg(
            "/buffer_allocRead",
            vec![
                OscType::Int(0),
                OscType::String(path.to_str().unwrap().into()),
            ],
        )
    };
    let same = ir_wav(48_000, 1000);
    let other = ir_wav(44_100, 1000);
    let fine = render_conv_from(read(&same), whole, 4);
    let err = render_conv_from(read(&other), whole, 4);
    let _ = std::fs::remove_file(&same);
    let _ = std::fs::remove_file(&other);
    assert!(fine.is_ok(), "{fine:?}");
    let err = err.unwrap_err();
    assert!(
        err.contains("is at 44100 Hz and the server at 48000 Hz"),
        "{err}"
    );
}
