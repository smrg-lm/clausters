//! S5 tests: the wavetable format, the `/b_gen` generators (`sine1`/`cheby`),
//! and the table oscillators (`Osc`, `Shaper`) reading them through the engine.

use std::f32::consts::TAU;
use std::sync::Arc;

use clausters::dsp::buffer::Buffer;
use clausters::dsp::wavetable::{GenCommand, GenFlags, signal_to_wavetable, wt_interp};
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use serde_json::json;

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

fn wt(normalize: bool, wavetable: bool, clear: bool) -> GenFlags {
    GenFlags {
        normalize,
        wavetable,
        clear,
    }
}

/// Reads a wavetable at a fractional point (`points` = `table.len()/2`), the
/// way `Osc` does, for the pure-function tests.
fn read(table: &[f32], point: f64) -> f32 {
    let points = table.len() / 2;
    let k = (point as usize).min(points - 1);
    wt_interp(table, k, (point - k as f64) as f32)
}

#[test]
fn wavetable_read_reconstructs_a_sine() {
    // One period of a sine, sampled at N points.
    let n = 512;
    let signal: Vec<f32> = (0..n).map(|i| (TAU * i as f32 / n as f32).sin()).collect();
    let table = signal_to_wavetable(&signal, true);
    assert_eq!(table.len(), 2 * n);

    // Reading at each integer point returns the sample exactly...
    for (i, &s) in signal.iter().enumerate() {
        assert!((read(&table, i as f64) - s).abs() < 1e-6, "point {i}");
    }
    // ...and halfway between points stays on the sine within interpolation error.
    for i in 0..n {
        let x = i as f64 + 0.5;
        let expect = (TAU * x as f32 / n as f32).sin();
        assert!((read(&table, x) - expect).abs() < 1e-3, "midpoint {i}");
    }
}

#[test]
fn b_gen_sine1_builds_a_wavetable_sine() {
    // A 1024-sample buffer holds a 512-point wavetable.
    let buf = Buffer::zeroed(1024, 1, SR as f64);
    let cmd = GenCommand::Sine1 {
        flags: wt(true, true, true),
        amps: vec![1.0], // fundamental only
    };
    let out = cmd.apply(&buf);
    assert_eq!(out.data().len(), 1024);
    let table = out.data();
    let n = table.len() / 2;
    for i in 0..n {
        let expect = (TAU * i as f32 / n as f32).sin();
        assert!((read(table, i as f64) - expect).abs() < 1e-3, "point {i}");
    }
}

#[test]
fn b_gen_cheby_builds_the_transfer_curve() {
    // T_2(x) = 2x^2 - 1, no wavetable format so we read the raw samples.
    let n = 256;
    let buf = Buffer::zeroed(n, 1, SR as f64);
    let cmd = GenCommand::Cheby {
        flags: wt(false, false, true),
        coeffs: vec![0.0, 1.0], // weight on T_2
    };
    let out = cmd.apply(&buf);
    let data = out.data();
    assert_eq!(data.len(), n);
    for (j, &s) in data.iter().enumerate() {
        let x = 2.0 * j as f32 / (n as f32 - 1.0) - 1.0;
        let expect = 2.0 * x * x - 1.0;
        assert!((s - expect).abs() < 1e-4, "x={x}: {s} != {expect}");
    }
}

#[test]
fn b_gen_copy_overlays_a_source_range() {
    let dst = Buffer::new(vec![0.0; 8], 1, 8, SR as f64);
    let src = Arc::new(Buffer::new(vec![1.0, 2.0, 3.0, 4.0], 1, 4, SR as f64));
    let cmd = GenCommand::Copy {
        dst_start: 2,
        src,
        src_start: 1,
        num: 2,
    };
    let out = cmd.apply(&dst);
    assert_eq!(out.data(), &[0.0, 0.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn b_gen_without_clear_accumulates() {
    // Two sine1 passes without the clear flag sum into the same buffer.
    let base = Buffer::zeroed(256, 1, SR as f64);
    let first = GenCommand::Sine1 {
        flags: wt(false, false, true),
        amps: vec![1.0],
    }
    .apply(&base);
    let second = GenCommand::Sine1 {
        flags: wt(false, false, false), // no clear: add on top
        amps: vec![1.0],
    }
    .apply(&first);
    for (a, b) in first.data().iter().zip(second.data()) {
        assert!((b - 2.0 * a).abs() < 1e-5);
    }
}

// ---- through the engine ----

fn spec_synth(spec: serde_json::Value) -> UGenSynth {
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

#[test]
fn osc_reads_a_wavetable_as_a_sine() {
    let buf = Buffer::zeroed(2048, 1, SR as f64);
    let table = GenCommand::Sine1 {
        flags: wt(true, true, true),
        amps: vec![1.0],
    }
    .apply(&buf);

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(table)),
        })
        .ok()
        .unwrap();
    let freq = 220.0f32;
    let osc = spec_synth(json!({
        "name": "osc",
        "ugens": [
            {"kind": "Osc", "inputs": [{"const": 0.0}, {"const": freq}, {"const": 0.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }));
    handle.send(add_synth(1000, osc)).ok().unwrap();

    let out = render_channel(&mut engine, 8);
    // Reference: the same phase accumulation the oscillator does.
    let mut phase = 0.0f64;
    for (i, &s) in out.iter().enumerate() {
        let expect = (TAU * phase as f32).sin();
        assert!((s - expect).abs() < 2e-3, "sample {i}: {s} != {expect}");
        phase += freq as f64 / SR as f64;
        phase = phase.fract();
    }
}

#[test]
fn shaper_passes_through_with_a_linear_transfer() {
    // cheby [1] -> T_1(x) = x, so the waveshaper is the identity on [-1, 1].
    let buf = Buffer::zeroed(1024, 1, SR as f64);
    let table = GenCommand::Cheby {
        flags: wt(false, true, true),
        coeffs: vec![1.0],
    }
    .apply(&buf);

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(table)),
        })
        .ok()
        .unwrap();
    // Shape a slow sine (well inside [-1, 1]) and expect it back unchanged.
    let shaper = spec_synth(json!({
        "name": "shaper",
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 110.0}]},
            {"kind": "Shaper", "inputs": [{"const": 0.0}, {"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }));
    handle.send(add_synth(1000, shaper)).ok().unwrap();

    let out = render_channel(&mut engine, 8);
    let mut phase = 0.0f64;
    for (i, &s) in out.iter().enumerate() {
        let expect = phase.sin() as f32;
        assert!((s - expect).abs() < 3e-3, "sample {i}: {s} != {expect}");
        phase += TAU as f64 * 110.0 / SR as f64;
        if phase >= TAU as f64 {
            phase -= TAU as f64;
        }
    }
}
