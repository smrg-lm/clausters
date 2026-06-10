//! Processing engine, independent of the audio backend.
//!
//! [`engine_pair`] returns the two halves of the server: the [`Engine`] lives
//! on the audio thread, the [`EngineHandle`] on the network thread. They talk
//! exclusively through lock-free SPSC ring buffers: commands flow in fully
//! pre-built (the audio thread only plugs them in), freed memory flows back
//! out as [`Garbage`] to be dropped on the network side, and node lifecycle
//! events flow out as [`NodeEvent`]s for `/n_go`/`/n_end` notifications.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

pub use crate::dsp::BLOCK_SIZE;
use crate::dsp::{Buses, ControlBuses, NUM_AUDIO_BUSES, ProcessCtx};
use crate::node::{AddAction, FreedNode, Group, NodeKind, NodeTree, Place, SynthNode};

const CMD_FIFO_CAPACITY: usize = 1024;
const GARBAGE_FIFO_CAPACITY: usize = 1024;
/// Local holding list for when the garbage FIFO is full.
const PENDING_GARBAGE_CAPACITY: usize = 64;
const EVENT_FIFO_CAPACITY: usize = 2048;

/// Commands are built **completely** on the network thread (including boxed
/// synths and pre-reserved group child lists); applying them on the audio
/// thread never allocates.
pub enum Cmd {
    AddSynth {
        id: i32,
        target: i32,
        action: AddAction,
        synth: Box<dyn SynthNode>,
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
    buses: Buses,
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
    cmd_tx: Producer<Cmd>,
    garbage_rx: Consumer<Garbage>,
    events_rx: Consumer<NodeEvent>,
    control_buses: ControlBuses,
    counters: Arc<Counters>,
}

pub fn engine_pair(sample_rate: f32, channels: usize) -> (Engine, EngineHandle) {
    assert!(channels > 0 && channels <= NUM_AUDIO_BUSES);
    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_FIFO_CAPACITY);
    let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_FIFO_CAPACITY);
    let (events_tx, events_rx) = RingBuffer::new(EVENT_FIFO_CAPACITY);
    let counters = Arc::new(Counters {
        synths: AtomicU32::new(0),
        ugens: AtomicU32::new(0),
        // The root group exists before the first tick publishes counts.
        groups: AtomicU32::new(1),
    });
    let control_buses = ControlBuses::new();
    let engine = Engine {
        sample_rate,
        channels,
        tree: NodeTree::new(),
        buses: Buses::new(control_buses.clone()),
        cmd_rx,
        garbage_tx,
        pending_garbage: Vec::with_capacity(PENDING_GARBAGE_CAPACITY),
        events_tx,
        counters: Arc::clone(&counters),
    };
    let handle = EngineHandle {
        sample_rate,
        channels,
        cmd_tx,
        garbage_rx,
        events_rx,
        control_buses,
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
    pub fn process_block(&mut self, out: &mut [f32]) {
        debug_assert_eq!(out.len(), BLOCK_SIZE * self.channels);
        self.drain_commands();
        self.flush_pending_garbage();

        self.buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: self.sample_rate,
            buses: &mut self.buses,
        };
        self.tree.process(&mut ctx);
        // Buses 0..channels are the hardware outputs.
        for (f, frame) in out.chunks_exact_mut(self.channels).enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                *s = self.buses.audio[ch][f];
            }
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

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.pop() {
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
                } => {
                    match self.tree.insert(
                        id,
                        NodeKind::Synth(synth),
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
                        Err(NodeKind::Synth(synth)) => {
                            sink.push(Garbage::RejectedSynth { id, synth });
                        }
                        Err(NodeKind::Group(group)) => {
                            sink.push(Garbage::RejectedGroup { id, group });
                        }
                    }
                }
                Cmd::AddGroup {
                    id,
                    target,
                    action,
                    group,
                } => {
                    match self.tree.insert(
                        id,
                        NodeKind::Group(group),
                        target,
                        action,
                        &mut |f| sink.consume(f),
                    ) {
                        Ok(parent_id) => {
                            let _ = sink.events_tx.push(NodeEvent {
                                kind: NodeEventKind::Go,
                                id,
                                parent_id,
                                is_group: true,
                            });
                        }
                        Err(NodeKind::Synth(synth)) => {
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
                eprintln!("engine rejected node {id} (duplicate ID, bad target or full table)");
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

    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}
