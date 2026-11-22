//! The delay lines beyond the nine and the synth's own buffers, held
//! against the theory: `Pluck`'s pitch and decay, `PitchShift`'s ratio, the
//! tap pair's delay, `LocalBuf`'s numbering and reset, `SetBuf` before every
//! reader, `ClearBuf`, `ScopeOut`'s ring and `VDiskIn`'s rate.
//! `sc_reference` holds the rest against scsynth.

#![cfg(feature = "synth")]

use std::sync::Arc;

use crate::bench::{Run, render_def};
use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::buffer::{Buffer, NUM_BUFFERS};
use clausters::dsp::rand::Generators;
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::server::meters::Meters;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;

fn c(v: f32) -> Value {
    json!({ "const": v })
}

/// `n` samples of a graph whose last UGen goes to bus 0.
fn render(ugens: Value, n: usize, buffers: &[Option<Arc<Buffer>>]) -> Vec<f32> {
    let mut ugens = ugens.as_array().unwrap().clone();
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), {"ugen": last}]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    let run = Run {
        buffers,
        ..Run::new(n)
    };
    render_def(&def, &run).remove(0)
}

/// The power of `x` at `freq`, by Goertzel's recurrence.
fn power(x: &[f32], freq: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * freq / SR as f64;
    let k = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &v in x {
        let s = v as f64 + k * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - k * s1 * s2) / (x.len() as f64).powi(2)
}

/// A string `delaytime` long sounds at its reciprocal, and with no lowpass
/// (`coef` 0) falls by 60 dB in `decaytime`.
#[test]
fn pluck_sounds_its_period_and_decays_by_its_time() {
    let period = 0.005; // 200 Hz
    let n = 48_000;
    let y = render(
        json!([
            {"kind": "WhiteNoise", "inputs": []},
            {"kind": "Pluck", "inputs": [{"ugen": 0}, c(1.0), c(0.01), c(period), c(0.5), c(0.0)]}
        ]),
        n,
        &[],
    );
    let head = &y[..4800];
    let f0 = power(head, 200.0);
    for off in [150.0, 270.0, 330.0] {
        assert!(power(head, off) < f0 / 100.0, "{off} Hz against 200");
    }
    let rms = |x: &[f32]| (x.iter().map(|v| (v * v) as f64).sum::<f64>() / x.len() as f64).sqrt();
    // One period's level at the start against one half a second on.
    let a = rms(&y[240..480]);
    let b = rms(&y[24_240..24_480]);
    let db = 20.0 * (b / a).log10();
    assert!((db + 60.0).abs() < 1.5, "{db} dB after the decay time");
}

/// A sine through a shift of 2 comes out an octave up, and through 0.5 an
/// octave down: the strongest frequency is within half the rate the heads
/// start at of the shifted one (the heads' fades make lines that rate
/// apart), and the sine itself is gone.
///
/// At a frequency the heads read in phase: they start a quarter window
/// apart, and where that is half a period of what they read they cancel, as
/// any four-head shifter's do. A quarter window is 565 samples, read at
/// half speed 282.5 of the input's, so 3 periods in 282.5 samples are in
/// phase at both ratios.
#[test]
fn pitch_shift_moves_a_sine_by_its_ratio() {
    let f = 3.0 * SR as f64 / 282.5;
    for (ratio, want, not) in [(2.0, 2.0 * f, f), (0.5, 0.5 * f, f)] {
        let y = render(
            json!([
                {"kind": "Sine", "inputs": [c(f as f32), c(0.0)]},
                {"kind": "PitchShift", "inputs": [{"ugen": 0}, c(0.0471), c(ratio), c(0.0), c(0.0)]}
            ]),
            24_000,
            &[],
        );
        let tail = &y[4800..];
        let scan = (10..400).map(|k| k as f64 * 5.0);
        let peak = scan
            .max_by(|a, b| power(tail, *a).total_cmp(&power(tail, *b)))
            .unwrap();
        // A head starts every quarter window, 2260 / 4 samples.
        let rate = SR as f64 / 565.0;
        assert!(
            (peak - want).abs() < rate / 2.0,
            "ratio {ratio}: peak at {peak} Hz"
        );
        let (p, q) = (power(tail, peak), power(tail, not));
        assert!(
            q < p / 100.0,
            "ratio {ratio}: the sine is left, {q} against {p}"
        );
    }
}

/// A tap reads the writer's input exactly a whole delay behind, every
/// interpolation alike, across the buffer's wrap. The delay is 375 samples,
/// `2^-7` seconds, which a float holds exactly.
#[test]
fn a_tap_reads_its_writer_a_delay_behind() {
    let d = 375;
    let buf = vec![Some(Arc::new(Buffer::zeroed(500, 1, SR as f64)))];
    let x = render(json!([{"kind": "WhiteNoise", "inputs": []}]), 2048, &[]);
    for interp in [1.0, 2.0, 4.0] {
        let y = render(
            json!([
                {"kind": "WhiteNoise", "inputs": []},
                {"kind": "DelTapWr", "inputs": [c(0.0), {"ugen": 0}]},
                {"kind": "DelTapRd", "inputs": [c(0.0), {"ugen": 1}, c(0.007_812_5), c(interp)]}
            ]),
            2048,
            &buf,
        );
        assert!(y[..d].iter().all(|&v| v == 0.0), "silence before the delay");
        for i in d..2048 {
            assert_eq!(y[i], x[i - d], "{interp} at {i}");
        }
    }
}

/// A synth's buffers are numbered after the pool's, in def order; a reader
/// before its `SetBuf` in the def still reads what `SetBuf` wrote, from the
/// first sample.
#[test]
fn local_buffers_follow_the_pool_and_set_buf_runs_first() {
    let pool = vec![None; NUM_BUFFERS];
    let y = render(
        json!([
            {"kind": "LocalBuf", "rate": "ir", "inputs": [c(1.0), c(4.0)]},
            {"kind": "LocalBuf", "rate": "ir", "inputs": [c(1.0), c(4.0)]},
            {"kind": "Index", "inputs": [{"ugen": 1}, c(2.0)]},
            {"kind": "SetBuf", "rate": "ir", "inputs": [{"ugen": 1}, c(0.0), c(4.0), c(5.0), c(6.0), c(7.0), c(8.0)]},
            {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"ugen": 2}, c(1.0)]}
        ]),
        64,
        &pool,
    );
    assert!(y.iter().all(|&v| v == 7.0), "{}", y[0]);
    let n = render(
        json!([
            {"kind": "LocalBuf", "rate": "ir", "inputs": [c(1.0), c(4.0)]},
            {"kind": "LocalBuf", "rate": "ir", "inputs": [c(1.0), c(4.0)]}
        ]),
        1,
        &pool,
    );
    assert_eq!(n[0], NUM_BUFFERS as f32 + 1.0);
}

/// A recording into a local buffer plays back; a reset clears it and its
/// `SetBuf` writes it again.
#[test]
fn a_local_buffer_records_and_a_reset_starts_it_again() {
    let def = json!({"name": "t", "ugens": [
        {"kind": "LocalBuf", "rate": "ir", "inputs": [c(1.0), c(256.0)]},
        {"kind": "SetBuf", "rate": "ir", "inputs": [{"ugen": 0}, c(200.0), c(1.0), c(0.25)]},
        {"kind": "Phasor", "inputs": [c(0.0), c(1.0), c(0.0), c(256.0), c(0.0)]},
        {"kind": "BufRd", "inputs": [{"ugen": 0}, c(0.0), c(200.0), c(0.0), c(1.0)]},
        {"kind": "DC", "inputs": [c(0.5)]},
        {"kind": "BufWr", "inputs": [{"ugen": 0}, c(0.0), {"ugen": 2}, c(0.0), {"ugen": 4}]},
        {"kind": "Out", "inputs": [c(0.0), {"ugen": 3}]}
    ]})
    .to_string();
    let run = Run {
        reset_at: Some(6 * BLOCK_SIZE),
        ..Run::new(8 * BLOCK_SIZE)
    };
    let y = render_def(&def, &run).remove(0);
    // Frame 200 holds `SetBuf`'s value until the recording reaches it, in
    // the block after the one that writes it (the reader runs first).
    assert_eq!(y[0], 0.25);
    assert_eq!(y[4 * BLOCK_SIZE], 0.5, "recorded");
    assert_eq!(y[6 * BLOCK_SIZE], 0.25, "cleared and set again");
}

/// `ClearBuf` zeroes a pool buffer at the synth's start.
#[test]
fn clear_buf_zeroes_a_buffer() {
    let buf = vec![Some(Arc::new(Buffer::new(vec![3.0; 8], 1, 8, SR as f64)))];
    let y = render(
        json!([
            {"kind": "ClearBuf", "rate": "ir", "inputs": [c(0.0)]},
            {"kind": "Index", "inputs": [c(0.0), c(5.0)]}
        ]),
        64,
        &buf,
    );
    assert!(y.iter().all(|&v| v == 0.0));
}

/// `ScopeOut` writes its signals round the buffer's channels, the latest
/// frames last.
#[test]
fn scope_out_writes_round_its_buffer() {
    let buf = Arc::new(Buffer::zeroed(100, 2, SR as f64));
    let pool = vec![Some(Arc::clone(&buf))];
    render(
        json!([
            {"kind": "Phasor", "inputs": [c(0.0), c(1.0), c(0.0), c(1e6), c(0.0)]},
            {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"ugen": 0}, c(-1.0)]},
            {"kind": "ScopeOut", "inputs": [c(0.0), {"ugen": 0}, {"ugen": 1}]}
        ]),
        256,
        &pool,
    );
    // 256 frames round 100: frame k last held sample 200 + k, below 56.
    for k in 0..100 {
        let t = if k < 56 { 200 + k } else { 100 + k } as f32;
        assert_eq!(buf.sample(k, 0), t, "{k}");
        assert_eq!(buf.sample(k, 1), -t, "{k}");
    }
}

/// A float WAV of a ramp, frame `k` holding `k / 1000`, in the system's
/// temporary directory.
fn ramp_wav(frames: usize) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("clausters-vdisk-{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for k in 0..frames {
        w.write_sample(k as f32 / 1000.0).unwrap();
    }
    w.finalize().unwrap();
    path
}

/// `VDiskIn` at half speed plays frame `k` at sample `2 k` and the midpoint
/// between, and at a rate of 3 every third frame.
#[test]
fn vdisk_in_reads_at_its_rate() {
    let path = ramp_wav(4096);
    for (rate, step) in [(0.5f32, 0.5f32), (1.0, 1.0), (3.0, 3.0)] {
        let spec: SynthDefSpec = serde_json::from_value(json!({"name": "v", "ugens": [
            {"kind": "VDiskIn", "inputs": [c(0.0), c(rate)], "path": path.to_str().unwrap()},
            {"kind": "Out", "inputs": [c(0.0), {"ugen": 0}]}
        ]}))
        .unwrap();
        let mut synth = UGenSynth::built(
            Arc::new(compile(spec).unwrap()),
            SR,
            &Generators::seeded(SEED_STRIDE),
            Meters::detached(),
            true,
            0.0,
        );
        let mut buses = Buses::new(ControlBuses::new(16), 8);
        let mut y = Vec::new();
        for _ in 0..8 {
            buses.clear_audio();
            let mut ctx = ProcessCtx {
                sample_rate: SR,
                full_sample_rate: SR,
                buses: &buses,
                buffers: &[],
                local_buffers: &[],
                offset: 0,
                frames: BLOCK_SIZE,
                transport: Default::default(),
                server: clausters::dsp::ServerFacts::default(),
            };
            synth.process(&mut ctx);
            y.extend_from_slice(&buses.audio(0)[..BLOCK_SIZE]);
        }
        // From the second frame on: the frame before the first is silence,
        // not the ramp's.
        for (i, &v) in y.iter().enumerate().skip(2) {
            let want = i as f32 * step / 1000.0;
            assert!(
                (v - want).abs() < 1e-5,
                "rate {rate} at {i}: {v} against {want}"
            );
        }
    }
    let _ = std::fs::remove_file(path);
}
