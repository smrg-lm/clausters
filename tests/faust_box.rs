//! Box API tests at the FFI level (`ffi::Cbox*`), the box counterpart of
//! `faust_signal.rs`. Gated behind the `faust` feature:
//! `cargo test --features faust --test faust_box`.
//!
//! The split across the box suites: `faust_smoke.rs` is the F0 latency probe
//! (one sine through the Box API), `faust_json.rs` covers the JSON → Box
//! interpreter, and this file covers Box API *semantics* built directly with
//! FFI calls — `rec` feedback, `CDSPToBoxes` fragment arity, the CSE
//! guarantee that duplicated subtrees share their computation (the design
//! bet behind the Python client's box sugar) — plus the upstream copy-paste
//! bugs and their workaround:
//!
//! - **Canary**: libfaust's `boxCos()`/`boxFmod()` both return the `abs`
//!   primitive (copy-paste bug in `box_signal_api.cpp`, present from 2.81.10
//!   through at least 2.86.0), so `CboxCosAux` silently computes `abs` and
//!   `CboxFmodAux` is deliberately not even bound in `ffi.rs`. The canary
//!   asserts the bug is still there: when a libfaust upgrade makes it fail,
//!   the fragment workaround in `faust::boxes` can be retired.
//! - **Regression**: the workaround (`cos`/`fmod` built from a one-line
//!   `CDSPToBoxes` fragment) must produce the genuine primitives through the
//!   JSON schema. `faust_json.rs` guards `cos`; `fmod` is guarded here.

#![cfg(feature = "faust")]

use std::ffi::{CStr, CString, c_char, c_int};

use clausters::faust::compiler::{self, CompilePayload};
use clausters::faust::ffi::*;
use serde_json::json;

const SR: f32 = 48_000.0;
const BLOCK: usize = 64;

/// Holds the process-wide FFI lock with the libfaust context open, dropping
/// the context before the lock — the same bracket `faust::compiler` uses.
/// Boxes are arena pointers that die with the context; only the factory
/// survives it.
struct LibCtx {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl LibCtx {
    fn acquire() -> Self {
        let lock = compiler::ffi_lock();
        unsafe { createLibContext() };
        Self { _lock: lock }
    }
}

impl Drop for LibCtx {
    fn drop(&mut self) {
        unsafe { destroyLibContext() };
    }
}

/// JIT-compiles the box returned by `build` (which runs inside the
/// context bracket) into a factory. No compiler args: these graphs use no
/// stdlib imports and constant folding does not depend on `-ftz`.
fn factory_from_boxes(name: &str, build: impl FnOnce() -> FaustBox) -> *mut llvm_dsp_factory {
    let name_c = CString::new(name).unwrap();
    let target = CString::new("").unwrap();
    let mut error_msg = [0 as c_char; ERROR_MSG_SIZE];
    let ctx = LibCtx::acquire();
    let process = build();
    let factory = unsafe {
        createCDSPFactoryFromBoxes(
            name_c.as_ptr(),
            process,
            0,
            std::ptr::null(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        )
    };
    drop(ctx);
    assert!(
        !factory.is_null(),
        "factory creation failed: {}",
        unsafe { CStr::from_ptr(error_msg.as_ptr()) }.to_string_lossy()
    );
    factory
}

/// Renders `input` through a 1-in/1-out instance of `factory`, block by
/// block, audio-thread style. With `input` empty renders a 0-in def instead
/// for `samples` samples.
fn render(factory: *mut llvm_dsp_factory, input: &[f32], samples: usize) -> Vec<f32> {
    let dsp = unsafe { createCDSPInstance(factory) };
    assert!(!dsp.is_null(), "instance creation failed");
    unsafe { initCDSPInstance(dsp, SR as i32) };
    let expected_ins = if input.is_empty() { 0 } else { 1 };
    assert_eq!(unsafe { getNumInputsCDSPInstance(dsp) }, expected_ins);
    assert_eq!(unsafe { getNumOutputsCDSPInstance(dsp) }, 1);

    let total = if input.is_empty() {
        samples
    } else {
        input.len()
    };
    let mut inb = [0.0f32; BLOCK];
    let mut outb = [0.0f32; BLOCK];
    let mut out = Vec::with_capacity(total);
    let mut i = 0;
    while i < total {
        let n = BLOCK.min(total - i);
        let mut ins: [*mut f32; 1] = [inb.as_mut_ptr()];
        let inputs = if input.is_empty() {
            std::ptr::null_mut()
        } else {
            inb[..n].copy_from_slice(&input[i..i + n]);
            inb[n..].fill(0.0);
            ins.as_mut_ptr()
        };
        let mut outs: [*mut f32; 1] = [outb.as_mut_ptr()];
        unsafe { computeCDSPInstance(dsp, BLOCK as i32, inputs, outs.as_mut_ptr()) };
        out.extend_from_slice(&outb[..n]);
        i += n;
    }
    unsafe { deleteCDSPInstance(dsp) };
    out
}

/// `rec` feedback at the FFI level: `*(1-a) : (+ ~ *(a))` is the one-pole
/// `y[n] = (1-a)·x[n] + a·y[n-1]` — `~` carries one implicit sample of
/// delay, exactly like the Signal API's `recursion`/`self` pair (the same
/// filter and assertions as `faust_signal.rs`, so the two feedback forms are
/// pinned to identical semantics).
#[test]
fn box_rec_feedback_makes_a_one_pole_filter() {
    let a = 0.5f64;
    let factory = factory_from_boxes("bpole", || unsafe {
        let adder = CboxAddAux(CboxWire(), CboxWire());
        let fb = CboxMulAux(CboxWire(), CboxReal(a));
        let pre = CboxMulAux(CboxWire(), CboxReal(1.0 - a));
        CboxSeq(pre, CboxRec(adder, fb))
    });

    let mut impulse = vec![0.0f32; 512];
    impulse[0] = 1.0;
    let y = render(factory, &impulse, 0);
    unsafe { deleteCDSPFactory(factory) };

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

/// Canary for the upstream copy-paste bug: `CboxCosAux(0.5)` must still
/// compute `abs(0.5) = 0.5` — NOT the cosine. If this test ever fails with a
/// cosine coming out, the linked libfaust has fixed `boxCos()`/`boxFmod()`
/// (both return the abs primitive in `box_signal_api.cpp`) and the fragment
/// workaround in `faust::boxes` (plus this canary and the unbound
/// `CboxFmodAux` note in `ffi.rs`) can be retired.
#[test]
fn upstream_boxcos_still_computes_abs() {
    let factory = factory_from_boxes("bcanary", || unsafe { CboxCosAux(CboxReal(0.5)) });
    let out = render(factory, &[], 16);
    unsafe { deleteCDSPFactory(factory) };
    let v = out[0];
    assert!(
        (v - 0.5).abs() < 1e-6,
        "CboxCosAux(0.5) = {v}: upstream fixed boxCos() — \
         retire the fragment workaround in faust::boxes"
    );
}

/// Regression for the `fmod` workaround (the twin of `faust_json.rs`'s `cos`
/// one): through the JSON schema, `fmod(5.25, 2.0)` must be the genuine
/// primitive, 1.25 — not `abs` (5.25) nor a compile error, the two faces of
/// the upstream bug.
#[test]
fn box_fmod_computes_fmod_not_abs() {
    let graph = json!({"op": "fmod", "in": [5.25, 2.0]});
    let def = compiler::compile("bfmod", &CompilePayload::Json(graph.to_string()))
        .expect("fmod must compile through the fragment workaround");
    let out = render(def.factory().as_ptr(), &[], 16);
    let v = out[0];
    assert!((v - 1.25).abs() < 1e-6, "fmod(5.25, 2.0) = {v}");
}

/// `CDSPToBoxes` reports the fragment's I/O arity through its out-params
/// (which `faust::boxes` ignores), and the resulting box composes with
/// primitives: `(3, 4) : +` must render 7.
#[test]
fn cdsp_to_boxes_reports_arity_and_composes() {
    let name = CString::new("frag").unwrap();
    let src = CString::new("process = +;").unwrap();
    let target = CString::new("").unwrap();
    let mut error_msg = [0 as c_char; ERROR_MSG_SIZE];
    let (mut ins, mut outs) = (-1 as c_int, -1 as c_int);

    let ctx = LibCtx::acquire();
    let fragment = unsafe {
        CDSPToBoxes(
            name.as_ptr(),
            src.as_ptr(),
            0,
            std::ptr::null(),
            &mut ins,
            &mut outs,
            error_msg.as_mut_ptr(),
        )
    };
    assert!(
        !fragment.is_null(),
        "fragment failed: {}",
        unsafe { CStr::from_ptr(error_msg.as_ptr()) }.to_string_lossy()
    );
    assert_eq!((ins, outs), (2, 1), "`+` is a 2-in/1-out box");

    let name_c = CString::new("bfrag").unwrap();
    let process = unsafe { CboxSeq(CboxPar(CboxReal(3.0), CboxReal(4.0)), fragment) };
    let factory = unsafe {
        createCDSPFactoryFromBoxes(
            name_c.as_ptr(),
            process,
            0,
            std::ptr::null(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        )
    };
    drop(ctx);
    assert!(
        !factory.is_null(),
        "factory creation failed: {}",
        unsafe { CStr::from_ptr(error_msg.as_ptr()) }.to_string_lossy()
    );

    let out = render(factory, &[], 16);
    unsafe { deleteCDSPFactory(factory) };
    assert!(out.iter().all(|&v| v == 7.0), "3 + 4 = {}", out[0]);
}

/// Parity: a graph mixing `faust` fragments with schema ops (the exact JSON
/// shape the Python box builder emits: `__call__` = seq(par(args), fragment),
/// arithmetic as binary ops) must render identically to the same DSP written
/// as one pure Faust source program — they are the same signal normal form.
#[test]
fn mixed_fragments_and_ops_match_pure_source() {
    let source = "import(\"stdfaust.lib\"); \
                  process = os.osc(hslider(\"freq\", 330.0, 20.0, 2000.0, 0.1)) \
                            * 0.2 : fi.lowpass(3, 1200.0);";
    let src_def = compiler::compile("par_src", &CompilePayload::Source(source.into()))
        .expect("source must compile");

    let slider = json!({"op": "hslider", "label": "freq", "init": 330.0,
                        "min": 20.0, "max": 2000.0, "step": 0.1});
    let osc = json!({"op": "seq", "in": [
        slider,
        {"op": "faust", "src": "import(\"stdfaust.lib\"); process = os.osc;"}]});
    let graph = json!({"op": "seq", "in": [
        {"op": "par", "in": [
            1200.0,
            {"op": "mul", "in": [osc, 0.2]}]},
        {"op": "faust", "src": "import(\"stdfaust.lib\"); process = fi.lowpass(3);"}]});
    let box_def = compiler::compile("par_box", &CompilePayload::Json(graph.to_string()))
        .expect("mixed graph must compile");

    let a = render(src_def.factory().as_ptr(), &[], 1024);
    let b = render(box_def.factory().as_ptr(), &[], 1024);
    let rms = (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt();
    assert!(rms > 0.05, "the chain must actually sound, rms = {rms}");
    assert_eq!(
        a, b,
        "mixed box graph and pure source must be sample-identical"
    );
}

// ---- CSE: duplicated subtrees share their computation ----
//
// A box client that exposes fragments as reusable values (`x = fragment;
// use x twice`) emits the *same JSON subtree in two positions* — there is no
// reference node in the schema. That is only acceptable because Faust is
// referentially transparent and hash-conses the signal stage: identical
// subtrees become one computation (and identical widgets become one zone).
// These tests pin that guarantee; if they ever fail, value-style reuse on
// the client must be redesigned (explicit `split` routing instead).

/// A stateful library fragment with a named control — exactly the shape the
/// client's `faust` escape hatch emits.
fn osc_fragment() -> serde_json::Value {
    json!({
        "op": "faust",
        "src": "import(\"stdfaust.lib\"); \
                process = os.osc(hslider(\"freq\", 330.0, 20.0, 2000.0, 0.1));"
    })
}

/// Writes the factory's LLVM bitcode to a throwaway file and returns its
/// size — a structural proxy for "how much code was generated" that cannot
/// be fooled by determinism (duplicated oscillators *sound* identical to a
/// shared one; they do not *measure* identical).
fn bitcode_size(factory: *mut llvm_dsp_factory, tag: &str) -> u64 {
    let path = std::env::temp_dir().join(format!("clausters-cse-{}-{tag}.bc", std::process::id()));
    let path_c = CString::new(path.to_str().unwrap()).unwrap();
    let ok = unsafe { writeCDSPFactoryToBitcodeFile(factory, path_c.as_ptr()) };
    assert!(ok, "writeCDSPFactoryToBitcodeFile failed for {tag}");
    let size = std::fs::metadata(&path).unwrap().len();
    let _ = std::fs::remove_file(&path);
    size
}

/// Reusing a fragment by value (duplicated subtree, what `x + x` emits) and
/// routing it explicitly (`x <: +`) must be the same program: identical
/// samples, and the duplicated `hslider` collapses into a single control.
#[test]
fn duplicated_subtree_equals_explicit_split() {
    let frag = osc_fragment();
    let dup = json!({"op": "add", "in": [frag.clone(), frag.clone()]});
    let split = json!({"op": "split", "in": [frag, {"op": "add", "in": ["_", "_"]}]});

    let dup_def = compiler::compile("cse_dup", &CompilePayload::Json(dup.to_string()))
        .expect("duplicated subtree must compile");
    let split_def = compiler::compile("cse_split", &CompilePayload::Json(split.to_string()))
        .expect("split routing must compile");

    // Same label + same parameters -> same signal node -> one widget/zone.
    let names: Vec<&str> = dup_def.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(dup_def.params.len(), 1, "params: {names:?}");
    assert_eq!(split_def.params.len(), 1);

    let a = render(dup_def.factory().as_ptr(), &[], 512);
    let b = render(split_def.factory().as_ptr(), &[], 512);
    let rms = (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt();
    assert!(rms > 0.1, "the oscillator must actually sound, rms = {rms}");
    assert_eq!(a, b, "duplicated and split forms must be sample-identical");
}

/// Nested value-style reuse must not blow up the generated code: 10 levels
/// of `x + x` with duplicated subtrees is 2^10 oscillator copies *textually*
/// but must compile to one oscillator plus a few adds. The bitcode size is
/// the observable: without CSE it would be orders of magnitude larger.
#[test]
fn nested_duplication_does_not_explode_generated_code() {
    let single_def = compiler::compile(
        "cse_single",
        &CompilePayload::Json(osc_fragment().to_string()),
    )
    .expect("single fragment must compile");

    let mut node = osc_fragment();
    for _ in 0..10 {
        node = json!({"op": "add", "in": [node.clone(), node]});
    }
    let deep_def = compiler::compile("cse_deep", &CompilePayload::Json(node.to_string()))
        .expect("deep duplication must compile");
    assert_eq!(deep_def.params.len(), 1);

    let out = render(deep_def.factory().as_ptr(), &[], 128);
    assert!(out.iter().all(|v| v.is_finite()));

    let single = bitcode_size(single_def.factory().as_ptr(), "single");
    let deep = bitcode_size(deep_def.factory().as_ptr(), "deep");
    assert!(
        deep < single * 4,
        "generated code grew from {single} to {deep} bytes: \
         duplicated subtrees are not being shared"
    );
}
