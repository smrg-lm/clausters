//! `FaustSynth` (F3): a JIT-compiled Faust instance living in the node tree.
//!
//! [`FaustDef`] is what the def table stores: the compiled factory plus the
//! parameters and I/O arity discovered by probing one throwaway instance on
//! the compiler thread. [`FaustSynth`] is the per-node instance: built on the
//! network thread (`createCDSPInstance` + `init` allocate), processed on the
//! audio thread (`compute` is the only RT-safe libfaust call) and dropped on
//! the network thread via the garbage FIFO.
//!
//! # Controls
//!
//! Control indices `0..params.len()` are the def's UI parameters in
//! declaration order; `/n_set` writes the value through the instance's
//! `FAUSTFLOAT*` zone — a plain aligned store, RT-safe (Faust reads zones at
//! block boundaries). Two reserved names follow the Faust parameters:
//! `"out"` (index `params.len()`) and `"in"` (one past it), the first audio
//! bus the synth's outputs/inputs map to. Parameter names are the bare UI
//! labels (group paths are ignored); on a collision the first declaration
//! wins. Defaults: `out = 0` (hardware), `in = 0`.
//!
//! # Bus mapping
//!
//! Faust I/O is non-interleaved `float**`, the same layout as our buses, but
//! the synth stages through private block buffers: outputs **sum** into the
//! buses (`Out` UGen semantics, so synths mix) and inputs are copied out
//! before outputs are written (an in-place `in == out` chain stays correct).

use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use crate::dsp::{Block, NUM_AUDIO_BUSES, ProcessCtx};
use crate::faust::factory::FaustFactory;
use crate::faust::ffi;
use crate::node::{ControlMap, SynthNode};

/// One named parameter of a Faust def, as declared by its UI elements.
pub struct ParamSpec {
    pub name: String,
    pub init: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

/// A compiled Faust def ready to instantiate: the factory plus the metadata
/// shared by every instance. The def table holds it as `Arc<FaustDef>` and
/// every [`FaustSynth`] keeps a clone, so `/d_free` with live synths cannot
/// delete the factory under them (`deleteCDSPFactory` with live instances is
/// UB).
pub struct FaustDef {
    factory: FaustFactory,
    pub params: Vec<ParamSpec>,
    pub num_inputs: usize,
    pub num_outputs: usize,
}

impl FaustDef {
    /// Probes a throwaway instance to discover parameters and I/O arity.
    /// Allocates and must run off the audio thread (the compiler thread does,
    /// right after creating the factory).
    pub fn probe(factory: FaustFactory) -> Result<Self, String> {
        let dsp = unsafe { ffi::createCDSPInstance(factory.as_ptr()) };
        let Some(dsp) = NonNull::new(dsp) else {
            return Err("probe instance creation failed".into());
        };
        // Parameters and arity do not depend on the sample rate; any valid
        // one will do for the probe.
        unsafe { ffi::initCDSPInstance(dsp.as_ptr(), 48_000) };
        let ui = collect_ui(dsp.as_ptr());
        let num_inputs = unsafe { ffi::getNumInputsCDSPInstance(dsp.as_ptr()) } as usize;
        let num_outputs = unsafe { ffi::getNumOutputsCDSPInstance(dsp.as_ptr()) } as usize;
        unsafe { ffi::deleteCDSPInstance(dsp.as_ptr()) };
        if num_inputs > NUM_AUDIO_BUSES || num_outputs > NUM_AUDIO_BUSES {
            return Err(format!(
                "def has {num_inputs} inputs / {num_outputs} outputs, more than the {NUM_AUDIO_BUSES} audio buses"
            ));
        }
        Ok(Self {
            factory,
            params: ui.specs,
            num_inputs,
            num_outputs,
        })
    }

    /// Control reference for `/s_new`/`/n_set` by name: Faust parameters
    /// first (declaration order), then the reserved `out` and `in` buses.
    pub fn control_index(&self, name: &str) -> Option<u32> {
        if let Some(i) = self.params.iter().position(|p| p.name == name) {
            return Some(i as u32);
        }
        match name {
            "out" => Some(self.params.len() as u32),
            "in" => Some(self.params.len() as u32 + 1),
            _ => None,
        }
    }

    pub fn factory(&self) -> &FaustFactory {
        &self.factory
    }
}

/// A live instance in the node tree. See the module docs for the threading
/// and control conventions.
pub struct FaustSynth {
    /// Keeps the factory alive for as long as this instance exists.
    def: Arc<FaustDef>,
    dsp: NonNull<ffi::llvm_dsp>,
    /// Parameter zones inside the instance, aligned with `def.params`.
    zones: Vec<*mut f32>,
    /// Bus mappings parallel to `zones` (`/n_map`/`/n_mapa`). The reserved
    /// `out`/`in` routing controls are not mappable.
    maps: Vec<ControlMap>,
    out_bus: usize,
    in_bus: usize,
    in_bufs: Vec<Block>,
    out_bufs: Vec<Block>,
    /// Scratch pointer arrays for `compute`, refreshed every block.
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
}

// SAFETY: the instance pointer is owned exclusively and the zone pointers
// reach into that same instance. Only one thread touches the synth at a
// time: built on the network thread, handed to the audio thread through the
// command FIFO, returned through the garbage FIFO (both provide the
// necessary synchronization), dropped on the network thread.
unsafe impl Send for FaustSynth {}

impl FaustSynth {
    /// Instantiates the def at the engine's sample rate (frozen by `init`;
    /// see PLAN.md on hot SR changes). Allocates: network thread only.
    pub fn new(def: Arc<FaustDef>, sample_rate: f32) -> Result<Self, String> {
        let dsp = unsafe { ffi::createCDSPInstance(def.factory().as_ptr()) };
        let Some(dsp) = NonNull::new(dsp) else {
            return Err("Faust instance creation failed".into());
        };
        unsafe { ffi::initCDSPInstance(dsp.as_ptr(), sample_rate as i32) };
        let ui = collect_ui(dsp.as_ptr());
        debug_assert_eq!(
            ui.zones.len(),
            def.params.len(),
            "instance UI must match the def probe"
        );
        let (num_inputs, num_outputs) = (def.num_inputs, def.num_outputs);
        let maps = vec![ControlMap::UNMAPPED; ui.zones.len()];
        Ok(Self {
            def,
            dsp,
            zones: ui.zones,
            maps,
            out_bus: 0,
            in_bus: 0,
            in_bufs: vec![Block::SILENCE; num_inputs],
            out_bufs: vec![Block::SILENCE; num_outputs],
            in_ptrs: vec![std::ptr::null_mut(); num_inputs],
            out_ptrs: vec![std::ptr::null_mut(); num_outputs],
        })
    }
}

impl SynthNode for FaustSynth {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        // Scheduled bundles (M6) may split the block: only the
        // `offset..offset+frames` range of the buses belongs to this call.
        let (offset, frames) = (ctx.offset, ctx.frames);
        // Pull bus-mapped parameters into their zones before `compute`
        // reads them: a control bus, or one frame of an audio bus
        // (control-rate, `/n_mapa`). Zones are scalar, so audio mappings are
        // always sampled — Faust has no audio-rate parameter.
        for i in 0..self.maps.len() {
            let m = self.maps[i];
            if m.bus >= 0 {
                let v = if m.audio {
                    ctx.buses.audio((m.bus as usize).min(NUM_AUDIO_BUSES - 1))[offset]
                } else {
                    ctx.buses.control.get(m.bus as usize)
                };
                unsafe { self.zones[i].write(v) };
            }
        }
        for i in 0..self.in_bufs.len() {
            let bus = (self.in_bus + i).min(NUM_AUDIO_BUSES - 1);
            self.in_bufs[i].0[..frames]
                .copy_from_slice(&ctx.buses.audio(bus)[offset..offset + frames]);
            self.in_ptrs[i] = self.in_bufs[i].0.as_mut_ptr();
        }
        for i in 0..self.out_bufs.len() {
            self.out_ptrs[i] = self.out_bufs[i].0.as_mut_ptr();
        }
        unsafe {
            ffi::computeCDSPInstance(
                self.dsp.as_ptr(),
                frames as i32,
                self.in_ptrs.as_mut_ptr(),
                self.out_ptrs.as_mut_ptr(),
            );
        }
        for (i, buf) in self.out_bufs.iter().enumerate() {
            let bus = (self.out_bus + i).min(NUM_AUDIO_BUSES - 1);
            // SAFETY: M13 stage disjointness — no other thread touches
            // this bus while we sum into it.
            for (d, s) in unsafe { ctx.buses.audio_mut(bus) }[offset..offset + frames]
                .iter_mut()
                .zip(&buf.0[..frames])
            {
                *d += s;
            }
        }
    }

    fn set_control(&mut self, index: u32, value: f32) {
        let i = index as usize;
        // An explicit set overrides and clears any mapping (scsynth).
        if let Some(m) = self.maps.get_mut(i) {
            m.bus = -1;
        }
        if let Some(zone) = self.zones.get(i) {
            unsafe { zone.write(value) };
        } else if i == self.zones.len() {
            self.out_bus = clamp_first_bus(value, self.def.num_outputs);
        } else if i == self.zones.len() + 1 {
            self.in_bus = clamp_first_bus(value, self.def.num_inputs);
        }
        // anything else is ignored, like scsynth
    }

    fn map_control(&mut self, index: u32, bus: i32, audio: bool) {
        // Only the parameter zones are mappable; `out`/`in` routing is not.
        if let Some(m) = self.maps.get_mut(index as usize) {
            *m = ControlMap { bus, audio };
        }
    }

    /// The whole JIT instance counts as one UGen in `/status.reply`.
    fn ugen_count(&self) -> usize {
        1
    }
}

impl Drop for FaustSynth {
    /// Frees the instance: must run off the audio thread. Freed nodes always
    /// leave through the garbage FIFO and drop on the network thread; the
    /// `def` Arc (and with it the factory) outlives this call by definition.
    fn drop(&mut self) {
        unsafe { ffi::deleteCDSPInstance(self.dsp.as_ptr()) };
    }
}

/// Clamps a bus control value so the synth's whole channel span stays inside
/// the audio buses.
fn clamp_first_bus(value: f32, width: usize) -> usize {
    let max_first = NUM_AUDIO_BUSES - width.max(1);
    (value.max(0.0) as usize).min(max_first)
}

/// Output of one `buildUserInterface` walk: parameter specs and the matching
/// zone pointers, in declaration order.
struct CollectedUi {
    specs: Vec<ParamSpec>,
    zones: Vec<*mut f32>,
}

fn collect_ui(dsp: *mut ffi::llvm_dsp) -> CollectedUi {
    let mut ui = CollectedUi {
        specs: Vec::new(),
        zones: Vec::new(),
    };
    let mut glue = ffi::UIGlue {
        uiInterface: &mut ui as *mut CollectedUi as *mut c_void,
        openTabBox: open_box,
        openHorizontalBox: open_box,
        openVerticalBox: open_box,
        closeBox: close_box,
        addButton: add_button,
        addCheckButton: add_button,
        addVerticalSlider: add_slider,
        addHorizontalSlider: add_slider,
        addNumEntry: add_slider,
        addHorizontalBargraph: add_bargraph,
        addVerticalBargraph: add_bargraph,
        addSoundfile: add_soundfile,
        declare,
    };
    unsafe { ffi::buildUserInterfaceCDSPInstance(dsp, &mut glue) };
    ui
}

unsafe fn collector<'a>(ui: *mut c_void) -> &'a mut CollectedUi {
    unsafe { &mut *(ui as *mut CollectedUi) }
}

unsafe fn add_param(ui: *mut c_void, label: *const c_char, zone: *mut f32, spec: [f32; 4]) {
    let ui = unsafe { collector(ui) };
    let name = unsafe { CStr::from_ptr(label) }
        .to_string_lossy()
        .into_owned();
    let [init, min, max, step] = spec;
    ui.specs.push(ParamSpec {
        name,
        init,
        min,
        max,
        step,
    });
    ui.zones.push(zone);
}

unsafe extern "C" fn add_slider(
    ui: *mut c_void,
    label: *const c_char,
    zone: *mut f32,
    init: f32,
    min: f32,
    max: f32,
    step: f32,
) {
    unsafe { add_param(ui, label, zone, [init, min, max, step]) };
}

unsafe extern "C" fn add_button(ui: *mut c_void, label: *const c_char, zone: *mut f32) {
    unsafe { add_param(ui, label, zone, [0.0, 0.0, 1.0, 1.0]) };
}

// Group structure is flattened (bare labels) and passive widgets are not
// settable controls: ignored.
unsafe extern "C" fn open_box(_ui: *mut c_void, _label: *const c_char) {}
unsafe extern "C" fn close_box(_ui: *mut c_void) {}
unsafe extern "C" fn add_bargraph(
    _ui: *mut c_void,
    _label: *const c_char,
    _zone: *mut f32,
    _min: f32,
    _max: f32,
) {
}
unsafe extern "C" fn add_soundfile(
    _ui: *mut c_void,
    _label: *const c_char,
    _url: *const c_char,
    _sf_zone: *mut *mut c_void,
) {
}
unsafe extern "C" fn declare(
    _ui: *mut c_void,
    _zone: *mut f32,
    _key: *const c_char,
    _value: *const c_char,
) {
}
