//! Raw FFI over the libfaust C API. Hand-written against the headers of the
//! libfaust build we link (faust 2.85.5):
//!
//! - `faust/dsp/libfaust-signal-c.h` — lib context + Signal API (`Csig*`).
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

/// `Signal` in the C API (Signal API, `libfaust-signal-c.h`). The same opaque
/// arena node as a box; a distinct alias keeps the two interpreters readable.
pub type FaustSignal = *mut CTree;

/// Scalar type of a foreign constant/variable (`enum SType` in the C API).
/// Used by `CsigFConst`/`CsigFVar` (and the box twins) to declare e.g. the
/// integer runtime constant `fSamplingFreq` behind `ma.SR`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum SType {
    Int = 0,
    Real = 1,
}

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
pub type AddButtonFun = unsafe extern "C" fn(ui: *mut c_void, label: *const c_char, zone: *mut f32);
pub type AddSliderFun = unsafe extern "C" fn(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut f32,
    init: f32,
    min: f32,
    max: f32,
    step: f32,
);
pub type AddBargraphFun =
    unsafe extern "C" fn(ui: *mut c_void, label: *const c_char, zone: *mut f32, min: f32, max: f32);
pub type AddSoundfileFun = unsafe extern "C" fn(
    ui: *mut c_void,
    label: *const c_char,
    url: *const c_char,
    sf_zone: *mut *mut c_void,
);

// ---- soundfile (faust/gui/Soundfile.h) ----
// The DSP reads `fBuffers[chan][fOffset[part] + clamp(i, 0, fLength[part])]`,
// so each channel array needs `fOffset + fLength + 1` samples (the index is
// inclusive). `fBuffers` holds `FAUST_MAX_CHAN` pointers; channels past the
// real ones alias earlier ones (Faust's `shareBuffers`).

/// Max channel pointers in a `Soundfile`'s `fBuffers` (`MAX_CHAN`).
pub const FAUST_MAX_CHAN: usize = 64;
/// Parts array length (`MAX_SOUNDFILE_PARTS`); we fill them all with one part.
pub const FAUST_MAX_SOUNDFILE_PARTS: usize = 256;
/// Length/SR of an empty (silent) soundfile (`BUFFER_SIZE`/`SAMPLE_RATE`).
pub const FAUST_SOUNDFILE_EMPTY_FRAMES: i32 = 1024;
pub const FAUST_SOUNDFILE_EMPTY_SR: i32 = 44100;

/// Faust's `Soundfile` struct. **Packed** — the LLVM backend accesses it with
/// no padding (the header is `__attribute__((packed))`). We build and own one
/// per `soundfile` zone; Faust never frees it.
#[repr(C, packed)]
pub struct Soundfile {
    /// `float**`: `FAUST_MAX_CHAN` non-interleaved channel arrays.
    pub fBuffers: *mut c_void,
    /// `int[FAUST_MAX_SOUNDFILE_PARTS]`: frames per part.
    pub fLength: *mut c_int,
    /// `int[FAUST_MAX_SOUNDFILE_PARTS]`: sample rate per part.
    pub fSR: *mut c_int,
    /// `int[FAUST_MAX_SOUNDFILE_PARTS]`: start frame of each part.
    pub fOffset: *mut c_int,
    /// Number of real (non-aliased) channels.
    pub fChannels: c_int,
    /// Number of loaded parts.
    pub fParts: c_int,
    /// Sample format: always `false` (we compile `-single`, f32).
    pub fIsDouble: bool,
}
pub type DeclareFun =
    unsafe extern "C" fn(ui: *mut c_void, zone: *mut f32, key: *const c_char, value: *const c_char);

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
    // 2.81.10 through 2.85.5), `boxFmod()` returns the `abs` primitive.
    // `boxes` builds fmod through a `CDSPToBoxes` fragment instead (same for
    // `cos`, whose `CboxCosAux` is the other broken one).
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
    pub fn CboxSelect3Aux(selector: FaustBox, b1: FaustBox, b2: FaustBox, b3: FaustBox)
    -> FaustBox;

    // UI elements: named parameters, the future `/node_set` controls (F3).
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
    // Foreign constant/variable, box twin of `CsigFConst`/`CsigFVar`.
    pub fn CboxFConst(ty: SType, name: *const c_char, file: *const c_char) -> FaustBox;
    pub fn CboxFVar(ty: SType, name: *const c_char, file: *const c_char) -> FaustBox;
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

    // ---- Signal API (libfaust-signal-c.h) ----
    // The lower-level API: each `Signal` is one output. Inputs are explicit
    // (`CsigInput`), delays are explicit (`CsigDelay`/`CsigDelay1`), and
    // feedback is `CsigSelf()` inside `CsigRecursion(s)`. Built inside the
    // same lib-context bracket as boxes. (See `faust::signals` for the schema.)
    pub fn CsigInt(n: c_int) -> FaustSignal;
    pub fn CsigReal(n: f64) -> FaustSignal;
    pub fn CsigInput(idx: c_int) -> FaustSignal;
    pub fn CsigDelay(s: FaustSignal, del: FaustSignal) -> FaustSignal;
    pub fn CsigDelay1(s: FaustSignal) -> FaustSignal;
    pub fn CsigIntCast(s: FaustSignal) -> FaustSignal;
    pub fn CsigFloatCast(s: FaustSignal) -> FaustSignal;

    // Foreign constant/variable: a scalar read from the runtime (e.g. the
    // `int fSamplingFreq` behind `ma.SR`) named by `name`, declared in include
    // `file`. The compiler resolves it at `initCDSPInstance` time, so it tracks
    // the engine's actual sample rate.
    pub fn CsigFConst(ty: SType, name: *const c_char, file: *const c_char) -> FaustSignal;
    pub fn CsigFVar(ty: SType, name: *const c_char, file: *const c_char) -> FaustSignal;

    // Feedback: `CsigSelf()` refers to the recursive signal inside the body
    // passed to `CsigRecursion` (one implicit sample of delay).
    pub fn CsigSelf() -> FaustSignal;
    pub fn CsigRecursion(s: FaustSignal) -> FaustSignal;

    // Binary operators (explicit, no `Aux` suffix in the Signal API).
    pub fn CsigAdd(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigSub(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigMul(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigDiv(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigRem(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigLeftShift(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    // `CsigLRightShift` is deliberately not bound: the signal type checker
    // has no case for the `kLRsh` opcode (faust#1264, fixed in PR
    // faust#1272), so factory creation fails with an `ASSERT : unrecognized
    // opcode : 7` — and in 2.81.x aborted the host process. The schema's
    // `rsh` is the arithmetic shift below; the canary in
    // `tests/faust_signal.rs` flags when the bug is gone and `lrsh` can be
    // exposed.
    pub fn CsigARightShift(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigGT(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigLT(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigGE(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigLE(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigEQ(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigNE(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigAND(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigOR(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigXOR(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigPow(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigMin(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigMax(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigFmod(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigRemainder(x: FaustSignal, y: FaustSignal) -> FaustSignal;
    pub fn CsigAtan2(x: FaustSignal, y: FaustSignal) -> FaustSignal;

    // Unary math (the Signal API exposes `rint` but not `round`).
    pub fn CsigAbs(x: FaustSignal) -> FaustSignal;
    pub fn CsigAcos(x: FaustSignal) -> FaustSignal;
    pub fn CsigAsin(x: FaustSignal) -> FaustSignal;
    pub fn CsigAtan(x: FaustSignal) -> FaustSignal;
    pub fn CsigCos(x: FaustSignal) -> FaustSignal;
    pub fn CsigSin(x: FaustSignal) -> FaustSignal;
    pub fn CsigTan(x: FaustSignal) -> FaustSignal;
    pub fn CsigExp(x: FaustSignal) -> FaustSignal;
    pub fn CsigExp10(x: FaustSignal) -> FaustSignal;
    pub fn CsigLog(x: FaustSignal) -> FaustSignal;
    pub fn CsigLog10(x: FaustSignal) -> FaustSignal;
    pub fn CsigSqrt(x: FaustSignal) -> FaustSignal;
    pub fn CsigFloor(x: FaustSignal) -> FaustSignal;
    pub fn CsigCeil(x: FaustSignal) -> FaustSignal;
    pub fn CsigRint(x: FaustSignal) -> FaustSignal;

    pub fn CsigSelect2(selector: FaustSignal, s1: FaustSignal, s2: FaustSignal) -> FaustSignal;
    pub fn CsigSelect3(
        selector: FaustSignal,
        s1: FaustSignal,
        s2: FaustSignal,
        s3: FaustSignal,
    ) -> FaustSignal;

    // Tables: the Signal API primitives take their inputs directly (no
    // `seq`/`par` wrapping). `CsigWaveform` wants a NULL-terminated array of
    // `CsigInt`/`CsigReal` signals (its size is `CsigInt(values.len())`).
    pub fn CsigReadOnlyTable(n: FaustSignal, init: FaustSignal, ridx: FaustSignal) -> FaustSignal;
    pub fn CsigWriteReadTable(
        n: FaustSignal,
        init: FaustSignal,
        widx: FaustSignal,
        wsig: FaustSignal,
        ridx: FaustSignal,
    ) -> FaustSignal;
    pub fn CsigWaveform(wf: *mut FaustSignal) -> FaustSignal;

    // UI: init/min/max/step are `Signal`s (build them with `CsigReal`).
    pub fn CsigButton(label: *const c_char) -> FaustSignal;
    pub fn CsigCheckbox(label: *const c_char) -> FaustSignal;
    pub fn CsigHSlider(
        label: *const c_char,
        init: FaustSignal,
        min: FaustSignal,
        max: FaustSignal,
        step: FaustSignal,
    ) -> FaustSignal;
    pub fn CsigVSlider(
        label: *const c_char,
        init: FaustSignal,
        min: FaustSignal,
        max: FaustSignal,
        step: FaustSignal,
    ) -> FaustSignal;
    pub fn CsigNumEntry(
        label: *const c_char,
        init: FaustSignal,
        min: FaustSignal,
        max: FaustSignal,
        step: FaustSignal,
    ) -> FaustSignal;
    pub fn CsigHBargraph(
        label: *const c_char,
        min: FaustSignal,
        max: FaustSignal,
        s: FaustSignal,
    ) -> FaustSignal;
    pub fn CsigVBargraph(
        label: *const c_char,
        min: FaustSignal,
        max: FaustSignal,
        s: FaustSignal,
    ) -> FaustSignal;

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
    /// Signal API factory: `signals` is a NULL-terminated array of output
    /// signals (the DSP's outputs).
    pub fn createCDSPFactoryFromSignals(
        name_app: *const c_char,
        signals: *mut FaustSignal,
        argc: c_int,
        argv: *const *const c_char,
        target: *const c_char,
        error_msg: *mut c_char,
        opt_level: c_int,
    ) -> *mut llvm_dsp_factory;
    pub fn deleteCDSPFactory(factory: *mut llvm_dsp_factory) -> bool;

    /// libfaust version string (static storage, never freed). Keys the
    /// bitcode cache: a different libfaust/LLVM may emit incompatible IR.
    pub fn getCLibFaustVersion() -> *const c_char;
    /// Writes the factory's LLVM bitcode to `bit_code_path`; `false` on
    /// failure. The IR is target-independent (re-JIT'd to the host on read),
    /// so a cached `.bc` is portable across machines of the same libfaust.
    pub fn writeCDSPFactoryToBitcodeFile(
        factory: *mut llvm_dsp_factory,
        bit_code_path: *const c_char,
    ) -> bool;
    /// Re-creates a factory from a bitcode file (`target` `""` = host).
    /// Returns null on failure (e.g. incompatible/corrupt bitcode), filling
    /// `error_msg`; callers fall back to a fresh compile.
    pub fn readCDSPFactoryFromBitcodeFile(
        bit_code_path: *const c_char,
        target: *const c_char,
        error_msg: *mut c_char,
        opt_level: c_int,
    ) -> *mut llvm_dsp_factory;

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
