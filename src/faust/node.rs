//! What a Faust instance does in the node tree, whichever backend compiled it.
//!
//! The native backend holds a libfaust instance and the page's a struct in the
//! engine's own memory, but everything around the one `compute` call is the
//! same code: the controls are the def's UI parameters in declaration order
//! followed by the reserved `out` and `in` buses, `/node_set` is an aligned
//! store into a zone, bus-mapped parameters are pulled into their zones before
//! `compute`, inputs are copied out before outputs are summed into the buses,
//! and the node counts as one UGen. [`FaustBody`] is that code, and each
//! backend's `FaustSynth` holds one beside its instance.

use crate::dsp::{Block, NUM_AUDIO_BUSES, ProcessCtx};
use crate::node::ControlMap;

/// The node-tree half of a Faust instance: its zones, their bus maps, the
/// buses it reads and writes, and the staging buffers `compute` runs over.
pub struct FaustBody {
    /// Parameter zones inside the instance, aligned with the def's params.
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
}

impl FaustBody {
    /// A body over `zones` (pointers into the instance, which must outlive
    /// it) with `num_inputs` inputs and `num_outputs` outputs. Allocates:
    /// network thread only.
    pub fn new(zones: Vec<*mut f32>, num_inputs: usize, num_outputs: usize) -> Self {
        let maps = vec![ControlMap::UNMAPPED; zones.len()];
        FaustBody {
            zones,
            maps,
            out_bus: 0,
            in_bus: 0,
            in_bufs: vec![Block::SILENCE; num_inputs],
            out_bufs: vec![Block::SILENCE; num_outputs],
            in_ptrs: vec![std::ptr::null_mut(); num_inputs],
            out_ptrs: vec![std::ptr::null_mut(); num_outputs],
        }
    }

    /// One call's worth of processing around `compute(frames, inputs,
    /// outputs)`, the backend's own entry point.
    pub fn process(
        &mut self,
        ctx: &mut ProcessCtx,
        compute: impl FnOnce(i32, *mut *mut f32, *mut *mut f32),
    ) {
        // Scheduled bundles may split the block: only the
        // `offset..offset+frames` range of the buses belongs to this call.
        let (offset, frames) = (ctx.offset, ctx.frames);
        // Pull bus-mapped parameters into their zones before `compute` reads
        // them: a control bus, or one frame of an audio bus (control-rate,
        // `/node_mapAudio`). Zones are scalar, so audio mappings are always
        // sampled -- Faust has no audio-rate parameter.
        for i in 0..self.maps.len() {
            let m = self.maps[i];
            if m.bus >= 0 {
                let v = if m.audio {
                    ctx.buses.audio((m.bus as usize).min(NUM_AUDIO_BUSES - 1))[offset]
                } else {
                    ctx.buses.control.get(m.bus as usize)
                };
                // SAFETY: a zone points into the instance, which outlives the
                // body (see `new`).
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
        compute(
            frames as i32,
            self.in_ptrs.as_mut_ptr(),
            self.out_ptrs.as_mut_ptr(),
        );
        for (i, buf) in self.out_bufs.iter().enumerate() {
            let bus = (self.out_bus + i).min(NUM_AUDIO_BUSES - 1);
            // SAFETY: stage disjointness -- no other thread touches this bus
            // while we sum into it.
            for (d, s) in unsafe { ctx.buses.audio_mut(bus) }[offset..offset + frames]
                .iter_mut()
                .zip(&buf.0[..frames])
            {
                *d += s;
            }
        }
    }

    pub fn set_control(&mut self, index: u32, value: f32) {
        let i = index as usize;
        // An explicit set overrides and clears any mapping (scsynth).
        if let Some(m) = self.maps.get_mut(i) {
            m.bus = -1;
        }
        if let Some(zone) = self.zones.get(i) {
            // SAFETY: as in `process`.
            unsafe { zone.write(value) };
        } else if i == self.zones.len() {
            self.out_bus = clamp_first_bus(value, self.out_bufs.len());
        } else if i == self.zones.len() + 1 {
            self.in_bus = clamp_first_bus(value, self.in_bufs.len());
        }
        // anything else is ignored, like scsynth
    }

    pub fn map_control(&mut self, index: u32, bus: i32, audio: bool) {
        // Only the parameter zones are mappable; `out`/`in` routing is not.
        if let Some(m) = self.maps.get_mut(index as usize) {
            *m = ControlMap { bus, audio };
        }
    }
}

/// Clamps a bus control value so the synth's whole channel span stays inside
/// the audio buses.
fn clamp_first_bus(value: f32, width: usize) -> usize {
    let max_first = NUM_AUDIO_BUSES - width.max(1);
    (value.max(0.0) as usize).min(max_first)
}
