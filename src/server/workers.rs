//! Worker pool: fork-join execution of parallel-group stages.
//!
//! The conductor (the audio thread, inside `Engine::process_block`)
//! publishes one stage at a time; the workers and the conductor race
//! through it with an atomic cursor (work stealing), then the conductor
//! waits for completion and moves on. Stages never overlap, so a stage's
//! raw pointers are only dereferenced while the conductor's stack frame —
//! the owner of tree, buses and buffers — is pinned in `run_stage`.
//! Publishing is seqlock-style (odd epoch = job being rewritten) and a
//! worker validates the epoch *after* registering in `active`, so a
//! late-waking worker can never read a job mid-rewrite.
//!
//! The hot path is syscall- and allocation-free: publishing is a handful
//! of atomic stores, waiting is bounded spinning. Workers that find no
//! work for a while park themselves (their `unpark` is the only syscall,
//! paid when transitioning from idle to busy, never per stage while hot).
//!
//! Correctness of concurrent access relies on the stage scheduler
//! (`node::NodeTree::process_parallel`): stage members are disjoint
//! subtrees touching pairwise disjoint buses, so any interleaving produces
//! the same samples as sequential execution — parallel rendering is
//! **bit-identical** to single-threaded rendering.

use std::cell::{Cell, UnsafeCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

use crate::dsp::buffer::Buffer;
use crate::dsp::{Buses, ProcessCtx};
use crate::node::NodeTree;

const STATE_HOT: u8 = 0;
const STATE_PARKED: u8 = 1;

/// Spins before yielding, yields before parking.
const SPINS_BEFORE_YIELD: u32 = 4_000;
const YIELDS_BEFORE_PARK: u32 = 64;

thread_local! {
    /// Marks pool worker threads (diagnostics; workers run the sequential
    /// traversal directly, so they never re-enter the pool).
    static IS_WORKER: Cell<bool> = const { Cell::new(false) };
}

/// One published stage, type-erased to raw pointers. Only dereferenced
/// between the epoch publish and stage completion, both inside the
/// conductor's `run_stage` frame, which keeps every pointee alive.
#[derive(Clone, Copy)]
struct Job {
    tree: *const NodeTree,
    stage: *const usize,
    stage_len: usize,
    buses: *const Buses,
    buffers: *const Option<Arc<Buffer>>,
    buffers_len: usize,
    sample_rate: f32,
    offset: usize,
    frames: usize,
}

impl Job {
    const EMPTY: Job = Job {
        tree: std::ptr::null(),
        stage: std::ptr::null(),
        stage_len: 0,
        buses: std::ptr::null(),
        buffers: std::ptr::null(),
        buffers_len: 0,
        sample_rate: 0.0,
        offset: 0,
        frames: 0,
    };
}

struct Shared {
    /// Written by the conductor strictly before the `epoch` Release store.
    job: UnsafeCell<Job>,
    epoch: AtomicU64,
    /// Work-stealing cursor into the stage slice.
    cursor: AtomicUsize,
    /// Stage items not yet fully processed; the conductor waits on 0.
    remaining: AtomicUsize,
    /// Workers currently inside the grab loop. The conductor also waits on
    /// 0 so a straggler can never touch `cursor`/`job` across a republish.
    active: AtomicUsize,
    states: Box<[AtomicU8]>,
    shutdown: AtomicBool,
}

// SAFETY: the raw pointers in `job` are only dereferenced under the
// publish/complete protocol described in the module docs.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

/// The pool. `WorkerPool::new(0)` is a no-op pool: every stage runs inline,
/// sequentially — the default for `engine_pair` and the whole test suite.
pub struct WorkerPool {
    shared: Option<Arc<Shared>>,
    threads: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    pub fn new(workers: usize) -> Self {
        if workers == 0 {
            return Self {
                shared: None,
                threads: Vec::new(),
            };
        }
        let shared = Arc::new(Shared {
            job: UnsafeCell::new(Job::EMPTY),
            epoch: AtomicU64::new(0),
            cursor: AtomicUsize::new(0),
            remaining: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            states: (0..workers).map(|_| AtomicU8::new(STATE_HOT)).collect(),
            shutdown: AtomicBool::new(false),
        });
        let threads = (0..workers)
            .map(|i| {
                let shared = Arc::clone(&shared);
                thread::Builder::new()
                    .name(format!("clausters-dsp-{i}"))
                    .spawn(move || worker_main(&shared, i))
                    .expect("failed to spawn DSP worker")
            })
            .collect();
        Self {
            shared: Some(shared),
            threads,
        }
    }

    pub fn worker_count(&self) -> usize {
        self.threads.len()
    }

    /// Runs one stage of a parallel group. Inline and sequential without
    /// workers; otherwise fork-join with the conductor participating.
    ///
    /// RT-safe on the conductor: atomics, bounded spinning and (at worst)
    /// `unpark` — no allocation, no locks.
    pub(crate) fn run_stage(&self, tree: &NodeTree, stage: &[usize], ctx: &ProcessCtx) {
        let Some(shared) = &self.shared else {
            for &idx in stage {
                // SAFETY: sequential fallback — single visitor by trivially
                // running one subtree at a time.
                unsafe { tree.process_index(idx, ctx, self) };
            }
            return;
        };

        // Publish, seqlock style. The odd epoch marks "writing": a worker
        // that observed the previous epoch but has not yet registered in
        // `active` re-validates the epoch after registering and backs off,
        // so `job` is never read while it is being rewritten. The bump and
        // the `active` drain below are SeqCst because they form a Dekker
        // pair with the worker's register-then-validate sequence.
        shared.epoch.fetch_add(1, Ordering::SeqCst);
        while shared.active.load(Ordering::SeqCst) != 0 {
            std::hint::spin_loop();
        }
        unsafe {
            *shared.job.get() = Job {
                tree,
                stage: stage.as_ptr(),
                stage_len: stage.len(),
                buses: ctx.buses,
                buffers: ctx.buffers.as_ptr(),
                buffers_len: ctx.buffers.len(),
                sample_rate: ctx.sample_rate,
                offset: ctx.offset,
                frames: ctx.frames,
            };
        }
        shared.cursor.store(0, Ordering::Relaxed);
        shared.remaining.store(stage.len(), Ordering::Relaxed);
        shared.epoch.fetch_add(1, Ordering::Release); // even: published
        for (i, t) in self.threads.iter().enumerate() {
            if shared.states[i].load(Ordering::Relaxed) == STATE_PARKED {
                t.thread().unpark();
            }
        }

        // The conductor works the same queue.
        loop {
            let k = shared.cursor.fetch_add(1, Ordering::AcqRel);
            if k >= stage.len() {
                break;
            }
            // SAFETY: the cursor hands each subtree to exactly one thread.
            unsafe { tree.process_index_seq(stage[k], ctx) };
            shared.remaining.fetch_sub(1, Ordering::Release);
        }
        // Wait for the stage, then for stragglers to leave the grab loop —
        // only then may `cursor`/`job` be reused.
        while shared.remaining.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
        while shared.active.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.shutdown.store(true, Ordering::Relaxed);
            shared.epoch.fetch_add(1, Ordering::Release); // wake spinners
            for t in &self.threads {
                t.thread().unpark();
            }
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn worker_main(shared: &Shared, me: usize) {
    IS_WORKER.set(true);
    // Workers process DSP in both RT and NRT renders: same FPU mode as the
    // conductor, or parallel renders would not be sample-identical.
    crate::dsp::denormals::flush_to_zero();

    let mut seen = 0u64;
    'outer: loop {
        // Wait for a new epoch: spin hot, then yield, then park.
        let mut spins = 0u32;
        let epoch = loop {
            let e = shared.epoch.load(Ordering::Acquire);
            if e != seen && e.is_multiple_of(2) {
                break e; // odd = the conductor is writing the next job
            }
            if shared.shutdown.load(Ordering::Relaxed) {
                return;
            }
            spins += 1;
            if spins < SPINS_BEFORE_YIELD {
                std::hint::spin_loop();
            } else if spins < SPINS_BEFORE_YIELD + YIELDS_BEFORE_PARK {
                thread::yield_now();
            } else {
                shared.states[me].store(STATE_PARKED, Ordering::Relaxed);
                // Re-check after flagging to close the lost-wakeup window.
                if shared.epoch.load(Ordering::Acquire) == seen
                    && !shared.shutdown.load(Ordering::Relaxed)
                {
                    thread::park();
                }
                shared.states[me].store(STATE_HOT, Ordering::Relaxed);
                spins = 0;
            }
        };
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }

        // Register, then re-validate. The conductor rewrites `job` only
        // behind an odd epoch after draining `active`, so an epoch still
        // unchanged *after* registering pins the job until we deregister;
        // a changed one means a republish slipped in — back off and
        // retry. SeqCst: Dekker pair with the conductor's publish.
        shared.active.fetch_add(1, Ordering::SeqCst);
        if shared.epoch.load(Ordering::SeqCst) != epoch {
            shared.active.fetch_sub(1, Ordering::Release);
            continue 'outer;
        }
        seen = epoch;
        // SAFETY: `job` was written before the epoch publish observed
        // above, and cannot be rewritten while `active` holds our count.
        let job = unsafe { *shared.job.get() };
        if job.tree.is_null() {
            shared.active.fetch_sub(1, Ordering::Release);
            continue 'outer;
        }
        // SAFETY: pointees pinned by the conductor's `run_stage` frame.
        let tree = unsafe { &*job.tree };
        let stage = unsafe { std::slice::from_raw_parts(job.stage, job.stage_len) };
        let buffers = unsafe { std::slice::from_raw_parts(job.buffers, job.buffers_len) };
        let ctx = ProcessCtx {
            sample_rate: job.sample_rate,
            full_sample_rate: job.sample_rate,
            buses: unsafe { &*job.buses },
            buffers,
            offset: job.offset,
            frames: job.frames,
        };
        loop {
            let k = shared.cursor.fetch_add(1, Ordering::AcqRel);
            if k >= stage.len() {
                break;
            }
            // SAFETY: the cursor hands each subtree to exactly one thread;
            // bus disjointness comes from the stage scheduler.
            unsafe { tree.process_index_seq(stage[k], &ctx) };
            shared.remaining.fetch_sub(1, Ordering::Release);
        }
        shared.active.fetch_sub(1, Ordering::Release);
    }
}
