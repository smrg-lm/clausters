//! The processing threads run in flush-to-zero mode (see `dsp::denormals`):
//! subnormal floats from decaying DSP state would otherwise be resolved in
//! microcode, 10-100x slower — enough to blow the audio callback budget.
//!
//! Each `#[test]` runs on its own thread, so arming the FPU here does not
//! leak into other tests.

use std::hint::black_box;

use clausters::dsp::denormals::flush_to_zero;

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
    assert_eq!(black_box(f32::MIN_POSITIVE) * black_box(2.0), f32::MIN_POSITIVE * 2.0);
}
