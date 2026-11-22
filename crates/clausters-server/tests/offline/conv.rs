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
use clausters::server::render::{OscScore, RenderConfig, render_to_vec};
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
    // f32 transforms against an f64 sum: rounding, and nothing else.
    assert!(
        max_err < 5e-6,
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

/// A kernel swap, exactly: the output is the old kernel's convolution, then
/// one hop of `L` samples fading linearly from the old to the new (`t = (k +
/// 0.5) / L`), then the new kernel's -- here a unit delta and a half one, so
/// the three pieces are the input, delayed by `L`, at gain 1, a ramp, and
/// 0.5.
#[test]
fn kernel_swap_crossfades_exactly() {
    let fft_size = 512usize;
    let part = fft_size / 2;
    let sig = lcg_samples(16_384, 4242);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    set_buffer(
        &mut engine,
        &mut handle,
        0,
        Buffer::new(sig.clone(), 1, sig.len(), SR as f64),
    );
    set_buffer(&mut engine, &mut handle, 1, prepare(&[1.0], fft_size));
    set_buffer(&mut engine, &mut handle, 2, prepare(&[0.5], fft_size));

    let synth = spec_synth(json!({
        "name": "swap",
        "controls": [{"name": "kern", "default": 1.0}],
        "ugens": [
            {"kind": "PlayBuf",
             "inputs": [{"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                        {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]},
            {"kind": "Conv", "inputs": [{"ugen": 0}, {"control": 0}], "fft_size": 512},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();
    let mut got = render_channel(&mut engine, 100); // 6400 samples on kernel 1
    handle
        .send(Cmd::SetControl {
            id: 1,
            index: 0,
            value: 2.0,
        })
        .ok()
        .unwrap();
    got.extend(render_channel(&mut engine, 100));

    // The swap lands on the first hop boundary the change can reach; find
    // it, then hold every sample to the three pieces.
    let x = |t: usize| sig[t - part];
    let expect = |m: usize, t: usize| {
        let (start, end) = (m * part, (m + 1) * part);
        if t < start {
            x(t)
        } else if t < end {
            let tau = ((t - start) as f32 + 0.5) / part as f32;
            x(t) * (1.0 - tau) + 0.5 * x(t) * tau
        } else {
            0.5 * x(t)
        }
    };
    let err = |m: usize| {
        (part..got.len())
            .map(|t| (got[t] - expect(m, t)).abs())
            .fold(0.0f32, f32::max)
    };
    let (m, worst) = (6400 / part..6400 / part + 3)
        .map(|m| (m, err(m)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(
        worst < 1e-5,
        "no exact swap: best hop {m}, max error {worst}"
    );
    assert!(
        m * part >= 6400 && m * part < 6400 + 2 * part,
        "the swap lands within a hop of the change, not at {}",
        m * part
    );
}

/// The convolution is exact however the engine slices its blocks: a synth
/// started at sample 13 with a timed bundle every 97 samples convolves just
/// as the golden does, one partition late.
#[test]
fn conv_is_exact_across_split_blocks() {
    const START: usize = 13;
    let fft_size = 512usize;
    let latency = fft_size / 2;
    let sig = lcg_samples(8192, 777);
    let ir: Vec<f32> = lcg_samples(700, 31).iter().map(|x| x * 0.05).collect();

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    set_buffer(
        &mut engine,
        &mut handle,
        0,
        Buffer::new(sig.clone(), 1, sig.len(), SR as f64),
    );
    set_buffer(&mut engine, &mut handle, 1, prepare(&ir, fft_size));
    // Each `set_buffer` ran one block: the engine is at sample 128.
    let base = 2 * BLOCK_SIZE as u64;
    let synth = spec_synth(json!({
        "name": "convolve",
        "ugens": [
            {"kind": "PlayBuf",
             "inputs": [{"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                        {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]},
            {"kind": "Conv", "inputs": [{"ugen": 0}, {"const": 1.0}],
             "fft_size": 512, "partitions": 3},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }));
    handle
        .send(Cmd::Schedule {
            time: base + START as u64,
            cmds: vec![add_synth(1, synth)],
        })
        .ok()
        .unwrap();
    for k in 0..60u64 {
        handle
            .send(Cmd::Schedule {
                time: base + 200 + 97 * k,
                cmds: vec![],
            })
            .ok()
            .unwrap();
    }
    let got = render_channel(&mut engine, 100);

    let mut max_err = 0.0f32;
    for (t, &g) in got.iter().enumerate() {
        let want = t.checked_sub(START + latency).map_or(0.0, |n| {
            let mut acc = 0.0f64;
            for (j, &h) in ir.iter().enumerate().take(n + 1) {
                acc += h as f64 * sig[n - j] as f64;
            }
            acc as f32
        });
        max_err = max_err.max((g - want).abs());
    }
    assert!(
        max_err < 5e-6,
        "split-block convolution: max error {max_err}"
    );
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
    render_to_vec(&OscScore::new(events)?, &cfg).map(|_| ())
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

// ---- scsynth's convolution names ----

/// Direct convolution of `sig` with `ir` in f64, the first `len` samples.
fn direct(sig: &[f32], ir: &[f32], len: usize) -> Vec<f32> {
    (0..len)
        .map(|t| {
            let mut acc = 0.0f64;
            for (j, &h) in ir.iter().enumerate() {
                if t >= j {
                    acc += h as f64 * sig[t - j] as f64;
                }
            }
            acc as f32
        })
        .collect()
}

/// A def that plays buffer 0 through `ugen` (its input 0 the played signal)
/// to the output.
fn played_through(ugen: Value) -> UGenSynth {
    spec_synth(json!({
        "name": "through",
        "controls": [{"name": "kernel", "default": 1.0}, {"name": "trig", "default": 0.0}],
        "ugens": [
            {"kind": "PlayBuf",
             "inputs": [{"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                        {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]},
            ugen,
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }))
}

/// Renders `ugen` over the shared test signal, with `buffers` from index 1.
fn render_rows(ugen: Value, buffers: Vec<Buffer>, blocks: usize) -> Vec<f32> {
    let sig = lcg_samples(4096, 12345);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    set_buffer(
        &mut engine,
        &mut handle,
        0,
        Buffer::new(sig, 1, 4096, SR as f64),
    );
    for (k, b) in buffers.into_iter().enumerate() {
        set_buffer(&mut engine, &mut handle, k + 1, b);
    }
    handle
        .send(add_synth(1, played_through(ugen)))
        .ok()
        .unwrap();
    render_channel(&mut engine, blocks)
}

fn mono(samples: Vec<f32>) -> Buffer {
    let n = samples.len();
    Buffer::new(samples, 1, n, SR as f64)
}

fn assert_late(name: &str, got: &[f32], want: &[f32], late: usize, tol: f32) {
    let err = want
        .iter()
        .enumerate()
        .map(|(t, &w)| (got[late + t] - w).abs())
        .fold(0.0f32, f32::max);
    assert!(err < tol, "{name}: max error {err}");
    assert!(
        rms(want, 0, want.len()) > 0.05,
        "{name}: the reference is silent"
    );
}

/// Every one of scsynth's convolution names convolves: `PartConv` and the
/// raw-kernel rows are the input convolved with the kernel, one partition
/// late; `Convolution3` is that with no latency; each channel of
/// `StereoConvolution2L` is its own kernel's convolution.
#[test]
fn scsynths_convolution_rows_convolve() {
    let sig = lcg_samples(4096, 12345);
    let ir_a: Vec<f32> = lcg_samples(300, 999).iter().map(|x| x * 0.05).collect();
    let ir_b: Vec<f32> = lcg_samples(200, 31).iter().map(|x| x * 0.05).collect();
    let src = json!({"ugen": 0});
    let k = |v: f32| json!({"const": v});
    let rows: Vec<(&str, Value, Vec<f32>, usize)> = vec![
        (
            "Convolution2",
            json!({"kind": "Convolution2", "inputs": [src, k(1.0), k(0.0), k(512.0)]}),
            ir_a.clone(),
            512,
        ),
        (
            "Convolution2L",
            json!({"kind": "Convolution2L", "inputs": [src, k(1.0), k(0.0), k(512.0), k(1.0)]}),
            ir_a.clone(),
            512,
        ),
        (
            "StereoConvolution2L left",
            json!({"kind": "StereoConvolution2L",
                   "inputs": [src, k(1.0), k(2.0), k(0.0), k(512.0), k(1.0), k(0.0)]}),
            ir_a.clone(),
            512,
        ),
        (
            "StereoConvolution2L right",
            json!({"kind": "StereoConvolution2L",
                   "inputs": [src, k(1.0), k(2.0), k(0.0), k(512.0), k(1.0), k(1.0)]}),
            ir_b.clone(),
            512,
        ),
        (
            "Convolution3",
            json!({"kind": "Convolution3", "inputs": [src, k(1.0), k(0.0), k(300.0)]}),
            ir_a.clone(),
            0,
        ),
    ];
    for (name, ugen, ir, late) in rows {
        let kernels = vec![mono(ir_a.clone()), mono(ir_b.clone())];
        let got = render_rows(ugen, kernels, 48);
        assert_late(name, &got, &direct(&sig, &ir, 2500), late, 5e-6);
    }
    // `PartConv` reads a prepared kernel, `Conv`'s.
    let got = render_rows(
        json!({"kind": "PartConv", "inputs": [src, k(512.0), k(1.0)], "partitions": 2}),
        vec![prepare(&ir_a, 512)],
        48,
    );
    assert_late("PartConv", &got, &direct(&sig, &ir_a, 2500), 256, 5e-6);
}

/// `Convolution` convolves each `framesize` samples of its input with the
/// same samples of its kernel signal and overlap-adds them, `framesize`
/// late.
#[test]
fn convolution_convolves_two_signals_frame_by_frame() {
    let sig = lcg_samples(4096, 12345);
    let kern = lcg_samples(4096, 77);
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    set_buffer(&mut engine, &mut handle, 0, mono(sig.clone()));
    set_buffer(&mut engine, &mut handle, 1, mono(kern.clone()));
    let play = |b: f32| {
        json!({"kind": "PlayBuf",
               "inputs": [{"const": b}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                          {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]})
    };
    let synth = spec_synth(json!({
        "name": "cross",
        "ugens": [play(0.0), play(1.0),
            {"kind": "Convolution", "inputs": [{"ugen": 0}, {"ugen": 1}, {"const": 256.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 2}]}]
    }));
    handle.send(add_synth(1, synth)).ok().unwrap();
    let got = render_channel(&mut engine, 48);
    const F: usize = 256;
    let mut want = vec![0.0f64; 2600];
    for s in 0..8 {
        for i in 0..F {
            for j in 0..F {
                if s * F + i + j < want.len() {
                    want[s * F + i + j] += sig[s * F + i] as f64 * kern[s * F + j] as f64;
                }
            }
        }
    }
    let want: Vec<f32> = want[..2048].iter().map(|&x| x as f32).collect();
    assert_late("Convolution", &got, &want, F, 2e-4);
}

/// A raw kernel loads again at a trigger: `Convolution2` switches to it at
/// the next frame, `Convolution2L` fades to it over `crossfade` frames, and
/// before and after each is exactly one kernel's convolution.
#[test]
fn a_trigger_reloads_a_raw_kernel() {
    const F: usize = 512;
    let sig = lcg_samples(4096, 12345);
    let ir_a: Vec<f32> = lcg_samples(300, 999).iter().map(|x| x * 0.05).collect();
    let ir_b: Vec<f32> = lcg_samples(200, 31).iter().map(|x| x * 0.05).collect();
    let (wa, wb) = (direct(&sig, &ir_a, 4096), direct(&sig, &ir_b, 4096));
    for (kind, fade) in [("Convolution2", 0usize), ("Convolution2L", 2)] {
        let mut inputs = vec![
            json!({"ugen": 0}),
            json!({"control": 0}),
            json!({"control": 1}),
            json!({"const": F as f32}),
        ];
        if kind == "Convolution2L" {
            inputs.push(json!({"const": fade as f32}));
        }
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        set_buffer(&mut engine, &mut handle, 0, mono(sig.clone()));
        set_buffer(&mut engine, &mut handle, 1, mono(ir_a.clone()));
        set_buffer(&mut engine, &mut handle, 2, mono(ir_b.clone()));
        handle
            .send(add_synth(
                1,
                played_through(json!({"kind": kind, "inputs": inputs})),
            ))
            .ok()
            .unwrap();
        // Switch inside the synth's fourth frame.
        let mut got = render_channel(&mut engine, (3 * F + 64) / BLOCK_SIZE);
        handle
            .send(Cmd::SetControl {
                id: 1,
                index: 0,
                value: 2.0,
            })
            .ok()
            .unwrap();
        handle
            .send(Cmd::SetControl {
                id: 1,
                index: 1,
                value: 1.0,
            })
            .ok()
            .unwrap();
        got.extend(render_channel(&mut engine, 50));
        // The frame closing after the trigger is the first of kernel B: its
        // output plays from 4F, the convolution at 3F.
        for t in F..4 * F {
            assert!((got[t] - wa[t - F]).abs() < 5e-6, "{kind}: kernel A at {t}");
        }
        let end = 4 * F + fade * F;
        for t in 4 * F..end {
            let x = ((t - 4 * F) as f32 + 0.5) / (fade * F) as f32;
            let want = wa[t - F] * (1.0 - x) + wb[t - F] * x;
            assert!((got[t] - want).abs() < 5e-6, "{kind}: the fade at {t}");
        }
        for t in end..end + 2 * F {
            assert!((got[t] - wb[t - F]).abs() < 5e-6, "{kind}: kernel B at {t}");
        }
    }
}

/// A raw kernel longer than the `framesize` the UGen holds is refused, not
/// cut: silence, reported once with the buffer, its frames and the
/// framesize.
#[test]
fn a_raw_kernel_longer_than_its_framesize_plays_silence_and_says_so() {
    for (kind, size) in [("Convolution2", 256.0f32), ("Convolution3", 100.0)] {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        set_buffer(&mut engine, &mut handle, 0, mono(lcg_samples(4096, 1)));
        set_buffer(&mut engine, &mut handle, 1, mono(lcg_samples(300, 2)));
        let ugen = json!({"kind": kind, "inputs": [{"ugen": 0}, {"const": 1.0},
                                                    {"const": 0.0}, {"const": size}]});
        handle
            .send(add_synth(1, played_through(ugen)))
            .ok()
            .unwrap();
        let out = render_channel(&mut engine, 30);
        assert!(
            out.iter().all(|&x| x == 0.0),
            "{kind}: a refused kernel is silence"
        );
        let faults = replies(&mut handle);
        assert_eq!(faults.len(), 1, "{kind}: reported once");
        assert_eq!(faults[0].id, fault::RAW_TOO_LONG);
        assert_eq!(faults[0].values(), &[1.0, 300.0, size]);
        assert!(describe_fault(&faults[0]).contains("has 300 frames, more than the framesize"));
    }
}

/// The compiler takes a convolution row's size from a constant and holds it
/// to a size the transform supports.
#[test]
fn compiler_holds_a_convolution_size_to_a_constant_it_supports() {
    let compile = |size: Value| {
        let spec: SynthDefSpec = serde_json::from_value(json!({
            "name": "c",
            "controls": [{"name": "size", "default": 512.0}],
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "Convolution2", "inputs": [{"ugen": 0}, {"const": 1.0},
                                                     {"const": 0.0}, size]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
            ]
        }))
        .unwrap();
        clausters::synthdef::compile(spec)
    };
    assert!(compile(json!({"const": 512.0})).is_ok());
    let err = compile(json!({"control": 0})).err().unwrap();
    assert!(err.contains("must be a constant"), "{err}");
    let err = compile(json!({"const": 300.0})).err().unwrap();
    assert!(err.contains("unsupported framesize 300"), "{err}");
}
