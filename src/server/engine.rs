//! Processing engine, independent of the audio backend.
//!
//! [`engine_pair`] returns the two halves of the server: the [`Engine`] lives
//! on the audio thread, the [`EngineHandle`] on the network thread. They talk
//! exclusively through lock-free SPSC ring buffers: commands flow in fully
//! pre-built (the audio thread only plugs them in), freed memory flows back
//! out as [`Garbage`] to be dropped on the network side, and node lifecycle
//! events flow out as [`NodeEvent`]s for `/node_start`/`/node_end` notifications.
//!
//! Timed bundles arrive as [`Cmd::Schedule`] carrying an absolute
//! target in samples; the engine keeps them in a pre-allocated queue sorted
//! by time (FIFO for equal times) and executes them **sample-accurately**,
//! splitting the block at each event's offset. The engine publishes its
//! sample counter so the network thread can convert NTP timetags.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

pub use crate::dsp::BLOCK_SIZE;
use crate::dsp::BusUsage;
use crate::dsp::buffer::{Buffer, BufferPool, empty_pool_with};
use crate::dsp::{
    Buses, ControlBuses, Limits, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES, ProcessCtx, ReplyMsg,
    TransportCtx,
};
use crate::node::{AddAction, FreedNode, Group, NodeKind, NodeTree, Place, SynthNode};
use crate::server::clock_axis::{DeviceSample, PiecePosition, PositionAnchor, TransportSample};
use crate::server::ipc::Segment;
use crate::server::workers::WorkerPool;

const CMD_FIFO_CAPACITY: usize = 1024;
/// Floor for the garbage FIFO; scaled to `2 * max_nodes` at boot so a
/// mass-free of a full tree never spills into the leak path.
const GARBAGE_FIFO_CAPACITY: usize = 1024;
/// Local holding list for when the garbage FIFO is full.
const PENDING_GARBAGE_CAPACITY: usize = 64;
/// Floor for the node-event FIFO; scaled to `2 * max_nodes` at boot. Events
/// stay best-effort, but the id registries recycle off `/node_end`, so the
/// capacity must cover at least one full-tree turnover per drain — a dropped
/// end event is a client-side id that never comes back.
const EVENT_FIFO_CAPACITY: usize = 2048;
/// Side-effect reply messages (`SendReply`/`SendTrig`/`Poll`) buffered from
/// the audio thread to the network thread; over capacity they drop, best-effort
/// like the node events.
const REPLY_FIFO_CAPACITY: usize = 2048;
/// Pre-allocated capacity of the scheduled-bundle queue; bundles beyond it
/// are rejected (shipped back through the garbage FIFO).
const SCHED_CAPACITY: usize = 1024;

/// Commands are built **completely** on the network thread (including boxed
/// synths and pre-reserved group child lists); applying them on the audio
/// thread never allocates.
pub enum Cmd {
    AddSynth {
        id: i32,
        target: i32,
        action: AddAction,
        synth: Box<dyn SynthNode>,
        /// Bus masks analyzed at build time; the parallel scheduler
        /// partitions stages from this engine-owned copy.
        usage: BusUsage,
    },
    AddGroup {
        id: i32,
        target: i32,
        action: AddAction,
        group: Group,
    },
    FreeNode {
        id: i32,
    },
    /// `/group_freeAll`: free all children of a group; the group stays.
    FreeAllInGroup {
        id: i32,
    },
    /// `/group_deepFree`: free all synths in a group and its subgroups.
    DeepFreeGroup {
        id: i32,
    },
    /// `/node_run`: pause (`run = false`) or resume (`true`) a node — a synth or a
    /// whole group. Makes `DoneAction::PauseSelf` non-terminal.
    RunNode {
        id: i32,
        run: bool,
    },
    /// Rolls (`rolling = true`) or stops (`false`) the transport. Stopped, the
    /// transport clock holds and the transport queue cannot fall due; the
    /// device clock is untouched either way.
    TransportRun {
        rolling: bool,
    },
    /// Binds the group the transport governs; `id < 0` unbinds. Unbinding
    /// thaws the group, so no frozen ownerless subtree is left behind.
    TransportGroup {
        id: i32,
    },
    /// `/transport_locate`: moves the piece's position, leaving both clocks
    /// alone. One store of the anchor — see `server::clock_axis`.
    TransportLocate {
        position: u64,
    },
    /// `/transport_loop`: the span the position wraps inside, `None` to stop
    /// looping. An empty or inverted span is not a loop and is rejected before
    /// it reaches here.
    TransportLoop {
        span: Option<Range<u64>>,
    },
    /// `/node_before` / `/node_after`.
    MoveNode {
        id: i32,
        target: i32,
        place: Place,
    },
    SetControl {
        id: i32,
        index: u32,
        value: f32,
    },
    /// `/node_map` (`audio = false`) / `/node_mapAudio` (`audio = true`): binds a
    /// control to a bus the synth reads at the start of every block, or
    /// `bus = -1` to unbind. RT-safe: it only flips an entry in the synth's
    /// pre-allocated mapping table.
    MapControl {
        id: i32,
        index: u32,
        bus: i32,
        audio: bool,
    },
    /// Installs (`Some`) or removes (`None`) a buffer in the pool. The
    /// buffer arrives fully built by the NRT thread; the replaced one leaves
    /// through the garbage FIFO.
    SetBuffer {
        index: usize,
        buffer: Option<Arc<Buffer>>,
    },
    /// `/bus_set` inside a timed bundle: the immediate form writes the shared
    /// atomics from the network thread, but a scheduled write must land at
    /// its exact sample, so it travels to the audio thread like any command.
    SetControlBus {
        index: usize,
        value: f32,
    },
    /// `/bus_tap`: routes audio bus `bus` into audio-tap ring `tap` of the IPC
    /// segment (the engine appends that bus's block to the ring at the end of
    /// every block); `bus = -1` stops the tap. RT-safe: it only flips an entry
    /// in the engine's pre-allocated tap table.
    SetTap {
        tap: usize,
        bus: i32,
    },
    /// `/node_set` on a control used as a bus index: ships the re-analyzed
    /// masks so the parallel scheduler stays in sync.
    SetUsage {
        id: i32,
        usage: BusUsage,
    },
    /// `/group_parallel`: children of this group run in dependency stages on
    /// the worker pool.
    SetGroupParallel {
        id: i32,
        parallel: bool,
    },
    /// A timed bundle: `cmds` execute back to back when the stream reaches
    /// `time` (absolute, in samples), splitting the block at that offset.
    /// Built on the network thread; the spent `Vec` shell returns as
    /// [`Garbage::SpentBundle`].
    Schedule {
        time: u64,
        cmds: Vec<Cmd>,
    },
    /// `/sched_clear`: drop every pending timed bundle. Each drained bundle's
    /// `Vec<Cmd>` (with its boxed synths) leaves through the garbage FIFO as
    /// [`Garbage::SpentBundle`], so nothing is freed on the audio thread.
    ClearSched,
    /// `/node_ugenCmd`: a typed command addressed to one UGen instance inside a synth.
    /// The payload is inline (no heap), so applying it allocates nothing.
    UGenCommand {
        id: i32,
        ugen_index: u32,
        command: crate::dsp::UGenCmd,
    },
}

/// Heap memory leaving the audio thread to be dropped on the network side.
pub enum Garbage {
    FreedSynth {
        id: i32,
        synth: Box<dyn SynthNode>,
    },
    FreedGroup {
        id: i32,
        group: Group,
    },
    /// Command the engine could not apply (duplicate ID, unknown target,
    /// full node table or full group).
    RejectedSynth {
        id: i32,
        synth: Box<dyn SynthNode>,
    },
    RejectedGroup {
        id: i32,
        group: Group,
    },
    /// A buffer replaced or removed from the pool; this clone is dropped on
    /// the network side so the deallocation (if it is the last `Arc`) never
    /// happens on the audio thread.
    FreedBuffer(Arc<Buffer>),
    /// The drained shell of an executed scheduled bundle (its heap capacity
    /// must be freed on the network side) — or, if non-empty, a bundle the
    /// engine rejected because the schedule queue was full.
    SpentBundle(Vec<Cmd>),
}

/// A timed bundle waiting in the engine's queue.
struct ScheduledBundle {
    time: u64,
    cmds: Vec<Cmd>,
}

/// A timed bundle on the transport axis. Same shape as [`ScheduledBundle`];
/// the type of `time` is the whole difference, and it is what keeps the two
/// queues from being fed each other's stamps.
struct ScheduledBundleT {
    time: TransportSample,
    cmds: Vec<Cmd>,
}

/// The node a command acts on, if it acts on one. For a node being created it
/// is the **target** it is added relative to — the node itself does not exist
/// yet, so it cannot be walked.
///
/// Every variant is listed: no catch-all arm, so a `Cmd` added later fails to
/// compile here rather than being silently classified as ungoverned.
pub(crate) fn cmd_target_nodes(cmd: &Cmd) -> [Option<i32>; 2] {
    match cmd {
        // A node being created does not exist yet, so the end that can be
        // walked is where it is going.
        Cmd::AddSynth { target, .. } | Cmd::AddGroup { target, .. } => [Some(*target), None],
        // A move touches **both** ends, and either one being governed governs
        // the bundle. Classifying a move by its source alone would let
        // `/node_before` splice a node into a frozen subtree while it is
        // frozen, while `/node_add` -- the same structural edit -- waited for
        // the resume: two answers to one question, decided by which command the
        // client happened to use.
        Cmd::MoveNode { id, target, .. } => [Some(*id), Some(*target)],
        Cmd::FreeNode { id }
        | Cmd::FreeAllInGroup { id }
        | Cmd::DeepFreeGroup { id }
        | Cmd::RunNode { id, .. }
        | Cmd::SetControl { id, .. }
        | Cmd::MapControl { id, .. }
        | Cmd::SetUsage { id, .. }
        | Cmd::SetGroupParallel { id, .. }
        | Cmd::UGenCommand { id, .. } => [Some(*id), None],
        // No node target: a bus write, a buffer swap, a tap route, the
        // transport's own controls, or a queue-wide operation. These carry no
        // opinion about which axis the bundle belongs to.
        Cmd::TransportRun { .. }
        | Cmd::TransportGroup { .. }
        | Cmd::TransportLocate { .. }
        | Cmd::TransportLoop { .. }
        | Cmd::SetBuffer { .. }
        | Cmd::SetControlBus { .. }
        | Cmd::SetTap { .. }
        | Cmd::ClearSched => [None, None],
        // A nested bundle classifies itself when it is applied, against the
        // tree and the frozen total of that moment; deciding for it here would
        // only duplicate that, at a time when it is not yet due.
        Cmd::Schedule { .. } => [None, None],
    }
}

/// Node lifecycle event for `/node_start`/`/node_end` notifications. POD; delivery is
/// best-effort (dropped silently if the FIFO is full).
#[derive(Clone, Copy, Debug)]
pub struct NodeEvent {
    pub kind: NodeEventKind,
    pub id: i32,
    pub parent_id: i32,
    pub is_group: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeEventKind {
    Go,
    End,
}

/// Counts published by the audio thread (relaxed stores) and read by the
/// network thread for `/server_status.reply`.
pub struct Counters {
    pub synths: AtomicU32,
    pub ugens: AtomicU32,
    pub groups: AtomicU32,
    /// Average DSP load as a fraction of the block budget
    /// (`BLOCK_SIZE / sample_rate` wall time), an EMA with a ~1 s time
    /// constant. `f32` bits in an `AtomicU32`; only meaningful in real time
    /// (NRT renders run unpaced, so the fraction is just render speed).
    pub avg_cpu: AtomicU32,
    /// Highest per-block load since the last [`Counters::take_peak_cpu`]
    /// (`f32` bits; non-negative floats order like their bit patterns, so
    /// `fetch_max` on the bits is a float max).
    pub peak_cpu: AtomicU32,
    /// Blocks whose processing exceeded their real-time budget (cumulative
    /// since boot) — the engine-side xrun proxy: the callback cannot have met
    /// its deadline for that block unless the host buffered extra latency.
    pub late_blocks: AtomicU32,
}

impl Counters {
    pub fn avg_cpu(&self) -> f32 {
        f32::from_bits(self.avg_cpu.load(Ordering::Relaxed))
    }

    /// Returns the peak per-block load since the previous call and resets it,
    /// so every `/server_status` poll reports the peak of its own window.
    pub fn take_peak_cpu(&self) -> f32 {
        f32::from_bits(self.peak_cpu.swap(0, Ordering::Relaxed))
    }

    pub fn late_blocks(&self) -> u32 {
        self.late_blocks.load(Ordering::Relaxed)
    }
}

/// Routes freed nodes to the garbage and event FIFOs. Borrows the individual
/// engine fields so the tree (also a field) can stay mutably borrowed.
struct GarbageSink<'a> {
    garbage_tx: &'a mut Producer<Garbage>,
    pending_garbage: &'a mut Vec<Garbage>,
    events_tx: &'a mut Producer<NodeEvent>,
}

impl GarbageSink<'_> {
    fn consume(&mut self, freed: FreedNode) {
        match freed {
            FreedNode::Synth {
                id,
                parent_id,
                synth,
            } => {
                self.event(id, parent_id, false);
                self.push(Garbage::FreedSynth { id, synth });
            }
            FreedNode::Group {
                id,
                parent_id,
                group,
            } => {
                self.event(id, parent_id, true);
                self.push(Garbage::FreedGroup { id, group });
            }
        }
    }

    fn event(&mut self, id: i32, parent_id: i32, is_group: bool) {
        let _ = self.events_tx.push(NodeEvent {
            kind: NodeEventKind::End,
            id,
            parent_id,
            is_group,
        });
    }

    fn push(&mut self, garbage: Garbage) {
        if let Err(PushError::Full(g)) = self.garbage_tx.push(garbage) {
            if self.pending_garbage.len() < self.pending_garbage.capacity() {
                self.pending_garbage.push(g);
            } else {
                // FIFO and holding list both full. Leaking is the only RT-safe
                // option left: dropping here would free memory on this thread.
                std::mem::forget(g);
            }
        }
    }
}

/// Per-block release factor of the published audio-bus levels: how much a
/// held peak decays each block, so a meter reading at any rate — a display
/// frame is a dozen blocks — sees a transient instead of missing it between
/// looks. [`LEVEL_RELEASE_DB_PER_SEC`] dB per second, the usual peak-meter
/// ballistic; a decay (rather than a max the reader clears) is what keeps it
/// correct for **several** readers of the same bus at once.
fn level_release(sample_rate: f32) -> f32 {
    let block_secs = BLOCK_SIZE as f32 / sample_rate.max(1.0);
    10.0f32.powf(-LEVEL_RELEASE_DB_PER_SEC / 20.0 * block_secs)
}

/// Release rate of a held bus level, in dB per second.
pub const LEVEL_RELEASE_DB_PER_SEC: f32 = 20.0;

/// Audio-thread half. `process_block` does not allocate, lock or do I/O.
pub struct Engine {
    sample_rate: f32,
    /// Per-block decay applied to the published bus levels (see
    /// [`level_release`]), computed once from the sample rate.
    level_release: f32,
    channels: usize,
    tree: NodeTree,
    /// DSP workers for parallel groups; empty pool = sequential.
    pool: WorkerPool,
    buses: Buses,
    buffers: BufferPool,
    /// Live hardware input: decoded interleaved frames arriving from the
    /// cpal input stream through a lock-free ring. `0` channels / `None`
    /// consumer means no input stream is open. Read at each block start into
    /// audio buses `channels..channels + input_channels`, which `In`/`In.ar`
    /// then read like any bus.
    input_channels: usize,
    input_rx: Option<Consumer<f32>>,
    /// Samples processed since start; the stream clock scheduled bundles
    /// are measured against.
    now: u64,
    /// Whether the transport rolls. Stopped, `frozen_total` accumulates and
    /// `transport_now` holds.
    transport_rolling: bool,
    /// Total samples the transport has spent stopped since boot. The whole of
    /// the device -> transport conversion (see `server::clock_axis`).
    frozen_total: u64,
    /// Block-accurate mirror of the transport clock for the network thread,
    /// beside `sample_clock`.
    transport_clock: Arc<AtomicU64>,
    /// Total samples the transport has spent stopped, mirrored for the network
    /// thread. Published beside the clock rather than derived from
    /// `current_samples() - current_transport_samples()`, because those are two
    /// separate loads and can straddle a block.
    frozen_clock: Arc<AtomicU64>,
    /// The group the transport governs, frozen while the transport is stopped.
    transport_group: Option<i32>,
    /// Where the piece is, as an anchor onto the transport clock: the position
    /// a locate put it at, and the transport sample that locate landed on. A
    /// read is one add, so the position costs the per-sample path nothing.
    position: PositionAnchor,
    /// Block-accurate mirror of the piece's position for the network thread
    /// and the segment.
    position_clock: Arc<AtomicU64>,
    /// The span the position wraps inside while looping. Always non-empty:
    /// an empty or inverted span is refused before it reaches the engine, and
    /// the wrap below would not terminate over one.
    transport_loop: Option<Range<u64>>,
    /// How far into the current block the engine is standing, in samples.
    /// Zero outside [`Engine::process_block`]'s cut loop; see
    /// [`Engine::transport_here`] for why anything reads it.
    cursor: usize,
    /// Pending timed bundles, sorted by time (stable for equal times).
    /// Pre-allocated: insertion and removal never allocate.
    sched: Vec<ScheduledBundle>,
    /// Pending timed bundles on the transport axis. Frozen with the transport:
    /// while stopped nothing here can fall due, and nothing here is rewritten.
    /// Pre-allocated like `sched`, to the same capacity.
    sched_transport: Vec<ScheduledBundleT>,
    sample_clock: Arc<AtomicU64>,
    /// block-accurate mirror of the sample clock into the IPC segment
    /// (one extra Release store per block); the Arc pins the mapping.
    ipc: Option<Arc<Segment>>,
    /// Which audio bus each segment tap ring records (`-1` = off), indexed by
    /// tap. Pre-allocated to the segment's tap count; `/bus_tap` flips entries.
    tap_buses: Vec<i32>,
    cmd_rx: Consumer<Cmd>,
    garbage_tx: Producer<Garbage>,
    pending_garbage: Vec<Garbage>,
    events_tx: Producer<NodeEvent>,
    reply_tx: Producer<ReplyMsg>,
    counters: Arc<Counters>,
    /// EMA state of the CPU meter (fraction of the block budget, ~1 s time
    /// constant); published to `counters.avg_cpu` every block. Compiled out
    /// on wasm32 with the meter itself.
    #[cfg(not(target_arch = "wasm32"))]
    avg_cpu: f32,
}

/// Network-thread half: sends commands, collects garbage and events, reads
/// counters, serves the control buses directly.
pub struct EngineHandle {
    pub sample_rate: f32,
    pub channels: usize,
    /// Configured audio bus count (after clamping to the 128 ceiling).
    pub audio_buses: usize,
    /// Live hardware input channels; `0` when no input stream is open.
    /// Set by the backend once it has negotiated the input device.
    pub input_channels: usize,
    /// Boot-time pool capacities, surfaced in `/server_query.reply` so a client
    /// can discover the server's limits instead of hardcoding them.
    pub limits: Limits,
    cmd_tx: Producer<Cmd>,
    garbage_rx: Consumer<Garbage>,
    events_rx: Consumer<NodeEvent>,
    reply_rx: Consumer<ReplyMsg>,
    control_buses: ControlBuses,
    sample_clock: Arc<AtomicU64>,
    transport_clock: Arc<AtomicU64>,
    /// Total samples the transport has spent stopped, mirrored for the network
    /// thread. Published beside the clock rather than derived from
    /// `current_samples() - current_transport_samples()`, because those are two
    /// separate loads and can straddle a block.
    frozen_clock: Arc<AtomicU64>,
    /// Block-accurate mirror of the piece's position (`/transport_locate`),
    /// which is not a clock: it jumps and it wraps. See `server::clock_axis`.
    position_clock: Arc<AtomicU64>,
    counters: Arc<Counters>,
    /// The IPC segment when one exists — the network thread reads the audio
    /// taps from here (`/bus_tapStream`) without an engine round-trip.
    segment: Option<Arc<Segment>>,
}

pub fn engine_pair(sample_rate: f32, channels: usize) -> (Engine, EngineHandle) {
    engine_pair_with_workers(sample_rate, channels, 0)
}

/// Default bus counts (scsynth `-a`/`-c`), used by the simple constructors and
/// the NRT renderer. The live server can override them with `--audio-buses`/
/// `--control-buses`; audio is capped at 128 (the `BusUsage` mask is a `u128`).
pub const DEFAULT_AUDIO_BUSES: usize = NUM_AUDIO_BUSES;
pub const DEFAULT_CONTROL_BUSES: usize = NUM_CONTROL_BUSES;

/// Like [`engine_pair`], plus a worker pool of `workers` DSP threads
/// for parallel groups (`/group_parallel`). `workers == 0` is fully sequential
/// — identical behavior and output either way (stages are bit-identical to
/// sequential execution by construction).
pub fn engine_pair_with_workers(
    sample_rate: f32,
    channels: usize,
    workers: usize,
) -> (Engine, EngineHandle) {
    engine_pair_full(
        sample_rate,
        channels,
        workers,
        None,
        DEFAULT_AUDIO_BUSES,
        DEFAULT_CONTROL_BUSES,
        Limits::default(),
    )
}

/// Full form: with an IPC segment, the control buses live *inside the
/// segment* (clients on the other side write the very atomics `InCtl`
/// reads) and the engine mirrors its sample clock into it every block.
pub fn engine_pair_full(
    sample_rate: f32,
    channels: usize,
    workers: usize,
    ipc: Option<Arc<Segment>>,
    audio_buses: usize,
    control_buses: usize,
    limits: Limits,
) -> (Engine, EngineHandle) {
    let limits = limits.clamped();
    // The mask is a `u128`, so 128 is the hard ceiling for audio buses.
    let audio_buses = audio_buses.clamp(channels.max(1), NUM_AUDIO_BUSES);
    assert!(channels > 0 && channels <= audio_buses);
    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_FIFO_CAPACITY);
    let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_FIFO_CAPACITY.max(2 * limits.max_nodes));
    let (events_tx, events_rx) = RingBuffer::new(EVENT_FIFO_CAPACITY.max(2 * limits.max_nodes));
    let (reply_tx, reply_rx) = RingBuffer::new(REPLY_FIFO_CAPACITY);
    let counters = Arc::new(Counters {
        synths: AtomicU32::new(0),
        ugens: AtomicU32::new(0),
        // The root group exists before the first tick publishes counts.
        groups: AtomicU32::new(1),
        avg_cpu: AtomicU32::new(0),
        peak_cpu: AtomicU32::new(0),
        late_blocks: AtomicU32::new(0),
    });
    // With an IPC segment the control buses live inside it, so their count is
    // whatever the segment was created with (read back from its header).
    let control_buses = match &ipc {
        Some(segment) => {
            segment.set_sample_rate(sample_rate as f64);
            segment.control_buses()
        }
        None => ControlBuses::new(control_buses),
    };
    let sample_clock = Arc::new(AtomicU64::new(0));
    let transport_clock = Arc::new(AtomicU64::new(0));
    let frozen_clock = Arc::new(AtomicU64::new(0));
    let position_clock = Arc::new(AtomicU64::new(0));
    let tap_buses = vec![-1i32; ipc.as_ref().map_or(0, |s| s.taps())];
    let segment = ipc.clone();
    let engine = Engine {
        sample_rate,
        level_release: level_release(sample_rate),
        channels,
        tree: NodeTree::with_capacity(limits.max_nodes),
        pool: WorkerPool::new(workers),
        buses: Buses::new(control_buses.clone(), audio_buses),
        buffers: empty_pool_with(limits.max_buffers),
        input_channels: 0,
        input_rx: None,
        now: 0,
        transport_rolling: false,
        frozen_total: 0,
        transport_clock: Arc::clone(&transport_clock),
        position: PositionAnchor::default(),
        cursor: 0,
        position_clock: Arc::clone(&position_clock),
        transport_loop: None,
        frozen_clock: Arc::clone(&frozen_clock),
        transport_group: None,
        sched: Vec::with_capacity(SCHED_CAPACITY),
        sched_transport: Vec::with_capacity(SCHED_CAPACITY),
        sample_clock: Arc::clone(&sample_clock),
        ipc,
        tap_buses,
        cmd_rx,
        garbage_tx,
        pending_garbage: Vec::with_capacity(PENDING_GARBAGE_CAPACITY),
        events_tx,
        reply_tx,
        counters: Arc::clone(&counters),
        #[cfg(not(target_arch = "wasm32"))]
        avg_cpu: 0.0,
    };
    let handle = EngineHandle {
        sample_rate,
        channels,
        audio_buses,
        input_channels: 0,
        limits,
        cmd_tx,
        garbage_rx,
        events_rx,
        reply_rx,
        control_buses,
        sample_clock,
        transport_clock,
        position_clock,
        frozen_clock,
        counters,
        segment,
    };
    (engine, handle)
}

impl Engine {
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The transport clock: samples elapsed under the transport.
    pub fn transport_now(&self) -> TransportSample {
        DeviceSample::new(self.now).to_transport(self.frozen_total)
    }

    /// The transport clock **at the cursor** — where inside the current block
    /// the engine is standing, rather than at its first sample.
    ///
    /// A locate arrives inside a timed bundle and lands on an exact sample, so
    /// anchoring it at the block's start would put the piece up to a block
    /// away from where the client asked. Same reason `frozen_total` is
    /// credited at the sample the transport flips rather than a block at a
    /// time. Outside the block-cut loop the cursor is 0 and this is
    /// [`Self::transport_now`].
    fn transport_here(&self) -> TransportSample {
        DeviceSample::new(self.now + self.cursor as u64).to_transport(self.frozen_total)
    }

    /// Where the piece is at the cursor.
    fn position_here(&self) -> PiecePosition {
        self.position.at(self.transport_here())
    }

    /// The device sample at which the position reaches the loop's end, when a
    /// loop is on, the transport rolls and the end is still ahead — what the
    /// block is cut at so a wrap lands on its exact sample.
    fn loop_wrap_due(&self) -> Option<u64> {
        if !self.transport_rolling {
            return None;
        }
        let span = self.transport_loop.as_ref()?;
        let here = self.transport_here();
        self.position
            .reaching(PiecePosition::new(span.end), here)
            .map(|t| t.to_device(self.frozen_total).get())
    }

    /// Whether the transport queue holds nothing. A server that never binds a
    /// group never puts a bundle here, so this staying true is the observable
    /// form of "scheduling behaves exactly as it did before the transport".
    pub fn transport_queue_is_empty(&self) -> bool {
        self.sched_transport.is_empty()
    }

    /// Whether a scheduled bundle belongs to the transport queue.
    ///
    /// A bundle is atomic, so it goes whole to one queue: if **any** message
    /// targets a node at or under the governed group, the bundle is governed.
    /// A command with no node target (a bus write, a buffer, a def) carries no
    /// opinion and rides the bundle's verdict; a bundle of nothing but those
    /// goes to the device queue.
    ///
    /// RT-safe: `is_descendant_of` is a bounded walk up the `parent` links and
    /// allocates nothing, so this is a plain scan of the bundle.
    fn bundle_is_governed(&self, cmds: &[Cmd]) -> bool {
        let Some(group) = self.transport_group else {
            return false;
        };
        cmds.iter().any(|cmd| {
            cmd_target_nodes(cmd)
                .iter()
                .flatten()
                .any(|id| self.tree.is_descendant_of(*id, group))
        })
    }

    /// Wires live hardware input to this engine: `channels` interleaved
    /// input channels arrive through `rx`, filled every block into audio buses
    /// `channels..channels + input_channels` (scsynth's convention: outputs
    /// first, then inputs). Call once, before the engine starts processing. The
    /// producer end lives in the cpal input callback.
    pub fn attach_input(&mut self, channels: usize, rx: Consumer<f32>) {
        self.input_channels = channels;
        self.input_rx = Some(rx);
    }

    /// Creates the input ring, attaches its consumer to this engine, and hands
    /// back the producer to push interleaved frames — the test-side counterpart
    /// of the cpal input stream. `capacity` is in samples (channels × frames).
    pub fn input_ring(&mut self, channels: usize, capacity: usize) -> Producer<f32> {
        let (tx, rx) = RingBuffer::new(capacity.max(1));
        self.attach_input(channels, rx);
        tx
    }

    /// Drains one block's worth of interleaved input frames into the hardware
    /// input buses. An underrun (producer behind) reads as silence for the
    /// missing samples — never a stall. RT-safe: ring pops and bus writes only.
    fn fill_input_buses(&mut self) {
        let Some(rx) = &mut self.input_rx else { return };
        let ich = self.input_channels;
        if ich == 0 {
            return;
        }
        for f in 0..BLOCK_SIZE {
            for ch in 0..ich {
                let s = rx.pop().unwrap_or(0.0);
                // The output buses are `0..channels`; inputs follow them.
                // `audio_mut` is sound here: single-threaded at block start,
                // before the parallel stage scheduler runs.
                unsafe {
                    self.buses.audio_mut(self.channels + ch)[f] = s;
                }
            }
        }
    }

    /// Applies what has arrived **without advancing time**: the two steps
    /// [`Self::process_block`] begins with, and none of the rest.
    ///
    /// A pulled driver needs this because a command can only take effect
    /// through the FIFO — installing a buffer is `Cmd::SetBuffer`, so a
    /// `/buffer_alloc` that has completed on the NRT side is still not in the
    /// pool until the engine drains. In real time the next block does that a
    /// millisecond later and nobody notices; a driver whose clock only moves
    /// during an operation (`server::nrtsession`) would otherwise have to
    /// process a block it does not want in order to load a buffer, which is
    /// exactly the clock it is defined as not having.
    ///
    /// Same RT discipline as `process_block`: no allocation, no locking. It
    /// is safe to call from the audio thread, and nothing there needs to.
    pub fn drain(&mut self) {
        self.drain_commands();
        self.flush_pending_garbage();
    }

    /// Processes one block. `out` is interleaved and its length must be
    /// `BLOCK_SIZE * channels`. Runs on the audio thread: does not allocate.
    ///
    /// Immediate commands apply at the block start; scheduled bundles whose
    /// time falls inside this block execute at their exact sample, splitting
    /// the processing into slices around each event (late ones at offset 0).
    pub fn process_block(&mut self, out: &mut [f32]) {
        debug_assert_eq!(out.len(), BLOCK_SIZE * self.channels);
        // CPU meter start. `Instant::now` is RT-safe on the platforms we
        // target: `clock_gettime(CLOCK_MONOTONIC)` through the vDSO — no
        // allocation, no lock, no kernel trap. On wasm32 `Instant::now`
        // panics (no monotonic clock in the bare target), so the meter is
        // compiled out and `/server_status` CPU fields read 0 there.
        #[cfg(not(target_arch = "wasm32"))]
        let meter_start = std::time::Instant::now();
        self.drain_commands();
        self.flush_pending_garbage();

        self.buses.clear_audio();
        // Live input: fill the input buses after clearing, before any node
        // runs, so `In` reads this block's captured samples.
        self.fill_input_buses();
        let block_start = self.now;
        let block_end = block_start + BLOCK_SIZE as u64;
        // The block is cut by the union of both queues: a transport entry
        // is projected onto the device axis with the frozen total known at
        // this instant, and a stopped transport can never reach its own queue.
        let mut offset = 0usize;
        // Where inside this block the current frozen run began, if the
        // transport is stopped. Frozen time is credited **at the sample the
        // transport flips**, not a whole block at a time: a stop and a
        // resume both land mid-block, and crediting a flat `BLOCK_SIZE`
        // whenever the transport happened to be stopped at the boundary
        // loses (stop offset - resume offset) samples on every cycle, an
        // error that accumulates without bound. Crediting at the flip also
        // keeps `frozen_total` correct *during* the block, which is what
        // the transport-queue projection below reads.
        let mut frozen_from = if self.transport_rolling {
            None
        } else {
            Some(0usize)
        };
        loop {
            let device_due = self.sched.first().map(|b| b.time);
            // A transport entry's device time only exists while rolling: a
            // stopped transport can never reach it. Both this and
            // `frozen_total` are read afresh on every iteration, because a
            // bundle applied below may have carried a `TransportRun` — a
            // stop scheduled mid-block freezes the transport queue from
            // that sample on, which is the wanted behaviour.
            let transport_due = if self.transport_rolling {
                self.sched_transport
                    .first()
                    .map(|b| b.time.to_device(self.frozen_total).get())
            } else {
                None
            };
            let take_transport = match (device_due, transport_due) {
                (_, None) => false,
                (None, Some(_)) => true,
                // Ties go to the device queue: a fixed preference, because
                // cross-queue enqueue order is not recoverable at fire time
                // (a transport entry's device time is not fixed when it is
                // enqueued). Device-first is the right side, since it makes
                // an empty transport queue indistinguishable from a single
                // queue over the device axis.
                (Some(d), Some(t)) => t < d,
            };
            let queue_due = if take_transport {
                transport_due
            } else {
                device_due
            };
            let queue_due = queue_due.filter(|t| *t < block_end);
            // A loop's end is the third thing that cuts a block, and it is cut
            // for the same reason the other two are: the wrap lands on an
            // exact sample. Cutting there is also what keeps the position
            // *linear inside every slice*, so a reader following it ramps by
            // one per sample and never has to know a loop exists.
            //
            // `<= block_end`, where a bundle is `<`: a bundle at the boundary
            // belongs to the next block, but a wrap there belongs to *this*
            // one, because the position published at the end of a block is
            // what the next block's first sample plays -- and that sample is
            // the loop's start. Reading it a block late is a playhead that
            // overshoots the loop by a block, once per pass.
            let wrap_due = self.loop_wrap_due().filter(|w| *w <= block_end);
            // A wrap ties with a bundle by yielding to it: the queues keep the
            // device-first preference they already had among themselves, and a
            // wrap that stays due is taken on the next turn of the loop.
            let take_wrap = match (wrap_due, queue_due) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(w), Some(q)) => w < q,
            };
            let Some(due_time) = (if take_wrap { wrap_due } else { queue_due }) else {
                break;
            };
            let at = due_time.saturating_sub(block_start) as usize;
            if at > offset {
                self.process_slice(offset, at - offset);
                offset = at;
            }
            self.cursor = offset;
            if take_wrap {
                // Back to the loop's start, re-anchored here so the position
                // goes on advancing by one per sample from the seam. The span
                // is half-open, so the end sample is never played and the
                // first sample after the last one of the loop is its first.
                let start = self.transport_loop.as_ref().map_or(0, |span| span.start);
                self.position = self
                    .position
                    .wrapped_to(PiecePosition::new(start), self.transport_here());
                continue;
            }
            // Vec::remove on the pre-allocated queue: memmove, no (de)alloc.
            let mut cmds = if take_transport {
                self.sched_transport.remove(0).cmds
            } else {
                self.sched.remove(0).cmds
            };
            for cmd in cmds.drain(..) {
                self.apply(cmd);
            }
            self.push_garbage(Garbage::SpentBundle(cmds));
            // The bundle may have carried a `TransportRun`. Close or open
            // the frozen run at this exact sample. A bundle holding both a
            // stop and a resume nets to no frozen time, which is right:
            // they land on the same sample.
            match (frozen_from, self.transport_rolling) {
                (Some(from), true) => {
                    self.frozen_total += (offset - from) as u64;
                    frozen_from = None;
                }
                (None, false) => frozen_from = Some(offset),
                _ => {}
            }
        }
        self.cursor = 0;
        self.process_slice(offset, BLOCK_SIZE - offset);
        // The block ends with the transport still stopped: credit the tail.
        if let Some(from) = frozen_from {
            self.frozen_total += (BLOCK_SIZE - from) as u64;
        }

        // Buses 0..channels are the hardware outputs.
        for (f, frame) in out.chunks_exact_mut(self.channels).enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                *s = self.buses.audio(ch)[f];
            }
        }

        self.now = block_end;
        // `frozen_total` was already credited to the sample inside the
        // block-cut loop above; here the clock is only published.
        self.transport_clock
            .store(self.transport_now().get(), Ordering::Relaxed);
        self.frozen_clock
            .store(self.frozen_total, Ordering::Relaxed);
        // Published at the block's end like the clocks, and read there too:
        // the position at `block_end` is where the next block starts playing.
        self.position_clock
            .store(self.position_here().get(), Ordering::Relaxed);
        self.sample_clock.store(block_end, Ordering::Relaxed);
        if let Some(segment) = &self.ipc {
            // Audio taps first, then the clock: a reader that sees clock N
            // sees every tap sample of block N. One memcpy + one Release
            // store per active tap — no allocation, no lock (RT-safe).
            for (i, &bus) in self.tap_buses.iter().enumerate() {
                if bus >= 0 && (bus as usize) < self.buses.audio_count() {
                    segment.tap_write(i, self.buses.audio(bus as usize));
                }
            }
            // Then the per-bus level a meter reads: this block's peak, held
            // against the decaying previous one. The hold is what makes the
            // number correct for a reader running slower than the engine — a
            // display frame is a dozen blocks — and the decay (rather than a
            // max the reader clears) keeps it correct for several readers of
            // the same bus at once. One pass over the block per bus, one load
            // and one relaxed store: no allocation, no lock.
            for bus in 0..self.buses.audio_count().min(segment.audio_buses()) {
                let peak = self
                    .buses
                    .audio(bus)
                    .iter()
                    .fold(0.0f32, |acc, s| acc.max(s.abs()));
                let held = segment.level(bus) * self.level_release;
                segment.set_level(bus, peak.max(held));
            }
            // The transport clock goes out before the device clock, for the
            // same reason the taps do: a reader that sees device clock N has
            // seen everything block N published.
            segment
                .transport_clock()
                .store(self.transport_now().get(), Ordering::Relaxed);
            segment
                .transport_position()
                .store(self.position_here().get(), Ordering::Relaxed);
            segment.clock().store(block_end, Ordering::Release);
        }
        self.counters
            .synths
            .store(self.tree.synth_count() as u32, Ordering::Relaxed);
        self.counters
            .ugens
            .store(self.tree.ugen_count() as u32, Ordering::Relaxed);
        self.counters
            .groups
            .store(self.tree.group_count() as u32, Ordering::Relaxed);

        // Apply the freeing done actions collected during this block's walk
        // (`PauseSelf` was applied inline in the tree). Read id + action and act
        // one at a time so the tree is never borrowed twice at once; a `free` of
        // an already-gone id (a split block can queue one twice, or two synths
        // in a group both request `FreeGroup`) is a harmless no-op.
        let n_done = self.tree.take_done_count();
        for k in 0..n_done {
            let id = self.tree.done_node(k);
            let action = self.tree.done_action_at(k);
            let mut sink = GarbageSink {
                garbage_tx: &mut self.garbage_tx,
                pending_garbage: &mut self.pending_garbage,
                events_tx: &mut self.events_tx,
            };
            self.tree
                .apply_done_action(id, action, &mut |f| sink.consume(f));
        }

        // Drain the side-effect replies buffered this block (`SendReply`/
        // `SendTrig`/`Poll`) into the reply FIFO for the network thread to
        // turn into OSC. Disjoint field borrows: the tree walk reads the synths,
        // the producer takes the messages.
        let tree = &mut self.tree;
        let reply_tx = &mut self.reply_tx;
        tree.drain_replies(&mut |msg| {
            let _ = reply_tx.push(msg);
        });

        // CPU meter end: this block's wall time as a fraction of its real-time
        // budget (`BLOCK_SIZE / sample_rate`). Only meaningful when the caller
        // is paced by an audio device; NRT renders just measure render speed.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let budget = BLOCK_SIZE as f64 / self.sample_rate as f64;
            let busy = (meter_start.elapsed().as_secs_f64() / budget) as f32;
            // EMA with a ~1 s time constant: alpha = block duration / 1 s.
            self.avg_cpu += (busy - self.avg_cpu) * budget as f32;
            self.counters
                .avg_cpu
                .store(self.avg_cpu.to_bits(), Ordering::Relaxed);
            // Non-negative floats order like their bit patterns: a bitwise
            // `fetch_max` is a float max.
            self.counters
                .peak_cpu
                .fetch_max(busy.to_bits(), Ordering::Relaxed);
            if busy > 1.0 {
                self.counters.late_blocks.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Runs the node tree over `offset..offset+frames` of the current block.
    fn process_slice(&mut self, offset: usize, frames: usize) {
        if frames == 0 {
            return;
        }
        let ctx = ProcessCtx {
            // The two agree at the engine boundary: a node runs at the engine
            // rate, and the per-UGen rate is derived inside the synth.
            sample_rate: self.sample_rate,
            full_sample_rate: self.sample_rate,
            buses: &self.buses,
            buffers: &self.buffers,
            offset,
            frames,
            // Where the piece is at this slice's **first** frame. The block is
            // cut at every loop wrap, so the position advances by exactly one
            // per sample for the whole slice and a UGen reading it only has to
            // ramp — no wrap arithmetic, and nothing needs to know the loop
            // points but the engine.
            transport: TransportCtx {
                position: self
                    .position
                    .at(DeviceSample::new(self.now + offset as u64).to_transport(self.frozen_total))
                    .get(),
                rolling: self.transport_rolling,
            },
        };
        self.tree.process(&ctx, &self.pool);
    }

    fn push_garbage(&mut self, garbage: Garbage) {
        let mut sink = GarbageSink {
            garbage_tx: &mut self.garbage_tx,
            pending_garbage: &mut self.pending_garbage,
            events_tx: &mut self.events_tx,
        };
        sink.push(garbage);
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.pop() {
            self.apply(cmd);
        }
    }

    /// Applies one command. Called when draining the FIFO at block start and
    /// when a scheduled bundle fires mid-block.
    fn apply(&mut self, cmd: Cmd) {
        // Classified here rather than in the arm below because the garbage
        // sink borrows three of our fields for the whole `match`, and the
        // classification wants `&self`. A discriminant test per command.
        let governed = match &cmd {
            Cmd::Schedule { cmds, .. } => self.bundle_is_governed(cmds),
            _ => false,
        };
        {
            let mut sink = GarbageSink {
                garbage_tx: &mut self.garbage_tx,
                pending_garbage: &mut self.pending_garbage,
                events_tx: &mut self.events_tx,
            };
            let Some(cmd) = apply_to_tree(&mut self.tree, &mut sink, cmd) else {
                return;
            };
            match cmd {
                Cmd::TransportRun { rolling } => {
                    self.transport_rolling = rolling;
                    if let Some(group) = self.transport_group {
                        self.tree.set_paused(group, !rolling);
                    }
                }
                Cmd::TransportGroup { id } => {
                    // Thaw whatever we governed before letting it go, and
                    // freeze the new one if we are already stopped.
                    if let Some(previous) = self.transport_group.take() {
                        self.tree.set_paused(previous, false);
                    }
                    if id >= 0 {
                        self.transport_group = Some(id);
                        if !self.transport_rolling {
                            self.tree.set_paused(id, true);
                        }
                    }
                }
                Cmd::TransportLocate { position } => {
                    // One store, at the sample the locate lands on: the
                    // position is anchored rather than accumulated, so this
                    // is the whole of a seek on the audio thread.
                    self.position = PositionAnchor::located(
                        PiecePosition::new(position),
                        self.transport_here(),
                    );
                }
                Cmd::TransportLoop { span } => {
                    // Re-anchored at this sample, so turning a loop on does
                    // not move the piece: it keeps playing from where it is
                    // and wraps when it first reaches the end.
                    self.position = self.position.wrapped_to(
                        self.position.at(self.transport_here()),
                        self.transport_here(),
                    );
                    self.transport_loop = span.filter(|s| s.start < s.end);
                }
                Cmd::SetBuffer { index, buffer } => {
                    if let Some(slot) = self.buffers.get_mut(index) {
                        if let Some(old) = std::mem::replace(slot, buffer) {
                            sink.push(Garbage::FreedBuffer(old));
                        }
                    } else if let Some(buffer) = buffer {
                        // Out-of-range index (the network thread validates,
                        // so this is belt and braces): ship it back.
                        sink.push(Garbage::FreedBuffer(buffer));
                    }
                }
                Cmd::SetControlBus { index, value } => {
                    self.buses.control.set(index, value);
                }
                Cmd::SetTap { tap, bus } => {
                    // Out-of-range indices were rejected on the network side.
                    if let Some(slot) = self.tap_buses.get_mut(tap) {
                        let previous = *slot;
                        *slot = bus;
                        // Publish the inverse in the segment, so a reader looks
                        // the bus up instead of being told a ring index. One
                        // relaxed-path store each, no allocation (RT-safe).
                        if let Some(segment) = &self.ipc {
                            if previous >= 0 {
                                segment.set_tap_of_bus(previous as usize, None);
                            }
                            if bus >= 0 {
                                segment.set_tap_of_bus(bus as usize, Some(tap));
                            }
                        }
                    }
                }
                Cmd::Schedule { time, cmds } => {
                    if governed {
                        // The stamp arrives on the device axis (the network
                        // thread built it against the device clock); convert
                        // here, once, where the frozen total is known.
                        let at = DeviceSample::new(time).to_transport(self.frozen_total);
                        if self.sched_transport.len() == self.sched_transport.capacity() {
                            sink.push(Garbage::SpentBundle(cmds));
                        } else {
                            // Sorted insert, after equal times, exactly as the
                            // device queue does.
                            let pos = self.sched_transport.partition_point(|b| b.time <= at);
                            self.sched_transport
                                .insert(pos, ScheduledBundleT { time: at, cmds });
                        }
                    } else if self.sched.len() == self.sched.capacity() {
                        sink.push(Garbage::SpentBundle(cmds));
                    } else {
                        let pos = self.sched.partition_point(|b| b.time <= time);
                        self.sched.insert(pos, ScheduledBundle { time, cmds });
                    }
                }
                Cmd::ClearSched => {
                    // `drain` keeps the queue's capacity (no dealloc here); each
                    // bundle's heap is freed on the network side.
                    for bundle in self.sched.drain(..) {
                        sink.push(Garbage::SpentBundle(bundle.cmds));
                    }
                    // Both queues: `/sched_clear` drops *every* pending bundle,
                    // and a governed one left behind would fire on the next
                    // resume with nothing left to explain it.
                    for bundle in self.sched_transport.drain(..) {
                        sink.push(Garbage::SpentBundle(bundle.cmds));
                    }
                }
                // Named rather than left to a `_`, so the compiler still
                // refuses a `Cmd` variant that neither this match nor
                // `apply_to_tree` handles — a wildcard here would turn that
                // omission into a panic on the audio thread.
                Cmd::AddSynth { .. }
                | Cmd::AddGroup { .. }
                | Cmd::FreeNode { .. }
                | Cmd::FreeAllInGroup { .. }
                | Cmd::DeepFreeGroup { .. }
                | Cmd::RunNode { .. }
                | Cmd::MoveNode { .. }
                | Cmd::SetControl { .. }
                | Cmd::MapControl { .. }
                | Cmd::SetUsage { .. }
                | Cmd::SetGroupParallel { .. }
                | Cmd::UGenCommand { .. } => {
                    debug_assert!(false, "apply_to_tree returned a node command");
                }
            }
        }
    }

    fn flush_pending_garbage(&mut self) {
        while let Some(g) = self.pending_garbage.pop() {
            if let Err(PushError::Full(g)) = self.garbage_tx.push(g) {
                self.pending_garbage.push(g);
                break;
            }
        }
    }
}

/// The node-tree half of [`Engine::apply`] — every command whose only engine
/// state is the tree itself, which is twelve of the nineteen.
///
/// A free function taking the two things it touches, rather than a method:
/// `sink` borrows three of the engine's fields for the whole match, so a
/// `&mut self` helper could not coexist with it. Returns the command back when
/// it is not one of these, so the caller matches the remaining seven and no
/// command is classified twice.
#[inline]
fn apply_to_tree(tree: &mut NodeTree, sink: &mut GarbageSink, cmd: Cmd) -> Option<Cmd> {
    match cmd {
        Cmd::AddSynth {
            id,
            target,
            action,
            mut synth,
            usage,
        } => {
            // Every add path funnels here, so this is the one place a
            // synth learns its id (arithmetic only — RT-safe). See
            // `SynthNode::set_node_id`.
            synth.set_node_id(id);
            match tree.insert(
                id,
                NodeKind::Synth { node: synth, usage },
                target,
                action,
                &mut |f| sink.consume(f),
            ) {
                Ok(parent_id) => {
                    let _ = sink.events_tx.push(NodeEvent {
                        kind: NodeEventKind::Go,
                        id,
                        parent_id,
                        is_group: false,
                    });
                }
                Err(NodeKind::Synth { node: synth, .. }) => {
                    sink.push(Garbage::RejectedSynth { id, synth });
                }
                Err(NodeKind::Group(group)) => {
                    sink.push(Garbage::RejectedGroup { id, group });
                }
            }
        }
        Cmd::SetUsage { id, usage } => tree.set_usage(id, usage),
        Cmd::SetGroupParallel { id, parallel } => {
            // Unknown or non-group IDs are ignored, like /node_set.
            let _ = tree.set_parallel(id, parallel);
        }
        Cmd::AddGroup {
            id,
            target,
            action,
            group,
        } => {
            match tree.insert(id, NodeKind::Group(group), target, action, &mut |f| {
                sink.consume(f)
            }) {
                Ok(parent_id) => {
                    let _ = sink.events_tx.push(NodeEvent {
                        kind: NodeEventKind::Go,
                        id,
                        parent_id,
                        is_group: true,
                    });
                }
                Err(NodeKind::Synth { node: synth, .. }) => {
                    sink.push(Garbage::RejectedSynth { id, synth });
                }
                Err(NodeKind::Group(group)) => {
                    sink.push(Garbage::RejectedGroup { id, group });
                }
            }
        }
        Cmd::FreeNode { id } => {
            // Unknown IDs are silently ignored here; the network
            // thread already replied /fail where it could tell.
            tree.free(id, &mut |f| sink.consume(f));
        }
        Cmd::FreeAllInGroup { id } => {
            tree.free_all(id, &mut |f| sink.consume(f));
        }
        Cmd::DeepFreeGroup { id } => {
            tree.deep_free(id, &mut |f| sink.consume(f));
        }
        Cmd::RunNode { id, run } => {
            tree.set_paused(id, !run);
        }
        Cmd::MoveNode { id, target, place } => {
            tree.move_node(id, target, place);
        }
        Cmd::SetControl { id, index, value } => {
            if let Some(synth) = tree.synth_mut(id) {
                synth.set_control(index, value);
            }
        }
        Cmd::MapControl {
            id,
            index,
            bus,
            audio,
        } => {
            if let Some(synth) = tree.synth_mut(id) {
                synth.map_control(index, bus, audio);
            }
        }
        Cmd::UGenCommand {
            id,
            ugen_index,
            command,
        } => {
            if let Some(synth) = tree.synth_mut(id) {
                synth.ugen_command(ugen_index, &command);
            }
        }
        other => return Some(other),
    }
    None
}

impl EngineHandle {
    /// Enqueues a command. Returns it back if the FIFO is full so the caller
    /// can retry or report failure.
    pub fn send(&mut self, cmd: Cmd) -> Result<(), Cmd> {
        self.cmd_tx.push(cmd).map_err(|PushError::Full(c)| c)
    }

    /// Pops one item of garbage, if any. The caller drops it (we are on the
    /// network thread) and may use the ID for bookkeeping.
    pub fn pop_garbage(&mut self) -> Option<Garbage> {
        self.garbage_rx.pop().ok()
    }

    /// Pops one node lifecycle event, if any.
    pub fn pop_event(&mut self) -> Option<NodeEvent> {
        self.events_rx.pop().ok()
    }

    /// Pops one side-effect reply message (`SendReply`/`SendTrig`/`Poll`),
    /// if any. The network thread turns each into an OSC reply / console line.
    pub fn pop_reply(&mut self) -> Option<ReplyMsg> {
        self.reply_rx.pop().ok()
    }

    /// Drops everything the audio thread discarded. Returns how many items
    /// were collected.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while let Some(g) = self.pop_garbage() {
            if let Garbage::RejectedSynth { id, .. } | Garbage::RejectedGroup { id, .. } = &g {
                tracing::warn!(
                    "engine rejected node {id} (duplicate ID, bad target or full table)"
                );
            }
            drop(g);
            n += 1;
        }
        n
    }

    /// Control buses are shared atomics: `/bus_set`/`/bus_get` are served right
    /// here on the network thread, no command round-trip.
    pub fn control_buses(&self) -> &ControlBuses {
        &self.control_buses
    }

    /// The IPC segment when one exists. The audio taps are read from here
    /// (`/bus_tapStream` snapshots), like the control buses: shared memory, no
    /// engine round-trip.
    pub fn segment(&self) -> Option<&Arc<Segment>> {
        self.segment.as_ref()
    }

    /// The engine's stream clock: samples processed so far, published once
    /// per block. Timetag→sample conversion anchors on this.
    pub fn current_samples(&self) -> u64 {
        self.sample_clock.load(Ordering::Relaxed)
    }

    /// The transport clock as of the last completed block.
    pub fn current_transport_samples(&self) -> u64 {
        self.transport_clock.load(Ordering::Relaxed)
    }

    /// Where the piece is, as of the last completed block. Unlike the two
    /// clocks this one jumps: a locate moves it and a loop wraps it.
    pub fn current_transport_position(&self) -> u64 {
        self.position_clock.load(Ordering::Relaxed)
    }

    /// Total samples the transport has spent stopped, as of the last completed
    /// block -- the whole of the device <-> transport axis conversion.
    pub fn current_frozen_total(&self) -> u64 {
        self.frozen_clock.load(Ordering::Relaxed)
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}
