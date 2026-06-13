//! Runtime instance of a SynthDef: the UGen vector plus its wiring.

use std::sync::Arc;

use crate::dsp::{Block, MAX_UGEN_INPUTS, NUM_AUDIO_BUSES, ProcessCtx, UGen, registry};
use crate::node::{ControlMap, SynthNode};
use crate::synthdef::{InputRef, SynthDef};

/// Built entirely on the network thread (allocates); `process` runs on the
/// audio thread and does not allocate.
pub struct UGenSynth {
    def: Arc<SynthDef>,
    controls: Vec<f32>,
    /// Bus mappings parallel to `controls` (`/n_map`/`/n_mapa`).
    maps: Vec<ControlMap>,
    ugens: Vec<Box<dyn UGen>>,
    /// One output wire per UGen, cache-line aligned (M10).
    wires: Vec<Block>,
}

impl UGenSynth {
    pub fn new(def: Arc<SynthDef>) -> Self {
        let controls = def.control_defaults.clone();
        let maps = vec![ControlMap::UNMAPPED; controls.len()];
        let ugens: Vec<_> = def.ugens.iter().map(|u| registry::build(u.kind)).collect();
        let wires = vec![Block::SILENCE; ugens.len()];
        Self {
            def,
            controls,
            maps,
            ugens,
            wires,
        }
    }
}

impl SynthNode for UGenSynth {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        // Pull any bus-mapped controls before running UGens: a control bus
        // value, or one frame of an audio bus (control-rate, `/n_mapa`).
        // Written straight to `controls`, never through `set_control` (which
        // would clear the mapping).
        for i in 0..self.maps.len() {
            let m = self.maps[i];
            if m.bus >= 0 {
                self.controls[i] = if m.audio {
                    ctx.buses.audio((m.bus as usize).min(NUM_AUDIO_BUSES - 1))[ctx.offset]
                } else {
                    ctx.buses.control.get(m.bus as usize)
                };
            }
        }
        // `ctx.frames` < BLOCK_SIZE when a scheduled bundle split the block:
        // every wire then carries only the slice being processed.
        for i in 0..self.ugens.len() {
            // Topological order guarantees inputs only reference earlier wires.
            let (earlier, rest) = self.wires.split_at_mut(i);
            let output = &mut rest[0].0[..ctx.frames];

            let mut inputs: [&[f32]; MAX_UGEN_INPUTS] = [&[]; MAX_UGEN_INPUTS];
            let refs = &self.def.ugens[i].inputs;
            for (k, r) in refs.iter().enumerate() {
                inputs[k] = match r {
                    InputRef::Const(c) => std::slice::from_ref(&self.def.constants[*c]),
                    InputRef::Control(c) => std::slice::from_ref(&self.controls[*c]),
                    InputRef::Wire(w) => &earlier[*w].0[..ctx.frames],
                };
            }
            self.ugens[i].process(ctx, &inputs[..refs.len()], output);
        }
    }

    fn set_control(&mut self, index: u32, value: f32) {
        let i = index as usize;
        // An explicit set overrides and clears any mapping (scsynth).
        if let Some(m) = self.maps.get_mut(i) {
            m.bus = -1;
        }
        // unknown indices are ignored, like scsynth
        if let Some(c) = self.controls.get_mut(i) {
            *c = value;
        }
    }

    fn map_control(&mut self, index: u32, bus: i32, audio: bool) {
        if let Some(m) = self.maps.get_mut(index as usize) {
            *m = ControlMap { bus, audio };
        }
    }

    fn ugen_count(&self) -> usize {
        self.ugens.len()
    }
}
