//! SynthDef format and interpreter tests: JSON parsing, compile-time
//! validation, and the audio produced by interpreted instances.

use std::sync::Arc;

use claudesufa::node::SynthNode;
use claudesufa::server::engine::BLOCK_SIZE;
use claudesufa::synthdef::instance::UGenSynth;
use claudesufa::synthdef::{SynthDefSpec, compile, default_spec};

const SR: f32 = 48_000.0;

fn render(synth: &mut UGenSynth, blocks: usize) -> Vec<f32> {
    let mut block = [0.0f32; BLOCK_SIZE];
    let mut out = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        synth.process(SR, &mut block);
        out.extend_from_slice(&block);
    }
    out
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn estimated_freq(buf: &[f32]) -> f32 {
    let crossings = buf
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    crossings as f32 * SR / buf.len() as f32
}

fn spec_from_json(json: &str) -> SynthDefSpec {
    serde_json::from_str(json).unwrap()
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
            {"kind": "SinOsc", "inputs": [{"control": 0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"control": 1}]}
        ],
        "out": 1
    }"#;
    let def = Arc::new(compile(spec_from_json(json)).unwrap());
    assert_eq!(def.control_index("freq"), Some(0));
    assert_eq!(def.control_index("amp"), Some(1));
    assert_eq!(def.control_index("nope"), None);

    let mut synth = UGenSynth::new(def);
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
            {"kind": "SinOsc", "inputs": [{"const": 880.0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.3}]}
        ],
        "out": 1
    }"#;
    let mut synth = UGenSynth::new(Arc::new(compile(spec_from_json(json)).unwrap()));
    let out = render(&mut synth, 750);
    assert!((estimated_freq(&out) - 880.0).abs() < 8.0);
}

#[test]
fn add_mixes_two_oscillators() {
    // two in-phase sines at the same freq added: amplitude doubles
    let json = r#"{
        "name": "two",
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 440.0}]},
            {"kind": "SinOsc", "inputs": [{"const": 440.0}]},
            {"kind": "Add",    "inputs": [{"ugen": 0}, {"ugen": 1}]},
            {"kind": "Mul",    "inputs": [{"ugen": 2}, {"const": 0.1}]}
        ],
        "out": 3
    }"#;
    let mut synth = UGenSynth::new(Arc::new(compile(spec_from_json(json)).unwrap()));
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
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]}
        ],
        "out": 1
    }"#;
    let mut synth = UGenSynth::new(Arc::new(compile(spec_from_json(json)).unwrap()));
    let out = render(&mut synth, 200);
    assert!(out.iter().all(|x| x.is_finite()));
    // uniform noise in [-0.5, 0.5]: RMS ≈ 0.5/√3 ≈ 0.289
    let r = rms(&out);
    assert!((0.2..0.4).contains(&r), "rms = {r}");
}

#[test]
fn audible_modulation_fm_style() {
    // SinOsc modulating another's frequency: freq = 440 + mod*100
    let json = r#"{
        "name": "vibrato",
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 5.0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 100.0}]},
            {"kind": "Add",    "inputs": [{"ugen": 1}, {"const": 440.0}]},
            {"kind": "SinOsc", "inputs": [{"ugen": 2}]},
            {"kind": "Mul",    "inputs": [{"ugen": 3}, {"const": 0.2}]}
        ],
        "out": 4
    }"#;
    let mut synth = UGenSynth::new(Arc::new(compile(spec_from_json(json)).unwrap()));
    let out = render(&mut synth, 1500); // 2 s
    assert!(out.iter().all(|x| x.is_finite()));
    // average frequency stays around the carrier
    let freq = estimated_freq(&out);
    assert!((freq - 440.0).abs() < 15.0, "estimated freq = {freq}");
}

// ---- compile-time validation ----

#[test]
fn default_spec_compiles() {
    assert!(compile(default_spec()).is_ok());
}

#[test]
fn rejects_unknown_kind() {
    let json = r#"{"name":"x","ugens":[{"kind":"Nope","inputs":[]}],"out":0}"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("unknown kind"), "{err}");
}

#[test]
fn rejects_forward_wire_reference() {
    let json = r#"{
        "name": "x",
        "ugens": [
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"const": 1.0}]},
            {"kind": "SinOsc", "inputs": [{"const": 440.0}]}
        ],
        "out": 0
    }"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("earlier"), "{err}");
}

#[test]
fn rejects_bad_control_index() {
    let json = r#"{"name":"x","ugens":[{"kind":"SinOsc","inputs":[{"control":3}]}],"out":0}"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("out of range"), "{err}");
}

#[test]
fn rejects_wrong_arity() {
    let json = r#"{"name":"x","ugens":[{"kind":"SinOsc","inputs":[]}],"out":0}"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("expected 1 inputs"), "{err}");
}

#[test]
fn rejects_out_of_range_output() {
    let json = r#"{"name":"x","ugens":[{"kind":"SinOsc","inputs":[{"const":1.0}]}],"out":5}"#;
    let err = compile(spec_from_json(json)).unwrap_err();
    assert!(err.contains("out index"), "{err}");
}

#[test]
fn rejects_empty_def() {
    let json = r#"{"name":"x","ugens":[],"out":0}"#;
    assert!(compile(spec_from_json(json)).is_err());
}
