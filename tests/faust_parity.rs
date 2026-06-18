//! F4 golden tests: a UGen graph and its Faust equivalent render side by
//! side in the same engine and must agree. Gated behind the `faust` feature:
//! `cargo test --features faust --test faust_parity`.
//!
//! Two levels of "equivalent":
//! - Stateless arithmetic on the same input signal is **bit-exact**: both
//!   sides compute the same f32 ops on the same samples.
//! - Oscillators agree only **within a float tolerance**: our `SinOsc`
//!   accumulates phase in f64 while Faust (`-single`) accumulates in f32, so
//!   the phases drift apart a few ULP per sample. The tolerance is chosen
//!   well below the error a one-sample phase offset would cause, which keeps
//!   the test discriminating (and a shifted-signal assert proves it).

#![cfg(feature = "faust")]

use std::sync::Arc;
use std::time::Duration;

use clausters::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};
use clausters::faust::synth::{FaustDef, FaustSynth};
use clausters::node::{AddAction, Group, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use serde_json::json;

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
const COMPILE_DEADLINE: Duration = Duration::from_secs(10);

fn compile_faust(name: &str, payload: CompilePayload) -> Arc<FaustDef> {
    let compiler = CompilerThread::spawn();
    compiler
        .submit(CompileRequest {
            name: name.into(),
            payload,
            client: Some(clausters::osc::ClientId::Udp(
                "127.0.0.1:1".parse().unwrap(),
            )),
            cache: None,
        })
        .ok()
        .unwrap();
    let result = compiler
        .recv_result_timeout(COMPILE_DEADLINE)
        .expect("compilation must finish");
    Arc::new(result.outcome.expect("def must compile"))
}

fn ugen_synth(spec_json: &str) -> Box<dyn SynthNode> {
    let spec: SynthDefSpec = serde_json::from_str(spec_json).expect("valid spec");
    let def = Arc::new(compile(spec).expect("spec must compile"));
    Box::new(UGenSynth::new(def))
}

fn faust_synth(def: &Arc<FaustDef>, controls: &[(&str, f32)]) -> Box<dyn SynthNode> {
    let mut synth = Box::new(FaustSynth::new(Arc::clone(def), SR).expect("instantiation"));
    for (name, value) in controls {
        let index = def.control_index(name).expect("control must exist");
        synth.set_control(index, *value);
    }
    synth
}

fn add_tail(id: i32, target: i32, synth: Box<dyn SynthNode>) -> Cmd {
    Cmd::AddSynth {
        id,
        target,
        action: AddAction::Tail,
        synth,
        usage: Default::default(),
    }
}

/// Renders `blocks` blocks and returns the deinterleaved hardware channels.
fn render_stereo(engine: &mut Engine, blocks: usize) -> (Vec<f32>, Vec<f32>) {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut left = Vec::with_capacity(blocks * BLOCK_SIZE);
    let mut right = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        left.extend(out.iter().step_by(CHANNELS).copied());
        right.extend(out.iter().skip(1).step_by(CHANNELS).copied());
    }
    (left, right)
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// `SinOsc(440) * 0.2` as a UGen def, summing into the given bus.
fn sine_spec(name: &str, bus: f32) -> String {
    json!({
        "name": name,
        "ugens": [
            {"kind": "SinOsc", "inputs": [{"const": 440.0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.2}]},
            {"kind": "Out",    "inputs": [{"const": bus}, {"ugen": 1}]}
        ]
    })
    .to_string()
}

/// The same graph through the JSON→Box schema, phase-aligned with `SinOsc`:
/// our oscillator emits `sin(2π·n·f/SR)` starting at 0, while the raw Faust
/// phasor `(+(f/SR) : wrap) ~ _` starts at `f/SR` — the 1-sample `delay`
/// (init 0) realigns it.
fn sine_box_json() -> String {
    let wrap = json!({"op": "split", "in": [
        "_", {"op": "sub", "in": ["_", {"op": "floor", "in": ["_"]}]}
    ]});
    let phasor = json!({"op": "rec", "in": [
        {"op": "seq", "in": [
            {"op": "add", "in": ["_", {"op": "div", "in": [
                {"op": "hslider", "label": "freq",
                 "init": 440.0, "min": 20.0, "max": 20000.0, "step": 0.01},
                f64::from(SR)
            ]}]},
            wrap
        ]},
        "_"
    ]});
    json!({"op": "mul", "in": [
        {"op": "sin", "in": [{"op": "mul", "in": [
            std::f64::consts::TAU,
            {"op": "delay", "in": [phasor, 1]}
        ]}]},
        0.2
    ]})
    .to_string()
}

/// The same `sin(2π·phasor)·0.2`, but via the **Signal API**: the phasor is
/// `recursion(sub(add(self, freq/SR), floor(add(self, freq/SR))))` — explicit
/// `self`/`recursion` feedback instead of the box `~`.
fn sine_signal_json() -> String {
    let freq = || {
        json!({"op": "hslider", "label": "freq",
               "init": 440.0, "min": 20.0, "max": 20000.0, "step": 0.01})
    };
    let acc = || {
        json!({"op": "add", "in": [
        {"op": "self"}, {"op": "div", "in": [freq(), f64::from(SR)]}]})
    };
    let recur = json!({"op": "recursion", "in": [
        {"op": "sub", "in": [acc(), {"op": "floor", "in": [acc()]}]}]});
    // The box `sine_box_json` delays its phasor by 1 to start at 0 (UGen
    // alignment); match that so the two track sample for sample.
    let phasor = json!({"op": "delay1", "in": [recur]});
    json!({"signals": [{"op": "mul", "in": [
        {"op": "sin", "in": [{"op": "mul", "in": [std::f64::consts::TAU, phasor]}]},
        0.2]}]})
    .to_string()
}

#[test]
fn signal_and_box_sines_agree_within_float_tolerance() {
    // Box sine on channel 0, the equivalent Signal-API sine on channel 1: same
    // algorithm, same f32 precision, so they track each other tightly.
    let bdef = compile_faust("bsine", CompilePayload::Json(sine_box_json()));
    let sdef = compile_faust("ssine", CompilePayload::Signal(sine_signal_json()));
    assert_eq!((sdef.num_inputs, sdef.num_outputs), (0, 1));

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_tail(
            1000,
            ROOT_NODE_ID,
            faust_synth(&bdef, &[("out", 0.0)]),
        ))
        .ok()
        .unwrap();
    handle
        .send(add_tail(
            1001,
            ROOT_NODE_ID,
            faust_synth(&sdef, &[("out", 1.0)]),
        ))
        .ok()
        .unwrap();

    let (left, right) = render_stereo(&mut engine, 250);
    assert!(rms(&left) > 0.1, "box sine must play");
    assert!(rms(&right) > 0.1, "signal sine must play");
    const TOL: f32 = 4e-3;
    let diff = max_abs_diff(&left, &right);
    assert!(diff < TOL, "max sample difference {diff} exceeds {TOL}");
}

#[test]
fn sine_graphs_agree_within_float_tolerance() {
    // UGen sine on channel 0, the equivalent Faust box graph on channel 1,
    // rendered by the same engine over the same blocks.
    let fdef = compile_faust("psine", CompilePayload::Json(sine_box_json()));
    assert_eq!((fdef.num_inputs, fdef.num_outputs), (0, 1));

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_tail(
            1000,
            ROOT_NODE_ID,
            ugen_synth(&sine_spec("usine", 0.0)),
        ))
        .ok()
        .unwrap();
    handle
        .send(add_tail(
            1001,
            ROOT_NODE_ID,
            faust_synth(&fdef, &[("out", 1.0)]),
        ))
        .ok()
        .unwrap();

    let (left, right) = render_stereo(&mut engine, 250); // 16000 samples
    assert!(rms(&left) > 0.1, "UGen sine must actually play");
    assert!(rms(&right) > 0.1, "Faust sine must actually play");

    // f64 vs f32 phase accumulation drifts ~6e-4 over this span; a one-sample
    // phase offset would peak at 0.2·2π·440/48000 ≈ 0.0115, far above TOL.
    const TOL: f32 = 4e-3;
    let diff = max_abs_diff(&left, &right);
    assert!(diff < TOL, "max sample difference {diff} exceeds {TOL}");

    // Sanity: the tolerance does discriminate — the same signals offset by
    // one sample must fail it.
    let shifted = max_abs_diff(&left[1..], &right[..right.len() - 1]);
    assert!(
        shifted > TOL,
        "shifted diff {shifted} should exceed {TOL}; tolerance is vacuous"
    );
}

#[test]
fn gain_stages_are_bit_exact_on_the_same_input() {
    // One UGen sine feeds private bus 4; a UGen `In·0.5` chain and a Faust
    // `_ * 0.5` chain read it in the same block and write channels 0 and 1.
    // Same f32 multiply on the same samples: the outputs must be identical
    // down to the bit.
    let chain_spec = json!({
        "name": "uchain",
        "ugens": [
            {"kind": "In",  "inputs": [{"const": 4.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"const": 0.5}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    })
    .to_string();
    let fgain = compile_faust("pgain", CompilePayload::Source("process = _ * 0.5;".into()));
    assert_eq!((fgain.num_inputs, fgain.num_outputs), (1, 1));

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_tail(
            1000,
            ROOT_NODE_ID,
            ugen_synth(&sine_spec("src", 4.0)),
        ))
        .ok()
        .unwrap();
    handle
        .send(add_tail(1001, ROOT_NODE_ID, ugen_synth(&chain_spec)))
        .ok()
        .unwrap();
    handle
        .send(add_tail(
            1002,
            ROOT_NODE_ID,
            faust_synth(&fgain, &[("in", 4.0), ("out", 1.0)]),
        ))
        .ok()
        .unwrap();

    let (left, right) = render_stereo(&mut engine, 250);
    assert!(rms(&left) > 0.05, "the chain must carry signal");
    let mismatches = left
        .iter()
        .zip(&right)
        .filter(|(l, r)| l.to_bits() != r.to_bits())
        .count();
    assert_eq!(
        mismatches,
        0,
        "UGen and Faust gain stages diverge (max diff {})",
        max_abs_diff(&left, &right)
    );
}

#[test]
fn ugen_and_faust_synths_share_a_group() {
    // Both kinds live as siblings inside a non-root group, mix into the same
    // bus, and a single /g_freeAll-style command frees them together.
    const FSINE_SRC: &str = "process = sin(6.283185307179586 * \
        ((+(440.0/48000.0) : (_ <: _ - floor(_))) ~ _)) * 0.2;";
    let fdef = compile_faust("gsine", CompilePayload::Source(FSINE_SRC.into()));

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::new(),
        })
        .ok()
        .unwrap();
    handle
        .send(add_tail(1000, 1, ugen_synth(&sine_spec("usine", 0.0))))
        .ok()
        .unwrap();
    handle
        .send(add_tail(1001, 1, faust_synth(&fdef, &[])))
        .ok()
        .unwrap();

    // Same frequency, ≤ 1 sample of phase offset: amplitudes add.
    let (left, _) = render_stereo(&mut engine, 750);
    let expected_rms = 0.4 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&left) - expected_rms).abs() < 0.01,
        "rms = {}, expected ≈ {expected_rms} (the two synths must mix)",
        rms(&left)
    );

    handle.send(Cmd::FreeAllInGroup { id: 1 }).ok().unwrap();
    let (left, _) = render_stereo(&mut engine, 100);
    assert!(rms(&left) < 1e-9, "group must be silent after free-all");
    assert_eq!(handle.collect_garbage(), 2, "both synths leave as garbage");
}
