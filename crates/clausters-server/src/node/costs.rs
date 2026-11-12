//! What each synth costs the audio thread, while somebody is profiling.
//!
//! The per-role table (`server::meters`) says how much of the block went to
//! the tree; this says **which node** it went to. Timing a node is two clock
//! reads per node per block, which is a real share of the block in a session
//! of a thousand synths, so it runs only while a client has asked for it
//! (`/server_profile 1`): the switch is one relaxed load per node, and a
//! server nobody profiles pays nothing else.
//!
//! **One entry per slot of the node slab**, allocated with the slab and never
//! resized, so the audio thread writes it without allocating and the network
//! thread reads it without asking the engine anything. An entry belongs to a
//! synth from the moment the synth is inserted until it leaves the tree; the
//! departing synth's totals go home with its garbage, so a note that lived
//! between two readings is still counted under its def.
//!
//! **Cumulative, like the role table.** An entry only grows while its synth
//! lives, and a reader differences two snapshots.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use crate::server::meters::{Stamp, stamp};

/// One slot's reading.
#[derive(Debug, Default)]
struct SlotCost {
    /// Whether a synth holds the slot. A group's slot is never live here: a
    /// group costs what its children cost.
    live: AtomicBool,
    id: AtomicI32,
    /// Nanoseconds of `process`, while profiled.
    busy: AtomicU64,
    /// Blocks (or slices) the synth ran in, while profiled.
    runs: AtomicU64,
}

/// What a synth cost while it was profiled: its node id, nanoseconds and
/// runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeCost {
    pub id: i32,
    pub busy_nanos: u64,
    pub runs: u64,
}

/// The table, shared between the tree (which writes it) and the network
/// thread (which reads it and flips the switch).
#[derive(Debug)]
pub struct NodeCosts {
    profiling: AtomicBool,
    slots: Box<[SlotCost]>,
}

impl NodeCosts {
    /// A table for a slab of `slots` nodes, profiling off.
    pub fn new(slots: usize) -> Self {
        Self {
            profiling: AtomicBool::new(false),
            slots: (0..slots).map(|_| SlotCost::default()).collect(),
        }
    }

    /// Turns the per-node timing on or off. Totals already counted stay.
    pub fn set_profiling(&self, on: bool) {
        self.profiling.store(on, Ordering::Relaxed);
    }

    pub fn profiling(&self) -> bool {
        self.profiling.load(Ordering::Relaxed)
    }

    /// The bracket a synth's `process` opens: a stamp while profiling, and
    /// nothing at all otherwise. RT-safe.
    #[inline]
    pub(crate) fn open(&self) -> Option<Stamp> {
        self.profiling().then(stamp)
    }

    /// Closes [`open`](Self::open)'s bracket on slot `idx`. The slot is
    /// visited by one thread per slice, so the two adds never contend.
    #[inline]
    pub(crate) fn close(&self, idx: usize, opened: Option<Stamp>) {
        if let (Some(at), Some(slot)) = (opened, self.slots.get(idx)) {
            slot.busy.fetch_add(at.elapsed_nanos(), Ordering::Relaxed);
            slot.runs.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A synth took slot `idx`: its entry starts from zero.
    pub(crate) fn begin(&self, idx: usize, id: i32) {
        if let Some(slot) = self.slots.get(idx) {
            slot.id.store(id, Ordering::Relaxed);
            slot.busy.store(0, Ordering::Relaxed);
            slot.runs.store(0, Ordering::Relaxed);
            slot.live.store(true, Ordering::Relaxed);
        }
    }

    /// The synth in slot `idx` left the tree: what it cost, and the slot is
    /// no longer reported.
    pub(crate) fn end(&self, idx: usize) -> NodeCost {
        let Some(slot) = self.slots.get(idx) else {
            return NodeCost::default();
        };
        slot.live.store(false, Ordering::Relaxed);
        NodeCost {
            id: slot.id.load(Ordering::Relaxed),
            busy_nanos: slot.busy.load(Ordering::Relaxed),
            runs: slot.runs.load(Ordering::Relaxed),
        }
    }

    /// Every live synth's totals. Allocates: the network thread's call.
    ///
    /// Read without the engine's help, so a synth that leaves while this runs
    /// may be listed once more or not at all; its totals reach the reader
    /// through its garbage either way.
    pub fn snapshot(&self) -> Vec<NodeCost> {
        self.slots
            .iter()
            .filter(|slot| slot.live.load(Ordering::Relaxed))
            .map(|slot| NodeCost {
                id: slot.id.load(Ordering::Relaxed),
                busy_nanos: slot.busy.load(Ordering::Relaxed),
                runs: slot.runs.load(Ordering::Relaxed),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_timed_until_profiling_is_on() {
        let costs = NodeCosts::new(4);
        costs.begin(2, 1001);
        let opened = costs.open();
        assert!(opened.is_none(), "no stamp while nobody profiles");
        costs.close(2, opened);
        assert_eq!(costs.snapshot()[0].runs, 0);

        costs.set_profiling(true);
        let opened = costs.open();
        costs.close(2, opened);
        let row = costs.snapshot()[0];
        assert_eq!(row.id, 1001);
        assert_eq!(row.runs, 1);
    }

    #[test]
    fn a_departing_synth_takes_its_totals_and_leaves_the_snapshot() {
        let costs = NodeCosts::new(4);
        costs.set_profiling(true);
        costs.begin(1, 7);
        costs.close(1, costs.open());
        costs.close(1, costs.open());
        let gone = costs.end(1);
        assert_eq!((gone.id, gone.runs), (7, 2));
        assert!(costs.snapshot().is_empty());

        // The slot's next synth starts from zero.
        costs.begin(1, 8);
        assert_eq!(
            costs.snapshot(),
            [NodeCost {
                id: 8,
                ..NodeCost::default()
            }]
        );
    }
}
