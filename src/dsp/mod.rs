//! UGens, buses and DSP algorithms.
//!
//! A UGen processes one block through `process`: inputs are slices that are
//! either a full block (a wire from an earlier UGen) or a single sample (a
//! constant or a control). Use [`at`] to read them uniformly. The context
//! carries the global buses; only I/O UGens touch them. Everything here runs
//! on the audio thread: no allocation.

pub mod binop;
pub mod buf;
pub mod buffer;
pub mod denormals;
pub mod io;
pub mod noise;
pub mod registry;
pub mod sinosc;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Frames per processing block, like scsynth.
pub const BLOCK_SIZE: usize = 64;
/// Audio buses (scsynth `-a`); buses `0..channels` are the hardware outputs.
pub const NUM_AUDIO_BUSES: usize = 128;
/// Control buses (scsynth `-c`).
pub const NUM_CONTROL_BUSES: usize = 1024;
/// Maximum inputs per UGen; lets the synth build its input list on the stack.
pub const MAX_UGEN_INPUTS: usize = 8;

/// Control buses are single floats shared between threads: the network
/// thread serves `/c_set`/`/c_get` directly, the audio thread reads them via
/// the `InCtl` UGen. Plain atomic bit-cast stores — lock-free on both sides.
#[derive(Clone)]
pub struct ControlBuses(Arc<Vec<AtomicU32>>);

impl Default for ControlBuses {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlBuses {
    pub fn new() -> Self {
        Self(Arc::new(
            (0..NUM_CONTROL_BUSES)
                .map(|_| AtomicU32::new(0.0f32.to_bits()))
                .collect(),
        ))
    }

    pub fn get(&self, index: usize) -> f32 {
        self.0
            .get(index)
            .map_or(0.0, |b| f32::from_bits(b.load(Ordering::Relaxed)))
    }

    pub fn set(&self, index: usize, value: f32) {
        if let Some(b) = self.0.get(index) {
            b.store(value.to_bits(), Ordering::Relaxed);
        }
    }
}

/// Global buses. Audio buses live on the audio thread and are cleared every
/// block; control buses persist and are shared (see [`ControlBuses`]).
pub struct Buses {
    pub audio: Vec<[f32; BLOCK_SIZE]>,
    pub control: ControlBuses,
}

impl Buses {
    pub fn new(control: ControlBuses) -> Self {
        Self {
            audio: vec![[0.0; BLOCK_SIZE]; NUM_AUDIO_BUSES],
            control,
        }
    }

    pub fn clear_audio(&mut self) {
        for bus in &mut self.audio {
            bus.fill(0.0);
        }
    }
}

/// One processing slice. Normally a whole block (`offset` 0, `frames` =
/// [`BLOCK_SIZE`]), but scheduled bundles (M6) split the block at the
/// event's sample: synths then process the sub-range `offset..offset+frames`
/// of the current block, and bus I/O must index buses at `offset`.
pub struct ProcessCtx<'a> {
    pub sample_rate: f32,
    pub buses: &'a mut Buses,
    /// The engine's buffer pool; read-only on the audio thread (see
    /// [`buffer`] for the immutability contract).
    pub buffers: &'a [Option<Arc<buffer::Buffer>>],
    /// First frame of this slice within the block.
    pub offset: usize,
    /// Slice length in frames.
    pub frames: usize,
}

pub trait UGen: Send {
    /// Writes one block into `output`. `inputs` are full-block or length-1
    /// slices, already resolved by the synth.
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]);
}

/// Reads input `i` from a block or a single-sample slice.
#[inline(always)]
pub fn at(input: &[f32], i: usize) -> f32 {
    if input.len() == 1 { input[0] } else { input[i] }
}
