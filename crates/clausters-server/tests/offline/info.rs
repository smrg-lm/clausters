//! The information UGens and scsynth's other bus readers and writers, held
//! in an engine: the facts each server's configuration sets (which a
//! reference cannot share), `InFeedback` reading a bus written later, `InTrig`
//! firing on a set, `LagIn`'s lag, `XOut`'s crossfade, and the subsample
//! offset of a timed bundle.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use clausters::server::render::{OscScore, RenderConfig, render_to_vec};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use rosc::{OscMessage, OscType};
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;

fn c(v: f32) -> Value {
    json!({ "const": v })
}

fn synth(ugens: Value) -> Box<dyn SynthNode> {
    let spec: SynthDefSpec = serde_json::from_value(json!({"name": "t", "ugens": ugens})).unwrap();
    Box::new(UGenSynth::new(
        Arc::new(compile(spec).unwrap()),
        SR,
        SEED_STRIDE,
    ))
}

fn add(handle: &mut EngineHandle, id: i32, action: AddAction, ugens: Value) {
    handle
        .send(Cmd::AddSynth {
            id,
            target: ROOT_NODE_ID,
            action,
            synth: synth(ugens),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
}

/// `blocks` blocks of both output channels, interleaved.
fn render(engine: &mut Engine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut all = Vec::new();
    for _ in 0..blocks {
        engine.process_block(&mut out);
        all.extend_from_slice(&out);
    }
    all
}

/// A synth writing `kind` (no inputs) to output `bus`.
fn fact(kind: &str, bus: f32) -> Value {
    json!([
        {"kind": kind, "rate": "kr", "inputs": []},
        {"kind": "Out", "inputs": [c(bus), {"ugen": 0}]}
    ])
}

/// The engine's facts, as its configuration and its tree set them.
#[test]
fn the_information_ugens_report_the_engine() {
    for (kind, want) in [
        ("NumOutputBuses", CHANNELS as f32),
        ("NumInputBuses", 0.0),
        ("NumAudioBuses", 1024.0),
        ("NumControlBuses", 16384.0),
        ("NumBuffers", 4096.0),
        ("BlockSize", 64.0),
        ("ControlRate", SR / 64.0),
    ] {
        let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
        add(&mut handle, 1000, AddAction::Tail, fact(kind, 0.0));
        let out = render(&mut engine, 1);
        assert_eq!(out[0], want, "{kind}");
    }
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    add(&mut handle, 1234, AddAction::Tail, fact("NodeID", 0.0));
    add(
        &mut handle,
        1235,
        AddAction::Tail,
        fact("NumRunningSynths", 1.0),
    );
    let out = render(&mut engine, 1);
    assert_eq!(out[0], 1234.0, "NodeID");
    assert_eq!(out[1], 2.0, "NumRunningSynths");
}

/// `InFeedback` at the head reads what a node at the tail wrote the block
/// before, where `In` reads nothing.
#[test]
fn in_feedback_reads_a_bus_written_later() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    // The tail writes a ramp to bus 10 and to output 1.
    add(
        &mut handle,
        1001,
        AddAction::Tail,
        json!([
            {"kind": "Line", "inputs": [c(0.0), c(1.0), c(0.01), c(0.0)]},
            {"kind": "Out", "inputs": [c(10.0), {"ugen": 0}]},
            {"kind": "Out", "inputs": [c(1.0), {"ugen": 0}]}
        ]),
    );
    // The head reads it back on output 0.
    add(
        &mut handle,
        1000,
        AddAction::Head,
        json!([
            {"kind": "InFeedback", "inputs": [c(10.0)]},
            {"kind": "Out", "inputs": [c(0.0), {"ugen": 0}]}
        ]),
    );
    let out = render(&mut engine, 4);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
    assert!(
        left[..BLOCK_SIZE].iter().all(|&x| x == 0.0),
        "nothing before the first write"
    );
    for i in BLOCK_SIZE..4 * BLOCK_SIZE {
        assert_eq!(left[i], right[i - BLOCK_SIZE], "a block late at {i}");
    }
    // `In` in the same place reads the bus before the tail writes it.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    add(
        &mut handle,
        1001,
        AddAction::Tail,
        json!([{"kind": "DC", "inputs": [c(0.5)]}, {"kind": "Out", "inputs": [c(10.0), {"ugen": 0}]}]),
    );
    add(
        &mut handle,
        1000,
        AddAction::Head,
        json!([{"kind": "In", "inputs": [c(10.0)]}, {"kind": "Out", "inputs": [c(0.0), {"ugen": 0}]}]),
    );
    let out = render(&mut engine, 3);
    assert!(out.iter().step_by(2).all(|&x| x == 0.0), "In reads silence");
}

/// `InTrig` fires in the block a bus is set, the same value included, and
/// is 0 otherwise; `LagIn` follows the bus with its lag.
#[test]
fn in_trig_fires_on_a_set_and_lag_in_lags() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    add(
        &mut handle,
        1000,
        AddAction::Tail,
        json!([
            {"kind": "InTrig", "inputs": [c(4.0)]},
            {"kind": "Out", "inputs": [c(0.0), {"ugen": 0}]},
            {"kind": "LagIn", "inputs": [c(4.0), c(0.05)]},
            {"kind": "Out", "inputs": [c(1.0), {"ugen": 2}]}
        ]),
    );
    let block = |engine: &mut Engine| render(engine, 1);
    block(&mut engine);
    let buses = handle.control_buses().clone();
    for set in 0..3 {
        buses.set(4, 0.7);
        let out = block(&mut engine);
        assert_eq!(out[0], 0.7, "fires on set {set}");
        let out = block(&mut engine);
        assert_eq!(out[0], 0.0, "and not in the block after");
    }
    // LagIn: 0.05 s to -60 dB from 0, at the control rate.
    let mut lagged = 0.0;
    for _ in 0..38 {
        lagged = block(&mut engine)[1];
    }
    assert!(lagged > 0.6 && lagged < 0.7, "{lagged}");
}

/// `XOut` crossfades into what the bus holds.
#[test]
fn x_out_crossfades_into_the_bus() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    add(
        &mut handle,
        1000,
        AddAction::Tail,
        json!([{"kind": "DC", "inputs": [c(1.0)]}, {"kind": "Out", "inputs": [c(0.0), {"ugen": 0}]}]),
    );
    add(
        &mut handle,
        1001,
        AddAction::Tail,
        json!([{"kind": "DC", "inputs": [c(-1.0)]}, {"kind": "XOut", "inputs": [c(0.0), c(0.25), {"ugen": 0}]}]),
    );
    let out = render(&mut engine, 1);
    // 1 + 0.25 (-1 - 1).
    assert!(
        out.iter().step_by(2).all(|&x| (x - 0.5).abs() < 1e-6),
        "{}",
        out[0]
    );
}

/// A synth started by a timed bundle reports where in its first sample the
/// bundle fell: scsynth's offset, the time plus a half less its floor.
#[test]
fn subsample_offset_is_the_bundles_fraction() {
    let def = json!({"name": "s", "ugens": [
        {"kind": "SubsampleOffset", "inputs": []},
        {"kind": "Out", "inputs": [c(0.0), {"ugen": 0}]}
    ]});
    let msg = |addr: &str, args: Vec<OscType>| OscMessage {
        addr: addr.into(),
        args,
    };
    let send = msg(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(def.to_string().into_bytes()),
        ],
    );
    // 100.3 samples in: the node starts at sample 100, 0.8 of a sample on.
    let at = 100.3 / SR as f64;
    let new = msg(
        "/synth_new",
        vec![
            OscType::String("s".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    let score = OscScore::new(vec![(0.0, vec![send]), (at, vec![new]), (0.01, vec![])]).unwrap();
    let cfg = RenderConfig {
        sample_rate: SR as f64,
        channels: 1,
        ..RenderConfig::default()
    };
    let out = render_to_vec(&score, &cfg).unwrap().0;
    assert_eq!(out[99], 0.0, "not yet started");
    assert!((out[100] - 0.8).abs() < 1e-4, "{}", out[100]);
}
