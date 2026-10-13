//! Denormal (subnormal) float protection.
//!
//! Recursive DSP state decaying towards zero -- filter tails, envelope ends,
//! Faust recursions -- lands in subnormal floats, which many CPUs resolve in
//! microcode 10-100x slower than normal numbers: enough to blow the audio
//! callback budget exactly when a sound fades out. Two complementary
//! measures keep them out of the signal path:
//!
//! - [`flush_to_zero`] switches the **calling thread's** FPU to treat
//!   subnormals as zero. The cpal callback re-arms it on every buffer
//!   (`crate::server::backend`) and the offline renderer arms it once per
//!   render ([`crate::server::render`]) -- both modes, so an NRT render stays
//!   sample-identical to a live take.
//! - Faust factories are created with `-ftz 2`
//!   (`FaustArgs::defaults` in `crate::faust::compiler`): the generated code itself
//!   flushes recursive variables below the normal range, independent of the
//!   host FPU mode and of the architecture.
//!
//! Guarded by `tests/denormals.rs` (the FPU switch) and the Faust tail test
//! in `tests/golden.rs` (`-ftz 2` end to end).

pub use clausters_core::denormals::{flush_to_zero, normal_precision};
