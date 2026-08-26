//! `FaustSynth` in a page: a Faust wasm module linked into the engine's memory.
//!
//! The native backend holds an opaque libfaust instance pointer and calls
//! `computeCDSPInstance` once per block. Here the instance is a struct **we**
//! allocate inside the engine's own linear memory, and `compute` is a function
//! of a second wasm module whose `env.memory` is that same memory — reached
//! through the engine's `__indirect_function_table`, so the call is a plain
//! `call_indirect` with no JavaScript frame on the audio path. See
//! `docs/decisions.md`, "The page's Faust is a second wasm module linked into
//! the engine's own memory".
//!
//! Everything above the call is the native code: the controls are the def's UI
//! parameters in declaration order followed by the reserved `out` and `in`
//! buses, `/node_set` is an aligned store into a zone, inputs are copied out
//! before outputs are summed into the buses, and the node counts as one UGen.
//!
//! # What the host must have done first
//!
//! [`FaustDef::link`] is handed table indices, not bytes. The host compiles the
//! def in the Worker, strips the module's data segment
//! ([`crate::faust::wasm_module::strip_data_section`]), instantiates it against
//! the engine's memory and table with its math imports bound to the engine's
//! own exports, and passes the resulting slots here. A wrong index is a
//! signature mismatch at the first `call_indirect`, which traps rather than
//! corrupting anything.

use std::sync::Arc;

use crate::dsp::buffer::BufferPool;
use crate::dsp::{Block, NUM_AUDIO_BUSES, ProcessCtx};
use crate::faust::ffi;
use crate::faust::json_ui::{DefLayout, SoundfileSlot};
use crate::faust::soundfile::SoundfileData;
use crate::node::{ControlMap, SynthNode};

pub use crate::faust::ParamSpec;

/// `void compute(dsp*, int count, float** inputs, float** outputs)` — the ABI
/// the Faust wasm backend emits, and the one `computeCDSPInstance` has.
type ComputeFn = extern "C" fn(*mut u8, i32, *mut *mut f32, *mut *mut f32);
/// `void init(dsp*, int sampleRate)`.
type InitFn = extern "C" fn(*mut u8, i32);

/// A compiled Faust def ready to instantiate. Holds no memory of its own: the
/// module lives in the engine's instance for the page's lifetime and the def
/// only remembers where its entry points landed in the table.
pub struct FaustDef {
    compute: ComputeFn,
    init: InitFn,
    /// Byte size of one DSP struct, from the compiler's JSON (`"size"`).
    struct_bytes: usize,
    /// Byte offset of each parameter's zone inside the struct, aligned with
    /// `params` (`"index"` per UI element).
    offsets: Vec<usize>,
    pub params: Vec<ParamSpec>,
    pub num_inputs: usize,
    pub num_outputs: usize,
    /// Each `soundfile` the def declares. The wasm backend moves those fields
    /// to the front of the DSP struct and leaves them for whoever instantiates
    /// it to fill.
    soundfiles: Vec<SoundfileSlot>,
}

// SAFETY: a def is immutable after `link`, and the two function pointers are
// table indices into the one instance every thread of this build shares (a
// page's engine has exactly one). Nothing here is mutated after construction.
unsafe impl Send for FaustDef {}
unsafe impl Sync for FaustDef {}

impl FaustDef {
    /// Adopts a module the host has already instantiated against the engine.
    ///
    /// # Safety
    /// `compute` and `init` must be live slots of the engine's
    /// `__indirect_function_table` holding functions with exactly the
    /// signatures above, belonging to a module instantiated against **this**
    /// linear memory, and `struct_bytes`/`offsets` must be the ones that
    /// module's own JSON declared. A mismatch is undefined behaviour in the
    /// engine's memory, not a Faust error.
    pub unsafe fn link(compute: u32, init: u32, layout: DefLayout) -> Result<Self, String> {
        let DefLayout {
            struct_bytes,
            params,
            offsets,
            num_inputs,
            num_outputs,
            soundfiles,
        } = layout;
        if compute == 0 || init == 0 {
            return Err("faust module exports landed at table slot 0".into());
        }
        if offsets.len() != params.len() {
            return Err(format!(
                "faust def declares {} parameters but {} zone offsets",
                params.len(),
                offsets.len()
            ));
        }
        if let Some(bad) = offsets.iter().find(|o| **o + 4 > struct_bytes) {
            return Err(format!(
                "faust parameter zone at byte {bad} is outside the {struct_bytes}-byte struct"
            ));
        }
        // A `Soundfile*` is a pointer, so four bytes on this target, and it is
        // checked for the same reason a zone is: what goes in there is written
        // by us into the engine's own memory.
        if let Some(bad) = soundfiles.iter().find(|s| s.offset + 4 > struct_bytes) {
            return Err(format!(
                "faust soundfile field at byte {} is outside the {struct_bytes}-byte struct",
                bad.offset
            ));
        }
        if num_inputs > NUM_AUDIO_BUSES || num_outputs > NUM_AUDIO_BUSES {
            return Err(format!(
                "def has {num_inputs} inputs / {num_outputs} outputs, more than the {NUM_AUDIO_BUSES} audio buses"
            ));
        }
        // On wasm32 a function pointer *is* its table index, so the two
        // transmutes below are the whole of the linking step.
        let (compute, init) = unsafe {
            (
                std::mem::transmute::<usize, ComputeFn>(compute as usize),
                std::mem::transmute::<usize, InitFn>(init as usize),
            )
        };
        Ok(Self {
            compute,
            init,
            struct_bytes,
            offsets,
            params,
            num_inputs,
            num_outputs,
            soundfiles,
        })
    }

    /// Control reference for `/synth_new`/`/node_set` by name: Faust parameters
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
}

/// A live instance in the node tree. See the module docs.
pub struct FaustSynth {
    /// Keeps the def (and with it the entry points) alive for this instance.
    def: Arc<FaustDef>,
    /// The DSP struct itself. `f64` elements so the allocation is 8-byte
    /// aligned, which is what the widest field Faust emits needs.
    dsp: Vec<f64>,
    /// Parameter zones inside `dsp`, aligned with `def.params`.
    zones: Vec<*mut f32>,
    /// Bus mappings parallel to `zones` (`/node_map`/`/node_mapAudio`). The
    /// reserved `out`/`in` routing controls are not mappable.
    maps: Vec<ControlMap>,
    out_bus: usize,
    in_bus: usize,
    in_bufs: Vec<Block>,
    out_bufs: Vec<Block>,
    /// Scratch pointer arrays for `compute`, refreshed every block.
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    /// Backing memory for the instance's `soundfile` fields, one per declared
    /// `soundfile`. Kept alive for the instance's life: the DSP holds raw
    /// pointers into it.
    _soundfiles: Vec<SoundfileData>,
}

// SAFETY: the struct is owned exclusively and the zone pointers reach into
// that same allocation. Only one thread touches the synth at a time: built on
// the network thread, handed to the audio thread through the command FIFO,
// returned through the garbage FIFO, dropped on the network thread.
unsafe impl Send for FaustSynth {}

impl FaustSynth {
    /// Instantiates the def at the engine's sample rate (frozen by `init`).
    /// Allocates: network thread only. `buffers` is the pool a
    /// `soundfile("<bufnum>", n)` names, read here as a snapshot.
    pub fn new(def: Arc<FaustDef>, sample_rate: f32, buffers: &BufferPool) -> Result<Self, String> {
        let words = def.struct_bytes.div_ceil(8).max(1);
        let mut dsp = vec![0.0f64; words];
        let base = dsp.as_mut_ptr() as *mut u8;
        // The soundfile fields go in **before** `init`: the wasm backend moves
        // them to the front of the struct and `instanceInit` reads a file's
        // sample rate out of them. A null there is what made a def that
        // declares one silent rather than sounding.
        let mut soundfiles = Vec::with_capacity(def.soundfiles.len());
        for slot in &def.soundfiles {
            let buffer = slot
                .bufnum
                .and_then(|i| buffers.get(i).and_then(|held| held.clone()));
            let data = SoundfileData::new(buffer.as_deref());
            // SAFETY: `offset + 4` was checked against `struct_bytes` in
            // `link`, and the allocation covers that many bytes. The address
            // stays valid because `data` is kept in `_soundfiles` for as long
            // as the instance lives.
            unsafe {
                let field = base.add(slot.offset) as *mut u32;
                field.write_unaligned(&*data.soundfile as *const ffi::Soundfile as u32);
            }
            soundfiles.push(data);
        }
        (def.init)(base, sample_rate as i32);
        // SAFETY: every offset was checked against `struct_bytes` in `link`,
        // and the allocation covers that many bytes.
        let zones: Vec<*mut f32> = def
            .offsets
            .iter()
            .map(|o| unsafe { base.add(*o) } as *mut f32)
            .collect();
        let (num_inputs, num_outputs) = (def.num_inputs, def.num_outputs);
        let maps = vec![ControlMap::UNMAPPED; zones.len()];
        Ok(Self {
            def,
            dsp,
            zones,
            maps,
            out_bus: 0,
            in_bus: 0,
            in_bufs: vec![Block::SILENCE; num_inputs],
            out_bufs: vec![Block::SILENCE; num_outputs],
            in_ptrs: vec![std::ptr::null_mut(); num_inputs],
            out_ptrs: vec![std::ptr::null_mut(); num_outputs],
            _soundfiles: soundfiles,
        })
    }
}

impl SynthNode for FaustSynth {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        // Scheduled bundles may split the block: only the
        // `offset..offset+frames` range of the buses belongs to this call.
        let (offset, frames) = (ctx.offset, ctx.frames);
        // Pull bus-mapped parameters into their zones before `compute` reads
        // them: a control bus, or one frame of an audio bus (control-rate,
        // `/node_mapAudio`). Zones are scalar, so audio mappings are always
        // sampled — Faust has no audio-rate parameter.
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
        (self.def.compute)(
            self.dsp.as_mut_ptr() as *mut u8,
            frames as i32,
            self.in_ptrs.as_mut_ptr(),
            self.out_ptrs.as_mut_ptr(),
        );
        for (i, buf) in self.out_bufs.iter().enumerate() {
            let bus = (self.out_bus + i).min(NUM_AUDIO_BUSES - 1);
            // SAFETY: stage disjointness — no other thread touches this bus
            // while we sum into it.
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

    /// The whole instance counts as one UGen in `/server_status.reply`.
    fn ugen_count(&self) -> usize {
        1
    }
}

/// Clamps a bus control value so the synth's whole channel span stays inside
/// the audio buses.
fn clamp_first_bus(value: f32, width: usize) -> usize {
    let max_first = NUM_AUDIO_BUSES - width.max(1);
    (value.max(0.0) as usize).min(max_first)
}
