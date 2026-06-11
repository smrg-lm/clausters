//! Denormal (subnormal) float protection.
//!
//! Recursive DSP state decaying towards zero — filter tails, envelope ends,
//! Faust recursions — lands in subnormal floats, which many CPUs resolve in
//! microcode 10-100x slower than normal numbers: enough to blow the audio
//! callback budget exactly when a sound fades out. Two complementary
//! measures keep them out of the signal path:
//!
//! - [`flush_to_zero`] switches the **calling thread's** FPU to treat
//!   subnormals as zero. The cpal callback re-arms it on every buffer
//!   ([`crate::server::backend`]) and the offline renderer arms it once per
//!   render ([`crate::server::render`]) — both modes, so an NRT render stays
//!   sample-identical to a live take (the M7 guarantee).
//! - Faust factories are created with `-ftz 2`
//!   ([`crate::faust::compiler::FaustArgs`]): the generated code itself
//!   flushes recursive variables below the normal range, independent of the
//!   host FPU mode and of the architecture.
//!
//! Guarded by `tests/denormals.rs` (the FPU switch) and the Faust tail test
//! in `tests/golden.rs` (`-ftz 2` end to end).

/// Enables flush-to-zero (and treat-denormal-operands-as-zero where the
/// architecture has it) for the calling thread. Idempotent and cheap — a
/// couple of register accesses — so calling it once per audio callback is
/// fine. On architectures without an implementation it is a no-op:
/// correctness never changes, only the worst-case speed of subnormal math.
///
/// Altering the FPU mode is the long-standing audio practice (scsynth and
/// JUCE do exactly this); it affects every float operation on the thread
/// afterwards, which is the point.
pub fn flush_to_zero() {
    // MXCSR bit 15 = FTZ (flush results), bit 6 = DAZ (flush operands).
    // The `_mm_getcsr`/`_mm_setcsr` intrinsics are deprecated in favor of
    // inline asm, so read-modify-write the register directly.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut csr: u32 = 0;
        core::arch::asm!("stmxcsr [{0}]", in(reg) &mut csr, options(nostack));
        csr |= 0x8040;
        core::arch::asm!("ldmxcsr [{0}]", in(reg) &csr, options(nostack));
    }
    // FPCR bit 24 = FZ: flush subnormal inputs and outputs to zero.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let fpcr: u64;
        core::arch::asm!("mrs {0}, fpcr", out(reg) fpcr, options(nomem, nostack));
        core::arch::asm!("msr fpcr, {0}", in(reg) fpcr | (1 << 24), options(nomem, nostack));
    }
}
