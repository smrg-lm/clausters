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

use std::cell::UnsafeCell;
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

/// Which audio buses a node reads and writes, as `u128` bitmasks (M12/M13).
/// Computed by the network thread from the def and the node's current
/// control values (`osc::graph`); shipped to the engine inside
/// `Cmd::AddSynth` so the parallel scheduler (M13) partitions stages from
/// engine-owned data — safety never depends on possibly stale mirror state.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct BusUsage {
    pub reads: u128,
    pub writes: u128,
    /// A bus index fed by a computed signal: the node may touch *any* bus,
    /// so it keeps its position and never runs in parallel with anything.
    pub dynamic: bool,
}

const _: () = assert!(NUM_AUDIO_BUSES <= 128, "BusUsage bitmasks are u128");

impl BusUsage {
    pub fn union(self, other: Self) -> Self {
        Self {
            reads: self.reads | other.reads,
            writes: self.writes | other.writes,
            dynamic: self.dynamic || other.dynamic,
        }
    }

    /// Marks one bus, converting like `dsp::io::audio_bus` does at run time.
    pub fn mark(&mut self, value: f32, read: bool, write: bool) {
        let bus = (value.max(0.0) as usize).min(NUM_AUDIO_BUSES - 1);
        if read {
            self.reads |= 1 << bus;
        }
        if write {
            self.writes |= 1 << bus;
        }
    }
}

/// Global buses. Audio buses live on the audio thread and are cleared every
/// block; control buses persist and are shared (see [`ControlBuses`]).
///
/// Each audio bus sits in its own [`UnsafeCell`] so the M13 worker threads
/// can write **disjoint** buses concurrently through a shared `&Buses`: the
/// stage scheduler (`node::NodeTree::process`) guarantees, from the
/// [`BusUsage`] masks, that no two nodes of a parallel stage touch
/// overlapping buses (and that nothing reads what the stage writes).
pub struct Buses {
    audio: Vec<UnsafeCell<[f32; BLOCK_SIZE]>>,
    pub control: ControlBuses,
}

// SAFETY: concurrent access to `audio` only happens during a parallel stage,
// where the scheduler proves per-bus disjointness; everything else is
// single-threaded on the audio thread.
unsafe impl Send for Buses {}
unsafe impl Sync for Buses {}

impl Buses {
    pub fn new(control: ControlBuses) -> Self {
        Self {
            audio: (0..NUM_AUDIO_BUSES)
                .map(|_| UnsafeCell::new([0.0; BLOCK_SIZE]))
                .collect(),
            control,
        }
    }

    pub fn clear_audio(&mut self) {
        for bus in &mut self.audio {
            bus.get_mut().fill(0.0);
        }
    }

    /// Shared read of one audio bus.
    ///
    /// During a parallel stage this may race only with writes to *other*
    /// buses (scheduler invariant), so the plain reference is sound.
    #[inline]
    pub fn audio(&self, bus: usize) -> &[f32; BLOCK_SIZE] {
        unsafe { &*self.audio[bus].get() }
    }

    /// Mutable access to one audio bus through a shared reference.
    ///
    /// # Safety
    /// The caller must be the only thread touching `bus` for the lifetime
    /// of the returned reference. Inside `process` this holds because the
    /// M13 stage scheduler only runs nodes with disjoint bus usage in
    /// parallel; single-threaded callers hold it trivially.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn audio_mut(&self, bus: usize) -> &mut [f32; BLOCK_SIZE] {
        unsafe { &mut *self.audio[bus].get() }
    }
}

/// One processing slice. Normally a whole block (`offset` 0, `frames` =
/// [`BLOCK_SIZE`]), but scheduled bundles (M6) split the block at the
/// event's sample: synths then process the sub-range `offset..offset+frames`
/// of the current block, and bus I/O must index buses at `offset`.
/// `buses` is a shared reference since M13: bus *writes* go through
/// [`Buses::audio_mut`] under the parallel scheduler's disjointness rule.
/// The struct is `Copy` so every worker carries its own.
#[derive(Clone, Copy)]
pub struct ProcessCtx<'a> {
    pub sample_rate: f32,
    pub buses: &'a Buses,
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
