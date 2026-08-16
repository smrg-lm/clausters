//! Typed-control tests (S2): trigger (`tr`) controls fire for one block,
//! lagged controls smooth a step, scalar (`ir`) controls freeze under `/node_set`,
//! plus the compiler validation for control types and lag.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};

const SR: f32 = 48_000.0;

fn make(json: &str) -> UGenSynth {
    let spec: SynthDefSpec = serde_json::from_str(json).unwrap();
    UGenSynth::new(Arc::new(compile(spec).unwrap()), SR, SEED_STRIDE)
}

fn compile_err(json: &str) -> String {
    compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap_err()
}

/// Processes one full block and returns audio bus 0.
fn step(synth: &mut UGenSynth, buses: &mut Buses) -> Vec<f32> {
    buses.clear_audio();
    let mut ctx = ProcessCtx {
        sample_rate: SR,
        full_sample_rate: SR,
        buses,
        buffers: &[],
        offset: 0,
        frames: BLOCK_SIZE,
        transport: Default::default(),
    };
    synth.process(&mut ctx);
    buses.audio(0).to_vec()
}

fn fresh_buses() -> Buses {
    Buses::new(ControlBuses::new(16), 8)
}

// ---- trigger control (tr) ----

#[test]
fn trigger_control_fires_exactly_one_block() {
    // The control is written straight to bus 0, so the output is its value.
    let mut synth = make(
        r#"{
            "name": "trig",
            "controls": [{"name": "t", "default": 0.0, "rate": "tr"}],
            "ugens": [{"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}]
        }"#,
    );
    let mut buses = fresh_buses();

    // Block 0: default 0.
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x == 0.0));
    // /node_set t = 1, then one block: the trigger is live.
    synth.set_control(0, 1.0);
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x == 1.0));
    // Next block with no set: the engine has reset it to 0.
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x == 0.0));
    // And it stays 0.
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x == 0.0));
}

// ---- scalar control (ir) ----

#[test]
fn scalar_control_freezes_under_n_set() {
    let mut synth = make(
        r#"{
            "name": "scal",
            "controls": [{"name": "x", "default": 3.0, "rate": "ir"}],
            "ugens": [{"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}]
        }"#,
    );
    let mut buses = fresh_buses();

    // An init-time set (before the first block, as /synth_new does) still takes.
    synth.set_control(0, 5.0);
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x == 5.0));
    // A /node_set after the synth has run is ignored — the value is frozen.
    synth.set_control(0, 9.0);
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x == 5.0));
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x == 5.0));
}

#[test]
fn ir_control_may_feed_an_ir_ugen_but_kr_may_not() {
    // Rand.ir requires ir inputs; an ir control now qualifies (S2 pairs it
    // with S1's ir rate), while a plain kr control does not.
    let ok = r#"{
        "name": "ok",
        "controls": [{"name": "lo", "default": 0.0, "rate": "ir"}],
        "ugens": [
            {"kind": "Rand", "inputs": [{"control": 0}, {"const": 1.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert!(compile(serde_json::from_str::<SynthDefSpec>(ok).unwrap()).is_ok());

    let bad = r#"{
        "name": "bad",
        "controls": [{"name": "lo", "default": 0.0}],
        "ugens": [{"kind": "Rand", "inputs": [{"control": 0}, {"const": 1.0}]}]
    }"#;
    assert!(compile_err(bad).contains("requires ir inputs"));
}

// ---- lagged control (lag) ----

#[test]
fn lag_control_smooths_a_step() {
    let mut synth = make(
        r#"{
            "name": "lagd",
            "controls": [{"name": "f", "default": 0.0, "lag": 0.1}],
            "ugens": [{"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}]
        }"#,
    );
    let mut buses = fresh_buses();

    // Primed at 0.
    assert!(step(&mut synth, &mut buses).iter().all(|&x| x.abs() < 1e-6));
    // Step the control to 1.0: the output must *glide*, not jump — the first
    // block after the step is well below the target.
    synth.set_control(0, 1.0);
    let b1 = step(&mut synth, &mut buses);
    assert!(
        b1[0] > 0.0 && b1[0] < b1[BLOCK_SIZE - 1],
        "must rise within block"
    );
    assert!(
        *b1.last().unwrap() < 0.5,
        "0.1s lag is far from the target after ~1.3ms"
    );
    // Monotonic non-decreasing across the whole glide.
    let mut last = *b1.last().unwrap();
    let mut peak = last;
    for _ in 0..200 {
        let b = step(&mut synth, &mut buses);
        assert!(b[0] >= last - 1e-6, "glide should not go backwards");
        last = *b.last().unwrap();
        peak = last;
    }
    // After ~0.27s (> the 0.1s lag) it has essentially reached the target.
    assert!(peak > 0.99, "should converge to the target, got {peak}");
}

#[test]
fn varlag_rises_and_falls_at_different_rates() {
    // Fast up (0.001s), slow down (0.2s): a step up reaches the target quickly,
    // a step down decays slowly.
    let mut synth = make(
        r#"{
            "name": "vlag",
            "controls": [{"name": "f", "default": 0.0, "lag": 0.001, "lag_down": 0.2}],
            "ugens": [{"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}]
        }"#,
    );
    let mut buses = fresh_buses();
    step(&mut synth, &mut buses); // prime at 0

    synth.set_control(0, 1.0);
    let mut up = 0.0f32;
    for _ in 0..20 {
        up = *step(&mut synth, &mut buses).last().unwrap();
    }
    assert!(up > 0.99, "fast up should reach the target, got {up}");

    synth.set_control(0, 0.0);
    let down = *step(&mut synth, &mut buses).last().unwrap();
    assert!(
        down > 0.9,
        "slow down barely moves in one block, got {down}"
    );
}

// ---- compiler validation ----

#[test]
fn rejects_unknown_control_type() {
    let json = r#"{"name":"x","controls":[{"name":"c","default":0.0,"rate":"xr"}],
        "ugens":[{"kind":"Out","inputs":[{"const":0.0},{"control":0}]}]}"#;
    assert!(compile_err(json).contains("unknown control type"));
}

#[test]
fn rejects_lag_on_non_kr_control() {
    let json = r#"{"name":"x","controls":[{"name":"c","default":0.0,"rate":"tr","lag":0.1}],
        "ugens":[{"kind":"Out","inputs":[{"const":0.0},{"control":0}]}]}"#;
    assert!(compile_err(json).contains("lag is only valid on a kr"));
}

#[test]
fn rejects_lag_down_without_lag() {
    let json = r#"{"name":"x","controls":[{"name":"c","default":0.0,"lag_down":0.1}],
        "ugens":[{"kind":"Out","inputs":[{"const":0.0},{"control":0}]}]}"#;
    assert!(compile_err(json).contains("lag_down requires lag"));
}
