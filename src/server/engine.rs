//! Processing engine, independent of the audio backend.
//!
//! [`engine_pair`] returns the two halves of the server: the [`Engine`] lives
//! on the audio thread, the [`EngineHandle`] on the network thread. They talk
//! exclusively through lock-free SPSC ring buffers: commands flow in fully
//! pre-built (the audio thread only plugs them in), freed memory flows back
//! out as [`Garbage`] to be dropped on the network side, and node lifecycle
//! events flow out as [`NodeEvent`]s for `/n_go`/`/n_end` notifications.
//!
//! Timed bundles (M6) arrive as [`Cmd::Schedule`] carrying an absolute
//! target in samples; the engine keeps them in a pre-allocated queue sorted
//! by time (FIFO for equal times) and executes them **sample-accurately**,
//! splitting the block at each event's offset. The engine publishes its
//! sample counter so the network thread can convert NTP timetags.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

pub use crate::dsp::BLOCK_SIZE;
use crate::dsp::BusUsage;
use crate::dsp::buffer::{Buffer, BufferPool, empty_pool_with};
use crate::dsp::{
    Buses, ControlBuses, Limits, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES, ProcessCtx, ReplyMsg,
};
use crate::node::{AddAction, FreedNode, Group, NodeKind, NodeTree, Place, SynthNode};
use crate::server::ipc::Segment;
use crate::server::workers::WorkerPool;

const CMD_FIFO_CAPACITY: usize = 1024;
const GARBAGE_FIFO_CAPACITY: usize = 1024;
/// Local holding list for when the garbage FIFO is full.
const PENDING_GARBAGE_CAPACITY: usize = 64;
const EVENT_FIFO_CAPACITY: usize = 2048;
/// Side-effect reply messages (`SendReply`/`SendTrig`/`Poll`, S9) buffered from
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
        /// Bus masks analyzed at build time; the M13 parallel scheduler
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
    /// `/g_freeAll`: free all children of a group; the group stays.
    FreeAllInGroup {
        id: i32,
    },
    /// `/g_deepFree`: free all synths in a group and its subgroups.
    DeepFreeGroup {
        id: i32,
    },
    /// `/n_run`: pause (`run = false`) or resume (`true`) a node — a synth or a
    /// whole group. Makes `DoneAction::PauseSelf` non-terminal.
    RunNode {
        id: i32,
        run: bool,
    },
    /// `/n_before` / `/n_after`.
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
    /// `/n_map` (`audio = false`) / `/n_mapa` (`audio = true`): binds a
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
    /// `/c_set` inside a timed bundle: the immediate form writes the shared
    /// atomics from the network thread, but a scheduled write must land at
    /// its exact sample, so it travels to the audio thread like any command.
    SetControlBus {
        index: usize,
        value: f32,
    },
    /// `/n_set` on a control used as a bus index: ships the re-analyzed
    /// masks so the parallel scheduler stays in sync (M13).
    SetUsage {
        id: i32,
        usage: BusUsage,
    },
    /// `/g_parallel`: children of this group run in dependency stages on
    /// the worker pool (M13).
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
    /// `/clearSched`: drop every pending timed bundle. Each drained bundle's
    /// `Vec<Cmd>` (with its boxed synths) leaves through the garbage FIFO as
    /// [`Garbage::SpentBundle`], so nothing is freed on the audio thread.
    ClearSched,
    /// `/u_cmd`: a typed command addressed to one UGen instance inside a synth.
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

/// Node lifecycle event for `/n_go`/`/n_end` notifications. POD; delivery is
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
/// network thread for `/status.reply`.
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
    /// so every `/status` poll reports the peak of its own window.
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

/// Audio-thread half. `process_block` does not allocate, lock or do I/O.
pub struct Engine {
    sample_rate: f32,
    channels: usize,
    tree: NodeTree,
    /// M13 DSP workers for parallel groups; empty pool = sequential.
    pool: WorkerPool,
    buses: Buses,
    buffers: BufferPool,
    /// Live hardware input (S7): decoded interleaved frames arriving from the
    /// cpal input stream through a lock-free ring. `0` channels / `None`
    /// consumer means no input stream is open. Read at each block start into
    /// audio buses `channels..channels + input_channels`, which `In`/`In.ar`
    /// then read like any bus.
    input_channels: usize,
    input_rx: Option<Consumer<f32>>,
    /// Samples processed since start; the stream clock scheduled bundles
    /// are measured against.
    now: u64,
    /// Pending timed bundles, sorted by time (stable for equal times).
    /// Pre-allocated: insertion and removal never allocate.
    sched: Vec<ScheduledBundle>,
    sample_clock: Arc<AtomicU64>,
    /// M14: block-accurate mirror of the sample clock into the IPC segment
    /// (one extra Release store per block); the Arc pins the mapping.
    ipc: Option<Arc<Segment>>,
    cmd_rx: Consumer<Cmd>,
    garbage_tx: Producer<Garbage>,
    pending_garbage: Vec<Garbage>,
    events_tx: Producer<NodeEvent>,
    reply_tx: Producer<ReplyMsg>,
    counters: Arc<Counters>,
    /// EMA state of the CPU meter (fraction of the block budget, ~1 s time
    /// constant); published to `counters.avg_cpu` every block.
    avg_cpu: f32,
}

/// Network-thread half: sends commands, collects garbage and events, reads
/// counters, serves the control buses directly.
pub struct EngineHandle {
    pub sample_rate: f32,
    pub channels: usize,
    /// Configured audio bus count (after clamping to the 128 ceiling).
    pub audio_buses: usize,
    /// Live hardware input channels (S7); `0` when no input stream is open.
    /// Set by the backend once it has negotiated the input device.
    pub input_channels: usize,
    /// Boot-time pool capacities, surfaced in `/server_info.reply` so a client
    /// can discover the server's limits instead of hardcoding them.
    pub limits: Limits,
    cmd_tx: Producer<Cmd>,
    garbage_rx: Consumer<Garbage>,
    events_rx: Consumer<NodeEvent>,
    reply_rx: Consumer<ReplyMsg>,
    control_buses: ControlBuses,
    sample_clock: Arc<AtomicU64>,
    counters: Arc<Counters>,
}

pub fn engine_pair(sample_rate: f32, channels: usize) -> (Engine, EngineHandle) {
    engine_pair_with_workers(sample_rate, channels, 0)
}

/// Default bus counts (scsynth `-a`/`-c`), used by the simple constructors and
/// the NRT renderer. The live server can override them with `--audio-buses`/
/// `--control-buses`; audio is capped at 128 (the `BusUsage` mask is a `u128`).
pub const DEFAULT_AUDIO_BUSES: usize = NUM_AUDIO_BUSES;
pub const DEFAULT_CONTROL_BUSES: usize = NUM_CONTROL_BUSES;

/// Like [`engine_pair`], plus an M13 worker pool of `workers` DSP threads
/// for parallel groups (`/g_parallel`). `workers == 0` is fully sequential
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

/// Full form (M14): with an IPC segment, the control buses live *inside the
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
    let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_FIFO_CAPACITY);
    let (events_tx, events_rx) = RingBuffer::new(EVENT_FIFO_CAPACITY);
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
    let engine = Engine {
        sample_rate,
        channels,
        tree: NodeTree::with_capacity(limits.max_nodes),
        pool: WorkerPool::new(workers),
        buses: Buses::new(control_buses.clone(), audio_buses),
        buffers: empty_pool_with(limits.max_buffers),
        input_channels: 0,
        input_rx: None,
        now: 0,
        sched: Vec::with_capacity(SCHED_CAPACITY),
        sample_clock: Arc::clone(&sample_clock),
        ipc,
        cmd_rx,
        garbage_tx,
        pending_garbage: Vec::with_capacity(PENDING_GARBAGE_CAPACITY),
        events_tx,
        reply_tx,
        counters: Arc::clone(&counters),
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
        counters,
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

    /// Wires live hardware input (S7) to this engine: `channels` interleaved
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
        // allocation, no lock, no kernel trap.
        let meter_start = std::time::Instant::now();
        self.drain_commands();
        self.flush_pending_garbage();

        self.buses.clear_audio();
        // Live input (S7): fill the input buses after clearing, before any node
        // runs, so `In` reads this block's captured samples.
        self.fill_input_buses();
        let block_start = self.now;
        let block_end = block_start + BLOCK_SIZE as u64;
        let mut offset = 0usize;
        while self.sched.first().is_some_and(|b| b.time < block_end) {
            // Vec::remove on the pre-allocated queue: memmove, no (de)alloc.
            let due = self.sched.remove(0);
            let at = due.time.saturating_sub(block_start) as usize;
            if at > offset {
                self.process_slice(offset, at - offset);
                offset = at;
            }
            let mut cmds = due.cmds;
            for cmd in cmds.drain(..) {
                self.apply(cmd);
            }
            self.push_garbage(Garbage::SpentBundle(cmds));
        }
        self.process_slice(offset, BLOCK_SIZE - offset);

        // Buses 0..channels are the hardware outputs.
        for (f, frame) in out.chunks_exact_mut(self.channels).enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                *s = self.buses.audio(ch)[f];
            }
        }

        self.now = block_end;
        self.sample_clock.store(block_end, Ordering::Relaxed);
        if let Some(segment) = &self.ipc {
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
        // `SendTrig`/`Poll`, S9) into the reply FIFO for the network thread to
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

    /// Runs the node tree over `offset..offset+frames` of the current block.
    fn process_slice(&mut self, offset: usize, frames: usize) {
        if frames == 0 {
            return;
        }
        let ctx = ProcessCtx {
            sample_rate: self.sample_rate,
            buses: &self.buses,
            buffers: &self.buffers,
            offset,
            frames,
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
        {
            let mut sink = GarbageSink {
                garbage_tx: &mut self.garbage_tx,
                pending_garbage: &mut self.pending_garbage,
                events_tx: &mut self.events_tx,
            };
            match cmd {
                Cmd::AddSynth {
                    id,
                    target,
                    action,
                    synth,
                    usage,
                } => {
                    match self.tree.insert(
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
                Cmd::SetUsage { id, usage } => self.tree.set_usage(id, usage),
                Cmd::SetGroupParallel { id, parallel } => {
                    // Unknown or non-group IDs are ignored, like /n_set.
                    let _ = self.tree.set_parallel(id, parallel);
                }
                Cmd::AddGroup {
                    id,
                    target,
                    action,
                    group,
                } => {
                    match self
                        .tree
                        .insert(id, NodeKind::Group(group), target, action, &mut |f| {
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
                    self.tree.free(id, &mut |f| sink.consume(f));
                }
                Cmd::FreeAllInGroup { id } => {
                    self.tree.free_all(id, &mut |f| sink.consume(f));
                }
                Cmd::DeepFreeGroup { id } => {
                    self.tree.deep_free(id, &mut |f| sink.consume(f));
                }
                Cmd::RunNode { id, run } => {
                    self.tree.set_paused(id, !run);
                }
                Cmd::MoveNode { id, target, place } => {
                    self.tree.move_node(id, target, place);
                }
                Cmd::SetControl { id, index, value } => {
                    if let Some(synth) = self.tree.synth_mut(id) {
                        synth.set_control(index, value);
                    }
                }
                Cmd::MapControl {
                    id,
                    index,
                    bus,
                    audio,
                } => {
                    if let Some(synth) = self.tree.synth_mut(id) {
                        synth.map_control(index, bus, audio);
                    }
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
                Cmd::Schedule { time, cmds } => {
                    if self.sched.len() == self.sched.capacity() {
                        // Queue full: reject the whole bundle; the network
                        // side logs it when it collects the garbage.
                        sink.push(Garbage::SpentBundle(cmds));
                    } else {
                        // Sorted insert, after equal times (FIFO ties).
                        // Vec::insert below capacity does not allocate.
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
                }
                Cmd::UGenCommand {
                    id,
                    ugen_index,
                    command,
                } => {
                    if let Some(synth) = self.tree.synth_mut(id) {
                        synth.ugen_command(ugen_index, &command);
                    }
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

    /// Pops one side-effect reply message (`SendReply`/`SendTrig`/`Poll`, S9),
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

    /// Control buses are shared atomics: `/c_set`/`/c_get` are served right
    /// here on the network thread, no command round-trip.
    pub fn control_buses(&self) -> &ControlBuses {
        &self.control_buses
    }

    /// The engine's stream clock: samples processed so far, published once
    /// per block. Timetag→sample conversion anchors on this.
    pub fn current_samples(&self) -> u64 {
        self.sample_clock.load(Ordering::Relaxed)
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}
