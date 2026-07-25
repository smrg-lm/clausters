//! F2 tests: the JSON → Box API interpreter. Gated behind the `faust`
//! feature: `cargo test --features faust --test faust_json`.
//!
//! Covers: the JSON sine graph built from primitives (parity with the F0
//! smoke test), the `faust` escape hatch into the stdlib (`os.osc`), stdlib
//! imports from raw source (the `-I` path), validation errors with the
//! offending JSON node path, and a kitchen-sink graph that touches every op
//! of the schema (lazy dynamic linking: a misnamed FFI symbol only explodes
//! when called, so each one must be exercised at least once).

#![cfg(feature = "faust")]

#[path = "common/signal.rs"]
mod signal;

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

fn compile_json(name: &str, graph: &Value) -> Result<FaustDef, String> {
    compile(name, CompilePayload::Json(graph.to_string()))
}

/// Renders a 0-in/1-out def for `seconds`, audio-thread style
/// (non-interleaved block buffers).
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

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn estimated_freq(buf: &[f32]) -> f32 {
    signal::zero_crossing_freq(buf, SR)
}

/// The F0 smoke graph, now as JSON: `sin(2π·phasor(freq)) * 0.2` with
/// `phasor = (+(freq/SR) : wrap) ~ _` and `wrap = _ <: _ - floor(_)`.
fn sine_graph() -> Value {
    let freq = json!({
        "op": "hslider", "label": "freq",
        "init": 440.0, "min": 20.0, "max": 20000.0, "step": 0.01
    });
    let wrap = json!({
        "op": "split",
        "in": ["_", {"op": "sub", "in": ["_", {"op": "floor", "in": ["_"]}]}]
    });
    let phasor = json!({
        "op": "rec",
        "in": [
            {"op": "seq", "in": [
                {"op": "add", "in": ["_", {"op": "div", "in": [freq, SR]}]},
                wrap
            ]},
            "_"
        ]
    });
    json!({
        "op": "mul",
        "in": [
            {"op": "sin", "in": [{"op": "mul", "in": [std::f64::consts::TAU, phasor]}]},
            0.2
        ]
    })
}

#[test]
fn json_sine_graph_compiles_and_plays_at_440() {
    let def = compile_json("jsine", &sine_graph()).expect("sine graph must compile");
    let out = render_mono(&def, 1.0);
    assert!(out.iter().all(|x| x.is_finite()));
    let freq = estimated_freq(&out);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
    let expected_rms = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&out) - expected_rms).abs() < 0.005,
        "rms = {}, expected ≈ {expected_rms}",
        rms(&out)
    );
}

#[test]
fn faust_op_embeds_stdlib_source_as_a_composable_box() {
    // A stdlib oscillator composed with primitive boxes: os.osc : *(0.5).
    let graph = json!({
        "op": "seq",
        "in": [
            {"op": "faust", "src": "import(\"stdfaust.lib\"); process = os.osc(440);"},
            {"op": "mul", "in": ["_", 0.5]}
        ]
    });
    let def = compile_json("josc", &graph).expect("stdlib fragment must compile");
    let out = render_mono(&def, 1.0);
    let freq = estimated_freq(&out);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
    let expected_rms = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&out) - expected_rms).abs() < 0.01,
        "rms = {}, expected ≈ {expected_rms}",
        rms(&out)
    );
}

#[test]
fn raw_source_defs_resolve_stdlib_imports() {
    // F1 deliberately avoided imports; the `-I` stdlib path added in F2
    // covers `createCDSPFactoryFromString` too.
    let src = "import(\"stdfaust.lib\"); process = os.osc(440) * 0.2;";
    compile("ssine", CompilePayload::Source(src.into()))
        .expect("raw source with stdlib import must compile");
}

/// F5: a `waveform` standing in for (size, init) of `rdtable`, read by a
/// wrapping integer counter — the output must walk the table verbatim.
#[test]
fn waveform_rdtable_cycles_through_the_table() {
    // counter = (+(1) ~ _) - 1 = 0, 1, 2, …; idx = counter & 3.
    let counter = json!({"op": "sub", "in": [
        {"op": "rec", "in": [{"op": "add", "in": ["_", {"op": "int", "value": 1}]}, "_"]},
        {"op": "int", "value": 1}
    ]});
    let idx = json!({"op": "and", "in": [
        {"op": "intcast", "in": [counter]}, {"op": "int", "value": 3}
    ]});
    let graph = json!({"op": "rdtable", "in": [
        {"op": "waveform", "values": [0.0, 0.25, 0.5, 0.75]},
        idx
    ]});
    let def = compile_json("jwave", &graph).expect("waveform + rdtable must compile");
    let out = render_mono(&def, 0.01);
    let table = [0.0f32, 0.25, 0.5, 0.75];
    for (i, &x) in out.iter().take(16).enumerate() {
        assert_eq!(x, table[i % 4], "sample {i} must read table[{}]", i % 4);
    }
}

/// F5's real use case: a wavetable oscillator whose table the client computed
/// numerically (here a 64-point sine) instead of serializing Faust source.
#[test]
fn computed_wavetable_oscillator_plays_at_440() {
    let n = 64;
    let values: Vec<f64> = (0..n)
        .map(|i| (std::f64::consts::TAU * i as f64 / n as f64).sin())
        .collect();
    let wrap = json!({
        "op": "split",
        "in": ["_", {"op": "sub", "in": ["_", {"op": "floor", "in": ["_"]}]}]
    });
    let phasor = json!({"op": "rec", "in": [
        {"op": "seq", "in": [{"op": "add", "in": ["_", 440.0 / SR as f64]}, wrap]},
        "_"
    ]});
    let idx = json!({"op": "intcast", "in": [{"op": "mul", "in": [phasor, n]}]});
    let graph = json!({"op": "rdtable", "in": [{"op": "waveform", "values": values}, idx]});
    let def = compile_json("jwtosc", &graph).expect("wavetable oscillator must compile");
    let out = render_mono(&def, 1.0);
    let freq = estimated_freq(&out);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
    // A full-period sine table has RMS 1/√2 regardless of the stair-stepping.
    let expected_rms = std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&out) - expected_rms).abs() < 0.02,
        "rms = {}, expected ≈ {expected_rms}",
        rms(&out)
    );
}

/// The explicit 3-box form: size and init as plain constant boxes.
#[test]
fn rdtable_accepts_explicit_size_and_init() {
    let graph = json!({"op": "rdtable", "in": [
        {"op": "int", "value": 4}, 0.5, {"op": "int", "value": 2}
    ]});
    let def = compile_json("jrdc", &graph).expect("explicit rdtable must compile");
    let out = render_mono(&def, 0.01);
    assert!(
        out.iter().all(|&x| x == 0.5),
        "constant-init table must read 0.5"
    );
}

#[test]
fn rwtable_reads_back_what_it_writes() {
    // Write 0.25 at index 0, read index 0. Whether the first sample sees the
    // init value or the fresh write is a Faust ordering detail; from the
    // second sample on it must be the written value.
    let graph = json!({"op": "rwtable", "in": [
        {"op": "int", "value": 4}, 0.0,
        {"op": "int", "value": 0}, 0.25, {"op": "int", "value": 0}
    ]});
    let def = compile_json("jrwt", &graph).expect("rwtable must compile");
    let out = render_mono(&def, 0.01);
    assert!(out[0] == 0.0 || out[0] == 0.25, "first sample = {}", out[0]);
    assert!(
        out[1..].iter().all(|&x| x == 0.25),
        "table must hold the write"
    );
}

#[test]
fn validation_errors_point_at_the_offending_node() {
    let cases: [(Value, &str); 10] = [
        // Unknown op at the root: the path is just `$`.
        (json!({"op": "mul3", "in": [1, 2]}), "unknown op \"mul3\""),
        // Nested: the path walks the `in` arrays.
        (
            json!({"op": "seq", "in": [{"op": "mul", "in": ["_", {"op": "zzz"}]}, "_"]}),
            "at $.in[0].in[1]:",
        ),
        (json!({"op": "hslider", "init": 440.0}), "label"),
        (json!({"op": "rec", "in": ["_"]}), "`rec` takes 2 in \"in\""),
        (json!({"op": "seq"}), "`seq` needs an \"in\" array"),
        (json!([1, 2]), "at $: expected a box"),
        (json!({"op": "waveform"}), "`waveform` needs a \"values\""),
        (json!({"op": "waveform", "values": []}), "must not be empty"),
        (
            json!({"op": "waveform", "values": [1.0, "x"]}),
            "values[1] must be a number",
        ),
        (
            json!({"op": "rdtable", "in": ["_"]}),
            "`rdtable` takes 2 to 3 in \"in\"",
        ),
    ];
    let compiler = CompilerThread::spawn();
    for (graph, _) in &cases {
        compiler
            .submit(CompileRequest {
                name: "bad".into(),
                payload: CompilePayload::Json(graph.to_string()),
                client: Some(dummy_client()),
                cache: None,
            })
            .ok()
            .unwrap();
    }
    for (graph, expected) in &cases {
        let result = compiler
            .recv_result_timeout(COMPILE_DEADLINE)
            .expect("validation must finish");
        let err = result.outcome.err().unwrap_or_else(|| {
            panic!("graph must be rejected: {graph}");
        });
        assert!(
            err.contains(expected),
            "error for {graph} must contain {expected:?}, got: {err}"
        );
    }
}

#[test]
fn bad_faust_fragment_reports_path_and_compiler_error() {
    let graph = json!({"op": "faust", "src": "process = nonsense(;"});
    let err = compile_json("jfrag", &graph)
        .err()
        .expect("broken fragment must fail");
    assert!(err.starts_with("at $:"), "missing node path: {err}");
    assert!(err.len() > "at $:".len(), "missing compiler message: {err}");
}

#[test]
fn semantic_errors_from_faust_are_forwarded() {
    // Structurally valid JSON, but `add` left with dangling inputs is not a
    // valid `process`: Faust's own error must come back.
    let graph = json!({"op": "add", "in": ["_", "_"]});
    // Some libfaust versions auto-wire open inputs instead of failing; both
    // outcomes are fine, what matters is no crash and a readable error.
    if let Err(err) = compile_json("jopen", &graph) {
        assert!(!err.is_empty());
    }
}

/// One compile touching every op of the schema. Constants feed everything so
/// the graph has no inputs; the output arity is whatever it is — the point
/// is that every `Cbox*` symbol gets called once (typos in the hand-written
/// FFI only fail at call time).
#[test]
fn kitchen_sink_graph_exercises_every_op() {
    let unaries = [
        "sin",
        "cos",
        "tan",
        "asin",
        "atan",
        "exp",
        "exp10",
        "log",
        "log10",
        "sqrt",
        "abs",
        "floor",
        "ceil",
        "rint",
        "round",
        "intcast",
        "floatcast",
    ];
    let binaries = [
        "add", "sub", "mul", "div", "fmod", "pow", "min", "max", "atan2", "gt", "lt", "ge", "le",
        "eq", "ne", "and", "or", "xor",
    ];
    let mut boxes: Vec<Value> = Vec::new();
    for op in unaries {
        boxes.push(json!({"op": op, "in": [0.5]}));
    }
    // acos(2.0) is NaN at compile-constant time; keep it in range.
    boxes.push(json!({"op": "acos", "in": [0.5]}));
    for op in binaries {
        boxes.push(json!({"op": op, "in": [{"op": "int", "value": 3}, 2]}));
    }
    boxes.push(json!({"op": "delay", "in": [0.5, {"op": "int", "value": 10}]}));
    boxes.push(json!({"op": "select2", "in": [
        {"op": "intcast", "in": [{"op": "button", "label": "gate"}]}, 0.0, 0.25
    ]}));
    boxes.push(json!({"op": "select3", "in": [
        {"op": "intcast", "in": [{"op": "checkbox", "label": "mode"}]}, 1.0, 2.0, 3.0
    ]}));
    boxes.push(json!({"op": "hgroup", "label": "g", "in": [
        {"op": "vslider", "label": "v", "init": 0.5, "min": 0.0, "max": 1.0, "step": 0.01}
    ]}));
    boxes.push(json!({"op": "vgroup", "label": "h", "in": [
        {"op": "nentry", "label": "n", "init": 1.0, "min": 0.0, "max": 8.0, "step": 1.0}
    ]}));
    boxes.push(json!({"op": "seq", "in": [1.5, {"op": "wire"}]}));
    boxes.push(json!({"op": "seq", "in": [{"op": "real", "value": 2.5}, {"op": "cut"}]}));
    boxes.push(json!({"op": "merge", "in": [{"op": "par", "in": [1.0, 2.0, "!"]}, "_"]}));
    boxes.push(json!({"op": "fconst",
        "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}));
    boxes.push(json!({"op": "fvar",
        "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}));
    boxes.push(json!({"op": "rdtable", "in": [
        {"op": "waveform", "values": [0.0, 1.0]}, {"op": "int", "value": 0}
    ]}));
    boxes.push(json!({"op": "rwtable", "in": [
        {"op": "int", "value": 2}, 0.0,
        {"op": "int", "value": 0}, 0.5, {"op": "int", "value": 1}
    ]}));

    let graph = json!({"op": "par", "in": boxes});
    compile_json("sink", &graph).expect("kitchen sink must compile");
}

/// Regression: `boxCos()` is broken upstream (returns the `abs` primitive), so
/// the `cos` op is built from a source fragment instead of `CboxCosAux`. A
/// constant `cos(0.5)` must be cos(0.5) ≈ 0.8776, not abs(0.5) = 0.5.
#[test]
fn box_cos_computes_cosine_not_abs() {
    let def = compile_json("bcos", &json!({"op": "cos", "in": [0.5]})).expect("compiles");
    let out = render_mono(&def, 0.01);
    let v = out[0];
    assert!(
        (v - 0.5_f32.cos()).abs() < 1e-4,
        "cos(0.5) = {v}, expected ≈ 0.8776"
    );
    assert!(
        (v - 0.5).abs() > 0.1,
        "cos returned abs(0.5) = 0.5 (the upstream bug)"
    );
}
