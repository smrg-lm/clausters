//! Operator-UGen tests (S3): the generic `BinaryOpUGen`/`UnaryOpUGen` selected
//! by a core opcode index, the fused `MulAdd`/`Sum3`/`Sum4`, and the compiler's
//! `op`-index validation. The bit-for-bit agreement with `clausters_core` over
//! the whole opcode table lives in `tests/core_parity.rs`; here we drive the
//! full compile+render path and the wire-format validation.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};

const SR: f32 = 48_000.0;

/// Renders one block of audio bus 0 and returns its constant value, asserting
/// the block really is constant (these defs feed constants, so it must be).
fn render_const(json: &str) -> f32 {
    let def = compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    buses.clear_audio();
    let mut ctx = ProcessCtx {
        sample_rate: SR,
        buses: &buses,
        buffers: &[],
        offset: 0,
        frames: BLOCK_SIZE,
    };
    synth.process(&mut ctx);
    let blk = buses.audio(0);
    assert!(
        blk.iter().all(|&x| x == blk[0]),
        "expected a constant block, got {blk:?}"
    );
    blk[0]
}

fn compile_err(json: &str) -> String {
    compile(serde_json::from_str::<SynthDefSpec>(json).unwrap()).unwrap_err()
}

// ---- BinaryOpUGen / UnaryOpUGen by opcode index ----

#[test]
fn binary_op_ugen_multiplies_by_name() {
    // op "mul": 3 * 4 = 12.
    let json = r#"{
        "name": "b",
        "ugens": [
            {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"const": 3.0}, {"const": 4.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert_eq!(render_const(json), 12.0);
}

#[test]
fn binary_op_ugen_extended_op_clips() {
    // op "clip2": clip 5 to [-1, 1] = 1.
    let json = r#"{
        "name": "c",
        "ugens": [
            {"kind": "BinaryOpUGen", "op": "clip2", "inputs": [{"const": 5.0}, {"const": 1.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert_eq!(render_const(json), 1.0);
}

#[test]
fn unary_op_ugen_midicps_by_name() {
    // op "midicps": note 69 -> 440 Hz.
    let json = r#"{
        "name": "u",
        "ugens": [
            {"kind": "UnaryOpUGen", "op": "midicps", "inputs": [{"const": 69.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert!((render_const(json) - 440.0).abs() < 1e-2);
}

// ---- fused forms ----

#[test]
fn mul_add_computes_a_times_b_plus_c() {
    let json = r#"{
        "name": "ma",
        "ugens": [
            {"kind": "MulAdd", "inputs": [{"const": 2.0}, {"const": 3.0}, {"const": 1.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert_eq!(render_const(json), 7.0);
}

#[test]
fn sum3_and_sum4_add_their_inputs() {
    let s3 = r#"{
        "name": "s3",
        "ugens": [
            {"kind": "Sum3", "inputs": [{"const": 1.0}, {"const": 2.0}, {"const": 3.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert_eq!(render_const(s3), 6.0);
    let s4 = r#"{
        "name": "s4",
        "ugens": [
            {"kind": "Sum4",
             "inputs": [{"const": 1.0}, {"const": 2.0}, {"const": 3.0}, {"const": 4.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert_eq!(render_const(s4), 10.0);
}

// ---- the Add/Sub/Mul/Div aliases still work (back-compat) ----

#[test]
fn alias_kinds_still_compile_and_run() {
    let json = r#"{
        "name": "alias",
        "ugens": [
            {"kind": "Mul", "inputs": [{"const": 6.0}, {"const": 7.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }"#;
    assert_eq!(render_const(json), 42.0);
}

// ---- compiler op-index validation ----

#[test]
fn rejects_missing_op_name() {
    let json = r#"{"name":"x","ugens":[
        {"kind":"BinaryOpUGen","inputs":[{"const":1.0},{"const":2.0}]}
    ]}"#;
    assert!(compile_err(json).contains("requires an 'op'"));
}

#[test]
fn rejects_unknown_op_name() {
    let json = r#"{"name":"x","ugens":[
        {"kind":"UnaryOpUGen","op":"bogus","inputs":[{"const":1.0}]}
    ]}"#;
    assert!(compile_err(json).contains("unknown"));
}

#[test]
fn rejects_wrong_arity_for_op_ugen() {
    // UnaryOpUGen takes exactly one input.
    let json = r#"{"name":"x","ugens":[
        {"kind":"UnaryOpUGen","op":"neg","inputs":[{"const":1.0},{"const":2.0}]}
    ]}"#;
    assert!(compile_err(json).contains("expected 1 inputs"));
}

/// The operators deferred when S3 landed (U0): they must resolve by name
/// through the real compile+render path, not only in the core's unit tests.
#[test]
fn deferred_s3_operators_resolve_and_compute() {
    let op = |name: &str, a: f32, b: f32| {
        let json = format!(
            r#"{{
                "name": "d",
                "ugens": [
                    {{"kind": "BinaryOpUGen", "op": "{name}",
                     "inputs": [{{"const": {a}}}, {{"const": {b}}}]}},
                    {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}
                ]
            }}"#
        );
        render_const(&json)
    };
    assert_eq!(op("fold2", 3.5, 1.0), -0.5);
    assert_eq!(op("wrap2", 3.5, 1.0), -0.5);
    assert_eq!(op("gcd", 48.0, 18.0), 6.0);
    assert_eq!(op("lcm", 4.0, 6.0), 12.0);
    // 3 + 4 - (sqrt(2) - 1) * 3.
    let expected = 7.0 - (std::f64::consts::SQRT_2 - 1.0) as f32 * 3.0;
    assert!((op("hypot_apx", 3.0, 4.0) - expected).abs() < 1e-6);
}
