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
use crate::dsp::buffer::{Buffer, BufferPool, empty_pool};
use crate::dsp::{Buses, ControlBuses, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES, ProcessCtx};
use crate::node::{AddAction, FreedNode, Group, NodeKind, NodeTree, Place, SynthNode};
use crate::server::ipc::Segment;
use crate::server::workers::WorkerPool;

const CMD_FIFO_CAPACITY: usize = 1024;
const GARBAGE_FIFO_CAPACITY: usize = 1024;
/// Local holding list for when the garbage FIFO is full.
const PENDING_GARBAGE_CAPACITY: usize = 64;
const EVENT_FIFO_CAPACITY: usize = 2048;
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
    counters: Arc<Counters>,
}

/// Network-thread half: sends commands, collects garbage and events, reads
/// counters, serves the control buses directly.
pub struct EngineHandle {
    pub sample_rate: f32,
    pub channels: usize,
    /// Configured audio bus count (after clamping to the 128 ceiling).
    pub audio_buses: usize,
    cmd_tx: Producer<Cmd>,
    garbage_rx: Consumer<Garbage>,
    events_rx: Consumer<NodeEvent>,
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
) -> (Engine, EngineHandle) {
    // The mask is a `u128`, so 128 is the hard ceiling for audio buses.
    let audio_buses = audio_buses.clamp(channels.max(1), NUM_AUDIO_BUSES);
    assert!(channels > 0 && channels <= audio_buses);
    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_FIFO_CAPACITY);
    let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_FIFO_CAPACITY);
    let (events_tx, events_rx) = RingBuffer::new(EVENT_FIFO_CAPACITY);
    let counters = Arc::new(Counters {
        synths: AtomicU32::new(0),
        ugens: AtomicU32::new(0),
        // The root group exists before the first tick publishes counts.
        groups: AtomicU32::new(1),
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
        tree: NodeTree::new(),
        pool: WorkerPool::new(workers),
        buses: Buses::new(control_buses.clone(), audio_buses),
        buffers: empty_pool(),
        now: 0,
        sched: Vec::with_capacity(SCHED_CAPACITY),
        sample_clock: Arc::clone(&sample_clock),
        ipc,
        cmd_rx,
        garbage_tx,
        pending_garbage: Vec::with_capacity(PENDING_GARBAGE_CAPACITY),
        events_tx,
        counters: Arc::clone(&counters),
    };
    let handle = EngineHandle {
        sample_rate,
        channels,
        audio_buses,
        cmd_tx,
        garbage_rx,
        events_rx,
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

    /// Processes one block. `out` is interleaved and its length must be
    /// `BLOCK_SIZE * channels`. Runs on the audio thread: does not allocate.
    ///
    /// Immediate commands apply at the block start; scheduled bundles whose
    /// time falls inside this block execute at their exact sample, splitting
    /// the processing into slices around each event (late ones at offset 0).
    pub fn process_block(&mut self, out: &mut [f32]) {
        debug_assert_eq!(out.len(), BLOCK_SIZE * self.channels);
        self.drain_commands();
        self.flush_pending_garbage();

        self.buses.clear_audio();
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
