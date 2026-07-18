//! Runtime instance of a SynthDef: the UGen vector plus its wiring.

use std::sync::Arc;

use crate::dsp::registry::{DEMAND_SOURCE_SLOT, ExecMode};
use crate::dsp::spectral::SpectralChain;
use crate::dsp::{
    Block, DoneAction, MAX_UGEN_INPUTS, NUM_AUDIO_BUSES, ProcessCtx, Rate, ReplyMsg, UGen, at,
};
use crate::node::{ControlMap, SynthNode};
use crate::synthdef::{ControlType, InputRef, SynthDef};

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
    /// Synth-private feedback channels (`LocalIn`/`LocalOut`): unlike `wires`,
    /// these **persist across blocks** — that persistence is the one-block
    /// feedback delay. Empty for defs without feedback.
    locals: Vec<Block>,
    /// Synth-private spectral frames (S8), one per `FFT` chain, shared by that
    /// chain's `FFT`/`PV_*`/`IFFT` UGens through their compile-assigned slot.
    /// Persistent across blocks and allocated once here (network thread), never
    /// on the audio thread. Empty for defs with no `FFT`.
    chains: Vec<SpectralChain>,
    /// False until the first `process` runs. The `ir` init pass (S1) runs each
    /// `ir` UGen exactly once, on that first block; its wire then holds the
    /// value (wires persist across blocks) and the UGen is skipped thereafter.
    initialized: bool,
    /// True when any UGen is a side-effect reply UGen (`SendReply`/`SendTrig`/
    /// `Poll`, S9); precomputed so `has_replies` is O(1) and the tree only
    /// enqueues these synths for the reply drain.
    has_reply_ugens: bool,
}

impl UGenSynth {
    pub fn new(def: Arc<SynthDef>) -> Self {
        let controls = def.control_defaults.clone();
        let maps = vec![ControlMap::UNMAPPED; controls.len()];
        let ugens: Vec<_> = def
            .ugens
            .iter()
            .map(|u| (u.desc.build)(&u.config))
            .collect();
        let wires = vec![Block::SILENCE; ugens.len()];
        let locals = vec![Block::SILENCE; def.num_locals];
        let chains = def
            .spectral_sizes
            .iter()
            .map(|&sz| SpectralChain::new(sz))
            .collect();
        let has_reply_ugens = ugens.iter().any(|u| u.is_reply());
        Self {
            def,
            controls,
            maps,
            ugens,
            wires,
            locals,
            chains,
            initialized: false,
            has_reply_ugens,
        }
    }
}

impl SynthNode for UGenSynth {
    fn set_node_id(&mut self, id: i32) {
        for u in &mut self.ugens {
            u.set_node_id(id);
        }
    }

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
        // every `ar` wire then carries only the slice being processed. A `kr`
        // or `ir` wire is length-1 (one value per block / per node life); the
        // `wire_len` helper picks the right slice length by the producer rate.
        let wire_len = |def: &SynthDef, w: usize, frames: usize| {
            if def.ugens[w].rate == Rate::Ar {
                frames
            } else {
                1
            }
        };
        for i in 0..self.ugens.len() {
            let rate = self.def.ugens[i].rate;
            // Demand-rate UGens produce nothing in block order — their driver
            // pulls them (see the Demand arm below).
            if rate == Rate::Dr {
                continue;
            }
            // `ir` UGens are computed once at init and then held: after the
            // first block their wire already carries the value, so skip them.
            if rate == Rate::Ir && self.initialized {
                continue;
            }
            // `ar`: one value per sample; `kr`/`ir`: one value per block.
            let out_len = if rate == Rate::Ar { ctx.frames } else { 1 };

            // Topological order guarantees inputs only reference earlier wires.
            let (earlier, rest) = self.wires.split_at_mut(i);
            let output = &mut rest[0].0[..out_len];

            let mut inputs: [&[f32]; MAX_UGEN_INPUTS] = [&[]; MAX_UGEN_INPUTS];
            let refs = &self.def.ugens[i].inputs;
            for (k, r) in refs.iter().enumerate() {
                inputs[k] = match r {
                    InputRef::Const(c) => std::slice::from_ref(&self.def.constants[*c]),
                    InputRef::Control(c) => std::slice::from_ref(&self.controls[*c]),
                    InputRef::Wire(w) => &earlier[*w].0[..wire_len(&self.def, *w, ctx.frames)],
                };
            }
            // LocalIn/LocalOut feed back through the persistent `locals`
            // buffer — synth-private state the UGen trait can't reach — so
            // they are handled here instead of by `process`. Reading before
            // writing (LocalIn precedes LocalOut, enforced at compile) is the
            // one-block delay.
            let (lo, hi) = (ctx.offset, ctx.offset + ctx.frames);
            match self.def.ugens[i].desc.exec {
                ExecMode::LocalIn => {
                    let ch = inputs[0][0] as usize;
                    output.copy_from_slice(&self.locals[ch].0[lo..hi]);
                }
                ExecMode::LocalOut => {
                    let ch = inputs[0][0] as usize;
                    let signal = inputs[1];
                    let dst = &mut self.locals[ch].0[lo..hi];
                    for (j, d) in dst.iter_mut().enumerate() {
                        *d = at(signal, j);
                    }
                    // Pass the signal through this UGen's wire too, so a
                    // LocalOut can sit mid-chain and still feed later UGens.
                    for (j, o) in output.iter_mut().enumerate() {
                        *o = at(signal, j);
                    }
                }
                // Demand driver (S1): pull the next value from its demand
                // source on each trigger. The source (a `dr` UGen, skipped in
                // block order) is reached only through the `step` callback, so
                // there is a single mutable path to it and no allocation.
                ExecMode::DemandDriver => {
                    let InputRef::Wire(j) = refs[DEMAND_SOURCE_SLOT] else {
                        // Compile guarantees a `dr` wire in this slot.
                        output.fill(0.0);
                        continue;
                    };
                    // Resolve the source's own inputs (its value list, etc.).
                    let src_refs = &self.def.ugens[j].inputs;
                    let mut src_inputs: [&[f32]; MAX_UGEN_INPUTS] = [&[]; MAX_UGEN_INPUTS];
                    for (k, r) in src_refs.iter().enumerate() {
                        src_inputs[k] = match r {
                            InputRef::Const(c) => std::slice::from_ref(&self.def.constants[*c]),
                            InputRef::Control(c) => std::slice::from_ref(&self.controls[*c]),
                            InputRef::Wire(w) => {
                                &earlier[*w].0[..wire_len(&self.def, *w, ctx.frames)]
                            }
                        };
                    }
                    let sn = src_refs.len();
                    let ctx_copy = *ctx; // Copy: shared refs, no aliasing
                    let (trig, reset) = (inputs[0], inputs[1]);
                    let (u_earlier, u_rest) = self.ugens.split_at_mut(i);
                    let source = &mut u_earlier[j];
                    let driver = &mut u_rest[0];
                    let mut step = |reset: bool| -> f32 {
                        if reset {
                            source.reset_demand();
                            f32::NAN
                        } else {
                            source.demand(&ctx_copy, &src_inputs[..sn])
                        }
                    };
                    driver.drive(trig, reset, output, &mut step);
                }
                // Spectral chain (S8): the FFT/PV_*/IFFT UGen runs with its
                // synth-private `SpectralChain`, resolved by the compile-
                // assigned slot. `chains` is a distinct field from `ugens`, so
                // both can be borrowed mutably at once.
                ExecMode::Spectral => {
                    let slot = self.def.ugens[i]
                        .chain_slot
                        .expect("compile assigns a chain slot to every spectral UGen");
                    // A two-chain combiner (M27) borrows both of its chains at
                    // once; compile guarantees the slots are distinct, so the
                    // split borrow is always disjoint.
                    if let Some(slot_b) = self.def.ugens[i].chain_slot_b {
                        let (a, b) = if slot < slot_b {
                            let (lo, hi) = self.chains.split_at_mut(slot_b);
                            (&mut lo[slot], &mut hi[0])
                        } else {
                            let (lo, hi) = self.chains.split_at_mut(slot);
                            (&mut hi[0], &mut lo[slot_b])
                        };
                        self.ugens[i].process_spectral_pair(
                            ctx,
                            &inputs[..refs.len()],
                            output,
                            a,
                            b,
                        );
                    } else {
                        self.ugens[i].process_spectral(
                            ctx,
                            &inputs[..refs.len()],
                            output,
                            &mut self.chains[slot],
                        );
                    }
                }
                ExecMode::Normal => self.ugens[i].process(ctx, &inputs[..refs.len()], output),
            }
        }
        // Trigger controls (S2) hold their `/n_set` value for exactly one
        // block, then reset to 0 so a rising edge fires once (e.g. an EnvGen
        // gate). Cheap: most defs carry no triggers.
        for (i, ty) in self.def.control_types.iter().enumerate() {
            if *ty == ControlType::Trigger {
                self.controls[i] = 0.0;
            }
        }
        // The `ir` init pass has now run (on the first block); mark it so `ir`
        // UGens are skipped from here on.
        self.initialized = true;
    }

    fn set_control(&mut self, index: u32, value: f32) {
        let i = index as usize;
        // Scalar (`ir`) controls are read once at init and frozen (S2): a
        // `/n_set` after the synth has run is ignored, per scsynth. The initial
        // `/s_new` values are applied before the first block (still `!initialized`),
        // so they take.
        if self.initialized && matches!(self.def.control_types.get(i), Some(ControlType::Scalar)) {
            return;
        }
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

    fn ugen_command(&mut self, index: u32, cmd: &crate::dsp::UGenCmd) {
        if let Some(ugen) = self.ugens.get_mut(index as usize) {
            ugen.command(cmd);
        }
    }

    fn has_replies(&self) -> bool {
        self.has_reply_ugens
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        for ugen in &mut self.ugens {
            ugen.drain_replies(node_id, sink);
        }
    }

    fn done_action(&self) -> DoneAction {
        let mut max_action = DoneAction::None;
        for u in &self.ugens {
            let act = u.done();
            if (act as u8) > (max_action as u8) {
                max_action = act;
            }
        }
        max_action
    }
}
