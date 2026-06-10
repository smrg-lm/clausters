//! Runtime instance of a SynthDef: the UGen vector plus its wiring.

use std::sync::Arc;

use crate::dsp::{BLOCK_SIZE, MAX_UGEN_INPUTS, ProcessCtx, UGen, registry};
use crate::node::SynthNode;
use crate::synthdef::{InputRef, SynthDef};

/// Built entirely on the network thread (allocates); `process` runs on the
/// audio thread and does not allocate.
pub struct UGenSynth {
    def: Arc<SynthDef>,
    controls: Vec<f32>,
    ugens: Vec<Box<dyn UGen>>,
    /// One output wire per UGen.
    wires: Vec<[f32; BLOCK_SIZE]>,
}

impl UGenSynth {
    pub fn new(def: Arc<SynthDef>) -> Self {
        let controls = def.control_defaults.clone();
        let ugens: Vec<_> = def.ugens.iter().map(|u| registry::build(u.kind)).collect();
        let wires = vec![[0.0; BLOCK_SIZE]; ugens.len()];
        Self {
            def,
            controls,
            ugens,
            wires,
        }
    }
}

impl SynthNode for UGenSynth {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        for i in 0..self.ugens.len() {
            // Topological order guarantees inputs only reference earlier wires.
            let (earlier, rest) = self.wires.split_at_mut(i);
            let output = &mut rest[0];

            let mut inputs: [&[f32]; MAX_UGEN_INPUTS] = [&[]; MAX_UGEN_INPUTS];
            let refs = &self.def.ugens[i].inputs;
            for (k, r) in refs.iter().enumerate() {
                inputs[k] = match r {
                    InputRef::Const(c) => std::slice::from_ref(&self.def.constants[*c]),
                    InputRef::Control(c) => std::slice::from_ref(&self.controls[*c]),
                    InputRef::Wire(w) => &earlier[*w][..],
                };
            }
            self.ugens[i].process(ctx, &inputs[..refs.len()], output);
        }
    }

    fn set_control(&mut self, index: u32, value: f32) {
        // unknown indices are ignored, like scsynth
        if let Some(c) = self.controls.get_mut(index as usize) {
            *c = value;
        }
    }

    fn ugen_count(&self) -> usize {
        self.ugens.len()
    }
}
