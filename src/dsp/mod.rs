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
pub mod demand;
pub mod denormals;
pub mod disk;
pub mod envgen;
pub mod impulse;
pub mod io;
pub mod local;
pub mod noise;
pub mod registry;
pub mod scalar;
pub mod sinosc;

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Frames per processing block, like scsynth.
pub const BLOCK_SIZE: usize = 64;

/// One block of samples, aligned to a cache line (M10): a block is exactly
/// four full 64-byte lines, so SIMD loads/stores never straddle a line and
/// autovectorization stays stable. Wires and audio buses use this; access
/// the samples through `.0`.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct Block(pub [f32; BLOCK_SIZE]);

impl Block {
    pub const SILENCE: Block = Block([0.0; BLOCK_SIZE]);
}
/// Audio buses (scsynth `-a`); buses `0..channels` are the hardware outputs.
pub const NUM_AUDIO_BUSES: usize = 128;
/// Control buses (scsynth `-c`).
pub const NUM_CONTROL_BUSES: usize = 1024;
/// Maximum inputs per UGen; lets the synth build its input list on the stack.
/// EnvGen requires many inputs (e.g. 21 for ADSR).
pub const MAX_UGEN_INPUTS: usize = 32;

/// Control buses are single floats shared between threads: the network
/// thread serves `/c_set`/`/c_get` directly, the audio thread reads them via
/// the `InCtl` UGen. Plain atomic bit-cast stores — lock-free on both sides.
///
/// Since M14 the backing storage is abstract: a heap array by default, or
/// the control-bus region of a shared-memory segment (`server::ipc`), where
/// other *processes* read and write the same atomics. `_owner` keeps the
/// backing alive (the `Vec` or the mapped segment).
pub struct ControlBuses {
    ptr: *const AtomicU32,
    len: usize,
    _owner: Arc<dyn std::any::Any + Send + Sync>,
}

// SAFETY: the pointee is a fixed array of atomics kept alive by `_owner`;
// atomics are Sync by nature.
unsafe impl Send for ControlBuses {}
unsafe impl Sync for ControlBuses {}

impl Clone for ControlBuses {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            len: self.len,
            _owner: Arc::clone(&self._owner),
        }
    }
}

impl Default for ControlBuses {
    fn default() -> Self {
        Self::new(NUM_CONTROL_BUSES)
    }
}

impl ControlBuses {
    pub fn new(count: usize) -> Self {
        let storage: Arc<Vec<AtomicU32>> = Arc::new(
            (0..count)
                .map(|_| AtomicU32::new(0.0f32.to_bits()))
                .collect(),
        );
        let ptr = storage.as_ptr();
        Self {
            ptr,
            len: count,
            _owner: storage,
        }
    }

    /// Control buses backed by external memory (the M14 IPC segment).
    ///
    /// # Safety
    /// `ptr` must point to `count` initialized `AtomicU32`s that stay valid
    /// and pinned for as long as `owner` is alive.
    pub unsafe fn from_raw(
        ptr: *const AtomicU32,
        count: usize,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            ptr,
            len: count,
            _owner: owner,
        }
    }

    /// Number of control buses backing this view.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn slot(&self, index: usize) -> Option<&AtomicU32> {
        // SAFETY: in-range offsets into the array `_owner` keeps alive.
        (index < self.len).then(|| unsafe { &*self.ptr.add(index) })
    }

    pub fn get(&self, index: usize) -> f32 {
        self.slot(index)
            .map_or(0.0, |b| f32::from_bits(b.load(Ordering::Relaxed)))
    }

    pub fn set(&self, index: usize, value: f32) {
        if let Some(b) = self.slot(index) {
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
    audio: Vec<UnsafeCell<Block>>,
    pub control: ControlBuses,
}

// SAFETY: concurrent access to `audio` only happens during a parallel stage,
// where the scheduler proves per-bus disjointness; everything else is
// single-threaded on the audio thread.
unsafe impl Send for Buses {}
unsafe impl Sync for Buses {}

impl Buses {
    pub fn new(control: ControlBuses, audio_count: usize) -> Self {
        Self {
            audio: (0..audio_count.max(1))
                .map(|_| UnsafeCell::new(Block::SILENCE))
                .collect(),
            control,
        }
    }

    /// Number of audio buses (the hardware outputs are buses `0..channels`).
    #[inline]
    pub fn audio_count(&self) -> usize {
        self.audio.len()
    }

    pub fn clear_audio(&mut self) {
        for bus in &mut self.audio {
            bus.get_mut().0.fill(0.0);
        }
    }

    /// Shared read of one audio bus. The index is clamped to the configured
    /// bus count, so an out-of-range index degrades to the last bus instead
    /// of an out-of-bounds panic (the same safety net as the `min` clamps at
    /// the call sites, but here it also covers a reduced `--audio-buses`).
    ///
    /// During a parallel stage this may race only with writes to *other*
    /// buses (scheduler invariant), so the plain reference is sound.
    #[inline]
    pub fn audio(&self, bus: usize) -> &[f32; BLOCK_SIZE] {
        let bus = bus.min(self.audio.len() - 1);
        unsafe { &(*self.audio[bus].get()).0 }
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
        let bus = bus.min(self.audio.len() - 1);
        unsafe { &mut (*self.audio[bus].get()).0 }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoneAction {
    None = 0,
    PauseSelf = 1,
    FreeSelf = 2,
    FreeGroup = 14,
}

/// Calculation rate of a UGen output (S1) — scsynth's four rates, made an
/// explicit, validated property of every UGen. It decides how much of the
/// UGen's output wire is meaningful and when the UGen runs:
/// - [`Ar`](Rate::Ar): one value per sample — a full [`Block`] wire, run every
///   block. The default for signal UGens and the only shape before S1.
/// - [`Kr`](Rate::Kr): one value per block — a length-1 wire computed once per
///   block (read back through [`at`] as a constant across the block).
/// - [`Ir`](Rate::Ir): one value computed at synth init and held for the
///   node's life — also a length-1 wire, but written once (`SampleRate.ir`,
///   `BufFrames.ir`, `Rand.ir`).
/// - [`Dr`](Rate::Dr): demand rate — values *pulled* by a driver (`Demand`),
///   not run in block order at all (see [`UGen::demand`]/[`UGen::drive`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    /// Initial/scalar rate: computed once at synth init, then held.
    Ir,
    /// Control rate: one value per block.
    Kr,
    /// Audio rate: one value per sample.
    Ar,
    /// Demand rate: pulled on demand by a driver, off the block schedule.
    Dr,
}

impl Rate {
    /// Coercion rank over the block-producing rates: `ir < kr < ar`. A lower
    /// rate widens into a higher-rate input for free (a constant broadcast, a
    /// block-constant read); the reverse cannot be frozen. `dr` sits off this
    /// axis (it only flows through a demand driver), so it ranks highest and
    /// is handled by dedicated compiler rules rather than by this order.
    pub fn rank(self) -> u8 {
        match self {
            Rate::Ir => 0,
            Rate::Kr => 1,
            Rate::Ar => 2,
            Rate::Dr => 3,
        }
    }

    /// The wire name used in the def format (`"ir"`/`"kr"`/`"ar"`/`"dr"`).
    pub fn parse(name: &str) -> Option<Rate> {
        match name {
            "ir" => Some(Rate::Ir),
            "kr" => Some(Rate::Kr),
            "ar" => Some(Rate::Ar),
            "dr" => Some(Rate::Dr),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Rate::Ir => "ir",
            Rate::Kr => "kr",
            Rate::Ar => "ar",
            Rate::Dr => "dr",
        }
    }
}

pub trait UGen: Send {
    /// Writes one block into `output`. `inputs` are full-block or length-1
    /// slices, already resolved by the synth. `output.len()` reflects the
    /// UGen's rate: [`Rate::Ar`] fills the whole slice, [`Rate::Kr`]/
    /// [`Rate::Ir`] a length-1 slice.
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]);

    /// Called after `process` to signal completion (e.g., EnvGen reaching its end).
    fn done(&self) -> DoneAction {
        DoneAction::None
    }

    /// Demand-rate pull (S1): a demand *source* (`Dseq`, and later the rest of
    /// the `D*` family) returns its next value when its driver pulls it, or
    /// `NaN` once the stream is exhausted. Non-demand UGens never see this.
    /// Runs on the audio thread — allocation-free, like `process`.
    fn demand(&mut self, _ctx: &ProcessCtx, _inputs: &[&[f32]]) -> f32 {
        f32::NAN
    }

    /// Resets a demand source's internal position (a driver's `reset` edge).
    fn reset_demand(&mut self) {}

    /// Demand *driver* (`Demand`): steps `output` one block, calling `step` to
    /// pull the next value (`step(false)`) or reset the source (`step(true)`).
    /// The synth wires `step` to the driver's demand source. Default: the UGen
    /// is not a driver, so this is never called.
    fn drive(
        &mut self,
        _trig: &[f32],
        _reset: &[f32],
        _output: &mut [f32],
        _step: &mut dyn FnMut(bool) -> f32,
    ) {
    }
}

/// Reads input `i` from a block or a single-sample slice.
#[inline(always)]
pub fn at(input: &[f32], i: usize) -> f32 {
    if input.len() == 1 { input[0] } else { input[i] }
}
