//! UGens, buses and DSP algorithms.
//!
//! A UGen processes one block through `process`: inputs are slices that are
//! either a full block (a wire from an earlier UGen) or a single sample (a
//! constant or a control). Use [`at`] to read them uniformly. The context
//! carries the global buses; only I/O UGens touch them. Everything here runs
//! on the audio thread: no allocation.

// Engine-core submodules, built with every feature set: the buffer pool
// (`/b_*` serves any def family), denormal control, and the `/b_gen`
// wavetable/generator commands (pure buffer math).
pub mod buffer;
pub mod denormals;
pub mod wavetable;

// The UGen library — the SynthDef family (`synth` feature). A Faust-only or
// core-only build carries none of it; Faust synths implement `node::SynthNode`
// directly and only touch the core types above.
#[cfg(feature = "synth")]
pub mod binop;
#[cfg(feature = "synth")]
pub mod buf;
#[cfg(feature = "synth")]
pub mod demand;
#[cfg(feature = "synth")]
pub mod disk;
#[cfg(feature = "synth")]
pub mod envgen;
#[cfg(feature = "synth")]
pub mod fused;
#[cfg(feature = "synth")]
pub mod impulse;
#[cfg(feature = "synth")]
pub mod io;
#[cfg(feature = "synth")]
pub mod lag;
#[cfg(feature = "synth")]
pub mod local;
#[cfg(feature = "synth")]
pub mod noise;
#[cfg(feature = "synth")]
pub mod osc;
#[cfg(feature = "synth")]
pub mod registry;
#[cfg(feature = "synth")]
pub mod reply;
#[cfg(feature = "synth")]
pub mod scalar;
#[cfg(feature = "synth")]
pub mod sinosc;
#[cfg(feature = "synth")]
pub mod spectral;
#[cfg(feature = "synth")]
pub mod unop;

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
pub const NUM_CONTROL_BUSES: usize = 16384;
/// Hard ceiling on inputs per UGen: the synth builds its input list on a
/// fixed stack array of this width (see `synthdef::instance`), so it is a
/// compile-time invariant, not a tunable. EnvGen already needs 21 (ADSR).
/// The boot-time `--max-ugen-inputs` (see [`Limits`]) is a *runtime* limit
/// clamped to this ceiling, the same way `--audio-buses` clamps to 128.
pub const MAX_UGEN_INPUTS: usize = 32;

/// Boot-time capacities for the pre-allocated pools (scsynth's `-n`/`-b`/…).
///
/// Every one of these sizes a slab or `Vec` built **once at server startup**:
/// they are fixed at runtime by the CLI/config, never at compile time. The
/// defaults match the historical compile-time constants. `max_ugen_inputs` is
/// clamped to the [`MAX_UGEN_INPUTS`] hard ceiling by [`Limits::clamped`]
/// (that one *is* a compile-time array width; the runtime knob can only make
/// it stricter, like audio buses cap at 128).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Node slab capacity, root included (`--max-nodes`, scsynth `-n`).
    pub max_nodes: usize,
    /// Buffer pool capacity (`--max-buffers`, scsynth `-b`).
    pub max_buffers: usize,
    /// Pre-reserved child capacity of a non-root group (`--max-graph-children`).
    pub max_group_children: usize,
    /// Accepted inputs per UGen when compiling a def (`--max-ugen-inputs`),
    /// clamped to [`MAX_UGEN_INPUTS`].
    pub max_ugen_inputs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Kept in sync with `node::MAX_NODES` / `node::MAX_GROUP_CHILDREN`
            // / `buffer::NUM_BUFFERS`; those consts stay the documented default.
            max_nodes: 8192,
            max_buffers: 4096,
            max_group_children: 512,
            max_ugen_inputs: MAX_UGEN_INPUTS,
        }
    }
}

impl Limits {
    /// Clamps each field to a usable minimum and the ugen-input hard ceiling,
    /// so an out-of-range CLI/config value degrades instead of panicking. The
    /// node slab keeps at least the root; a group keeps room for one child.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            max_nodes: self.max_nodes.max(1),
            max_buffers: self.max_buffers,
            max_group_children: self.max_group_children.max(1),
            max_ugen_inputs: self.max_ugen_inputs.clamp(1, MAX_UGEN_INPUTS),
        }
    }
}

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

/// What a UGen (via [`UGen::done`]) asks the engine to do when it finishes —
/// scsynth's full done-action set (`Done.schelp`, values 0–15). `None`/
/// `PauseSelf` are applied inline on the audio thread; every other action frees
/// this node (and possibly a sibling or the group) and is queued for the drain
/// after the block. The relative actions resolve the node's previous/next
/// sibling and head/tail-of-group; see `node::NodeTree::apply_done_action`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DoneAction {
    /// Do nothing (the envelope just holds its final level).
    None = 0,
    /// Pause this synth (skip it from now on; it stays in the tree). Cleared by
    /// `/n_run 1`.
    PauseSelf = 1,
    /// Free this synth.
    FreeSelf = 2,
    /// Free this synth and the preceding node.
    FreeSelfAndPrev = 3,
    /// Free this synth and the following node.
    FreeSelfAndNext = 4,
    /// Free this synth; if the preceding node is a group, free all its children
    /// (else free it).
    FreeSelfAndFreeAllInPrev = 5,
    /// Free this synth; if the following node is a group, free all its children.
    FreeSelfAndFreeAllInNext = 6,
    /// Free this synth and every preceding node in its group.
    FreeSelfToHead = 7,
    /// Free this synth and every following node in its group.
    FreeSelfToTail = 8,
    /// Free this synth and pause the preceding node.
    FreeSelfPausePrev = 9,
    /// Free this synth and pause the following node.
    FreeSelfPauseNext = 10,
    /// Free this synth; if the preceding node is a group, deep-free it (free its
    /// synths, keep the groups); else free it.
    FreeSelfAndDeepFreePrev = 11,
    /// Free this synth; if the following node is a group, deep-free it.
    FreeSelfAndDeepFreeNext = 12,
    /// Free this synth and every other node in its group.
    FreeAllInGroup = 13,
    /// Free the enclosing group (this synth included).
    FreeGroup = 14,
    /// Free this synth and resume (unpause) the following node.
    FreeSelfResumeNext = 15,
}

impl DoneAction {
    /// Maps a wire/UGen integer (an `EnvGen` `doneAction` input, or a queued
    /// action code) to the enum; out-of-range is [`DoneAction::None`].
    pub fn from_u8(v: u8) -> DoneAction {
        use DoneAction::*;
        match v {
            1 => PauseSelf,
            2 => FreeSelf,
            3 => FreeSelfAndPrev,
            4 => FreeSelfAndNext,
            5 => FreeSelfAndFreeAllInPrev,
            6 => FreeSelfAndFreeAllInNext,
            7 => FreeSelfToHead,
            8 => FreeSelfToTail,
            9 => FreeSelfPausePrev,
            10 => FreeSelfPauseNext,
            11 => FreeSelfAndDeepFreePrev,
            12 => FreeSelfAndDeepFreeNext,
            13 => FreeAllInGroup,
            14 => FreeGroup,
            15 => FreeSelfResumeNext,
            _ => None,
        }
    }

    /// As [`from_u8`](Self::from_u8) but from the `i32`/`f32`-derived value a
    /// UGen carries; negative or too-large values are [`DoneAction::None`].
    pub fn from_i32(v: i32) -> DoneAction {
        if (0..=15).contains(&v) {
            DoneAction::from_u8(v as u8)
        } else {
            DoneAction::None
        }
    }
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

    /// Receives a typed out-of-band command addressed to this instance
    /// (`/u_cmd`). The mechanism the future FFT/streaming UGens use to take
    /// parameters that are neither audio nor control inputs. Runs on the audio
    /// thread — the payload is inline, so this must stay allocation-free. The
    /// default ignores every command (an unknown selector is a no-op).
    fn command(&mut self, _cmd: &UGenCmd) {}

    /// Runs a spectral-chain UGen (`FFT`/`PV_*`/`IFFT`, S8) with access to its
    /// synth-private [`SpectralChain`](spectral::SpectralChain) — state the plain
    /// `process` path cannot reach, since the chain is shared across UGens. The
    /// synth calls this (instead of `process`) for [`ExecMode::Spectral`]
    /// UGens, resolving the compile-assigned chain slot. Runs on the audio
    /// thread: the transform reuses pre-allocated scratch and never allocates.
    /// Non-spectral UGens never see this.
    #[cfg(feature = "synth")]
    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        _output: &mut [f32],
        _chain: &mut spectral::SpectralChain,
    ) {
    }

    /// Whether this is a side-effect UGen (`SendReply`/`SendTrig`/`Poll`, S9)
    /// that emits reply messages instead of (or besides) audio. The synth uses
    /// it to enqueue itself for the reply drain after each block. Default: not
    /// a reply UGen.
    fn is_reply(&self) -> bool {
        false
    }

    /// Drains the side-effect messages this UGen buffered during the block into
    /// `sink`, each stamped with `node_id`. Called after the block on the audio
    /// thread — allocation-free (the buffer is a fixed inline array). Default:
    /// nothing to drain.
    fn drain_replies(&mut self, _node_id: i32, _sink: &mut dyn FnMut(ReplyMsg)) {}
}

/// Max side-effect messages a reply UGen (`SendReply`/`SendTrig`/`Poll`)
/// buffers within one block before the synth drains it. Extra triggers in the
/// same block are dropped — best-effort, like the node-event FIFO. Inline and
/// `Copy`, so buffering a trigger on the audio thread never allocates.
pub const REPLY_BUFFER_LEN: usize = 8;

/// Max float values one [`ReplyMsg`] carries (a `SendReply` value list).
pub const REPLY_MAX_VALUES: usize = 16;

/// Max bytes of a reply's name (a `SendReply` command name or a `Poll` label),
/// stored inline so the message stays `Copy` and heap-free.
pub const REPLY_NAME_MAX: usize = 31;

/// Which side-effect UGen produced a [`ReplyMsg`] — decides how the network
/// thread turns it into an OSC reply or console line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyKind {
    /// `SendTrig` — a `/tr nodeID trigID value` message.
    Trig,
    /// `SendReply` — a `cmdName nodeID replyID value…` message.
    Reply,
    /// `Poll` — a console line `label: value`, plus a `/tr` when its trigid ≥ 0.
    Poll,
}

/// A side-effect message a UGen emits on a trigger (`SendReply`/`SendTrig`/
/// `Poll`, S9): the payload that leaves the audio thread through the reply FIFO
/// and becomes an OSC reply (or a console post) on the network thread. Fully
/// inline and `Copy` — buffering and draining one allocates nothing.
#[derive(Clone, Copy, Debug)]
pub struct ReplyMsg {
    /// Emitting node; stamped by the synth when it drains the UGen.
    pub node_id: i32,
    /// Trigger/reply id (`SendTrig` id, `SendReply` replyID, `Poll` trigid).
    pub id: i32,
    pub kind: ReplyKind,
    /// Command name (`SendReply`) or label (`Poll`); empty for `SendTrig`.
    name: [u8; REPLY_NAME_MAX],
    name_len: u8,
    values: [f32; REPLY_MAX_VALUES],
    num_values: u8,
}

impl ReplyMsg {
    /// A message with no node id yet (the synth stamps it on drain), the given
    /// kind, id and inline name (truncated past [`REPLY_NAME_MAX`]). Append
    /// values with [`push_value`](Self::push_value).
    pub fn new(kind: ReplyKind, id: i32, name: &str) -> ReplyMsg {
        let mut bytes = [0u8; REPLY_NAME_MAX];
        let n = name.len().min(REPLY_NAME_MAX);
        bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
        ReplyMsg {
            node_id: 0,
            id,
            kind,
            name: bytes,
            name_len: n as u8,
            values: [0.0; REPLY_MAX_VALUES],
            num_values: 0,
        }
    }

    /// Appends one value; dropped once [`REPLY_MAX_VALUES`] is reached.
    #[inline]
    pub fn push_value(&mut self, v: f32) {
        let n = self.num_values as usize;
        if n < REPLY_MAX_VALUES {
            self.values[n] = v;
            self.num_values = (n + 1) as u8;
        }
    }

    /// The reply's name (command name / label); empty for `SendTrig`.
    pub fn name(&self) -> &str {
        std::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    /// The value list carried by this reply.
    pub fn values(&self) -> &[f32] {
        &self.values[..self.num_values as usize]
    }
}

impl Default for ReplyMsg {
    fn default() -> Self {
        ReplyMsg::new(ReplyKind::Trig, 0, "")
    }
}

/// Max inline float args a [`UGenCmd`] carries. Sized for realistic UGen
/// commands (a selector plus a few scalar params); keeps the payload `Copy` and
/// heap-free, so applying a `/u_cmd` on the audio thread allocates nothing.
pub const MAX_UGEN_CMD_ARGS: usize = 8;

/// A typed command addressed to one UGen instance (`/u_cmd`) — the discoverable
/// replacement for scsynth's untyped `/u_cmd` blob. The command name is hashed
/// to a stable `selector` on the network thread (so both sides agree without a
/// shared table); `args` are inline floats. Consumers are future UGens; today
/// every UGen's default [`UGen::command`] ignores it.
#[derive(Clone, Copy, Debug)]
pub struct UGenCmd {
    /// Stable hash of the command name (see [`ugen_cmd_selector`]).
    pub selector: u32,
    pub args: [f32; MAX_UGEN_CMD_ARGS],
    pub num_args: u8,
}

/// FNV-1a hash of a `/u_cmd` command name into its selector. Deterministic and
/// shared by the network thread (which resolves the name) and any consumer
/// UGen (which matches `ugen_cmd_selector("myCommand")` in its `command`).
pub fn ugen_cmd_selector(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Reads input `i` from a block or a single-sample slice.
#[inline(always)]
pub fn at(input: &[f32], i: usize) -> f32 {
    if input.len() == 1 { input[0] } else { input[i] }
}
