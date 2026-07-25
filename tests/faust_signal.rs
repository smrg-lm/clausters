//! Signal API tests: the JSON → Signal interpreter (`faust::signals`). Gated
//! behind the `faust` feature: `cargo test --features faust --test faust_signal`.
//!
//! Covers: a sine built from the Signal API's explicit `recursion`/`self`
//! phasor (parity with the box and UGen sines), the explicit-feedback
//! showcase (a one-pole filter whose impulse response decays at the pole),
//! multi-output defs, a kitchen-sink graph touching every op (lazy dynamic
//! linking only resolves a symbol when called), and validation errors carrying
//! the offending node's path.

#![cfg(feature = "faust")]

#[path = "common/signal.rs"]
mod signal;

use std::ffi::{CString, c_char};
use std::time::Duration;

use clausters::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};
use clausters::faust::ffi;
use clausters::faust::synth::FaustDef;
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const BLOCK: usize = 64;
const COMPILE_DEADLINE: Duration = Duration::from_secs(10);

fn dummy_client() -> clausters::osc::ClientId {
    clausters::osc::ClientId::Udp("127.0.0.1:1".parse().unwrap())
}

fn compile(name: &str, payload: CompilePayload) -> Result<FaustDef, String> {
    let compiler = CompilerThread::spawn();
    compiler
        .submit(CompileRequest {
            name: name.into(),
            payload,
            client: Some(dummy_client()),
            cache: None,
        })
        .ok()
        .unwrap();
    compiler
        .recv_result_timeout(COMPILE_DEADLINE)
        .expect("compilation must finish")
        .outcome
}

fn compile_signal(name: &str, graph: &Value) -> Result<FaustDef, String> {
    compile(name, CompilePayload::Signal(graph.to_string()))
}

fn render_mono(def: &FaustDef, seconds: f32) -> Vec<f32> {
    let dsp = unsafe { ffi::createCDSPInstance(def.factory().as_ptr()) };
    assert!(!dsp.is_null(), "instance creation failed");
    unsafe { ffi::initCDSPInstance(dsp, SR as i32) };
    assert_eq!(unsafe { ffi::getNumInputsCDSPInstance(dsp) }, 0);
    assert_eq!(unsafe { ffi::getNumOutputsCDSPInstance(dsp) }, 1);
    let blocks = (seconds * SR) as usize / BLOCK;
    let mut block = [0.0f32; BLOCK];
    let mut out = Vec::with_capacity(blocks * BLOCK);
    for _ in 0..blocks {
        let mut outputs: [*mut f32; 1] = [block.as_mut_ptr()];
        unsafe {
            ffi::computeCDSPInstance(
                dsp,
                BLOCK as i32,
                std::ptr::null_mut(),
                outputs.as_mut_ptr(),
            )
        };
        out.extend_from_slice(&block);
    }
    unsafe { ffi::deleteCDSPInstance(dsp) };
    out
}

/// Renders a 1-in/1-out def over `input`, block by block.
fn render_with_input(def: &FaustDef, input: &[f32]) -> Vec<f32> {
    let dsp = unsafe { ffi::createCDSPInstance(def.factory().as_ptr()) };
    assert!(!dsp.is_null(), "instance creation failed");
    unsafe { ffi::initCDSPInstance(dsp, SR as i32) };
    assert_eq!(unsafe { ffi::getNumInputsCDSPInstance(dsp) }, 1);
    assert_eq!(unsafe { ffi::getNumOutputsCDSPInstance(dsp) }, 1);
    let mut inb = [0.0f32; BLOCK];
    let mut outb = [0.0f32; BLOCK];
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let n = BLOCK.min(input.len() - i);
        inb[..n].copy_from_slice(&input[i..i + n]);
        inb[n..].fill(0.0);
        let mut ins: [*mut f32; 1] = [inb.as_mut_ptr()];
        let mut outs: [*mut f32; 1] = [outb.as_mut_ptr()];
        unsafe { ffi::computeCDSPInstance(dsp, BLOCK as i32, ins.as_mut_ptr(), outs.as_mut_ptr()) };
        out.extend_from_slice(&outb[..n]);
        i += n;
    }
    unsafe { ffi::deleteCDSPInstance(dsp) };
    out
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn estimated_freq(buf: &[f32]) -> f32 {
    signal::zero_crossing_freq(buf, SR)
}

fn un(op: &str, a: Value) -> Value {
    json!({"op": op, "in": [a]})
}
fn bin(op: &str, a: Value, b: Value) -> Value {
    json!({"op": op, "in": [a, b]})
}

/// `sin(2π·phasor(freq))·0.2` where the phasor is the Signal API recursion
/// `phasor = (self + freq/SR) wrapped`, with `self`/`recursion` the explicit
/// feedback. The accumulator is written twice (no implicit wire); Faust's CSE
/// merges the identical subtrees.
fn sine_signal(freq_init: f64) -> Value {
    let freq = json!({
        "op": "hslider", "label": "freq",
        "init": freq_init, "min": 20.0, "max": 20000.0, "step": 0.01
    });
    let acc = bin("add", json!({"op": "self"}), bin("div", freq, json!(SR)));
    let wrapped = bin("sub", acc.clone(), un("floor", acc));
    let phasor = json!({"op": "recursion", "in": [wrapped]});
    let sine = un("sin", bin("mul", json!(std::f64::consts::TAU), phasor));
    json!({"signals": [bin("mul", sine, json!(0.2))]})
}

#[test]
fn signal_sine_compiles_and_plays_at_440() {
    let def = compile_signal("ssine", &sine_signal(440.0)).expect("sine must compile");
    let out = render_mono(&def, 1.0);
    assert!(out.iter().all(|x| x.is_finite()));
    let freq = estimated_freq(&out);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
    let expected = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!((rms(&out) - expected).abs() < 0.005, "rms = {}", rms(&out));
}

#[test]
fn explicit_recursion_makes_a_one_pole_filter() {
    // y[n] = (1-a)·x[n] + a·y[n-1] — the Signal API's whole point: feedback
    // fused into one node, sample-accurate. Impulse response is geometric
    // with ratio a, which a block-rate LocalIn/LocalOut loop could not do.
    let a = 0.5f64;
    let x = json!({"op": "input", "index": 0});
    let body = bin(
        "add",
        bin("mul", json!(1.0 - a), x),
        bin("mul", json!(a), json!({"op": "self"})),
    );
    let graph = json!({"signals": [{"op": "recursion", "in": [body]}]});
    let def = compile_signal("onepole", &graph).expect("one-pole must compile");

    let mut impulse = vec![0.0f32; 512];
    impulse[0] = 1.0;
    let y = render_with_input(&def, &impulse);

    assert!((y[0] - (1.0 - a as f32)).abs() < 1e-5, "y[0] = {}", y[0]);
    for n in 0..8 {
        let ratio = y[n + 1] / y[n];
        assert!(
            (ratio - a as f32).abs() < 1e-4,
            "y[{}]/y[{n}] = {ratio}",
            n + 1
        );
    }
}

#[test]
fn multiple_signals_become_multiple_outputs() {
    let graph = json!({"signals": [0.1, 0.2, {"op": "mul", "in": [0.3, 0.3]}]});
    let def = compile_signal("multi", &graph).expect("must compile");
    assert_eq!(def.num_outputs, 3);
    assert_eq!(def.num_inputs, 0);
}

#[test]
fn signal_def_loads_and_instantiates_over_the_synth_path() {
    // The probed def is a normal FaustDef: it exposes the slider and the
    // reserved out/in controls just like a box def.
    let def = compile_signal("ssine2", &sine_signal(330.0)).expect("must compile");
    assert_eq!(def.params.len(), 1);
    assert_eq!(def.params[0].name, "freq");
    assert!(def.control_index("freq").is_some());
    assert!(def.control_index("out").is_some());
    assert!(def.control_index("in").is_some());
}

#[test]
fn kitchen_sink_graph_exercises_every_op() {
    // Touch every FFI symbol at least once (lazy linking). Exotic/int-typed
    // terms are scaled to 0 so they cannot blow up the output but the symbols
    // are still called at build time; output stays `0.5·input`.
    let x = json!({"op": "input", "index": 0});

    // Unary math on safe-domain constants + the input.
    let unary = [
        un("sin", x.clone()),
        un("cos", x.clone()),
        un("tan", json!(0.1)),
        un("asin", json!(0.5)),
        un("acos", json!(0.5)),
        un("atan", json!(0.5)),
        un("exp", json!(0.0)),
        un("exp10", json!(0.0)),
        un("log", json!(2.0)),
        un("log10", json!(2.0)),
        un("sqrt", json!(2.0)),
        un("abs", x.clone()),
        un("floor", json!(1.5)),
        un("ceil", json!(1.5)),
        un("rint", json!(1.4)),
        un("delay1", x.clone()),
    ];
    let mut sum = json!(0.0);
    for t in unary {
        sum = bin("add", sum, t);
    }

    // Binary ops (float-typed).
    for op in [
        "add",
        "sub",
        "mul",
        "div",
        "fmod",
        "remainder",
        "pow",
        "min",
        "max",
        "atan2",
        "gt",
        "lt",
        "ge",
        "le",
        "eq",
        "ne",
    ] {
        sum = bin("add", sum, bin(op, json!(0.7), json!(0.3)));
    }
    // delay by a constant.
    sum = bin("add", sum, bin("delay", x.clone(), json!(2)));
    // select2 / select3.
    sum = bin(
        "add",
        sum,
        json!({"op": "select2", "in": [json!(0), json!(0.1), json!(0.2)]}),
    );
    sum = bin(
        "add",
        sum,
        json!({"op": "select3", "in": [json!(1), json!(0.1), json!(0.2), json!(0.3)]}),
    );

    // Integer-typed terms: bitwise/shift ops need int operands, then floatcast.
    let i3 = un("intcast", json!(3));
    let i1 = un("intcast", json!(1));
    for op in ["and", "or", "xor", "lsh", "rsh", "rem"] {
        sum = bin("add", sum, un("floatcast", bin(op, i3.clone(), i1.clone())));
    }

    // UI elements + a recursive phasor + tables, all scaled into the sink.
    let ui = bin(
        "add",
        bin(
            "add",
            json!({"op": "vslider", "label": "v", "init": 0.0, "min": 0.0, "max": 1.0, "step": 0.01}),
            json!({"op": "nentry", "label": "n", "init": 0.0, "min": 0.0, "max": 1.0, "step": 0.01}),
        ),
        bin(
            "add",
            bin(
                "add",
                json!({"op": "button", "label": "b"}),
                json!({"op": "checkbox", "label": "c"}),
            ),
            json!({"op": "hbargraph", "label": "hb", "min": -1.0, "max": 1.0, "in": [x.clone()]}),
        ),
    );
    sum = bin("add", sum, ui);
    sum = bin(
        "add",
        sum,
        json!({"op": "vbargraph", "label": "vb", "min": -1.0, "max": 1.0, "in": [json!(0.0)]}),
    );
    // A recursive accumulator (self/recursion) and the two tables.
    sum = bin(
        "add",
        sum,
        json!({"op": "recursion", "in": [bin("mul", json!({"op": "self"}), json!(0.5))]}),
    );
    // Foreign constant/variable (the SR primitives): floatcast then fold in.
    sum = bin(
        "add",
        sum,
        un(
            "floatcast",
            json!({"op": "fconst", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}),
        ),
    );
    sum = bin(
        "add",
        sum,
        un(
            "floatcast",
            json!({"op": "fvar", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}),
        ),
    );
    let wf = json!({"op": "waveform", "values": [0.0, 0.5, 1.0, 0.5]});
    sum = bin(
        "add",
        sum,
        json!({"op": "rdtable", "in": [json!(4), wf, json!(0)]}),
    );
    sum = bin(
        "add",
        sum,
        json!({"op": "rwtable",
        "in": [json!(4), json!(0.0), json!(0), json!(0.25), json!(0)]}),
    );

    // Keep the output bounded and deterministic: 0·(everything) + 0.5·input.
    let out = bin(
        "add",
        bin("mul", sum, json!(0.0)),
        bin("mul", x, json!(0.5)),
    );
    let def = compile_signal("kitchen", &json!({"signals": [out]})).expect("must compile");

    let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
    let y = render_with_input(&def, &input);
    assert!(y.iter().all(|v| v.is_finite()));
    for (i, v) in y.iter().enumerate() {
        assert!((v - input[i] * 0.5).abs() < 1e-5, "y[{i}] = {v}");
    }
}

/// The Signal API twins of the broken box wrappers: upstream's `boxCos()`/
/// `boxFmod()` return the `abs` primitive (the copy-paste bug guarded in
/// `faust_box.rs`), while `sigCos()`/`sigFmod()` are correct — this is why
/// `faust::boxes` needs a fragment workaround and `faust::signals` does not.
/// Pin the actual values here: the kitchen sink scales everything by 0, so
/// without this a future libfaust could break the signal path silently too.
#[test]
fn signal_cos_and_fmod_are_not_hit_by_the_box_bug() {
    let cos = compile_signal("scos", &json!({"signals": [un("cos", json!(0.5))]}))
        .expect("cos must compile");
    let v = render_mono(&cos, 0.01)[0];
    assert!(
        (v - 0.5f32.cos()).abs() < 1e-6,
        "sigCos(0.5) = {v}, expected ≈ 0.8776 (abs(0.5) = 0.5 would be the box bug)"
    );

    let fmod = compile_signal(
        "sfmod",
        &json!({"signals": [bin("fmod", json!(5.25), json!(2.0))]}),
    )
    .expect("fmod must compile");
    let v = render_mono(&fmod, 0.01)[0];
    assert!((v - 1.25).abs() < 1e-6, "sigFmod(5.25, 2.0) = {v}");
}

/// Canary for the second bug of faust#1264: the signal type checker has no
/// case for the logical-right-shift opcode (`kLRsh`), so a factory built
/// from `CsigLRightShift` must fail with `ASSERT : unrecognized opcode : 7`
/// (2.81.x aborted the whole host process; 2.86.0 returns a null factory
/// with the assert in the error string). The symbol is deliberately not
/// bound in `ffi.rs` — it is declared locally here — and the schema's `rsh`
/// is the arithmetic shift. When this canary fails, the linked libfaust
/// carries the fix (PR faust#1272): bind the symbol and expose `lrsh`.
#[test]
fn upstream_lrsh_still_fails_the_type_checker() {
    unsafe extern "C" {
        fn CsigLRightShift(x: ffi::FaustSignal, y: ffi::FaustSignal) -> ffi::FaustSignal;
    }
    let name = CString::new("lrsh").unwrap();
    let target = CString::new("").unwrap();
    let mut error_msg = [0 as c_char; ffi::ERROR_MSG_SIZE];

    let guard = clausters::faust::compiler::ffi_lock();
    let factory = unsafe {
        ffi::createLibContext();
        let a = ffi::CsigIntCast(ffi::CsigReal(3.0));
        let b = ffi::CsigIntCast(ffi::CsigReal(1.0));
        let mut outs = [
            ffi::CsigFloatCast(CsigLRightShift(a, b)),
            std::ptr::null_mut(),
        ];
        let f = ffi::createCDSPFactoryFromSignals(
            name.as_ptr(),
            outs.as_mut_ptr(),
            0,
            std::ptr::null(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        );
        ffi::destroyLibContext();
        f
    };
    drop(guard);
    if !factory.is_null() {
        unsafe { ffi::deleteCDSPFactory(factory) };
        panic!(
            "CsigLRightShift now compiles: upstream fixed kLRsh (faust#1264) — \
             bind it in ffi.rs and expose `lrsh` in the signal schema"
        );
    }
}

#[test]
fn fconst_reads_the_engine_sample_rate() {
    // `ma.SR` is `floatcast(fconstant(int fSamplingFreq, <math.h>))`: a def
    // whose sole output is that foreign constant must render the rate passed to
    // `initCDSPInstance` (here `SR`), proving the value comes from the engine
    // and is not baked into the graph.
    let sr_sig = un(
        "floatcast",
        json!({"op": "fconst", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}),
    );
    let def = compile_signal("srconst", &json!({"signals": [sr_sig]})).expect("fconst compiles");
    let out = render_mono(&def, 0.01);
    assert!(!out.is_empty());
    assert!(
        out.iter().all(|&v| (v - SR).abs() < 1.0),
        "SR signal = {}",
        out[0]
    );
}

#[test]
fn fvar_probe_compiles() {
    // The runtime variable twin. fSamplingFreq is also exposed as a variable;
    // this only asserts the op builds and JIT-links (its value tracks the rate).
    let v = un(
        "floatcast",
        json!({"op": "fvar", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}),
    );
    let def = compile_signal("srvar", &json!({"signals": [v]})).expect("fvar compiles");
    let out = render_mono(&def, 0.01);
    assert!(out.iter().all(|v| v.is_finite()));
}

#[test]
fn validation_errors_point_at_the_offending_node() {
    let cases: &[(Value, &str)] = &[
        (json!([1, 2]), "must be a {\"signals\""),
        (json!({"foo": 1}), "missing \"signals\""),
        (json!({"signals": []}), "at least one output"),
        (
            json!({"signals": [{"op": "input"}]}),
            "non-negative integer \"index\"",
        ),
        (json!({"signals": [{"op": "frobnicate"}]}), "unknown op"),
        (json!({"signals": [{"op": "add", "in": [1.0]}]}), "takes 2"),
        (json!({"signals": [{"op": "sin"}]}), "needs an \"in\" array"),
        (
            json!({"signals": [{"op": "fconst", "name": "x"}]}),
            "needs \"ctype\"",
        ),
        (
            json!({"signals": [{"op": "fvar", "ctype": "int"}]}),
            "needs a string \"name\"",
        ),
    ];
    for (graph, needle) in cases {
        let err = compile_signal("bad", graph).err().unwrap();
        assert!(
            err.contains(needle),
            "graph {graph} -> {err:?}, expected {needle:?}"
        );
    }
    // The path reaches the offending nested node.
    let nested = json!({"signals": [{"op": "mul", "in": [0.5, {"op": "add", "in": [1.0]}]}]});
    let err = compile_signal("bad", &nested).err().unwrap();
    assert!(err.contains("$.signals[0].in[1]"), "path missing: {err}");
}
