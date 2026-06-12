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

use std::ffi::{c_char, c_int, c_void};

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

// ---- UI parameter discovery (faust/gui/CInterface.h) ----
//
// `buildUserInterfaceCDSPInstance` walks the instance's UI tree calling these
// callbacks; the zone pointers it hands out are `FAUSTFLOAT*` (f32, we always
// compile `-single`) into the instance's own memory — writing them is a plain
// aligned store, the RT-safe way to set parameters.

pub type OpenBoxFun = unsafe extern "C" fn(ui: *mut c_void, label: *const c_char);
pub type CloseBoxFun = unsafe extern "C" fn(ui: *mut c_void);
pub type AddButtonFun =
    unsafe extern "C" fn(ui: *mut c_void, label: *const c_char, zone: *mut f32);
pub type AddSliderFun = unsafe extern "C" fn(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut f32,
    init: f32,
    min: f32,
    max: f32,
    step: f32,
);
pub type AddBargraphFun = unsafe extern "C" fn(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut f32,
    min: f32,
    max: f32,
);
pub type AddSoundfileFun = unsafe extern "C" fn(
    ui: *mut c_void,
    label: *const c_char,
    url: *const c_char,
    sf_zone: *mut *mut c_void,
);
pub type DeclareFun = unsafe extern "C" fn(
    ui: *mut c_void,
    zone: *mut f32,
    key: *const c_char,
    value: *const c_char,
);

/// C UI visitor: every field must be a valid function pointer (libfaust calls
/// them unconditionally). `uiInterface` is passed back as the first argument.
#[repr(C)]
pub struct UIGlue {
    pub uiInterface: *mut c_void,
    pub openTabBox: OpenBoxFun,
    pub openHorizontalBox: OpenBoxFun,
    pub openVerticalBox: OpenBoxFun,
    pub closeBox: CloseBoxFun,
    pub addButton: AddButtonFun,
    pub addCheckButton: AddButtonFun,
    pub addVerticalSlider: AddSliderFun,
    pub addHorizontalSlider: AddSliderFun,
    pub addNumEntry: AddSliderFun,
    pub addHorizontalBargraph: AddBargraphFun,
    pub addVerticalBargraph: AddBargraphFun,
    pub addSoundfile: AddSoundfileFun,
    pub declare: DeclareFun,
}

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

    // Binary operators, applied form (`CboxAdd()` without arguments is the
    // primitive 2-input box; C has no overloading, hence the `Aux` suffix).
    pub fn CboxAddAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxSubAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxMulAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxDivAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    // `CboxFmodAux` is deliberately not bound: broken upstream (faust
    // 2.81.10 through master-dev), `boxFmod()` returns the `abs` primitive.
    // `boxes` builds fmod through a `CDSPToBoxes` fragment instead.
    pub fn CboxPowAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxMinAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxMaxAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxAtan2Aux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxGTAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxLTAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxGEAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxLEAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxEQAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxNEAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxANDAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxORAux(b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxXORAux(b1: FaustBox, b2: FaustBox) -> FaustBox;

    // Unary math, applied form.
    pub fn CboxSinAux(x: FaustBox) -> FaustBox;
    pub fn CboxCosAux(x: FaustBox) -> FaustBox;
    pub fn CboxTanAux(x: FaustBox) -> FaustBox;
    pub fn CboxAsinAux(x: FaustBox) -> FaustBox;
    pub fn CboxAcosAux(x: FaustBox) -> FaustBox;
    pub fn CboxAtanAux(x: FaustBox) -> FaustBox;
    pub fn CboxExpAux(x: FaustBox) -> FaustBox;
    pub fn CboxExp10Aux(x: FaustBox) -> FaustBox;
    pub fn CboxLogAux(x: FaustBox) -> FaustBox;
    pub fn CboxLog10Aux(x: FaustBox) -> FaustBox;
    pub fn CboxSqrtAux(x: FaustBox) -> FaustBox;
    pub fn CboxAbsAux(x: FaustBox) -> FaustBox;
    pub fn CboxFloorAux(x: FaustBox) -> FaustBox;
    pub fn CboxCeilAux(x: FaustBox) -> FaustBox;
    pub fn CboxRintAux(x: FaustBox) -> FaustBox;
    pub fn CboxRoundAux(x: FaustBox) -> FaustBox;
    pub fn CboxIntCastAux(x: FaustBox) -> FaustBox;
    pub fn CboxFloatCastAux(x: FaustBox) -> FaustBox;

    // Delays and selectors.
    pub fn CboxDelayAux(b: FaustBox, del: FaustBox) -> FaustBox;
    pub fn CboxSelect2Aux(selector: FaustBox, b1: FaustBox, b2: FaustBox) -> FaustBox;
    pub fn CboxSelect3Aux(
        selector: FaustBox,
        b1: FaustBox,
        b2: FaustBox,
        b3: FaustBox,
    ) -> FaustBox;

    // UI elements: named parameters, the future `/n_set` controls (F3).
    pub fn CboxHSlider(
        label: *const c_char,
        init: FaustBox,
        min: FaustBox,
        max: FaustBox,
        step: FaustBox,
    ) -> FaustBox;
    pub fn CboxVSlider(
        label: *const c_char,
        init: FaustBox,
        min: FaustBox,
        max: FaustBox,
        step: FaustBox,
    ) -> FaustBox;
    pub fn CboxNumEntry(
        label: *const c_char,
        init: FaustBox,
        min: FaustBox,
        max: FaustBox,
        step: FaustBox,
    ) -> FaustBox;
    pub fn CboxButton(label: *const c_char) -> FaustBox;
    pub fn CboxCheckbox(label: *const c_char) -> FaustBox;
    pub fn CboxHGroup(label: *const c_char, group: FaustBox) -> FaustBox;
    pub fn CboxVGroup(label: *const c_char, group: FaustBox) -> FaustBox;

    // Tables (F5). `CboxWaveform` takes a NULL-terminated array of
    // `CboxInt`/`CboxReal` boxes; the primitives are the 0-argument forms
    // (3 inputs for rdtable, 5 for rwtable), applied via `CboxSeq`/`CboxPar`
    // exactly like upstream's own `Cbox*TableAux` helpers.
    pub fn CboxWaveform(wf: *mut FaustBox) -> FaustBox;
    pub fn CboxReadOnlyTable() -> FaustBox;
    pub fn CboxWriteReadTable() -> FaustBox;

    /// Compiles Faust *source* into a box usable inside the current lib
    /// context — the schema's escape hatch to the Faust stdlib. On error
    /// returns NULL and fills `error_msg`.
    pub fn CDSPToBoxes(
        name_app: *const c_char,
        dsp_content: *const c_char,
        argc: c_int,
        argv: *const *const c_char,
        inputs: *mut c_int,
        outputs: *mut c_int,
        error_msg: *mut c_char,
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
    pub fn buildUserInterfaceCDSPInstance(dsp: *mut llvm_dsp, ui: *mut UIGlue);
    /// The only call allowed on the audio thread.
    pub fn computeCDSPInstance(
        dsp: *mut llvm_dsp,
        count: c_int,
        input: *mut *mut f32,
        output: *mut *mut f32,
    );
}
