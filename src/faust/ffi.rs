//! Raw FFI over the libfaust C API. Hand-written against the headers of the
//! libfaust build we link (faust 2.81.10):
//!
//! - `faust/dsp/libfaust-signal-c.h` — lib context.
//! - `faust/dsp/libfaust-box-c.h` — Box API (`Cbox*`).
//! - `faust/dsp/llvm-dsp-c.h` — LLVM JIT factory/instance.
//!
//! Only the surface needed so far. Naming follows the C symbols verbatim
//! (hence the lint allowances). `FAUSTFLOAT` is `float` (f32) — libfaust's
//! default and required to match our buses; factories must be created
//! without `-double`.
//!
//! Safety summary (full rules in the `faust-embedding` skill):
//! - `createLibContext`/`destroyLibContext` bracket *every* box construction;
//!   boxes are arena pointers valid only inside that bracket. The context is
//!   global and not thread-safe.
//! - A factory outlives the context; instances must not outlive their
//!   factory (`deleteCDSPFactory` with live instances is UB).
//! - Only `computeCDSPInstance` is RT-safe.

#![allow(non_snake_case, non_camel_case_types)]

use std::ffi::{c_char, c_int};

/// Opaque box-expression node, owned by the lib context arena.
#[repr(C)]
pub struct CTree {
    _private: [u8; 0],
}

/// `Box` in the C API; renamed to avoid clashing with `std::boxed::Box`.
pub type FaustBox = *mut CTree;

#[repr(C)]
pub struct llvm_dsp_factory {
    _private: [u8; 0],
}

#[repr(C)]
pub struct llvm_dsp {
    _private: [u8; 0],
}

/// Required size of the `error_msg` buffer for factory creation.
pub const ERROR_MSG_SIZE: usize = 4096;

unsafe extern "C" {
    // ---- lib context (libfaust-signal-c.h) ----
    pub fn createLibContext();
    pub fn destroyLibContext();

    // ---- Box API (libfaust-box-c.h) ----
    pub fn CboxInt(n: c_int) -> FaustBox;
    pub fn CboxReal(n: f64) -> FaustBox;
    pub fn CboxWire() -> FaustBox;
    pub fn CboxCut() -> FaustBox;
    pub fn CboxSeq(x: FaustBox, y: FaustBox) -> FaustBox;
    pub fn CboxPar(x: FaustBox, y: FaustBox) -> FaustBox;
    pub fn CboxSplit(x: FaustBox, y: FaustBox) -> FaustBox;
    pub fn CboxMerge(x: FaustBox, y: FaustBox) -> FaustBox;
    pub fn CboxRec(x: FaustBox, y: FaustBox) -> FaustBox;
    pub fn CboxAddAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxSubAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxMulAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxDivAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxSinAux(x: FaustBox) -> FaustBox;
    pub fn CboxFloorAux(x: FaustBox) -> FaustBox;
    pub fn CboxHSlider(
        label: *const c_char,
        init: FaustBox,
        min: FaustBox,
        max: FaustBox,
        step: FaustBox,
    ) -> FaustBox;

    // ---- LLVM JIT (llvm-dsp-c.h) ----
    /// Compiles Faust *source code* (no lib context needed). F1 uses this;
    /// F2 switches to JSON→Box construction + `createCDSPFactoryFromBoxes`.
    pub fn createCDSPFactoryFromString(
        name_app: *const c_char,
        dsp_content: *const c_char,
        argc: c_int,
        argv: *const *const c_char,
        target: *const c_char,
        error_msg: *mut c_char,
        opt_level: c_int,
    ) -> *mut llvm_dsp_factory;
    pub fn createCDSPFactoryFromBoxes(
        name_app: *const c_char,
        box_: FaustBox,
        argc: c_int,
        argv: *const *const c_char,
        target: *const c_char,
        error_msg: *mut c_char,
        opt_level: c_int,
    ) -> *mut llvm_dsp_factory;
    pub fn deleteCDSPFactory(factory: *mut llvm_dsp_factory) -> bool;

    pub fn createCDSPInstance(factory: *mut llvm_dsp_factory) -> *mut llvm_dsp;
    pub fn deleteCDSPInstance(dsp: *mut llvm_dsp);
    pub fn initCDSPInstance(dsp: *mut llvm_dsp, sample_rate: c_int);
    pub fn getNumInputsCDSPInstance(dsp: *mut llvm_dsp) -> c_int;
    pub fn getNumOutputsCDSPInstance(dsp: *mut llvm_dsp) -> c_int;
    /// The only call allowed on the audio thread.
    pub fn computeCDSPInstance(
        dsp: *mut llvm_dsp,
        count: c_int,
        input: *mut *mut f32,
        output: *mut *mut f32,
    );
}
