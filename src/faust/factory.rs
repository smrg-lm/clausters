//! Owned wrapper around a JIT-compiled libfaust factory.

use std::ptr::NonNull;

use crate::faust::ffi;

/// A compiled Faust DSP factory. Owns the underlying libfaust object and
/// deletes it on drop.
///
/// Refcounting: the factory lives inside an `Arc<FaustDef>` (see
/// `faust::synth`); every `FaustSynth` instance keeps a clone of that Arc,
/// so the factory cannot be deleted while instances live —
/// `deleteCDSPFactory` with live instances is UB.
pub struct FaustFactory {
    ptr: NonNull<ffi::llvm_dsp_factory>,
}

// SAFETY: the factory is an immutable handle after creation; libfaust
// factories are internally reference-counted and instance creation from a
// shared factory is supported. We create/delete instances and factories on
// non-RT threads only (compiler/network); the audio thread never touches a
// factory directly.
unsafe impl Send for FaustFactory {}
unsafe impl Sync for FaustFactory {}

impl FaustFactory {
    /// # Safety
    /// `ptr` must come from a successful `createCDSPFactory*` call and
    /// ownership moves to the wrapper (no other `deleteCDSPFactory` on it).
    pub unsafe fn from_raw(ptr: *mut ffi::llvm_dsp_factory) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self { ptr })
    }

    pub fn as_ptr(&self) -> *mut ffi::llvm_dsp_factory {
        self.ptr.as_ptr()
    }
}

impl Drop for FaustFactory {
    /// Must run on a non-RT thread (it frees memory). With the factory table
    /// on the network thread and instances refcounting through `Arc`, the
    /// last drop always happens there or on the compiler thread.
    fn drop(&mut self) {
        unsafe {
            ffi::deleteCDSPFactory(self.ptr.as_ptr());
        }
    }
}
