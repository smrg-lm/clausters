//! The processing threads run in flush-to-zero mode (see `dsp::denormals`):
//! subnormal floats from decaying DSP state would otherwise be resolved in
//! microcode, 10-100x slower — enough to blow the audio callback budget.
//!
//! Each `#[test]` runs on its own thread, so arming the FPU here does not
//! leak into other tests.

use std::hint::black_box;

use clausters::dsp::denormals::{flush_to_zero, normal_precision};

/// On the architectures with an implementation, arming the thread must make
/// subnormal results and operands collapse to zero.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn subnormal_math_flushes_after_arming() {
    flush_to_zero();
    // Result flushing: half the smallest normal would be subnormal.
    let halved = black_box(f32::MIN_POSITIVE) * black_box(0.5f32);
    assert_eq!(halved, 0.0, "subnormal result must flush to zero");
    // Operand flushing (DAZ on x86, FZ covers inputs on aarch64): a
    // subnormal input scaled back into the normal range stays zero.
    let scaled = black_box(1.0e-40f32) * black_box(1.0e30f32);
    assert_eq!(scaled, 0.0, "subnormal operand must be treated as zero");
}

/// Everywhere: calling it twice (the cpal callback re-arms every buffer)
/// must be harmless, and normal math must be untouched.
#[test]
fn arming_is_idempotent_and_preserves_normal_math() {
    flush_to_zero();
    flush_to_zero();
    let x = black_box(0.25f32) * black_box(8.0f32);
    assert_eq!(x, 2.0);
    assert_eq!(
        black_box(f32::MIN_POSITIVE) * black_box(2.0),
        f32::MIN_POSITIVE * 2.0
    );
}

/// `normal_precision` opens an IEEE window inside an armed thread and
/// restores the armed mode on exit.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn normal_precision_brackets_the_armed_mode() {
    flush_to_zero();
    let inside = normal_precision(|| black_box(f32::MIN_POSITIVE) * black_box(0.5f32));
    // Compare bit patterns: out here DAZ is armed again, so the subnormal
    // *value* would flush to zero as a comparison operand.
    assert_ne!(
        inside.to_bits(),
        0,
        "inside the bracket subnormals must survive"
    );
    let outside = black_box(f32::MIN_POSITIVE) * black_box(0.5f32);
    assert_eq!(outside, 0.0, "the armed mode must be restored on exit");
}

/// The regression that motivated `normal_precision`: the NRT renderer
/// compiles scored defs on its flush-to-zero render thread, and libfaust's
/// interval typing does double math that aborts the process under FTZ/DAZ
/// (`intervalPow.cpp: x.lo() > 0`) — `fi.lowpass` fed through box
/// composition is a minimal trigger. The compile must succeed from an armed
/// thread and leave the thread armed.
#[cfg(feature = "faust")]
#[test]
fn faust_compile_survives_a_flush_to_zero_thread() {
    use clausters::faust::compiler::{CompilePayload, compile};

    flush_to_zero();
    let graph = serde_json::json!({"op": "seq", "in": [
        {"op": "par", "in": [
            {"op": "hslider", "label": "cutoff",
             "init": 900.0, "min": 50.0, "max": 8000.0, "step": 1.0},
            {"op": "seq", "in": [
                {"op": "hslider", "label": "freq",
                 "init": 220.0, "min": 20.0, "max": 2000.0, "step": 0.1},
                {"op": "faust",
                 "src": "import(\"stdfaust.lib\"); process = os.osc;"}]}]},
        {"op": "faust",
         "src": "import(\"stdfaust.lib\"); process = fi.lowpass(3);"}]});
    let def = compile("ftz_lp", &CompilePayload::Json(graph.to_string()))
        .expect("compiling on an armed thread must not abort or fail");
    assert_eq!(def.params.len(), 2);
    let still_armed = black_box(f32::MIN_POSITIVE) * black_box(0.5f32);
    assert_eq!(still_armed, 0.0, "compile() must not leave the thread IEEE");
}
