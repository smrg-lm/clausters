//! Processing engine, independent of the audio backend.
//!
//! [`engine_pair`] returns the two halves of the server: the [`Engine`] lives
//! on the audio thread, the [`EngineHandle`] on the network thread. They talk
//! exclusively through lock-free SPSC ring buffers: commands flow in fully
//! pre-built (the audio thread only plugs them in), freed memory flows back
//! out as [`Garbage`] to be dropped on the network side.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::node::{AddAction, NodeTree, default_synth::DefaultSynth};

/// Frames per processing block, like scsynth.
pub const BLOCK_SIZE: usize = 64;

const CMD_FIFO_CAPACITY: usize = 1024;
const GARBAGE_FIFO_CAPACITY: usize = 1024;
/// Local holding list for when the garbage FIFO is full.
const PENDING_GARBAGE_CAPACITY: usize = 64;

/// Commands are built **completely** on the network thread (including the
/// boxed synth); applying them on the audio thread never allocates.
pub enum Cmd {
    AddSynth {
        id: i32,
        synth: Box<DefaultSynth>,
        action: AddAction,
    },
    FreeNode {
        id: i32,
    },
    SetControl {
        id: i32,
        index: u32,
        value: f32,
    },
}

/// Heap memory leaving the audio thread to be dropped on the network side.
pub enum Garbage {
    Freed { id: i32, synth: Box<DefaultSynth> },
    /// Command the engine could not apply (duplicate ID or full node table).
    Rejected { synth: Box<DefaultSynth> },
}

/// Counts published by the audio thread (relaxed stores) and read by the
/// network thread for `/status.reply`.
#[derive(Default)]
pub struct Counters {
    pub synths: AtomicU32,
    pub ugens: AtomicU32,
}

/// Audio-thread half. `process_block` does not allocate, lock or do I/O.
pub struct Engine {
    sample_rate: f32,
    channels: usize,
    tree: NodeTree,
    cmd_rx: Consumer<Cmd>,
    garbage_tx: Producer<Garbage>,
    pending_garbage: Vec<Garbage>,
    counters: Arc<Counters>,
    mix: [f32; BLOCK_SIZE],
    scratch: [f32; BLOCK_SIZE],
}

/// Network-thread half: sends commands, collects garbage, reads counters.
pub struct EngineHandle {
    pub sample_rate: f32,
    pub channels: usize,
    cmd_tx: Producer<Cmd>,
    garbage_rx: Consumer<Garbage>,
    counters: Arc<Counters>,
}

pub fn engine_pair(sample_rate: f32, channels: usize) -> (Engine, EngineHandle) {
    assert!(channels > 0);
    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_FIFO_CAPACITY);
    let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_FIFO_CAPACITY);
    let counters = Arc::new(Counters::default());
    let engine = Engine {
        sample_rate,
        channels,
        tree: NodeTree::new(),
        cmd_rx,
        garbage_tx,
        pending_garbage: Vec::with_capacity(PENDING_GARBAGE_CAPACITY),
        counters: Arc::clone(&counters),
        mix: [0.0; BLOCK_SIZE],
        scratch: [0.0; BLOCK_SIZE],
    };
    let handle = EngineHandle {
        sample_rate,
        channels,
        cmd_tx,
        garbage_rx,
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

        self.mix.fill(0.0);
        self.tree
            .process(self.sample_rate, &mut self.mix, &mut self.scratch);
        for (frame, &s) in out.chunks_exact_mut(self.channels).zip(self.mix.iter()) {
            frame.fill(s);
        }

        let n = self.tree.synth_count() as u32;
        self.counters.synths.store(n, Ordering::Relaxed);
        // one SinOsc per DefaultSynth; real UGen counts arrive with M3
        self.counters.ugens.store(n, Ordering::Relaxed);
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.pop() {
            match cmd {
                Cmd::AddSynth { id, synth, action } => {
                    if let Err(synth) = self.tree.insert_synth(id, synth, action) {
                        self.push_garbage(Garbage::Rejected { synth });
                    }
                }
                Cmd::FreeNode { id } => {
                    // Unknown IDs are silently ignored; the async /fail reply
                    // path arrives with the reply FIFO in M4.
                    if let Some(synth) = self.tree.free_synth(id) {
                        self.push_garbage(Garbage::Freed { id, synth });
                    }
                }
                Cmd::SetControl { id, index, value } => {
                    if let Some(synth) = self.tree.synth_mut(id) {
                        synth.set_control(index, value);
                    }
                }
            }
        }
    }

    fn push_garbage(&mut self, garbage: Garbage) {
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

    /// Drops everything the audio thread discarded. Call periodically from
    /// the network thread. Returns how many items were collected.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while let Ok(g) = self.garbage_rx.pop() {
            if matches!(g, Garbage::Rejected { .. }) {
                eprintln!("engine rejected a command (duplicate node ID or full node table)");
            }
            drop(g);
            n += 1;
        }
        n
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}
