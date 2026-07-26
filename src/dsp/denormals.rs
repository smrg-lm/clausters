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
//!   (`crate::server::backend`) and the offline renderer arms it once per
//!   render ([`crate::server::render`]) — both modes, so an NRT render stays
//!   sample-identical to a live take (the M7 guarantee).
//! - Faust factories are created with `-ftz 2`
//!   (`FaustArgs::defaults` in `crate::faust::compiler`): the generated code itself
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

/// Runs `f` with the thread's FPU in **normal** (IEEE, no flush-to-zero)
/// mode, restoring the previous mode afterwards — even on unwind.
///
/// The one legitimate caller is the Faust *compiler* path: libfaust's
/// front-end does real double math (the interval algebra behind its typing;
/// LLVM's constant folding), and running it with FTZ/DAZ armed both changes
/// results and trips its internal assertions (`intervalPow.cpp: x.lo() > 0`
/// aborts the process when a subnormal-positive bound is flushed to zero).
/// The live server compiles on the network/compiler thread, where FTZ is
/// never armed; the NRT renderer compiles scored defs *on the render
/// thread*, which is. This guard makes the two environments identical,
/// which is also what keeps an NRT render sample-identical to a live take.
/// Generated *DSP* code is unaffected: it flushes on its own (`-ftz 2`).
pub fn normal_precision<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(#[allow(dead_code)] u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            #[cfg(target_arch = "x86_64")]
            unsafe {
                let csr = self.0 as u32;
                core::arch::asm!("ldmxcsr [{0}]", in(reg) &csr, options(nostack));
            }
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("msr fpcr, {0}", in(reg) self.0, options(nomem, nostack));
            }
        }
    }
    let _restore;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut csr: u32 = 0;
        core::arch::asm!("stmxcsr [{0}]", in(reg) &mut csr, options(nostack));
        _restore = Restore(csr as u64);
        let normal = csr & !0x8040;
        core::arch::asm!("ldmxcsr [{0}]", in(reg) &normal, options(nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let fpcr: u64;
        core::arch::asm!("mrs {0}, fpcr", out(reg) fpcr, options(nomem, nostack));
        _restore = Restore(fpcr);
        core::arch::asm!("msr fpcr, {0}", in(reg) fpcr & !(1 << 24), options(nomem, nostack));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        _restore = Restore(0);
    }
    f()
}
