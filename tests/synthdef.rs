//! SynthDef format and interpreter tests: JSON parsing, compile-time
//! validation, and the audio produced by interpreted instances.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile, default_spec};

const SR: f32 = 48_000.0;

/// Renders `blocks` blocks into fresh buses and returns audio bus 0 — defs
/// under test write there through an `Out` UGen.
fn render(synth: &mut UGenSynth, blocks: usize) -> Vec<f32> {
    render_with(std::slice::from_mut(synth), blocks, |_| {})
}

/// Several synths in order over shared buses, with a pre-process hook for
/// setting control buses.
fn render_with(synths: &mut [UGenSynth], blocks: usize, setup: impl Fn(&ControlBuses)) -> Vec<f32> {
    let mut buses = Buses::new(ControlBuses::new(1024), 128);
    setup(&buses.control);
    let mut out = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            buses: &mut buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
        };
        for synth in synths.iter_mut() {
            synth.process(&mut ctx);
        }
        out.extend_from_slice(buses.audio(0));
    }
    out
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn estimated_freq(buf: &[f32]) -> f32 {
    let crossings = buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f32 * SR / buf.len() as f32
}

fn spec_from_json(json: &str) -> SynthDefSpec {
    serde_json::from_str(json).unwrap()
}

fn synth_from_json(json: &str) -> UGenSynth {
    UGenSynth::new(Arc::new(compile(spec_from_json(json)).unwrap()), SR)
}

#[test]
fn json_def_compiles_and_plays() {
    let json = r#"{
        "name": "beep",
        "controls": [
            {"name": "freq", "default": 330.0},
            {"name": "amp",  "default": 0.5}
        ],
        "ugens": [
            {"kind": "Sine", "inputs": [{"control": 0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"control": 1}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    let def = Arc::new(compile(spec_from_json(json)).unwrap());
    assert_eq!(def.control_index("freq"), Some(0));
    assert_eq!(def.control_index("amp"), Some(1));
    assert_eq!(def.control_index("nope"), None);

    let mut synth = UGenSynth::new(def, SR);
    let out = render(&mut synth, 750);
    assert!(out.iter().all(|x| x.is_finite()));
    assert!((estimated_freq(&out) - 330.0).abs() < 5.0);
    assert!((rms(&out) - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.005);
}

#[test]
fn const_input_works() {
    let json = r#"{
        "name": "fixed",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 880.0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.3}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    let mut synth = synth_from_json(json);
    let out = render(&mut synth, 750);
    assert!((estimated_freq(&out) - 880.0).abs() < 8.0);
}

#[test]
fn add_mixes_two_oscillators() {
    // two in-phase sines at the same freq added: amplitude doubles
    let json = r#"{
        "name": "two",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Add",    "inputs": [{"ugen": 0}, {"ugen": 1}]},
            {"kind": "Mul",    "inputs": [{"ugen": 2}, {"const": 0.1}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 3}]}
        ]
    }"#;
    let mut synth = synth_from_json(json);
    let out = render(&mut synth, 750);
    let expected = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!((rms(&out) - expected).abs() < 0.002);
}

#[test]
fn white_noise_is_loud_and_finite() {
    let json = r#"{
        "name": "noise",
        "ugens": [
            {"kind": "WhiteNoise", "inputs": []},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    let mut synth = synth_from_json(json);
    let out = render(&mut synth, 200);
    assert!(out.iter().all(|x| x.is_finite()));
    // uniform noise in [-0.5, 0.5]: RMS ≈ 0.5/√3 ≈ 0.289
    let r = rms(&out);
    assert!((0.2..0.4).contains(&r), "rms = {r}");
}

#[test]
fn audible_modulation_fm_style() {
    // Sine modulating another's frequency: freq = 440 + mod*100
    let json = r#"{
        "name": "vibrato",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 5.0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 100.0}]},
            {"kind": "Add",    "inputs": [{"ugen": 1}, {"const": 440.0}]},
            {"kind": "Sine", "inputs": [{"ugen": 2}]},
            {"kind": "Mul",    "inputs": [{"ugen": 3}, {"const": 0.2}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 4}]}
        ]
    }"#;
    let mut synth = synth_from_json(json);
    let out = render(&mut synth, 1500); // 2 s
    assert!(out.iter().all(|x| x.is_finite()));
    // average frequency stays around the carrier
    let freq = estimated_freq(&out);
    assert!((freq - 440.0).abs() < 15.0, "estimated freq = {freq}");
}

// ---- bus I/O semantics ----

#[test]
fn out_sums_but_replaceout_overwrites() {
    let dc = |value: f32, kind: &str| {
        synth_from_json(&format!(
            r#"{{
                "name": "dc",
                "ugens": [{{"kind": "{kind}", "inputs": [{{"const": 0.0}}, {{"const": {value}}}]}}]
            }}"#
        ))
    };
    // two Outs sum: 0.25 + 0.25 = 0.5
    let mut synths = [dc(0.25, "Out"), dc(0.25, "Out")];
    let out = render_with(&mut synths, 4, |_| {});
    assert!(out.iter().all(|&x| (x - 0.5).abs() < 1e-6));

    // a ReplaceOut after an Out wins the bus
    let mut synths = [dc(0.25, "Out"), dc(0.125, "ReplaceOut")];
    let out = render_with(&mut synths, 4, |_| {});
    assert!(out.iter().all(|&x| (x - 0.125).abs() < 1e-6));
}

#[test]
fn in_reads_an_audio_bus() {
    // writer puts DC 0.2 on bus 4; reader copies bus 4 * 2 to bus 0
    let writer = r#"{
        "name": "writer",
        "ugens": [{"kind": "Out", "inputs": [{"const": 4.0}, {"const": 0.2}]}]
    }"#;
    let reader = r#"{
        "name": "reader",
        "ugens": [
            {"kind": "In",  "inputs": [{"const": 4.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 2.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    let mut synths = [synth_from_json(writer), synth_from_json(reader)];
    let out = render_with(&mut synths, 4, |_| {});
    assert!(out.iter().all(|&x| (x - 0.4).abs() < 1e-6));
}

#[test]
fn inctl_reads_a_control_bus() {
    let json = r#"{
        "name": "ctl",
        "ugens": [
            {"kind": "InCtl", "inputs": [{"const": 3.0}]},
            {"kind": "Out",   "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    let mut synth = synth_from_json(json);
    let out = render_with(std::slice::from_mut(&mut synth), 4, |ctl| {
        ctl.set(3, 0.75);
    });
    assert!(out.iter().all(|&x| (x - 0.75).abs() < 1e-6));
}

#[test]
fn def_without_out_is_silent() {
    let json = r#"{
        "name": "mute",
        "ugens": [{"kind": "Sine", "inputs": [{"const": 440.0}]}]
    }"#;
    let mut synth = synth_from_json(json);
    let out = render(&mut synth, 100);
    assert!(rms(&out) < 1e-9);
}

// ---- compile-time validation ----

#[test]
fn default_spec_compiles() {
    assert!(compile(default_spec()).is_ok());
}

#[test]
fn rejects_unknown_kind() {
    let json = r#"{"name":"x","ugens":[{"kind":"Nope","inputs":[]}]}"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("unknown kind"), "{err}");
}

#[test]
fn rejects_forward_wire_reference() {
    let json = r#"{
        "name": "x",
        "ugens": [
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"const": 1.0}]},
            {"kind": "Sine", "inputs": [{"const": 440.0}]}
        ]
    }"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("earlier"), "{err}");
}

#[test]
fn rejects_bad_control_index() {
    let json = r#"{"name":"x","ugens":[{"kind":"Sine","inputs":[{"control":3}]}]}"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("out of range"), "{err}");
}

#[test]
fn rejects_wrong_arity() {
    let json = r#"{"name":"x","ugens":[{"kind":"Sine","inputs":[]}]}"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("expected 1 inputs"), "{err}");
}

#[test]
fn rejects_empty_def() {
    let json = r#"{"name":"x","ugens":[]}"#;
    assert!(compile(spec_from_json(json)).is_err());
}
